//! AI 助手浏览器工具：驱动运行中的指纹浏览器（CDP）。
//!
//! 门禁分层：Wayfern 内核对 Runtime.* 有付费门禁（自签 token 过不了，也不该绕），
//! 所以脚本路径先探测一次（结果按配置缓存）；被拒后全链路走浏览器不设防的
//! CDP 面 —— DOM 域做快照、Input 域做点击/输入、Page 域做导航与取页面信息。

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use rig_core::completion::message::ToolName;
use rig_core::tool::{DynamicTool, ToolErrorKind, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use crate::profile::types::BrowserProfile;
use crate::wayfern_cdp::{classify_refusal, BrowserRefusal, WayfernError, WayfernSession};

/// 单次快照最多收录的可交互元素数。
const MAX_ELEMENTS: usize = 150;
/// 导航等待 load 事件的超时。
const NAVIGATE_TIMEOUT: Duration = Duration::from_secs(30);
/// 快照文本/详情的截断长度。
const TEXT_LIMIT: usize = 80;

/// 快照收录的一个可交互元素。
#[derive(Debug, Clone)]
struct SnapshotElement {
  tag: String,
  text: String,
  detail: String,
  /// DOM 域路径的节点号，点击时再取实时坐标（页面滚动也不会点偏）。
  node_id: Option<i64>,
  /// Runtime 脚本路径拿到的中心坐标（快照时刻的视口坐标）。
  point: Option<(f64, f64)>,
}

/// 最近一次 browser_snapshot 收录的元素表，browser_click / browser_type 按序号取用。
static SNAPSHOT: LazyLock<tokio::sync::Mutex<Vec<SnapshotElement>>> =
  LazyLock::new(|| tokio::sync::Mutex::new(Vec::new()));

/// AI 助手当前绑定的浏览器配置：open_account_browser 写入，其余工具读取。
static CURRENT_PROFILE: LazyLock<tokio::sync::Mutex<Option<BrowserProfile>>> =
  LazyLock::new(|| tokio::sync::Mutex::new(None));

/// Runtime.* 门禁探测结果（true = 被内核拒绝），按配置 id 缓存，命中后不再探测。
static RUNTIME_BLOCKED: LazyLock<tokio::sync::Mutex<HashMap<String, bool>>> =
  LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

fn tool_err(message: impl Into<String>) -> ToolExecutionError {
  ToolExecutionError::new(ToolErrorKind::Other, message)
}

/// 浏览器工具的结果不注入数据范围说明（SCOPE_NOTE 是云端数据口径），截断放宽到
/// 4000 字符：一页可交互元素多，2000 会把清单腰斩。
async fn browser_output(value: Value) -> Result<ToolOutput, ToolExecutionError> {
  let mut value = value;
  crate::ai_agent::truncate_json(&mut value, 4000);
  Ok(ToolOutput::json(value))
}

fn refusal_of(error: &WayfernError) -> Option<BrowserRefusal> {
  match error {
    WayfernError::Cdp(e) => classify_refusal(e),
    _ => None,
  }
}

// ============================================================
// 配置定位与启动
// ============================================================

/// 按账号名/配置名找指纹浏览器配置：id 精确 → 名称精确 → 大小写不敏感精确 → 包含。
async fn find_profile(account: &str) -> Result<BrowserProfile, String> {
  let needle = account.trim();
  if needle.is_empty() {
    return Err("账号名为空".to_string());
  }
  let profiles = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("读取浏览器配置列表失败：{e}"))?;
  let needle_lower = needle.to_lowercase();
  let candidates = [
    |p: &BrowserProfile, n: &str| p.id.to_string() == n,
    |p: &BrowserProfile, n: &str| p.name == n,
    |p: &BrowserProfile, n: &str| p.name.to_lowercase() == n,
    |p: &BrowserProfile, n: &str| p.name.to_lowercase().contains(n),
  ];
  for matches in candidates.iter() {
    if let Some(profile) = profiles.iter().find(|p| matches(p, &needle_lower)) {
      return Ok(profile.clone());
    }
  }
  Err(format!(
    "找不到名为「{needle}」的浏览器配置。请确认账号名称（可用 open_account_browser 的 account 参数传配置名或账号关键字）。"
  ))
}

/// 配置没在运行就启动一个带调试端口的窗口（走与 REST run 接口相同的启动链路）。
pub(crate) async fn ensure_running(profile: &BrowserProfile) -> Result<BrowserProfile, String> {
  if profile
    .process_id
    .is_some_and(crate::proxy_storage::is_process_running)
  {
    return Ok(profile.clone());
  }
  let app_handle = crate::ai_agent::APP_HANDLE
    .get()
    .ok_or_else(|| "AI 助手尚未初始化，无法启动浏览器".to_string())?;
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .map_err(|e| format!("分配调试端口失败：{e}"))?;
  let port = listener
    .local_addr()
    .map_err(|e| format!("分配调试端口失败：{e}"))?
    .port();
  drop(listener);
  crate::browser_runner::launch_browser_profile_impl(
    app_handle.clone(),
    profile.clone(),
    None,
    crate::browser_runner::LaunchOptions::automation(Some(port), false),
  )
  .await
}

async fn open_session(profile: &BrowserProfile) -> Result<WayfernSession, String> {
  let target = crate::cdp_target::resolve(profile).await.map_err(|e| {
    format!(
      "无法连接「{}」的浏览器（{e}）。请先调用 open_account_browser。",
      profile.name
    )
  })?;
  WayfernSession::open(&target)
    .await
    .map_err(|e| format!("连接浏览器调试端口失败：{e}"))
}

/// 探测 Runtime.* 是否被内核门禁拒绝。结果按配置缓存，一个会话只探测一次。
async fn runtime_blocked(profile_id: &str, session: &mut WayfernSession) -> Result<bool, String> {
  if let Some(blocked) = RUNTIME_BLOCKED.lock().await.get(profile_id) {
    return Ok(*blocked);
  }
  let blocked = match session
    .call(
      "Runtime.evaluate",
      json!({ "expression": "1", "returnByValue": true }),
    )
    .await
  {
    Ok(_) => false,
    Err(e) if refusal_of(&e).is_some() => true,
    Err(e) => return Err(format!("连接浏览器失败：{e}")),
  };
  RUNTIME_BLOCKED
    .lock()
    .await
    .insert(profile_id.to_string(), blocked);
  Ok(blocked)
}

/// 当前页面的 url 与标题（Page 域实测未被内核门禁拦截）。
async fn page_info(session: &mut WayfernSession) -> (String, String) {
  if let Ok(reply) = session.call("Page.getNavigationHistory", json!({})).await {
    let index = reply.get("index").and_then(Value::as_u64).unwrap_or(0);
    if let Some(entry) = reply
      .get("entries")
      .and_then(Value::as_array)
      .and_then(|entries| entries.get(index as usize))
    {
      let url = entry
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
      let title = entry
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
      return (url, title);
    }
  }
  (String::new(), String::new())
}

// ============================================================
// DOM 域快照（脚本被门禁拒绝时的实现）
// ============================================================

/// 把 CDP 的扁平属性数组（[name, value, name, value, …]）拆成键值对。
fn attr_pairs(node: &Value) -> Vec<(String, String)> {
  let mut pairs = Vec::new();
  if let Some(list) = node.get("attributes").and_then(Value::as_array) {
    let mut i = 0;
    while i + 1 < list.len() {
      pairs.push((
        list[i].as_str().unwrap_or_default().to_string(),
        list[i + 1].as_str().unwrap_or_default().to_string(),
      ));
      i += 2;
    }
  }
  pairs
}

/// 判断一个元素节点是否可交互：语义标签、可编辑、ARIA 角色或内联 onclick。
fn is_interactive(name: &str, attrs: &[(String, String)]) -> bool {
  if matches!(
    name,
    "A" | "BUTTON" | "INPUT" | "TEXTAREA" | "SELECT" | "SUMMARY"
  ) {
    return true;
  }
  let attr = |key: &str| {
    attrs
      .iter()
      .find(|(k, _)| k.eq_ignore_ascii_case(key))
      .map(|(_, v)| v.as_str())
  };
  if attr("contenteditable").is_some_and(|v| !v.is_empty() && v != "false") {
    return true;
  }
  if attr("role").is_some_and(|role| {
    matches!(
      role,
      "button"
        | "link"
        | "textbox"
        | "tab"
        | "checkbox"
        | "radio"
        | "menuitem"
        | "option"
        | "switch"
        | "searchbox"
    )
  }) {
    return true;
  }
  attr("onclick").is_some()
}

/// 深度优先收集元素内的文本节点内容（innerText 的 DOM 域等价物）。
fn collect_text(node: &Value, out: &mut String) {
  if out.chars().count() >= TEXT_LIMIT {
    return;
  }
  match node.get("nodeType").and_then(Value::as_i64).unwrap_or(0) {
    3 => out.push_str(
      node
        .get("nodeValue")
        .and_then(Value::as_str)
        .unwrap_or_default(),
    ),
    1 => {
      for key in ["children", "shadowRoots"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
          for child in children {
            collect_text(child, out);
          }
        }
      }
      if let Some(content) = node.get("content") {
        collect_text(content, out);
      }
    }
    _ => {}
  }
}

/// 遍历 DOM 树收录可交互元素（含 shadow DOM 与 template content）。
fn walk_dom(node: &Value, out: &mut Vec<SnapshotElement>) {
  if out.len() >= MAX_ELEMENTS {
    return;
  }
  if node.get("nodeType").and_then(Value::as_i64) == Some(1) {
    let name = node
      .get("nodeName")
      .and_then(Value::as_str)
      .unwrap_or_default()
      .to_uppercase();
    let attrs = attr_pairs(node);
    if is_interactive(&name, &attrs) {
      let mut text = String::new();
      collect_text(node, &mut text);
      let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
      let value = attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("value"))
        .map(|(_, v)| v.clone());
      let text = if text.is_empty() {
        value.unwrap_or_default()
      } else {
        text
      };
      let keep = [
        "id",
        "class",
        "name",
        "placeholder",
        "aria-label",
        "value",
        "href",
        "type",
        "title",
        "role",
        "contenteditable",
      ];
      let detail = attrs
        .iter()
        .filter(|(k, v)| {
          !v.is_empty()
            && keep.iter().any(|key| k.eq_ignore_ascii_case(key))
            && !(k.eq_ignore_ascii_case("contenteditable") && v == "false")
        })
        .map(|(k, v)| format!("{k}={}", v.chars().take(60).collect::<String>()))
        .collect::<Vec<_>>()
        .join(" ");
      out.push(SnapshotElement {
        tag: name.to_lowercase(),
        text: text.chars().take(TEXT_LIMIT).collect(),
        detail,
        node_id: node.get("nodeId").and_then(Value::as_i64),
        point: None,
      });
    }
  }
  for key in ["children", "shadowRoots"] {
    if let Some(children) = node.get(key).and_then(Value::as_array) {
      for child in children {
        walk_dom(child, out);
      }
    }
  }
  if let Some(content) = node.get("content") {
    walk_dom(content, out);
  }
}

/// DOM 域整树快照。
async fn snapshot_via_dom(session: &mut WayfernSession) -> Result<Vec<SnapshotElement>, String> {
  let reply = session
    .call("DOM.getDocument", json!({ "depth": -1, "pierce": true }))
    .await
    .map_err(|e| format!("读取页面 DOM 失败：{e}"))?;
  let root = reply
    .pointer("/root")
    .ok_or_else(|| "页面 DOM 返回异常".to_string())?;
  let mut elements = Vec::new();
  walk_dom(root, &mut elements);
  Ok(elements)
}

// ============================================================
// Input 域操作
// ============================================================

/// 在视口坐标 (x, y) 处模拟一次真实鼠标点击。
async fn input_click(session: &mut WayfernSession, x: f64, y: f64) -> Result<(), String> {
  let x = x.round() as i64;
  let y = y.round() as i64;
  for (event, extra) in [
    ("mouseMoved", json!({})),
    ("mousePressed", json!({ "button": "left", "clickCount": 1 })),
    (
      "mouseReleased",
      json!({ "button": "left", "clickCount": 1 }),
    ),
  ] {
    let mut params = json!({ "type": event, "x": x, "y": y });
    if let Value::Object(map) = extra {
      for (key, value) in map {
        params[key] = value;
      }
    }
    session
      .call("Input.dispatchMouseEvent", params)
      .await
      .map_err(|e| format!("模拟点击失败：{e}"))?;
  }
  Ok(())
}

/// 在当前焦点处输入文本；press_enter 时补一个回车（搜索框 / 登录表单提交）。
async fn input_type(
  session: &mut WayfernSession,
  text: &str,
  press_enter: bool,
) -> Result<(), String> {
  session
    .call("Input.insertText", json!({ "text": text }))
    .await
    .map_err(|e| format!("输入文本失败：{e}"))?;
  if press_enter {
    for event in ["keyDown", "keyUp"] {
      session
        .call(
          "Input.dispatchKeyEvent",
          json!({
            "type": event,
            "key": "Enter",
            "code": "Enter",
            "windowsVirtualKeyCode": 13,
            "nativeVirtualKeyCode": 13,
            "text": "\r",
          }),
        )
        .await
        .map_err(|e| format!("发送回车失败：{e}"))?;
    }
  }
  Ok(())
}

/// 取快照元素当前的视口坐标：Runtime 路径用快照时记下的点，DOM 路径按节点号
/// 现查盒子模型（页面滚动后坐标仍是实时的）。
async fn element_point(
  session: &mut WayfernSession,
  element: &SnapshotElement,
) -> Result<(f64, f64), String> {
  if let Some(point) = element.point {
    return Ok(point);
  }
  let node_id = element
    .node_id
    .ok_or_else(|| "元素缺少定位信息，请重新 browser_snapshot".to_string())?;
  let reply = session
    .call("DOM.getBoxModel", json!({ "nodeId": node_id }))
    .await
    .map_err(|_| "元素已失效（页面可能变化过），请重新 browser_snapshot".to_string())?;
  let mut quad: Vec<f64> = reply
    .pointer("/model/border")
    .and_then(Value::as_array)
    .map(|list| list.iter().filter_map(Value::as_f64).collect())
    .unwrap_or_default();
  quad.truncate(8);
  if quad.len() < 8 {
    return Err("元素坐标返回异常，请重新 browser_snapshot".to_string());
  }
  let x = quad.iter().step_by(2).sum::<f64>() / 4.0;
  let y = quad.iter().skip(1).step_by(2).sum::<f64>() / 4.0;
  Ok((x, y))
}

async fn current_profile() -> Result<BrowserProfile, String> {
  CURRENT_PROFILE
    .lock()
    .await
    .clone()
    .ok_or_else(|| "尚未打开浏览器，请先调用 open_account_browser".to_string())
}

/// 点击快照里序号为 `index` 的元素。
async fn click_by_index(index: usize) -> Result<String, String> {
  let profile = current_profile().await?;
  let element = {
    let snapshot = SNAPSHOT.lock().await;
    snapshot.get(index).cloned().ok_or_else(|| {
      format!(
        "序号 {index} 不在最近一次快照里（共 {} 个元素），请重新 browser_snapshot",
        snapshot.len()
      )
    })?
  };
  let mut session = open_session(&profile).await?;
  let (x, y) = element_point(&mut session, &element).await?;
  input_click(&mut session, x, y).await?;
  let (url, title) = page_info(&mut session).await;
  Ok(format!(
    "已点击 [{}] {}（{}）。页面：{title} {url}",
    index, element.tag, element.text
  ))
}

// ============================================================
// 工具
// ============================================================

pub(crate) fn tool_open_account_browser() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("open_account_browser").expect("tool name is static and valid"),
    "打开（或连接已在运行的）指定账号的指纹浏览器窗口。account 填配置名或账号关键字（如 721040@qq.com）。要用浏览器发文、操作网页前必须先调用它。返回当前页面地址与标题。",
    json!({
        "type": "object",
        "properties": {
            "account": { "type": "string", "description": "账号名或浏览器配置名的关键字" }
        },
        "required": ["account"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let account = arg_str(&args, "account").ok_or_else(|| tool_err("缺少 account 参数"))?;
        let profile = find_profile(&account).await.map_err(tool_err)?;
        let profile = ensure_running(&profile).await.map_err(tool_err)?;
        let mut session = open_session(&profile).await.map_err(tool_err)?;
        let blocked = runtime_blocked(&profile.id.to_string(), &mut session)
          .await
          .map_err(tool_err)?;
        let (url, title) = page_info(&mut session).await;
        SNAPSHOT.lock().await.clear();
        *CURRENT_PROFILE.lock().await = Some(profile.clone());
        browser_output(json!({
            "profile": profile.name,
            "running": true,
            "script_mode": if blocked { "dom（Runtime 被内核门禁拦截，走 DOM/Input 路径）" } else { "script（Runtime 可用）" },
            "url": url,
            "title": title,
            "hint": "浏览器已就绪。用 browser_snapshot 获取可交互元素清单，再用 browser_click / browser_type 操作。",
        }))
        .await
      })
    },
  )
}

pub(crate) fn tool_browser_navigate() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("browser_navigate").expect("tool name is static and valid"),
    "在已打开的指纹浏览器当前标签页跳转到指定网址，等待页面加载后返回新页面的标题与地址。",
    json!({
        "type": "object",
        "properties": {
            "url": { "type": "string", "description": "目标网址（含 https:// 前缀）" }
        },
        "required": ["url"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let url = arg_str(&args, "url").ok_or_else(|| tool_err("缺少 url 参数"))?;
        let profile = current_profile().await.map_err(tool_err)?;
        let mut session = open_session(&profile).await.map_err(tool_err)?;
        let (reply, loaded) = session
          .call_then_await_event(
            "Page.navigate",
            json!({ "url": url }),
            "Page.loadEventFired",
            NAVIGATE_TIMEOUT,
          )
          .await
          .map_err(|e| tool_err(format!("导航失败：{e}")))?;
        if let Some(error_text) = reply.get("errorText").and_then(Value::as_str) {
          return Err(tool_err(format!("导航失败：{error_text}")));
        }
        let (final_url, title) = page_info(&mut session).await;
        SNAPSHOT.lock().await.clear();
        browser_output(json!({
            "url": final_url,
            "title": title,
            "load_completed": loaded,
            "hint": if loaded { "页面已加载。用 browser_snapshot 获取可交互元素清单。" } else { "加载超时（页面可能仍在加载），可稍后用 browser_snapshot 查看当前元素。" },
        }))
        .await
      })
    },
  )
}

pub(crate) fn tool_browser_snapshot() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("browser_snapshot").expect("tool name is static and valid"),
    "截取浏览器当前页面的可交互元素清单（链接、按钮、输入框、可编辑区等），每个元素带序号 index，browser_click / browser_type 用 index 指定元素。页面跳转或点击后元素序号会变，操作前要重新截取。",
    json!({ "type": "object", "properties": {} }),
    move |_args: Value| {
      Box::pin(async move {
        let profile = current_profile().await.map_err(tool_err)?;
        let mut session = open_session(&profile).await.map_err(tool_err)?;
        let blocked = runtime_blocked(&profile.id.to_string(), &mut session)
          .await
          .map_err(tool_err)?;
        let elements = if blocked {
          snapshot_via_dom(&mut session).await.map_err(tool_err)?
        } else {
          snapshot_via_script(&mut session).await.map_err(tool_err)?
        };
        let (url, title) = page_info(&mut session).await;
        let listing: Vec<Value> = elements
          .iter()
          .enumerate()
          .map(|(i, e)| {
            json!({
                "index": i,
                "tag": e.tag,
                "text": e.text,
                "detail": e.detail,
            })
          })
          .collect();
        let count = listing.len();
        *SNAPSHOT.lock().await = elements;
        browser_output(json!({
            "url": url,
            "title": title,
            "count": count,
            "elements": listing,
        }))
        .await
      })
    },
  )
}

/// Runtime 可用时的整页脚本快照：一次调用带回元素与实时坐标。
async fn snapshot_via_script(session: &mut WayfernSession) -> Result<Vec<SnapshotElement>, String> {
  let script = "JSON.stringify({elements:(function(){var out=[];var sel='a,button,input,textarea,select,[contenteditable],[role=button],[role=link],[role=textbox],[role=tab],[role=checkbox],[role=menuitem],[onclick]';var nodes=document.querySelectorAll(sel);for(var i=0;i<nodes.length&&out.length<150;i++){var el=nodes[i];if(el.getAttribute('contenteditable')==='false')continue;var r=el.getBoundingClientRect();if(r.width<=0||r.height<=0)continue;var txt=(el.innerText||el.value||el.placeholder||el.getAttribute('aria-label')||'').trim().replace(/\\s+/g,' ').slice(0,80);var cls=''+el.className;out.push({tag:el.tagName.toLowerCase(),text:txt,id:el.id||'',cls:cls.slice(0,40),href:(el.getAttribute('href')||'').slice(0,60),ph:el.placeholder||'',role:el.getAttribute('role')||'',type:el.getAttribute('type')||'',x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)})}return out})()})";
  let reply = session
    .call(
      "Runtime.evaluate",
      json!({ "expression": script, "returnByValue": true }),
    )
    .await
    .map_err(|e| format!("页面脚本执行失败：{e}"))?;
  if let Some(exception) = reply.get("exceptionDetails") {
    let message = exception
      .get("text")
      .and_then(Value::as_str)
      .unwrap_or("页面脚本执行出错");
    return Err(format!("页面脚本执行出错：{message}"));
  }
  let text = reply
    .pointer("/result/value")
    .and_then(Value::as_str)
    .ok_or_else(|| "页面脚本返回异常".to_string())?;
  let parsed: Value = serde_json::from_str(text).map_err(|e| format!("页面脚本返回异常：{e}"))?;
  let mut elements = Vec::new();
  for item in parsed
    .get("elements")
    .and_then(Value::as_array)
    .cloned()
    .unwrap_or_default()
  {
    let attr = |key: &str| {
      item
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
    };
    let detail = ["id", "cls", "href", "ph", "role", "type"]
      .iter()
      .map(|key| (key.to_string(), attr(key)))
      .filter(|(_, v)| !v.is_empty())
      .map(|(k, v)| format!("{k}={v}"))
      .collect::<Vec<_>>()
      .join(" ");
    elements.push(SnapshotElement {
      tag: attr("tag"),
      text: attr("text"),
      detail,
      node_id: None,
      point: Some((
        item.get("x").and_then(Value::as_f64).unwrap_or_default(),
        item.get("y").and_then(Value::as_f64).unwrap_or_default(),
      )),
    });
  }
  Ok(elements)
}

pub(crate) fn tool_browser_click() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("browser_click").expect("tool name is static and valid"),
    "点击页面上指定序号的元素（序号来自最近一次 browser_snapshot）。点击后页面可能变化，建议重新 snapshot 确认结果。",
    json!({
        "type": "object",
        "properties": {
            "index": { "type": "integer", "description": "browser_snapshot 返回的元素序号" }
        },
        "required": ["index"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let index = args
          .get("index")
          .and_then(Value::as_u64)
          .ok_or_else(|| tool_err("缺少 index 参数"))? as usize;
        let summary = click_by_index(index).await.map_err(tool_err)?;
        browser_output(json!({ "result": summary })).await
      })
    },
  )
}

pub(crate) fn tool_browser_type() -> DynamicTool {
  DynamicTool::new(
    ToolName::new("browser_type").expect("tool name is static and valid"),
    "在页面元素里输入文本：先点击序号 index 指定的输入框聚焦，再逐字输入（对富文本编辑器同样有效）。index 省略时在当前焦点处输入。press_enter 为 true 时输入完补一个回车（用于提交搜索或登录表单）。",
    json!({
        "type": "object",
        "properties": {
            "index": { "type": "integer", "description": "browser_snapshot 返回的输入框序号（可空，空则用当前焦点）" },
            "text": { "type": "string", "description": "要输入的文本" },
            "press_enter": { "type": "boolean", "description": "输入后是否按回车（可空，默认否）" }
        },
        "required": ["text"]
    }),
    move |args: Value| {
      Box::pin(async move {
        let text = arg_str(&args, "text").ok_or_else(|| tool_err("缺少 text 参数"))?;
        let press_enter = args.get("press_enter").and_then(Value::as_bool).unwrap_or(false);
        let profile = current_profile().await.map_err(tool_err)?;
        let mut session = open_session(&profile).await.map_err(tool_err)?;
        if let Some(index) = args.get("index").and_then(Value::as_u64) {
          let element = {
            let snapshot = SNAPSHOT.lock().await;
            snapshot.get(index as usize).cloned().ok_or_else(|| {
              tool_err(format!(
                "序号 {index} 不在最近一次快照里（共 {} 个元素），请重新 browser_snapshot",
                snapshot.len()
              ))
            })?
          };
          let (x, y) = element_point(&mut session, &element).await.map_err(tool_err)?;
          input_click(&mut session, x, y).await.map_err(tool_err)?;
        }
        input_type(&mut session, &text, press_enter)
          .await
          .map_err(tool_err)?;
        browser_output(json!({
            "result": format!("已输入 {} 个字符{}", text.chars().count(), if press_enter { "并回车" } else { "" }),
        }))
        .await
      })
    },
  )
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
  args.get(key).and_then(Value::as_str).map(str::to_string)
}
