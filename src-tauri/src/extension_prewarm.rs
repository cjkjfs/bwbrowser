//! 扩展 popup 预热（治本方案）。
//!
//! 背景：`--load-extension` 装载的 unpacked 扩展没有 V8 code cache。Chromium
//! 冷启动时会对每个 JS（尤其 popup 主包，如达人精灵 `assets/index.js` 3.6MB、
//! SpeakText `umi.js` 1.8MB）重新读取并完整解析，而解析结果不落盘。用户点击
//! 工具栏图标打开 popup 时才首次执行这段解析 —— 这就是「加载慢、等了半天才弹
//! 出」的直接来源，跟 Defender 实扫放大 IO 是两回事。
//!
//! 预热思路：启动成功后，后台静默把一个扩展的 popup 页面当成普通标签页打开
//! 一次，让 Chromium 把大 JS 解析进 V8 code cache。用户之后点击图标看到的
//! 「第二次解析」就快得多。关键点：
//!   - 打开的是 popup URL（`chrome-extension://<id>/<popup>`），就是用户点图
//!     标会加载的那份 JS，所以写得进同一个解析缓存。
//!   - 全后台，失败只记日志，绝不影响浏览器启动。
//!
//! 为什么用运行时拿 real_id：unpacked 扩展的 id 是装载路径经 SHA256 哈希推导
//! 的（前 16 字节映射到 a-p），与目录名/UUID 无关，且该算法未在本项目复现。
//! 最可靠的是让 Chromium 自己告诉我们：`/json` 里每个扩展的 service worker 或
//! 页面 target 的 URL 都以 `chrome-extension://<real_id>/...` 开头，运行时直接
//! 解析即可，不依赖任何离线推导。
//!
//! CDP 走 BROWSER 级 socket（`/json/version` 的 `webSocketDebuggerUrl`）：创建/
//! 关闭整个 target 只有 browser 端点实现，页面端点答 "not found"。`Runtime.*`
//! 类命令被本内核的自动化闸门拦截（code -32000），但 `Target.createTarget`、
//! `Target.attachToTarget`、`Page.enable`、`Target.closeTarget` 均可用。

use crate::profile::types::BrowserProfile;
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

/// 单个扩展一口气能等的加载时长上限。大 JS（秒级解析）远快于此，超时只说明这
/// 个 popup 没触发加载事件（例如真 404），静默放弃即可。
const POPUP_LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// 启动后等多久再开始预热。给 cookie 注入、首次导航留出窗口，也刚好错过启动时
/// 那段 CDP 被反复占用（setFingerprint / 指纹应用）的阶段。
const PREWARM_START_DELAY: Duration = Duration::from_secs(5);

/// 预热是给「有 popup 的扩展」用的。无 popup（如视频嗅探，action 为空）解析量
/// 极小，打开也白费，直接跳过。
pub fn should_prewarm(manifest: &Value) -> bool {
  let action = manifest
    .get("action")
    .or_else(|| manifest.get("browser_action"));
  action
    .and_then(|a| a.get("default_popup"))
    .and_then(Value::as_str)
    .map(|s| !s.trim().is_empty())
    .unwrap_or(false)
}

/// 后台启动一个 profile 的扩展 popup 预热。
///
/// 在 `launch_browser_internal` 的 Wayfern 成功分支里调用；`extension_paths` 是该
/// 次启动实际 `--load-extension` 装载的扩展目录列表（含嗅探/分组/营收盾）。
pub fn spawn_extension_prewarm(profile: &BrowserProfile, extension_paths: &[String]) {
  // 一个扩展都没有（或全无 popup）就不起任务，省一次后台轮询。
  let any_popup = extension_paths
    .iter()
    .any(|p| read_manifest(p).is_ok_and(|m| should_prewarm(&m)));
  if !any_popup {
    return;
  }

  let profile = profile.clone();
  let paths = extension_paths.to_vec();
  tauri::async_runtime::spawn(async move {
    tokio::time::sleep(PREWARM_START_DELAY).await;
    if let Err(e) = prewarm(&profile, &paths).await {
      log::warn!("extension_prewarm: {e}");
    }
  });
}

/// 逐个装载扩展的 popup 页面一次，让 V8 解析大 JS 写 code cache。
async fn prewarm(profile: &BrowserProfile, extension_paths: &[String]) -> Result<(), String> {
  // 浏览器可能还没把端口就绪（后台延迟已掩盖大部分），拿不到就安静退出。
  let Some(ws_url) = crate::cdp_target::local_browser_ws_url(profile).await else {
    log::info!("extension_prewarm: browser not CDP-ready, skipping prewarm");
    return Ok(());
  };

  // 从 /json 收集所有已装载扩展的 real_id，以及 SW 路径 → id 的配对表。
  let inventory = collect_extension_inventory(profile).await?;
  if inventory.real_ids.is_empty() {
    log::info!("extension_prewarm: no chrome-extension targets yet, skipping");
    return Ok(());
  }

  let mut opened = 0usize;
  for dir in extension_paths {
    let manifest = read_manifest(dir)?;
    if !should_prewarm(&manifest) {
      continue;
    }
    let popup = manifest
      .get("action")
      .or_else(|| manifest.get("browser_action"))
      .and_then(|a| a.get("default_popup"))
      .and_then(Value::as_str)
      .unwrap_or("");

    // 优先用 SW 路径单点配对：target 很少时最干净。撞名或拿不到时退回「逐个
    // real_id 试」。错配只是打开一个 404 页面（忍一下再关），没有任何损害。
    let sw = manifest
      .get("background")
      .and_then(|b| b.get("service_worker"))
      .and_then(Value::as_str)
      .unwrap_or("");
    let mut ids: Vec<&str> = if sw.is_empty() {
      inventory.real_ids.iter().map(|s| s.as_str()).collect()
    } else {
      let matched = inventory.sw_id_by_path(sw);
      if matched.len() == 1 {
        vec![matched[0]]
      } else {
        inventory.real_ids.iter().map(|s| s.as_str()).collect()
      }
    };
    // 去重，避免同一 id 重复开。
    ids.sort_unstable();
    ids.dedup();

    for id in &ids {
      let url = format!(
        "chrome-extension://{}/{}",
        id,
        popup.trim_start_matches('/')
      );
      match open_popup_once(&ws_url, &url).await {
        Ok(()) => {
          opened += 1;
          crate::bwbrowser_cloud::log_bwbrowser(
            "extension_prewarm",
            &format!("已预热扩展 popup: {dir} → {url}"),
          );
          // 命中一个真实 id 就够了（sw 配准时唯一；逐试兜底时第一个能用即停）。
          break;
        }
        Err(e) => log::debug!("extension_prewarm: open {url} failed: {e} (trying next candidate)"),
      }
    }
  }

  log::info!(
    "extension_prewarm: prewarmed {} extension popup(s) for profile '{}'",
    opened,
    profile.name
  );
  Ok(())
}

/// `/json` 里一个扩展的 id 与其 url path（service worker 相对路径）。
struct ExtensionInventory {
  real_ids: Vec<String>,
  /// `<sw_path> → Vec<id>`。多个扩展共享同名 SW 文件（如都是 background.js）
  /// 时会有多个 id，调用方据此决定单点或兜底。
  by_path: Vec<(String, String)>,
}

impl ExtensionInventory {
  fn sw_id_by_path(&self, sw: &str) -> Vec<&str> {
    self
      .by_path
      .iter()
      .filter(|(path, _)| path == sw)
      .map(|(_, id)| id.as_str())
      .collect()
  }
}

/// 收集一个运行中浏览器当前已装载的扩展 id 与它们的 SW 路径。
async fn collect_extension_inventory(
  profile: &BrowserProfile,
) -> Result<ExtensionInventory, String> {
  let port = crate::cdp_target::local_cdp_port_for(profile)
    .await
    .ok_or_else(|| "no local CDP port".to_string())?;
  let client = reqwest::Client::new();
  let resp = client
    .get(format!("http://127.0.0.1:{port}/json"))
    .timeout(Duration::from_secs(5))
    .send()
    .await
    .map_err(|e| format!("GET /json failed: {e}"))?;
  let targets: Vec<Value> = resp
    .json()
    .await
    .map_err(|e| format!("parse /json failed: {e}"))?;

  let mut real_ids = Vec::new();
  let mut by_path = Vec::new();
  for t in &targets {
    let url = t.get("url").and_then(Value::as_str).unwrap_or("");
    let Some(rest) = url.strip_prefix("chrome-extension://") else {
      continue;
    };
    // rest = "<id>/<path>" 或 "<id>/_generated_background_page.html"。
    let (id, path) = rest.split_once('/').unwrap_or((rest, ""));
    if id.is_empty() || id.len() > 64 {
      continue;
    }
    if !real_ids.iter().any(|s| s == id) {
      real_ids.push(id.to_string());
    }
    // 记录 SW 路径配对；页面 target 的 path 是导航产生的，不参与配对。
    if t.get("type").and_then(Value::as_str) == Some("service_worker") && !path.is_empty() {
      by_path.push((path.to_string(), id.to_string()));
    }
  }
  Ok(ExtensionInventory { real_ids, by_path })
}

fn read_manifest(dir: &str) -> Result<Value, String> {
  let manifest_path = std::path::Path::new(dir).join("manifest.json");
  let bytes = std::fs::read(&manifest_path)
    .map_err(|e| format!("read {} failed: {e}", manifest_path.display()))?;
  serde_json::from_slice(&bytes)
    .map_err(|e| format!("parse {} failed: {e}", manifest_path.display()))
}

/// 通过 BROWSER 级 CDP socket 打开一个 popup 页面并等它加载完。
async fn open_popup_once(ws_url: &str, url: &str) -> Result<(), String> {
  // `ws_url` 已经是 /json/version 的 browser 级 socket；browser 可能恰好在启动
  // 尾巴上正忙，重试几次观察的握手。
  let mut last_err = String::from("attempted");
  for attempt in 0..3 {
    if attempt > 0 {
      tokio::time::sleep(Duration::from_millis(300)).await;
    }
    match drive_one_popup(ws_url, url).await {
      Ok(()) => return Ok(()),
      Err(e) => last_err = e,
    }
  }
  Err(format!("all attempts failed: {last_err}"))
}

/// 在一条 browser 连接上完整走一遍预热：独立窗口开空白页 → 最小化整窗 →
/// 导航到 popup → 等 load → 关窗（target 关闭时窗口自动消失）。
///
/// 为什么不用 `Target.createTarget {background:true}`：该参数 Chromium 至今
/// 未实现（CDP 文档标注 not implemented yet），扩展页照样出现在标签栏里，
/// 用户每启动一个账号就看到两个「加载插件」的 tab 弹出。改成独立窗口路线：
/// about:blank 新窗口几乎瞬时创建，趁它还没渲染任何内容先 `Browser.setWindowBounds`
/// 压成最小化，再 `Page.navigate` 装载 popup 大 JS —— 解析全程都发生在最小化
/// 窗口里，任务栏只闪一个图标，标签栏永远干净。
async fn drive_one_popup(browser_ws: &str, url: &str) -> Result<(), String> {
  let request = browser_ws
    .into_client_request()
    .map_err(|e| format!("build ws request failed: {e}"))?;
  let config = WebSocketConfig::default()
    .max_message_size(Some(16 * 1024 * 1024))
    .max_frame_size(Some(16 * 1024 * 1024));
  let (mut ws, _) = tokio_tungstenite::connect_async_with_config(request, Some(config), false)
    .await
    .map_err(|e| format!("ws connect failed: {e}"))?;

  // 1. 独立窗口开一个空白 tab（内容为空，创建几乎瞬时，不装任何 JS）。
  let created = round_trip(
    &mut ws,
    1,
    None,
    "Target.createTarget",
    json!({
      "url": "about:blank",
      "newWindow": true,
    }),
  )
  .await?;
  let target_id = created
    .get("targetId")
    .and_then(Value::as_str)
    .ok_or_else(|| "createTarget returned no targetId".to_string())?
    .to_string();

  // 2. 赶在内容渲染前把整个窗口最小化。Browser.* 若被闸门拦/失败，只是退化
  //    为窗口可见几秒（旧行为），预热本身不受影响。
  if let Ok(win) = round_trip(
    &mut ws,
    2,
    None,
    "Browser.getWindowForTarget",
    json!({ "targetId": target_id }),
  )
  .await
  {
    if let Some(window_id) = win.get("windowId").cloned() {
      let _ = round_trip(
        &mut ws,
        3,
        None,
        "Browser.setWindowBounds",
        json!({
          "windowId": window_id,
          "bounds": { "windowState": "minimized" },
        }),
      )
      .await;
    }
  }

  // 3. flat attach，得到 session，再开 Page 事件。
  let attached = round_trip(
    &mut ws,
    4,
    None,
    "Target.attachToTarget",
    json!({
      "targetId": target_id,
      "flatten": true,
    }),
  )
  .await?;
  let session = attached
    .get("sessionId")
    .and_then(Value::as_str)
    .ok_or_else(|| "attachToTarget returned no sessionId".to_string())?
    .to_string();
  let _ = send_frame(&mut ws, 5, Some(&session), "Page.enable", json!({})).await;

  // 4. 窗口已最小化，此刻才装载 popup 页面 —— 重 JS 解析全程不可见。
  let _ = round_trip(
    &mut ws,
    6,
    Some(&session),
    "Page.navigate",
    json!({ "url": url }),
  )
  .await?;

  // 5. 等在 loadEventFired；同时吞掉其它事件/命令回包。
  let load_result = wait_for_load(&mut ws, &session, POPUP_LOAD_TIMEOUT).await;

  // 6. 关掉 target：窗口里唯一的 tab 没了，最小化窗口也随之自动消失。
  let _ = send_frame(&mut ws, 7, Some(&session), "Page.disable", json!({})).await;
  let _ = round_trip(
    &mut ws,
    8,
    None,
    "Target.closeTarget",
    json!({ "targetId": target_id }),
  )
  .await;
  let _ = ws.close(None).await;
  load_result
}

/// 发一条带指定 command id 的消息并返回匹配该 id 的响应结果。
///
/// `session` 为 `Some` 时消息走 flat session（Page.* 等 target 级命令），
/// `None` 时直接发在 browser 连接上（Target.createTarget/closeTarget、
/// Browser.* 等 browser 级命令）。
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
  let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
  loop {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
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

/// 发一条消息（flat 时带 sessionId），不等待回复。
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

/// 轮询读事件直到该 session 的 `Page.loadEventFired` 到达（或超时）。
async fn wait_for_load(
  ws: &mut tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
  >,
  session: &str,
  timeout: Duration,
) -> Result<(), String> {
  use futures_util::stream::StreamExt;
  let deadline = tokio::time::Instant::now() + timeout;
  loop {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
      return Err("timed out waiting for popup loadEventFired".to_string());
    }
    let msg = tokio::time::timeout(remaining, ws.next())
      .await
      .map_err(|_| "timed out waiting for popup load".to_string())?
      .ok_or_else(|| "ws closed while prewarming popup".to_string())?;
    let text = msg
      .map_err(|e| e.to_string())?
      .into_text()
      .map_err(|_| "non-text message".to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    if v.get("method").and_then(Value::as_str) == Some("Page.loadEventFired") {
      // flat 时事件自带 sessionId，确认是本 session 的（其它页面导航也会发）。
      if v.get("sessionId").and_then(Value::as_str) == Some(session) {
        return Ok(());
      }
    }
  }
}
