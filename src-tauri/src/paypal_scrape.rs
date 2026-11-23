//! PayPal 账户余额自动抓取（架构与 revenue_scrape 一致：启动后静默跑一次）。
//!
//! 触发：platform=paypal 的账号浏览器启动成功后，后台静默抓账户余额并
//! 上报 VPS 入库。全程零指纹且**只读**（用户明确要求「不要帮我导航」）：
//! 不开标签页、不导航、不关页面、不注入 JS——只 GET /json 枚举用户已打开的
//! 标签页，对其中 PayPal 页面 attach 后轮询 Accessibility.getFullAXTree 解析，
//! 失败只记日志，绝不影响浏览器启动。
//!
//! 服务端裁决（api_revenue_scrape.php，与 YouTube 收益抓取共用鉴权）：
//!   - paypal_precheck：全局收益开关 + 平台=paypal + cookie 登录态 +
//!     账号级收益抓取开关（revenue_enabled）+ 1 小时节流（balance_scraped_at）。
//!     任何一条不过都 skip，客户端连 PayPal 页面都不打开。
//!   - paypal_save：余额入库 tiktok_accounts.balance，变化时服务端写修改记录审计。
//!
//! 等待登录（用户说明：PayPal 每次启动都要重新登录）：用户自己完成登录并停在
//! summary 页。跟随式只读抓取：每轮枚举已打开的 PayPal 标签页，对每页 AXTree
//! 轮询（登录页继续等、渲染充分立即解析），没抓到歇 30 秒再来一轮；只要浏览器
//! 保持运行就一直跟随，抓到余额入库或浏览器关闭才终止。
//!
//! 页面解析（AXTree，节点顺序与 DOM 一致）：余额卡片 DOM 顺序为
//!   [金额 US$1,118.50] → [标签「可用余额」/ "Available balance"]，
//! 故取锚点文本前面最近的 US$ 金额；锚点缺失时全页只有一个 US$ 金额才接受。

use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// 启动后等多久才开始（错开 YouTube 收益抓取的 8 秒，避免 CDP 高峰叠加）。
const START_DELAY: Duration = Duration::from_secs(10);

/// 单轮对某个页面轮询 AXTree 的时长上限（用户指定 120 秒）。
const WAIT_TOTAL: Duration = Duration::from_secs(120);

/// 两轮之间的间隔：本轮没抓到（页面未登录/没开 PayPal 页/渲染不足），歇 30 秒
/// 再枚举一次。只要浏览器保持运行就持续跟随，抓到或浏览器关闭才终止。
const ROUND_GAP: Duration = Duration::from_secs(30);

/// 登录页特征文本（命中任一即认为该页面处于未登录态，继续等待而非提前结束）。
const LOGIN_HINTS: [&str; 3] = ["登录您的", "Log in to your", "登录以继续"];

/// 页面「渲染充分」的最低 StaticText 数（PayPal summary 页节点众多，低于该值
/// 说明没渲染完——代理慢/加载中，此时绝不解析）。
const RENDER_MIN_STATICTEXT: usize = 40;

/// 余额标签锚点文本（中英文界面）。
const BALANCE_ANCHORS: [&str; 2] = ["可用余额", "Available balance"];

/// 专属日志：走项目统一的 log_bwbrowser（tag = paypal_scrape）。
fn pp_log(profile: &str, msg: &str) {
  crate::bwbrowser_cloud::log_bwbrowser("paypal_scrape", &format!("[{profile}] {msg}"));
}

/// 无 profile 名时的日志（任务最早期）。
fn pp_log_raw(msg: &str) {
  crate::bwbrowser_cloud::log_bwbrowser("paypal_scrape", msg);
}

/// 后台启动一个 PayPal 账号的余额抓取任务。同步入口，内部 spawn 静默异步任务。
///
/// 跟随只读策略（用户要求「不要帮我导航，循环等待抓取即可」）：不开标签页、
/// 不导航——每轮 GET /json 枚举用户已打开的 PayPal 标签页并逐页 AXTree 轮询
///（最长 120 秒），用户登录后停在 summary 页即命中；本轮没抓到歇 30 秒再来
/// 一轮，浏览器保持运行就一直跟随，抓到入库或浏览器关闭才终止。预检不通过
///（开关关闭/非 PayPal/云端无登录态/节流）直接终止，不做无意义循环。
pub fn spawn_paypal_balance_task(profile_id: String) {
  pp_log_raw(&format!(
    "任务排程: profile_id={profile_id}, 延迟10秒后开始"
  ));
  tauri::async_runtime::spawn(async move {
    tokio::time::sleep(START_DELAY).await;
    let mut round = 1u32;
    loop {
      match balance_task(&profile_id, round).await {
        Ok(true) => break, // 成功入库 / 预检不通过 / 浏览器已关闭
        Ok(false) => { /* 本轮没抓到，浏览器还开着，继续跟随登录状态 */ }
        Err(e) => {
          pp_log_raw(&format!("任务异常(终止): profile_id={profile_id}: {e}"));
          break;
        }
      }
      round += 1;
      pp_log_raw(&format!(
        "本轮未抓到余额，{}秒后进入下一轮跟随（第{round}轮）",
        ROUND_GAP.as_secs()
      ));
      tokio::time::sleep(ROUND_GAP).await;
    }
  });
}

/// 主流程：读 profile → CDP 就绪 → VPS 预检（开关/平台/登录态/节流）→ 等待登录+抓余额 → 上报。
/// 返回 Ok(true) = 终态（成功入库 / 预检不通过 / 浏览器已关闭），外层终止循环；
/// 返回 Ok(false) = 本轮没抓到（浏览器还开着），外层继续下一轮跟随；
/// Err = 异常（外层记日志并终止）。
async fn balance_task(profile_id: &str, round: u32) -> Result<bool, String> {
  // 1. 读最新 profile（按 id）。
  let profiles = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("list_profiles failed: {e}"))?;
  let profile = profiles
    .into_iter()
    .find(|p| p.id.to_string() == profile_id)
    .ok_or_else(|| "profile not found".to_string())?;
  let name = profile.name.clone();

  pp_log(
    &name,
    &format!("===== 开始余额抓取(第{round}轮) ===== profile_id={profile_id}"),
  );

  // 2. 浏览器不在本地运行/无 CDP → 浏览器已关/远程会话，终止跟随循环。
  let Some(ws_url) = crate::cdp_target::local_browser_ws_url(&profile).await else {
    pp_log(&name, "跳过: 浏览器不在本地运行或无CDP端口（终止跟随）");
    return Ok(true);
  };
  pp_log(&name, "CDP浏览器级socket已就绪");

  // 3. 本地鉴权凭据。
  let Some((username, password)) = crate::bwbrowser_cloud::BWBROWSER_AUTH.get_credentials() else {
    pp_log(&name, "跳过: 客户端未登录云端（无鉴权凭据）（终止跟随）");
    return Ok(true);
  };

  // 4. VPS 预检：平台/登录态/收益抓取开关/节流全部由服务端裁决。
  //    账号级「收益抓取」开关（编辑账号 revenue_enabled）关闭 → 这里直接 skip，
  //    客户端不打开任何 PayPal 页面。
  pp_log(&name, "→ VPS paypal_precheck 预检");
  let pre = vps_call(
    &name,
    &[
      ("action", "paypal_precheck"),
      ("username", &username),
      ("password", &password),
      ("profile", &name),
    ],
  )
  .await;
  let pre = match pre {
    Ok(v) => v,
    Err(e) => {
      // 预检失败（网络/服务异常）：保守跳过，不碰 PayPal（风控敏感）。
      pp_log(&name, &format!("预检请求失败(保守跳过): {e}"));
      return Ok(true);
    }
  };
  let action = pre
    .get("action")
    .and_then(Value::as_str)
    .unwrap_or("skip")
    .to_string();
  let reason = pre
    .get("reason")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
  let msg = pre
    .get("message")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
  pp_log(&name, &format!("← 预检裁决: {action} ({reason}) {msg}"));
  if action != "go" {
    pp_log(&name, "===== 流程结束（预检不通过，终止跟随）=====");
    return Ok(true);
  }
  let account_name = pre
    .get("account_name")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();

  // 5. 抓余额：枚举已打开的 PayPal 标签页 → 逐页等待渲染/解析（只读，不开不导航）。
  let balance = scrape_balance(&ws_url, &name).await?;
  let Some(balance) = balance else {
    pp_log(
      &name,
      "本轮未解析到余额（未打开PayPal页/登录未完成/非概要页），继续下一轮跟随",
    );
    return Ok(false); // 浏览器还开着 → 外层歇30秒后继续下一轮跟随
  };
  pp_log(&name, &format!("解析到余额: {balance:.2}"));

  // 6. paypal_save 上报入库。
  let balance_str = format!("{balance:.2}");
  let save = vps_call(
    &name,
    &[
      ("action", "paypal_save"),
      ("username", &username),
      ("password", &password),
      ("profile", &name),
      ("balance", &balance_str),
    ],
  )
  .await;
  match save {
    Ok(v) => {
      let old = v.get("old_balance").and_then(Value::as_f64).unwrap_or(0.0);
      pp_log(
        &name,
        &format!("===== 完成: 账号 {account_name} 余额 {old:.2} → {balance:.2} 已入库 ====="),
      );
      Ok(true)
    }
    Err(e) => {
      pp_log(
        &name,
        &format!("上报失败(不影响浏览器), 下一轮跟随再试: {e}"),
      );
      Ok(false)
    }
  }
}

/// 抓取余额（只读跟随，用户要求「不要帮我导航，循环等待抓取即可」）：
/// GET /json 枚举用户已打开的标签页 → 过滤 PayPal 页 → 逐页 attach 后轮询
/// AXTree 解析。不开标签页、不导航、不关页面。返回 Ok(None) 表示本轮窗口内
/// 没解析到余额（没开 PayPal 页/登录未完成/改版/歧义），由外层跟随循环决定
/// 是否再来一轮。
async fn scrape_balance(ws_url: &str, name: &str) -> Result<Option<f64>, String> {
  // 1. 枚举当前打开的标签页（HTTP /json，与 revenue_scrape 探针同一机制）。
  let port = ws_url
    .rsplit(':')
    .next()
    .and_then(|p| p.split('/').next())
    .and_then(|p| p.parse::<u16>().ok())
    .ok_or_else(|| "ws_url 无有效端口".to_string())?;
  let client = reqwest::Client::new();
  let tabs: Vec<Value> = client
    .get(format!("http://127.0.0.1:{port}/json"))
    .timeout(Duration::from_secs(10))
    .send()
    .await
    .map_err(|e| format!("枚举标签页失败: {e}"))?
    .json()
    .await
    .map_err(|e| format!("标签页列表解析失败: {e}"))?;

  // 2. 过滤 PayPal 页面（type=page 且 URL 含 paypal.com）。
  let paypal_tabs: Vec<(String, String)> = tabs
    .iter()
    .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
    .filter_map(|t| {
      let url = t.get("url").and_then(Value::as_str).unwrap_or("");
      let id = t.get("id").and_then(Value::as_str)?.to_string();
      if url.contains("paypal.com") && !url.starts_with("devtools://") {
        Some((id, url.to_string()))
      } else {
        None
      }
    })
    .collect();
  if paypal_tabs.is_empty() {
    pp_log(
      name,
      &format!(
        "本轮无已打开的 PayPal 页面（共{}个标签页），等待用户打开后下一轮跟随",
        tabs.len()
      ),
    );
    return Ok(None);
  }
  pp_log(
    name,
    &format!(
      "发现{}个 PayPal 页面，逐页跟随解析: {:?}",
      paypal_tabs.len(),
      paypal_tabs
        .iter()
        .map(|(_, u)| truncate(u, 60))
        .collect::<Vec<_>>()
    ),
  );

  // 3. 逐页 attach + 轮询 AXTree 解析（第一个解析出余额的页面即用）。
  let mut id_seq = 1u64;
  for (tid, url) in &paypal_tabs {
    let request = ws_url
      .into_client_request()
      .map_err(|e| format!("build ws request failed: {e}"))?;
    let config = WebSocketConfig::default()
      .max_message_size(Some(16 * 1024 * 1024))
      .max_frame_size(Some(16 * 1024 * 1024));
    let (mut ws, _) = tokio_tungstenite::connect_async_with_config(request, Some(config), false)
      .await
      .map_err(|e| format!("ws connect failed: {e}"))?;

    // flat attach 拿 sessionId（只 attach，不动页面）。
    id_seq += 1;
    let attached = crate::revenue_scrape::round_trip(
      &mut ws,
      id_seq,
      None,
      "Target.attachToTarget",
      json!({ "targetId": tid, "flatten": true }),
    )
    .await?;
    let session = attached
      .get("sessionId")
      .and_then(Value::as_str)
      .ok_or_else(|| "attachToTarget returned no sessionId".to_string())?
      .to_string();
    pp_log(
      name,
      &format!("已attach页面 {}（只读跟随）", truncate(url, 80)),
    );

    // 轮询 AXTree（最长 120 秒）：登录页继续等（用户可能正在该页登录），
    // 渲染充分立即解析；每 4 秒一轮。
    let deadline = tokio::time::Instant::now() + WAIT_TOTAL;
    let mut attempt = 0u32;
    let mut texts_so_far: Vec<String> = Vec::new();
    let mut prev_nodes = 0usize;
    let mut stable_rounds = 0u32;
    let mut found: Option<f64> = None;
    loop {
      attempt += 1;
      tokio::time::sleep(Duration::from_secs(4)).await;
      let ax = match crate::revenue_scrape::round_trip(
        &mut ws,
        100 + attempt as u64,
        Some(&session),
        "Accessibility.getFullAXTree",
        json!({}),
      )
      .await
      {
        Ok(v) => v,
        Err(e) => {
          // 用户切页/关页导致 session 失效：正常现象，放弃该页继续下一页。
          pp_log(
            name,
            &format!("页面轮询#{attempt}: AXTree失败(页面已切换或关闭): {e}"),
          );
          break;
        }
      };
      let nodes = ax
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
      let collected: Vec<String> = nodes
        .iter()
        .filter(|n| {
          crate::revenue_scrape::node_role(n) == "StaticText"
            && !crate::revenue_scrape::node_name(n).is_empty()
        })
        .map(crate::revenue_scrape::node_name)
        .collect();
      pp_log(
        name,
        &format!(
          "页面轮询#{attempt}: 节点数={}, StaticText数={}",
          nodes.len(),
          collected.len()
        ),
      );
      // 登录页判定：命中特征词 → 用户还没登录完，继续等待。
      let on_login_page = LOGIN_HINTS
        .iter()
        .any(|h| collected.iter().any(|t| t.contains(h)));
      if on_login_page {
        pp_log(
          name,
          &format!(
            "检测到登录页，等待用户完成登录（窗口剩余{}秒）",
            deadline
              .saturating_duration_since(tokio::time::Instant::now())
              .as_secs()
          ),
        );
        texts_so_far = collected;
        prev_nodes = 0;
        stable_rounds = 0;
      } else if collected.len() >= RENDER_MIN_STATICTEXT {
        // 渲染充分：立即解析，解析到余额即结束该页轮询。
        if let Some(bal) = parse_balance(&collected) {
          found = Some(bal);
          texts_so_far = collected;
          break;
        }
        // 渲染充分但没解析到（非 summary 页/改版）→ 继续轮询到超时换页。
        texts_so_far = collected;
      } else {
        // 渲染不充分且非登录页：连续 3 轮稳定 → 提前结束该页（疑似风控页）。
        if nodes.len() == prev_nodes && !nodes.is_empty() {
          stable_rounds += 1;
          if stable_rounds >= 3 {
            pp_log(
              name,
              "页面节点数已稳定但仍渲染不充分（疑似风控页），跳过该页",
            );
            break;
          }
        } else {
          stable_rounds = 0;
        }
        prev_nodes = nodes.len();
        texts_so_far = collected;
      }
      if tokio::time::Instant::now() >= deadline {
        pp_log(name, "页面跟随超时(120秒)，换下一个页面");
        break;
      }
    }

    // 只关 WebSocket 连接，绝不动用户的标签页。
    let _ = ws.close(None).await;
    if let Some(bal) = found.or_else(|| parse_balance(&texts_so_far)) {
      return Ok(Some(bal));
    }
    pp_log(name, "该页面未解析到余额，尝试下一个 PayPal 页面");
  }

  Ok(None)
}

/// 解析余额：优先取「可用余额/Available balance」锚点前最近的 US$ 金额；
/// 无锚点时全页只有一个 US$ 金额才接受（避免误取待处理/其他卡片金额）。
fn parse_balance(texts: &[String]) -> Option<f64> {
  let amounts: Vec<(usize, f64)> = texts
    .iter()
    .enumerate()
    .filter_map(|(i, t)| parse_us_amount(t).map(|v| (i, v)))
    .collect();
  if amounts.is_empty() {
    return None;
  }

  for anchor in BALANCE_ANCHORS {
    let anchor_idx = texts.iter().position(|t| {
      let tt = t.trim();
      tt.eq_ignore_ascii_case(anchor) || tt.contains(anchor)
    });
    if let Some(ai) = anchor_idx {
      // DOM/AXTree 顺序：金额在前、标签在后 → 优先取锚点前面最近的金额。
      if let Some(&(_, v)) = amounts.iter().rev().find(|&&(i, _)| i < ai) {
        return Some(v);
      }
      // 兼容布局变化：锚点后面最近的金额。
      if let Some(&(_, v)) = amounts.iter().find(|&&(i, _)| i > ai) {
        return Some(v);
      }
    }
  }

  // 锚点没命中（改版/英文翻译差异）：全页唯一金额才接受。
  if amounts.len() == 1 {
    return Some(amounts[0].1);
  }
  None
}

/// 解析 "US$1,118.50" → 1118.50。必须带小数点（避免把 "US$" 残片误当金额）。
fn parse_us_amount(t: &str) -> Option<f64> {
  let t = t.trim();
  let rest = t.strip_prefix("US$")?;
  let cleaned: String = rest
    .chars()
    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
    .collect();
  if !cleaned.contains('.') {
    return None;
  }
  cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// 向 VPS 收益 API 发一次表单请求（与 revenue_scrape::vps_call 同一端点，
/// 日志 tag 区分为 paypal_scrape）。
async fn vps_call(name: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
  let client = reqwest::Client::new();
  let form = fields
    .iter()
    .map(|(k, v)| format!("{}={}", k, urlencode(v)))
    .collect::<Vec<_>>()
    .join("&");
  let url = format!(
    "{}/tk/api_revenue_scrape.php",
    crate::cloud_domain::CLOUD_ROOT
  );
  let started = std::time::Instant::now();
  let resp = client
    .post(&url)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form)
    .timeout(Duration::from_secs(30))
    .send()
    .await
    .map_err(|e| {
      pp_log(name, &format!("VPS请求失败: {e}"));
      format!("VPS API request failed: {e}")
    })?;
  let status_code = resp.status().as_u16();
  let body = match resp.text().await {
    Ok(b) => b,
    Err(e) => {
      pp_log(name, &format!("VPS响应读取失败(HTTP {status_code}): {e}"));
      return Err(format!("read VPS API body failed: {e}"));
    }
  };
  pp_log(
    name,
    &format!(
      "VPS响应: HTTP {status_code}, {} bytes, {}ms, body={}",
      body.len(),
      started.elapsed().as_millis(),
      truncate(&body, 500)
    ),
  );
  let v: Value = match serde_json::from_str(&body) {
    Ok(v) => v,
    Err(e) => {
      pp_log(name, &format!("VPS响应JSON解析失败: {e}"));
      return Err(format!(
        "parse VPS API response failed: {e} ({})",
        truncate(&body, 200)
      ));
    }
  };
  if v.get("success").and_then(Value::as_bool) == Some(false) {
    return Err(format!(
      "VPS API error: {}",
      v.get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    ));
  }
  Ok(v)
}

/// form-urlencoded 编码。
fn urlencode(s: &str) -> String {
  let mut out = String::with_capacity(s.len() * 3);
  for b in s.bytes() {
    match b {
      b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
      _ => out.push_str(&format!("%{:02X}", b)),
    }
  }
  out
}

fn truncate(s: &str, n: usize) -> String {
  if s.chars().count() <= n {
    s.to_string()
  } else {
    s.chars().take(n).collect()
  }
}
