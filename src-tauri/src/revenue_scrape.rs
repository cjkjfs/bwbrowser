//! YouTube 收益自动抓取（Rust 主程序内嵌版）。
//!
//! 架构背景：收益抓取最初是独立的 Python 看门狗进程，但外部常驻进程无法在本
//! 应用会话外稳定存续（作业对象回收 / 安全策略拦截脱离）。用户明确要求「必须
//! 集成进主程序」。本模块把抓取逻辑搬进 BwBrowser 自身：账号浏览器启动成功后，
//! 后台静默执行一次收益抓取，全部失败只记日志，绝不影响浏览器启动。
//!
//! 原理（与已验证的 Python 版一致，零指纹）：
//!   - 不装扩展、不注入 JS、不加 --remote-allow-origins；
//!   - 只用 CDP 放行的命令：Target.createTarget / Target.attachToTarget /
//!     Page.navigate / Accessibility.getFullAXTree / Target.closeTarget；
//!   - Accessibility.getFullAXTree 返回「列优先」：先所有金额(含总额 US$)，再
//!     所有日期，index 对齐即同一表格行（已与 lifetime_full_rows 交叉验证）。
//!
//! 流程（一个账号浏览器启动后）：
//!   探针识别登录频道 → status 问 VPS(开关/归属/节流) → go 则导航收益深链 →
//!   AXTree 解析 → save 上报 VPS 幂等入库 → 关标签页。
//!
//! VPS 端接口：api_revenue_scrape.php（鉴权与 bwbrowser_sync 相同，token/密码双
//! 通道）。节流/归属/无收益回查全部由服务端裁决，Rust 侧只负责抓数据和投递。
//!
//! 日志：全程走项目统一的 `log_bwbrowser("revenue_scrape", ...)`（tag 统一为
//! revenue_scrape）。后台「云端日志」开关开启时写入数据目录 bwbrowser_debug.log，
//! 排障时打开开关即可在日志文件里还原完整流程（探针/裁决/抓取/上报每步都有）。

use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

/// 启动后等多久才开始抓。给 cookie 注入、首次导航留窗口；也刚好错过启动期那
/// 段 CDP 被反复占用（setFingerprint / 指纹应用）的高峰。
const REVENUE_START_DELAY: Duration = Duration::from_secs(8);

/// 单个 AXTree 命令的最大等待时长。
const AX_TIMEOUT: Duration = Duration::from_secs(40);

/// 探针轮询 /json 的总时长上限。
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

/// 收益页「渲染充分」的最低 StaticText 数。正常 studio 页面有几百上千个节点；
/// 低于该值说明页面没渲染完（代理慢/加载中），此时绝不能误判为未开通收益。
const RENDER_MIN_STATICTEXT: usize = 40;

/// 渲染轮询的总时长上限（每 4 秒抓一次 AXTree 直到渲染充分或超时）。
const RENDER_WAIT_TOTAL: Duration = Duration::from_secs(60);

/// 「疑似未开通收益」的额外重试次数：第一次抓到（渲染充分却）无总额时，
/// 再重试抓取这么多次，全部无总额才判定未开通并标记 6 小时回查（共 1+N 次抓取）。
const NO_REVENUE_RETRIES: u32 = 2;

/// 未开通重试之间的间隔。
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// ASCII 无收益提示关键词（页面没有显示 US$ 金额 / 没有可显示的数据等）。
const NO_REV_HINTS: [&str; 9] = [
  "未启用",
  "不符合",
  "获利",
  "Monetization",
  "没有可显示的数据",
  "暂无数据",
  "无数据",
  "开通",
  "申请",
];

/// 专属日志：走项目统一的 log_bwbrowser（终端 + log::info；后台「云端日志」
/// 开关开启时额外写入数据目录 bwbrowser_debug.log，排障时打开开关即可看全程）。
fn rev_log(profile: &str, msg: &str) {
  crate::bwbrowser_cloud::log_bwbrowser("revenue_scrape", &format!("[{profile}] {msg}"));
}

/// 无 profile 名时的日志（任务最早期，profile 还没解析出来）。
fn rev_log_raw(msg: &str) {
  crate::bwbrowser_cloud::log_bwbrowser("revenue_scrape", msg);
}

/// 后台启动一个账号的收益抓取任务。同步入口，内部 spawn 静默异步任务。
pub fn spawn_revenue_task(profile_id: String) {
  rev_log_raw(&format!("任务排程: profile_id={profile_id}, 延迟8秒后开始"));
  tauri::async_runtime::spawn(async move {
    tokio::time::sleep(REVENUE_START_DELAY).await;
    if let Err(e) = revenue_task(&profile_id).await {
      rev_log_raw(&format!("任务失败: profile_id={profile_id}: {e}"));
    }
  });
}

/// 主流程：读 profile → 定位浏览器 → 探针识别频道 → 交 VPS 裁决 → 抓取上报。
async fn revenue_task(profile_id: &str) -> Result<(), String> {
  // 1. 读最新 profile（按 id）。
  let profiles = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("list_profiles failed: {e}"))?;
  let profile = profiles
    .into_iter()
    .find(|p| p.id.to_string() == profile_id)
    .ok_or_else(|| "profile not found".to_string())?;
  let name = profile.name.clone();

  rev_log(
    &name,
    &format!(
      "===== 开始抓取流程 ===== profile_id={profile_id}, browser={}, process_id={:?}",
      profile.browser, profile.process_id
    ),
  );

  // 2. 只对可 CDP 驱动的内核有效；拿不到 browser 级 ws 说明浏览器不在本地
  //    运行（手动开/远程会话），非本程序启动场景，安静跳过。
  let Some(ws_url) = crate::cdp_target::local_browser_ws_url(&profile).await else {
    rev_log(
      &name,
      "跳过: 浏览器不在本地运行或无CDP端口（手动开/远程会话场景）",
    );
    return Ok(());
  };
  rev_log(&name, "CDP浏览器级socket已就绪");

  // 2.5 登录确认门：未确认登录前不启动收益抓取。
  //     启动初期云端 cookie 恢复 / 手动登录是异步进行的（云端注入约 15 秒内
  //     完成，手动登录更久）；未登录就跑流程只会白开探针 tab、白打 VPS 请求，
  //     还有风控风险（2026-11-19 3873 实测：注入被拦、本地 7 分未登录态，
  //     收益流程照样跑完 precheck + 探针）。
  //     轮询页面亲证 LOGGED_IN：确认登录立即继续；超时仍未确认则整单跳过。
  //     亲证 None（CDP 读不到/无 YouTube tab）与 Some(false) 同等对待——
  //     「确认登录」才抓，宁可不抓。
  const LOGIN_WAIT_TIMEOUT: Duration = Duration::from_secs(180);
  const LOGIN_WAIT_INTERVAL: Duration = Duration::from_secs(5);
  let deadline = tokio::time::Instant::now() + LOGIN_WAIT_TIMEOUT;
  loop {
    match crate::cookie_health::youtube_logged_in_via_cdp(&profile).await {
      Some(true) => {
        rev_log(&name, "登录确认: 页面亲证 LOGGED_IN=true，继续收益抓取");
        break;
      }
      _ => {
        if tokio::time::Instant::now() >= deadline {
          rev_log(
            &name,
            "跳过: 等待登录确认超时（180秒内页面亲证未确认登录），不启动收益抓取",
          );
          return Ok(());
        }
      }
    }
    tokio::time::sleep(LOGIN_WAIT_INTERVAL).await;
  }

  // 3. 本地鉴权凭据（token 或密码）——预检需要，提前获取。
  let Some((username, password)) = crate::bwbrowser_cloud::BWBROWSER_AUTH.get_credentials() else {
    rev_log(&name, "跳过: 客户端未登录云端（无鉴权凭据），无法调VPS接口");
    return Ok(());
  };
  let pwd_hint = if password.len() == 40 {
    "token"
  } else {
    "密码"
  };
  rev_log(
    &name,
    &format!("鉴权凭据就绪: user={username}, 凭据类型={pwd_hint}"),
  );

  // 3.5 预检：仅 YouTube 平台且确认有登录态的账号才开启收益抓取。
  //     非 YouTube / 未登录账号在此直接跳过，连探针 tab 都不开（省资源、免打扰）。
  rev_log(&name, &format!("→ VPS precheck 预检: profile={name}"));
  let pre = vps_call(
    &name,
    &[
      ("action", "precheck"),
      ("username", &username),
      ("password", &password),
      ("profile", &name),
    ],
  )
  .await;
  match &pre {
    Ok(v) => {
      let action = v.get("action").and_then(Value::as_str).unwrap_or("skip");
      let reason = v.get("reason").and_then(Value::as_str).unwrap_or("");
      let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
      rev_log(
        &name,
        &format!("← VPS precheck 响应: {action} ({reason}) {msg}"),
      );
      if action != "go" {
        rev_log(
          &name,
          &format!("===== 流程结束（预检不通过: {reason}）====="),
        );
        return Ok(());
      }
    }
    Err(e) => {
      // 预检请求失败（网络/服务异常）：保守放行走原流程，由探针与 status 兜底。
      rev_log(&name, &format!("预检请求失败(保守放行，探针兜底): {e}"));
    }
  }

  // 4. 探针识别当前登录频道（新建 tab 导航 studio.youtube.com，重定向后读 URL）。
  let Some(cid) = detect_login_channel(&ws_url, &name).await else {
    rev_log(
      &name,
      "跳过: 探针未能识别登录频道（未登录YouTube或studio不可达）",
    );
    return Ok(());
  };
  rev_log(&name, &format!("探针识别到登录频道: {cid}"));

  // 5. status：问 VPS 开关 / 归属 / 节流。profile 名尾号带账号 id（如 *_1291），
  //    VPS 据此定位账号；若探针登录频道与该账号登记不一致，服务端会回传更新登记频道。
  rev_log(
    &name,
    &format!("→ VPS status 请求: channel_id={cid}, profile={name}"),
  );
  let status = vps_call(
    &name,
    &[
      ("action", "status"),
      ("username", &username),
      ("password", &password),
      ("channel_id", &cid),
      ("profile", &name),
    ],
  )
  .await?;
  rev_log(&name, &format!("← VPS status 响应: {status}"));
  let decision = status
    .get("action")
    .and_then(Value::as_str)
    .unwrap_or("skip")
    .to_string();
  let msg = status
    .get("message")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
  rev_log(&name, &format!("VPS裁决: {decision} ({msg})"));

  // 若服务端判定「探针频道与登记不一致，已回传更新登记频道」，记日志便于排障。
  if status.get("updated").and_then(Value::as_bool) == Some(true) {
    let old_cid = status.get("old_cid").and_then(Value::as_str).unwrap_or("?");
    rev_log(
      &name,
      &format!("频道回传更新: 登记 {old_cid} → 实际登录 {cid}（已同步 VPS 账号登记频道）"),
    );
  }

  if decision != "go" {
    rev_log(&name, "===== 流程结束（skip）=====");
    return Ok(());
  }
  let company_id = status
    .get("company_id")
    .and_then(Value::as_i64)
    .unwrap_or(0);
  let account_id = status
    .get("account_id")
    .and_then(Value::as_i64)
    .unwrap_or(0);
  let account_name = status
    .get("account_name")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
  rev_log(
    &name,
    &format!(
      "归属确认: company_id={company_id}, account_id={account_id}, account_name={account_name}"
    ),
  );

  // 6. 抓收益深链 + AXTree 解析。疑似未开通（渲染充分却无总额）时不立即判定，
  //    重试抓满 NO_REVENUE_RETRIES 次确认后再定。
  let mut fetch_no = 0u32;
  let (total, daily, all_texts, rendered) = loop {
    fetch_no += 1;
    let (t, d, a, r) = scrape_revenue(&ws_url, &cid, &name).await?;
    rev_log(
      &name,
      &format!(
        "抓取结果(第{fetch_no}次): 总额={:?}, 明细天数={}, 页面文本数={}, 渲染充分={r}",
        t,
        d.len(),
        a.len()
      ),
    );
    if r && d.is_empty() && t.is_none() && fetch_no <= NO_REVENUE_RETRIES {
      rev_log(
        &name,
        &format!(
          "疑似未开通(第{fetch_no}次抓取无总额)，{DELAY}秒后重试确认 ({}/{})",
          fetch_no,
          NO_REVENUE_RETRIES + 1,
          DELAY = RETRY_DELAY.as_secs()
        ),
      );
      tokio::time::sleep(RETRY_DELAY).await;
      continue;
    }
    if fetch_no > 1 {
      rev_log(&name, &format!("未开通判定经 {fetch_no} 次抓取确认"));
    }
    break (t, d, a, r);
  };

  // 7. 无明细分类（只在页面确实渲染充分时才允许判定「未开通收益」，页面没
  //    渲染出来的空结果绝不标记，避免误拉黑 6 小时）：
  //    - 有总额卡但无明细 → 开通没赚，6 小时后回查。
  //    - 渲染充分且连总额都没有 → 疑似未开通，6 小时后回查。
  if daily.is_empty() {
    if !rendered {
      rev_log(
        &name,
        "页面未渲染充分，本轮放弃且不标记无收益（下次启动会重试）",
      );
      rev_log(&name, "===== 流程结束（页面未渲染，未标记）=====");
      return Ok(());
    }
    let hit_hint: Vec<&str> = all_texts
      .iter()
      .filter(|t| NO_REV_HINTS.iter().any(|h| t.contains(h)))
      .take(5)
      .map(String::as_str)
      .collect();
    let hint = if total.is_none() {
      "疑似未开通收益"
    } else {
      "已有收益卡但无日明细"
    };
    rev_log(
      &name,
      &format!(
        "无收益判定: {hint}; 命中关键词={hit_hint:?}; 页面文本前10条={:?}",
        &all_texts[..all_texts.len().min(10)]
      ),
    );
    let mark = vps_call(
      &name,
      &[
        ("action", "mark_no_revenue"),
        ("username", &username),
        ("password", &password),
        ("channel_id", &cid),
        ("hint", hint),
      ],
    )
    .await;
    match mark {
      Ok(r) => rev_log(&name, &format!("已标记6小时回查: {r}")),
      Err(e) => rev_log(&name, &format!("标记6小时回查失败(不影响主流程): {e}")),
    }
    rev_log(&name, "===== 流程结束（无明细）=====");
    return Ok(());
  }

  // 明细预览（前5条+后2条），排障时能直接看到解析出了什么。
  let preview: Vec<String> = daily
    .iter()
    .take(5)
    .map(|d| {
      format!(
        "{}={}",
        d.get("date").and_then(Value::as_str).unwrap_or("?"),
        d.get("income").and_then(Value::as_f64).unwrap_or(0.0)
      )
    })
    .collect();
  rev_log(&name, &format!("明细预览(前5): {preview:?}"));

  // 8. save 上报。
  let daily_val = serde_json::to_string(&daily).map_err(|e| e.to_string())?;
  rev_log(
    &name,
    &format!("→ VPS save 请求: channel_id={cid}, days={}", daily.len()),
  );
  let save = vps_call(
    &name,
    &[
      ("action", "save"),
      ("username", &username),
      ("password", &password),
      ("channel_id", &cid),
      ("daily", &daily_val),
    ],
  )
  .await?;
  rev_log(&name, &format!("← VPS save 响应: {save}"));
  let written = save.get("written").and_then(Value::as_i64).unwrap_or(0);
  let rtotal = total.unwrap_or_else(|| "0".to_string());
  rev_log(
    &name,
    &format!(
      "===== 完成: 总额{rtotal}, 抓到{}天, 库中变化{written}条 =====",
      daily.len()
    ),
  );
  Ok(())
}

/// 探针识别登录频道：在浏览器级 socket 上新建 tab 导航 studio.youtube.com，
/// YouTube 会重定向到登录账号的频道页，轮询 /json 读该 tab 最终 URL 的
/// channel_id，然后关闭探针 tab。
async fn detect_login_channel(ws_url: &str, name: &str) -> Option<String> {
  let request = match ws_url.into_client_request() {
    Ok(r) => r,
    Err(e) => {
      rev_log(name, &format!("探针失败: ws请求构造失败: {e}"));
      return None;
    }
  };
  let config = WebSocketConfig::default()
    .max_message_size(Some(16 * 1024 * 1024))
    .max_frame_size(Some(16 * 1024 * 1024));
  let (mut ws, _) =
    match tokio_tungstenite::connect_async_with_config(request, Some(config), false).await {
      Ok(c) => c,
      Err(e) => {
        rev_log(name, &format!("探针失败: ws连接失败: {e}"));
        return None;
      }
    };

  // 新建探针 tab。
  let created = match round_trip(
    &mut ws,
    9001,
    None,
    "Target.createTarget",
    json!({
      "url": "https://studio.youtube.com/"
    }),
  )
  .await
  {
    Ok(c) => c,
    Err(e) => {
      rev_log(name, &format!("探针失败: createTarget 失败: {e}"));
      return None;
    }
  };
  let Some(tid) = created
    .get("targetId")
    .and_then(Value::as_str)
    .map(str::to_string)
  else {
    rev_log(name, "探针失败: createTarget 未返回 targetId");
    return None;
  };
  drop(ws);
  rev_log(
    name,
    &format!("探针tab已创建: targetId={tid}, 导航 studio.youtube.com"),
  );

  // 轮询 /json 读该 tab 的最终 URL（等重定向完成），提取 channel_id。
  let cid = poll_probe_url(ws_url, &tid, name).await;
  // 无论成败都关探针 tab。
  close_probe_tab(ws_url, &tid, name).await;
  cid
}

/// 轮询 /json 读探针 tab 的最终 URL 提取 channel_id。
async fn poll_probe_url(ws_url: &str, tid: &str, name: &str) -> Option<String> {
  // 从 ws_url 反推端口（ws://127.0.0.1:PORT/devtools/browser/...）。
  let port: u16 = ws_url
    .split("//")
    .nth(1)?
    .split(':')
    .nth(1)?
    .split('/')
    .next()?
    .parse()
    .ok()?;
  let client = reqwest::Client::new();
  let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
  let mut attempt = 0u32;
  while tokio::time::Instant::now() < deadline {
    tokio::time::sleep(Duration::from_secs(2)).await;
    attempt += 1;
    let targets = match client
      .get(format!("http://127.0.0.1:{port}/json"))
      .timeout(Duration::from_secs(5))
      .send()
      .await
    {
      Ok(resp) => match resp.json::<Vec<Value>>().await {
        Ok(t) => t,
        Err(e) => {
          rev_log(name, &format!("探针轮询#{attempt}: /json解析失败: {e}"));
          continue;
        }
      },
      Err(e) => {
        rev_log(name, &format!("探针轮询#{attempt}: /json请求失败: {e}"));
        continue;
      }
    };
    for t in &targets {
      if t.get("id").and_then(Value::as_str) != Some(tid) {
        continue;
      }
      let url = t.get("url").and_then(Value::as_str).unwrap_or("");
      if let Some(cid) = extract_channel_id(url) {
        rev_log(
          name,
          &format!("探针轮询#{attempt}: 命中频道 {cid} (url={url})"),
        );
        return Some(cid);
      }
      rev_log(
        name,
        &format!("探针轮询#{attempt}: tab URL={url} (尚未重定向出频道id)"),
      );
      break;
    }
  }
  rev_log(
    name,
    "探针轮询超时: 25秒内未拿到频道id（可能未登录YouTube）",
  );
  None
}

/// 关闭探针 tab（尽力而为）。
async fn close_probe_tab(ws_url: &str, tid: &str, name: &str) {
  let request = match ws_url.into_client_request() {
    Ok(r) => r,
    Err(_) => return,
  };
  let config = WebSocketConfig::default()
    .max_message_size(Some(16 * 1024 * 1024))
    .max_frame_size(Some(16 * 1024 * 1024));
  if let Ok((mut ws, _)) =
    tokio_tungstenite::connect_async_with_config(request, Some(config), false).await
  {
    match round_trip(
      &mut ws,
      9002,
      None,
      "Target.closeTarget",
      json!({ "targetId": tid }),
    )
    .await
    {
      Ok(_) => rev_log(name, "探针tab已关闭"),
      Err(e) => rev_log(name, &format!("探针tab关闭失败(无害): {e}")),
    }
    let _ = ws.close(None).await;
  }
}

/// 从 studio URL 中提取 channel_id（UC 开头 12 位以上）。
fn extract_channel_id(url: &str) -> Option<String> {
  let segs: Vec<&str> = url.split('/').collect();
  for (i, seg) in segs.iter().enumerate() {
    if *seg == "channel" {
      if let Some(id) = segs.get(i + 1) {
        if id.starts_with("UC") && id.len() >= 12 {
          return Some(id.to_string());
        }
      }
    }
  }
  None
}

/// 抓取一个频道：新建隐形 tab 导航收益 deep link → 轮询等渲染 → AXTree → 解析。
///
/// 返回 (总额, 日明细数组, 页面全部 StaticText, 页面是否渲染充分)。
async fn scrape_revenue(
  ws_url: &str,
  cid: &str,
  name: &str,
) -> Result<(Option<String>, Vec<Value>, Vec<String>, bool), String> {
  let request = ws_url
    .into_client_request()
    .map_err(|e| format!("build ws request failed: {e}"))?;
  let config = WebSocketConfig::default()
    .max_message_size(Some(16 * 1024 * 1024))
    .max_frame_size(Some(16 * 1024 * 1024));
  let (mut ws, _) = tokio_tungstenite::connect_async_with_config(request, Some(config), false)
    .await
    .map_err(|e| format!("ws connect failed: {e}"))?;

  // 1. 新建隐形（后台）标签页。
  let created = round_trip(
    &mut ws,
    1,
    None,
    "Target.createTarget",
    json!({
      "url": "about:blank",
      "background": true,
    }),
  )
  .await?;
  let tid = created
    .get("targetId")
    .and_then(Value::as_str)
    .ok_or_else(|| "createTarget returned no targetId".to_string())?
    .to_string();
  rev_log(name, &format!("抓取tab已创建: targetId={tid}"));

  // 2. flat attach 拿 sessionId。
  let attached = round_trip(
    &mut ws,
    2,
    None,
    "Target.attachToTarget",
    json!({
      "targetId": tid,
      "flatten": true,
    }),
  )
  .await?;
  let session = attached
    .get("sessionId")
    .and_then(Value::as_str)
    .ok_or_else(|| "attachToTarget returned no sessionId".to_string())?
    .to_string();

  // 3. 导航到收益深链（带 sessionId），记录导航结果（失败会导致页面空白）。
  let url = revenue_url(cid);
  let nav = round_trip(
    &mut ws,
    3,
    Some(&session),
    "Page.navigate",
    json!({ "url": url }),
  )
  .await;
  match &nav {
    Ok(v) => rev_log(
      name,
      &format!(
        "已导航收益深链: frameId={}, loaderId={}（轮询渲染，最长{}秒）",
        v.get("frameId").and_then(Value::as_str).unwrap_or("?"),
        v.get("loaderId").and_then(Value::as_str).unwrap_or("?"),
        RENDER_WAIT_TOTAL.as_secs()
      ),
    ),
    Err(e) => rev_log(name, &format!("Page.navigate 返回错误(继续尝试): {e}")),
  }

  // 4. 轮询 AXTree 直到页面渲染充分（StaticText 达标）或超时。
  //    固定等待不够：代理慢的浏览器 14 秒时页面只有十几个空节点，曾被误判为
  //    未开通收益。改为每 4 秒抓一次，渲染充分即停。
  let deadline = tokio::time::Instant::now() + RENDER_WAIT_TOTAL;
  let mut attempt = 0u32;
  let (ax, rendered) = loop {
    attempt += 1;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let ax = match round_trip(
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
        rev_log(name, &format!("渲染轮询#{attempt}: AXTree失败: {e}"));
        let _ = round_trip(
          &mut ws,
          900,
          None,
          "Target.closeTarget",
          json!({ "targetId": tid }),
        )
        .await;
        let _ = ws.close(None).await;
        return Err(format!("getFullAXTree failed: {e}"));
      }
    };
    let nodes = ax
      .get("nodes")
      .and_then(Value::as_array)
      .cloned()
      .unwrap_or_default();
    let static_count = nodes
      .iter()
      .filter(|n| node_role(n) == "StaticText" && !node_name(n).is_empty())
      .count();
    rev_log(
      name,
      &format!(
        "渲染轮询#{attempt}: 节点数={}, StaticText数={}",
        nodes.len(),
        static_count
      ),
    );
    if static_count >= RENDER_MIN_STATICTEXT {
      break (ax, true);
    }
    if tokio::time::Instant::now() >= deadline {
      rev_log(
        name,
        &format!(
          "渲染轮询超时({}秒): 页面始终未渲染充分",
          RENDER_WAIT_TOTAL.as_secs()
        ),
      );
      break (ax, false);
    }
  };
  let nodes = ax
    .get("nodes")
    .and_then(Value::as_array)
    .cloned()
    .unwrap_or_default();

  // 6. 解析列优先 AXTree。
  let mut amounts: Vec<String> = Vec::new();
  let mut dates: Vec<String> = Vec::new();
  let mut all_texts: Vec<String> = Vec::new();
  for n in nodes {
    let nm = node_name(&n);
    if nm.is_empty() {
      continue;
    }
    let role = node_role(&n);
    if role != "StaticText" {
      continue;
    }
    all_texts.push(nm.clone());
    if nm.starts_with("US$") {
      amounts.push(nm.clone());
    } else if looks_like_chinese_date(&nm) {
      dates.push(nm.clone());
    }
  }
  rev_log(
    name,
    &format!(
      "AXTree解析: StaticText数={}, 金额数={:?}, 日期数={:?}",
      all_texts.len(),
      amounts,
      dates
    ),
  );

  // 总额 = 第一个 US$（汇总卡）；明细金额 = 之后的。
  let total = amounts.first().cloned();
  let detail_amounts: &[String] = if amounts.len() > 1 {
    &amounts[1..]
  } else {
    &[]
  };

  // index 对齐：第 i 个金额 <-> 第 i 个日期。
  let mut daily: Vec<Value> = Vec::new();
  let n = dates.len().min(detail_amounts.len());
  for i in 0..n {
    let Some(parsed) = parse_ch_date(&dates[i]) else {
      continue;
    };
    let val = detail_amounts[i]
      .replace("US$", "")
      .replace(',', "")
      .trim()
      .parse::<f64>()
      .unwrap_or(0.0);
    daily.push(json!({ "date": parsed, "income": val }));
  }

  // 关标签页 + 关连接。
  let _ = round_trip(
    &mut ws,
    5,
    None,
    "Target.closeTarget",
    json!({ "targetId": tid }),
  )
  .await;
  let _ = ws.close(None).await;
  rev_log(name, "抓取tab已关闭");

  Ok((total, daily, all_texts, rendered))
}

/// 收益深链 URL（仅 4_weeks 和 lifetime 有效，dimension=DAY 日粒度 DESC）。
fn revenue_url(cid: &str) -> String {
  format!(
    "https://studio.youtube.com/channel/{cid}/analytics/hub_revenue/explore?entity_type=CHANNEL&entity_id={cid}&time_period=lifetime&explore_type=TABLE_AND_CHART&metrics_computation_type=DELTA&metric=TOTAL_ESTIMATED_EARNINGS&granularity=DAY&t_metrics=TOTAL_ESTIMATED_EARNINGS&dimension=DAY&o_column=TOTAL_ESTIMATED_EARNINGS&o_direction=ANALYTICS_ORDER_DIRECTION_DESC"
  )
}

/// AX 节点名。
fn node_name(n: &Value) -> String {
  n.get("name")
    .and_then(|nm| nm.get("value").and_then(Value::as_str).map(str::to_string))
    .unwrap_or_default()
}

/// AX 节点角色。
fn node_role(n: &Value) -> String {
  n.get("role")
    .and_then(|r| r.get("value").and_then(Value::as_str).map(str::to_string))
    .unwrap_or_default()
}

/// 20xx年x月x日 完整格式（与 Python re.match(r"20\d\d年\d{1,2}月\d{1,2}日") 对齐）。
fn looks_like_chinese_date(s: &str) -> bool {
  let Some((y, rest)) = s.split_once('年') else {
    return false;
  };
  let Some((m, d)) = rest.split_once('月') else {
    return false;
  };
  let Some(d) = d.strip_suffix('日') else {
    return false;
  };
  y.len() == 4
    && y.chars().all(|c| c.is_ascii_digit())
    && !m.trim().is_empty()
    && !d.trim().is_empty()
    && m.trim().chars().all(|c| c.is_ascii_digit())
    && d.trim().chars().all(|c| c.is_ascii_digit())
}

/// "2026年11月12日" -> "2026-11-12"。
fn parse_ch_date(zh: &str) -> Option<String> {
  let rest = zh.strip_suffix("日")?;
  let (y, rest) = rest.split_once('年')?;
  let (m, d) = rest.split_once('月')?;
  if y.len() != 4 || !y.chars().all(|c| c.is_ascii_digit()) {
    return None;
  }
  let mo: u32 = m.trim().parse().ok()?;
  let da: u32 = d.trim().parse().ok()?;
  if !(1..=12).contains(&mo) || !(1..=31).contains(&da) {
    return None;
  }
  Some(format!("{y}-{mo:02}-{da:02}"))
}

/// 向 VPS 收益 API 发一次表单请求，返回解析出的 JSON 对象。
/// 带详细请求/响应日志（响应全文记录，请求只记参数不记凭据）。
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
      rev_log(name, &format!("VPS请求失败: {e}"));
      format!("VPS API request failed: {e}")
    })?;
  let status_code = resp.status().as_u16();
  let body = match resp.text().await {
    Ok(b) => b,
    Err(e) => {
      rev_log(name, &format!("VPS响应读取失败(HTTP {status_code}): {e}"));
      return Err(format!("read VPS API body failed: {e}"));
    }
  };
  rev_log(
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
      rev_log(name, &format!("VPS响应JSON解析失败: {e}"));
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

/// 发一条带指定 command id 的消息并等待匹配响应（session 为 None 时走 browser
/// 级命令，Some 时走 flat session 的 target 级命令）。
async fn round_trip(
  ws: &mut tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
  >,
  id: u64,
  session: Option<&str>,
  method: &str,
  params: Value,
) -> Result<Value, String> {
  use futures_util::stream::StreamExt;
  send_frame(ws, id, session, method, params).await?;
  let deadline = tokio::time::Instant::now() + AX_TIMEOUT;
  loop {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
      return Err(format!("timeout waiting for {method} response"));
    }
    let msg = tokio::time::timeout(remaining, ws.next())
      .await
      .map_err(|_| format!("timeout waiting for {method} response"))?
      .ok_or_else(|| format!("ws closed while waiting for {method}"))?;
    let text = msg
      .map_err(|e| format!("ws error: {e}"))?
      .into_text()
      .map_err(|_| "non-text message".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    if v.get("id").and_then(Value::as_u64) == Some(id) {
      if let Some(err) = v.get("error") {
        return Err(format!("{method} error: {err}"));
      }
      return Ok(v.get("result").cloned().unwrap_or(json!({})));
    }
  }
}

/// 发一条消息（flat 时带 sessionId）。
async fn send_frame(
  ws: &mut tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
  >,
  id: u64,
  session: Option<&str>,
  method: &str,
  params: Value,
) -> Result<(), String> {
  use futures_util::sink::SinkExt;
  let mut msg = json!({ "id": id, "method": method, "params": params });
  if let Some(s) = session {
    if !s.is_empty() {
      msg["sessionId"] = json!(s);
    }
  }
  ws.send(Message::Text(msg.to_string().into()))
    .await
    .map_err(|e| format!("send {method} failed: {e}"))
}
