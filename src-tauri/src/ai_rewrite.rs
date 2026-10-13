//! ai_rewrite.rs — 本地 AI 洗稿（网关客户端 + 流水线）
//!
//! 分工边界（与服务器 `tk/ai_rewrite_api.php` 约定）：
//!   本地程序：下载(yt-dlp) → 抽音频 → ASR → 爆点分析 → 洗稿 → 打分，全部本地完成
//!   服务器  ：鉴权 / 下发密钥与提示词 / 链接去重 / 入库 / 用量计费
//!
//! 凭证复用：本地已保存的爆文浏览器云端账号（username/password），
//! 用它向网关换取 `ai_client_tokens` 令牌，后续请求带 X-Auth-Token。
//!
//! 自动复用：处理前先 `check`，命中历史转写/结果时直接复用，不再重复下载与调用 API。

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

use crate::bwbrowser_cloud::BWBROWSER_AUTH;
use crate::cloud_domain::{AI_AGENT_CRON_API_URL, AI_REWRITE_API_URL};
use crate::xray_worker_storage::XrayWorkerConfig;

// ============================================================
// 类型
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiProxyConfig {
  #[serde(default)]
  pub host: String,
  #[serde(default, rename = "type")]
  pub kind: String,
  #[serde(default)]
  pub user: String,
  #[serde(default)]
  pub pass: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiEndpointConfig {
  #[serde(default)]
  pub provider: String,
  #[serde(default)]
  pub base_url: String,
  #[serde(default)]
  pub api_key: String,
  #[serde(default)]
  pub model: String,
  #[serde(default)]
  pub proxy: AiProxyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiPrompts {
  #[serde(default)]
  pub analyze: String,
  #[serde(default)]
  pub rewrite: String,
  #[serde(default)]
  pub score: String,
  #[serde(default)]
  pub translate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiCredentials {
  #[serde(default)]
  pub asr: AiEndpointConfig,
  #[serde(default)]
  pub llm: AiEndpointConfig,
  #[serde(default)]
  pub translate: AiEndpointConfig,
  #[serde(default)]
  pub prompts: AiPrompts,
}

/// 可编辑的接口配置（与服务器后台「AI 接口配置」同一份数据，双向同步）。
/// 本程序与云端后台都写服务器 ai_rewrite_config 表，谁最后保存谁生效。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AiApiConfig {
  #[serde(default)]
  pub asr: AiEndpointConfig,
  #[serde(default)]
  pub llm: AiEndpointConfig,
  #[serde(default)]
  pub translate: AiEndpointConfig,
}

// ============================================================
// 管理器
// ============================================================

pub struct AiRewriteManager {
  client: Client,
  token: Mutex<Option<String>>,
}

lazy_static::lazy_static! {
  pub static ref AI_REWRITE: AiRewriteManager = AiRewriteManager::new();
  /// 运行中/刚结束任务的实时状态（task_key -> 最新 progress payload + _updated_at）。
  /// AI 助手的 query_running_tasks 工具读这里；前端任务面板继续走事件流。
  static ref RUNNING_TASKS: tokio::sync::Mutex<std::collections::HashMap<String, Value>> =
    tokio::sync::Mutex::new(std::collections::HashMap::new());
}

/// 终态条目在表里的最长保留时间：超过后写入时顺带清洗，防表无限膨胀。
const RUNNING_TASKS_TTL_SECS: u64 = 2 * 60 * 60;

/// 把一条 progress 上报记入运行表（emit 与助手查询共用的真相源）。
async fn track_running(payload: Value) {
  let mut map = RUNNING_TASKS.lock().await;
  let now = crate::proxy_manager::now_secs();
  // 清洗超时终态条目。
  let stale: Vec<String> = map
    .iter()
    .filter(|(_, v)| {
      let stage = v.get("stage").and_then(|s| s.as_str()).unwrap_or("");
      matches!(stage, "done" | "error")
        && v.get("_updated_at").and_then(|t| t.as_u64()).unwrap_or(0) + RUNNING_TASKS_TTL_SECS < now
    })
    .map(|(k, _)| k.clone())
    .collect();
  for k in stale {
    map.remove(&k);
  }
  let key = payload
    .get("task_key")
    .and_then(|k| k.as_str())
    .unwrap_or("")
    .to_string();
  if key.is_empty() {
    return;
  }
  let mut entry = payload;
  if let Value::Object(ref mut m) = entry {
    m.insert("_updated_at".to_string(), json!(now));
  }
  map.insert(key, entry);
}

/// 运行表快照（AI 助手的进度查询工具）。
#[tauri::command]
pub async fn ai_rewrite_running_tasks() -> Result<Value, String> {
  let map = RUNNING_TASKS.lock().await;
  let mut tasks: Vec<Value> = map.values().cloned().collect();
  drop(map);
  tasks.sort_by_key(|v| v.get("_updated_at").and_then(|t| t.as_u64()).unwrap_or(0));
  Ok(json!({ "success": true, "tasks": tasks }))
}

lazy_static::lazy_static! {
  /// AI 洗稿已启动的 Xray 侧车线路（URI → worker），按 URI 复用。
  static ref XRAY_WORKERS: Mutex<HashMap<String, XrayWorkerConfig>> =
    Mutex::new(HashMap::new());
}

impl Default for AiRewriteManager {
  fn default() -> Self {
    Self::new()
  }
}

fn truncate(s: &str, max: usize) -> String {
  let s = s.trim();
  if s.chars().count() <= max {
    return s.to_string();
  }
  let head: String = s.chars().take(max).collect();
  format!("{head}…")
}

/// 代理解析结果：直连 / 普通代理（reqwest 直用）/ 高级协议（Xray 侧车转本地 socks5）。
#[derive(Debug, PartialEq)]
enum ProxyTarget {
  Direct,
  Plain { url: String },
  Xray { uri: String },
}

/// 解析代理配置。host 为空表示直连；
/// vless/trojan 的 host 存完整 URI（与代理中心一致），经 Xray 侧车中转；
/// 其余为 host:port（或带认证的 host:port），由 reqwest 直连。
fn classify_proxy(p: &AiProxyConfig) -> ProxyTarget {
  let host = p.host.trim();
  if host.is_empty() {
    return ProxyTarget::Direct;
  }
  let kind = p.kind.trim().to_lowercase();
  let uri = if host.starts_with("vless://") || host.starts_with("trojan://") {
    Some(host.to_string())
  } else if kind == "vless" || kind == "trojan" {
    Some(format!("{kind}://{host}"))
  } else {
    None
  };
  if let Some(uri) = uri {
    return ProxyTarget::Xray { uri };
  }
  let scheme = match kind.as_str() {
    "socks5" | "socks" => "socks5",
    _ => "http",
  };
  let auth = if p.user.trim().is_empty() {
    String::new()
  } else if p.pass.is_empty() {
    format!("{}@", p.user)
  } else {
    format!("{}:{}@", p.user, p.pass)
  };
  ProxyTarget::Plain {
    url: format!("{scheme}://{auth}{host}"),
  }
}

/// 取 URI 对应的 Xray 侧车线路，没有则启动。
/// 命中缓存先做 SOCKS 握手存活检查，失联则停掉重建；侧车带 owner 看门狗，程序退出自灭。
async fn xray_worker_for(uri: &str) -> Result<XrayWorkerConfig, String> {
  let mut pool = XRAY_WORKERS.lock().await;
  if let Some(w) = pool.get(uri) {
    if crate::xray_worker_runner::authenticated_socks_ready(w).await {
      return Ok(w.clone());
    }
    log::warn!(
      "AI 洗稿 Xray 侧车线路已失联，正在重启（端口 {}）",
      w.local_port
    );
    let _ = crate::xray_worker_runner::stop_xray_worker(&w.id).await;
    pool.remove(uri);
  }
  let worker = crate::xray_worker_runner::start_xray_worker(None, uri)
    .await
    .map_err(|e| format!("Xray 代理线路启动失败：{e}"))?;
  log::info!("AI 洗稿 Xray 侧车线路就绪（端口 {}）", worker.local_port);
  pool.insert(uri.to_string(), worker.clone());
  Ok(worker)
}

/// 把代理配置解析成 reqwest 可用的代理 URL；直连返回 None。
pub(crate) async fn resolve_proxy_url(p: &AiProxyConfig) -> Result<Option<String>, String> {
  match classify_proxy(p) {
    ProxyTarget::Direct => Ok(None),
    ProxyTarget::Plain { url } => Ok(Some(url)),
    ProxyTarget::Xray { uri } => {
      let w = xray_worker_for(&uri).await?;
      Ok(Some(format!(
        "socks5://{}:{}@127.0.0.1:{}",
        w.username, w.password, w.local_port
      )))
    }
  }
}

impl AiRewriteManager {
  pub fn new() -> Self {
    let client = Client::builder()
      .timeout(std::time::Duration::from_secs(60))
      .connect_timeout(std::time::Duration::from_secs(10))
      .build()
      .unwrap_or_else(|_| Client::new());
    let token = Self::load_token();
    Self {
      client,
      token: Mutex::new(token),
    }
  }

  // --- 令牌持久化 ---

  fn token_file() -> PathBuf {
    crate::settings_manager::SettingsManager::instance()
      .get_settings_dir()
      .join("ai_rewrite_token.json")
  }

  fn load_token() -> Option<String> {
    let path = Self::token_file();
    let content = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    v.get("token")
      .and_then(|t| t.as_str())
      .filter(|t| !t.is_empty())
      .map(|t| t.to_string())
  }

  fn save_token(token: &str) {
    let path = Self::token_file();
    if let Some(parent) = path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    let body = json!({ "token": token, "saved_at": chrono::Local::now().to_rfc3339() });
    let _ = std::fs::write(path, body.to_string());
  }

  // --- 网关调用 ---

  /// 原始请求：不校验 success，返回服务器 JSON（用于 login 等需要读 message 的场景）。
  async fn call_raw(&self, body: Value) -> Result<Value, String> {
    self.post_json(AI_REWRITE_API_URL, body).await
  }

  async fn post_json(&self, url: &str, body: Value) -> Result<Value, String> {
    let token = self.token.lock().await.clone();
    let mut req = self.client.post(url).json(&body);
    if let Some(t) = token {
      req = req.header("X-Auth-Token", t);
    }
    let resp = req
      .send()
      .await
      .map_err(|e| format!("连接服务器失败：{e}"))?;
    let status = resp.status();
    let text = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败：{e}"))?;
    serde_json::from_str::<Value>(&text).map_err(|_| {
      format!(
        "服务器响应异常（HTTP {}）：{}",
        status.as_u16(),
        truncate(&text, 200)
      )
    })
  }

  /// 标准调用：action 必填，success!=true 时返回 message 作为错误。
  async fn call_to(&self, url: &str, action: &str, params: Value) -> Result<Value, String> {
    let mut map = match params {
      Value::Object(m) => m,
      _ => serde_json::Map::new(),
    };
    map.insert("action".to_string(), json!(action));
    let v = self.post_json(url, Value::Object(map)).await?;
    if v.get("success").and_then(|b| b.as_bool()) != Some(true) {
      let msg = v
        .get("message")
        .and_then(|s| s.as_str())
        .unwrap_or("请求失败");
      return Err(msg.to_string());
    }
    Ok(v)
  }

  /// 用本地保存的云端账号密码登录网关，换取令牌。
  async fn login(&self) -> Result<Value, String> {
    let (username, password) = BWBROWSER_AUTH
      .get_credentials()
      .ok_or_else(|| "请先在「爆文浏览器」登录云端账号".to_string())?;
    let v = self
      .call_raw(json!({
          "action": "login",
          "username": username,
          "password": password,
          "device": "desktop-local",
      }))
      .await?;
    if v.get("success").and_then(|b| b.as_bool()) != Some(true) {
      let msg = v
        .get("message")
        .and_then(|s| s.as_str())
        .unwrap_or("登录失败");
      return Err(msg.to_string());
    }
    let token = v
      .get("token")
      .and_then(|t| t.as_str())
      .ok_or("服务器未返回 token")?
      .to_string();
    *self.token.lock().await = Some(token.clone());
    Self::save_token(&token);
    Ok(v)
  }

  pub(crate) async fn ensure_login(&self) -> Result<(), String> {
    if self.token.lock().await.is_some() {
      return Ok(());
    }
    self.login().await.map(|_| ())
  }

  /// 带自动重登的调用：令牌失效（401）时自动重登一次。
  async fn call_auth(&self, action: &str, params: Value) -> Result<Value, String> {
    self.call_auth_to(AI_REWRITE_API_URL, action, params).await
  }

  async fn call_auth_to(&self, url: &str, action: &str, params: Value) -> Result<Value, String> {
    self.ensure_login().await?;
    match self.call_to(url, action, params.clone()).await {
      Ok(v) => Ok(v),
      Err(e) => {
        if e.contains("未登录") || e.contains("令牌") {
          self.login().await?;
          self.call_to(url, action, params).await
        } else {
          Err(e)
        }
      }
    }
  }

  // --- 业务接口 ---

  pub async fn ping(&self) -> Result<Value, String> {
    self.call_auth("ping", json!({})).await
  }

  pub async fn precheck(&self) -> Result<Value, String> {
    self.call_auth("precheck", json!({})).await
  }

  pub async fn credentials(&self) -> Result<AiCredentials, String> {
    let v = self.call_auth("credentials", json!({})).await?;
    serde_json::from_value::<AiCredentials>(v).map_err(|e| format!("解析服务器密钥配置失败：{e}"))
  }

  /// 读取可编辑的接口配置（与云端后台同一份，接口设置对话框用）。
  pub async fn api_config(&self) -> Result<AiApiConfig, String> {
    let v = self.call_auth("ai_config_get", json!({})).await?;
    serde_json::from_value::<AiApiConfig>(v).map_err(|e| format!("解析接口配置失败：{e}"))
  }

  /// 保存接口配置到服务器（云端后台与本程序共用一份数据）。
  pub async fn save_api_config(&self, cfg: &AiApiConfig) -> Result<(), String> {
    self
      .call_auth(
        "ai_config_save",
        json!({
            "config": {
                "asr": cfg.asr,
                "llm": cfg.llm,
                "translate": cfg.translate,
            }
        }),
      )
      .await?;
    Ok(())
  }

  pub async fn fetch_videos(&self, params: Value) -> Result<Value, String> {
    self.call_auth("videos", params).await
  }

  /// 已入库的洗稿成果列表。
  pub async fn fetch_results(&self, params: Value) -> Result<Value, String> {
    self.call_auth("results", params).await
  }

  /// 单条成果详情（转写 / 爆点分析 / 洗稿文案 / 打分）。
  pub async fn fetch_result(&self, task_id: i64) -> Result<Value, String> {
    self
      .call_auth("result", json!({ "task_id": task_id }))
      .await
  }

  pub async fn check(&self, source_url: &str, video_id: &str) -> Result<Value, String> {
    self
      .call_auth(
        "check",
        json!({ "source_url": source_url, "video_id": video_id }),
      )
      .await
  }

  /// 上报单步结果 + 用量（服务器入库并计费）。
  pub async fn report(&self, body: Value) -> Result<Value, String> {
    self.call_auth("report", body).await
  }

  /// AI 助手 cron 网关（ai_agent_cron_api.php）：平台数据更新触发/查询。
  /// 与洗稿网关同 token 表，401 重登语义一致。
  pub async fn cron_call(&self, action: &str, params: Value) -> Result<Value, String> {
    self
      .call_auth_to(AI_AGENT_CRON_API_URL, action, params)
      .await
  }
}

// ============================================================
// Tauri 命令：账号 / 状态 / 数据拉取
// ============================================================

/// 登录 + 余额预检 + 密钥就绪状态（页面进入时调用）。
#[tauri::command]
pub async fn ai_rewrite_status() -> Result<Value, String> {
  AI_REWRITE.ensure_login().await?;
  let ping = AI_REWRITE.ping().await.unwrap_or(json!({}));
  let pre = AI_REWRITE.precheck().await?;
  Ok(json!({
      "success": true,
      "server_time": ping.get("server_time").cloned().unwrap_or(Value::Null),
      "billing": pre.get("billing").cloned().unwrap_or(Value::Null),
      "config_ready": pre.get("config_ready").cloned().unwrap_or(Value::Bool(false)),
  }))
}

// ============================================================
// 本地流水线：下载 → ASR → 爆点分析 → 洗稿 → 打分
// ============================================================

/// 从模型输出里抽取 JSON 对象。
/// 兼容真实 LLM 输出的多种“不合法”形态：
///   1) 被 ```json ... ``` 代码围栏包裹；
///   2) 前后夹带解释性文字；
///   3) 字符串内出现裸换行 / 制表符（严格 JSON 不允许）；
///   4) 因 max_tokens 触顶被截断、括号未闭合。
fn extract_json(text: &str) -> Option<Value> {
  let trimmed = text.trim();

  // 1) 直接解析
  if let Some(v) = parse_json_value(trimmed) {
    return Some(v);
  }

  // 2) 去掉 markdown 代码围栏
  let unfenced = strip_code_fence(trimmed);
  let body = unfenced.trim();
  if body != trimmed {
    if let Some(v) = parse_json_value(body) {
      return Some(v);
    }
  }

  // 3) 截取首个平衡的 {...} / [...] 片段
  let (fragment, closed) = json_span(body)?;
  if closed {
    if let Some(v) = parse_json_value(&fragment) {
      return Some(v);
    }
  }

  // 4) 修正字符串内的裸控制字符后重试
  let cleaned = escape_control_chars(&fragment);
  if let Some(v) = parse_json_value(&cleaned) {
    return Some(v);
  }

  // 5) 输出被截断：补全未闭合的字符串与括号，尽量救回可用结果
  if !closed {
    log::warn!("[ai_rewrite] 模型输出疑似被截断，已修补为部分 JSON");
    if let Some(v) = parse_json_value(&close_open_json(&cleaned)) {
      return Some(v);
    }
  }
  None
}

/// 严格解析，只接受对象或数组（拒绝裸字符串/数字）。
fn parse_json_value(s: &str) -> Option<Value> {
  match serde_json::from_str::<Value>(s) {
    Ok(v) if v.is_object() || v.is_array() => Some(v),
    _ => None,
  }
}

/// 去掉 ```json ... ``` 代码围栏（含首行的语言标记）。
fn strip_code_fence(s: &str) -> String {
  let s = s.trim();
  if !s.starts_with("```") {
    return s.to_string();
  }
  // 丢弃首行（``` 或 ```json）
  let inner = match s.find('\n') {
    Some(i) => &s[i + 1..],
    None => "",
  };
  // 丢弃结尾的 ```
  match inner.rfind("```") {
    Some(i) => inner[..i].trim().to_string(),
    None => inner.trim().to_string(),
  }
}

/// 扫描首个平衡的 JSON 片段，返回 (片段, 是否完整闭合)。
/// 正确跳过字符串内部的括号与转义字符。
fn json_span(s: &str) -> Option<(String, bool)> {
  let start = s.find(['{', '['])?;
  let mut depth: i32 = 0;
  let mut in_str = false;
  let mut escaped = false;
  let mut end: Option<usize> = None;
  for (i, ch) in s.char_indices() {
    if i < start {
      continue;
    }
    if in_str {
      if escaped {
        escaped = false;
      } else if ch == '\\' {
        escaped = true;
      } else if ch == '"' {
        in_str = false;
      }
      continue;
    }
    match ch {
      '"' => in_str = true,
      '{' | '[' => depth += 1,
      '}' | ']' => {
        depth -= 1;
        if depth <= 0 {
          end = Some(i + ch.len_utf8());
          break;
        }
      }
      _ => {}
    }
  }
  match end {
    Some(e) => Some((s[start..e].to_string(), true)),
    None => Some((s[start..].to_string(), false)),
  }
}

/// 转义字符串内部的裸控制字符（换行/回车/制表符等），使其符合严格 JSON。
fn escape_control_chars(s: &str) -> String {
  let mut out = String::with_capacity(s.len() + 8);
  let mut in_str = false;
  let mut escaped = false;
  for ch in s.chars() {
    if in_str {
      if escaped {
        out.push(ch);
        escaped = false;
        continue;
      }
      match ch {
        '\\' => {
          out.push(ch);
          escaped = true;
        }
        '"' => {
          out.push(ch);
          in_str = false;
        }
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        c if (c as u32) < 0x20 => out.push(' '),
        c => out.push(c),
      }
    } else {
      out.push(ch);
      if ch == '"' {
        in_str = true;
      }
    }
  }
  out
}

/// 修补被截断的 JSON：丢弃不完整的尾部 token，补全未闭合的字符串与括号。
fn close_open_json(s: &str) -> String {
  let s = trim_dangling_tail(s);
  let mut out = String::with_capacity(s.len() + 16);
  let mut stack: Vec<char> = Vec::new();
  let mut in_str = false;
  let mut escaped = false;
  for ch in s.chars() {
    out.push(ch);
    if in_str {
      if escaped {
        escaped = false;
      } else if ch == '\\' {
        escaped = true;
      } else if ch == '"' {
        in_str = false;
      }
      continue;
    }
    match ch {
      '"' => in_str = true,
      '{' => stack.push('}'),
      '[' => stack.push(']'),
      '}' | ']' => {
        stack.pop();
      }
      _ => {}
    }
  }
  if in_str {
    out.push('"');
  }
  while let Some(c) = stack.pop() {
    out.push(c);
  }
  out
}

/// 丢弃被截断的尾部：尾部空白、逗号，以及只有键没有值的 `"key":`。
fn trim_dangling_tail(s: &str) -> String {
  let mut t = s.trim_end().to_string();
  loop {
    if t.ends_with(',') {
      t.pop();
      t = t.trim_end().to_string();
      continue;
    }
    if t.ends_with(':') {
      let at = t.rfind([',', '{', '[']).map(|p| p + 1).unwrap_or(0);
      t.truncate(at);
      t = t.trim_end().to_string();
      continue;
    }
    break;
  }
  t
}

/// 本地临时工作目录（每个任务一个子目录）。
fn task_workdir(task_key: &str) -> PathBuf {
  let dir = crate::app_dirs::data_dir()
    .join("ai-rewrite-tmp")
    .join(task_key);
  let _ = std::fs::create_dir_all(&dir);
  dir
}

/// 为无 video_id 的自定义链接生成稳定的任务目录 key。
fn url_task_key(url: &str) -> String {
  use std::collections::hash_map::DefaultHasher;
  use std::hash::{Hash, Hasher};
  let mut h = DefaultHasher::new();
  url.hash(&mut h);
  format!("u{:x}", h.finish())
}

/// 从链接推断平台（youtube / tiktok / douyin）；识别不出返回空串。
/// 粘贴链接等来源不带 platform 字段，补拉实时数据前用它兜底。
fn infer_platform_from_url(url: &str) -> String {
  let lower = url.to_lowercase();
  if lower.contains("youtube.com") || lower.contains("youtu.be") {
    "youtube".to_string()
  } else if lower.contains("tiktok.com") {
    "tiktok".to_string()
  } else if lower.contains("douyin.com") || lower.contains("v.douyin.com") {
    "douyin".to_string()
  } else {
    String::new()
  }
}

/// 从标题提取集数线索：S03E12 / EP12 / Episode 5 / 第12集 / 第十二话 / 12集。
/// 返回 0 表示没有可信线索。只扫标题，不扫长文案（避免正文数字误配）。
fn extract_episode_hint(title: &str) -> i64 {
  use regex_lite::Regex;
  let t = title.trim();
  if t.is_empty() {
    return 0;
  }
  // S03E12 / s3 e12
  if let Ok(re) = Regex::new(r"(?i)\bS\d{1,3}\s*E\s*(\d{1,4})\b") {
    if let Some(c) = re.captures(t) {
      if let Ok(n) = c[1].parse::<i64>() {
        if n > 0 {
          return n;
        }
      }
    }
  }
  // EP12 / Ep.12 / episode 5 / eps 12（要求 ep 开头紧跟数字或分隔符，避免误配普通单词）
  if let Ok(re) = Regex::new(r"(?i)\bEP(?:ISODE|S)?\s*[.\-#]?\s*(\d{1,4})\b") {
    if let Some(c) = re.captures(t) {
      if let Ok(n) = c[1].parse::<i64>() {
        if n > 0 {
          return n;
        }
      }
    }
  }
  // 第12集 / 第 12 话 / 第12回
  if let Ok(re) = Regex::new(r"第\s*(\d{1,4})\s*[集话回]") {
    if let Some(c) = re.captures(t) {
      if let Ok(n) = c[1].parse::<i64>() {
        if n > 0 {
          return n;
        }
      }
    }
  }
  // 第十二集（中文数字）：regex-lite 对 unicode 字符类支持有限，手动扫描「第…集/话/回」
  if let Some(start) = t.find('第') {
    let after = &t[start + '第'.len_utf8()..];
    if let Some(ei) = after.find(['集', '话', '回']) {
      let inner = after[..ei].trim();
      if !inner.is_empty()
        && inner
          .chars()
          .all(|c| "零〇一二两三四五六七八九十百".contains(c))
      {
        let n = cn_num_to_int(inner);
        if n > 0 {
          return n;
        }
      }
    }
  }
  // 「12集」「12话」紧凑写法
  if let Ok(re) = Regex::new(r"(\d{1,4})\s*[集话回]") {
    if let Some(c) = re.captures(t) {
      if let Ok(n) = c[1].parse::<i64>() {
        if n > 0 {
          return n;
        }
      }
    }
  }
  0
}

/// 中文数字转整数：支持 零〇一二两三四五六七八九十百 组合（如 十二=12、二十三=23、一百零五=105）。
fn cn_num_to_int(s: &str) -> i64 {
  let mut total = 0i64;
  let mut cur = 0i64;
  for ch in s.chars() {
    let dv = match ch {
      '零' | '〇' => 0,
      '一' => 1,
      '二' | '两' => 2,
      '三' => 3,
      '四' => 4,
      '五' => 5,
      '六' => 6,
      '七' => 7,
      '八' => 8,
      '九' => 9,
      '十' => {
        cur = if cur == 0 { 1 } else { cur };
        total += cur * 10;
        cur = 0;
        continue;
      }
      '百' => {
        cur = if cur == 0 { 1 } else { cur };
        total += cur * 100;
        cur = 0;
        continue;
      }
      _ => return 0,
    };
    cur = cur * 10 + dv;
  }
  total + cur
}

/// 把上游接口的报错（余额不足、鉴权失败等）原样带出来，
/// 避免只报「未返回文本」这种看不出原因的误导信息。
fn format_api_error(label: &str, v: &Value, raw: &str, http_status: u16) -> String {
  let flat = v
    .get("message")
    .and_then(|m| m.as_str())
    .unwrap_or("")
    .trim();
  let nested = v
    .pointer("/error/message")
    .and_then(|m| m.as_str())
    .unwrap_or("")
    .trim();
  let detail = if !flat.is_empty() { flat } else { nested };
  if !detail.is_empty() {
    let code = v
      .get("code")
      .and_then(|c| c.as_i64())
      .map(|c| format!("code {c}"))
      .or_else(|| {
        v.pointer("/error/code")
          .and_then(|c| c.as_str())
          .map(|c| c.to_string())
      });
    return match code {
      Some(c) => format!("{label} 接口报错（{c}）：{detail}"),
      None => format!("{label} 接口报错：{detail}"),
    };
  }
  format!(
    "{label} 未返回内容（HTTP {http_status}）：{}",
    truncate(raw, 200)
  )
}

/// OpenAI 兼容 ASR：POST {base_url}/audio/transcriptions（multipart）。
/// 先按 verbose_json 请求以获取真实逐段时间戳；供应商不支持时自动降级普通请求。
async fn asr_transcribe(
  cfg: &AiEndpointConfig,
  audio_path: &Path,
) -> Result<(String, Value, Vec<Value>), String> {
  if cfg.base_url.trim().is_empty() || cfg.api_key.trim().is_empty() {
    return Err("服务器未配置 ASR 密钥".to_string());
  }
  let base = cfg.base_url.trim().trim_end_matches('/');
  let url = if base.ends_with("/audio/transcriptions") {
    base.to_string()
  } else {
    format!("{base}/audio/transcriptions")
  };

  let mut builder = Client::builder().timeout(std::time::Duration::from_secs(300));
  if let Some(px) = resolve_proxy_url(&cfg.proxy).await? {
    builder = builder.proxy(reqwest::Proxy::all(&px).map_err(|e| format!("代理配置无效：{e}"))?);
  } else {
    // 直连 = 真直连：屏蔽系统/环境变量代理（TUN 等透明代理不受影响）
    builder = builder.no_proxy();
  }
  let client = builder
    .build()
    .map_err(|e| format!("HTTP 客户端创建失败：{e}"))?;

  let asr_post = |response_format: Option<&'static str>| {
    let client = client.clone();
    let url = url.clone();
    let api_key = cfg.api_key.clone();
    let model = cfg.model.clone();
    async move {
      let mut form = reqwest::multipart::Form::new().text("model", model);
      if let Some(rf) = response_format {
        form = form.text("response_format", rf);
      }
      let form = form
        .file("file", audio_path)
        .await
        .map_err(|e| format!("读取音频文件失败：{e}"))?;
      let resp = client
        .post(&url)
        .bearer_auth(&api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("ASR 请求失败：{}", err_chain(&e)))?;
      let status = resp.status();
      let raw = resp
        .text()
        .await
        .map_err(|e| format!("读取 ASR 响应失败：{e}"))?;
      let v: Value = serde_json::from_str(&raw).map_err(|_| {
        format!(
          "ASR 响应异常（HTTP {}）：{}",
          status.as_u16(),
          truncate(&raw, 200)
        )
      })?;
      Ok::<(Value, u16, String), String>((v, status.as_u16(), raw))
    }
  };

  let (v, status, raw) = match asr_post(Some("verbose_json")).await {
    Ok(ok) => ok,
    Err(_) => asr_post(None).await?,
  };
  let segs = asr_parse_segments(&v);

  let transcript = v
    .get("text")
    .and_then(|t| t.as_str())
    .unwrap_or("")
    .trim()
    .to_string();
  if transcript.is_empty() {
    return Err(format_api_error("ASR", &v, &raw, status));
  }

  let usage = json!({
      "duration": v.get("duration").cloned().unwrap_or(Value::Null),
      "seconds": v.get("duration").cloned().unwrap_or(Value::Null),
  });
  Ok((transcript, usage, segs))
}

/// 从 verbose_json 响应提取真实逐段时间轴（[{start,end,text}]，单位秒）。
fn asr_parse_segments(v: &Value) -> Vec<Value> {
  let mut out = Vec::new();
  if let Some(arr) = v.get("segments").and_then(|s| s.as_array()) {
    for s in arr {
      let start = s.get("start").and_then(|x| x.as_f64()).unwrap_or(0.0);
      let end = s.get("end").and_then(|x| x.as_f64()).unwrap_or(start);
      let text = s
        .get("text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
      if end > start && !text.is_empty() {
        out.push(json!({ "start": start, "end": end, "text": text }));
      }
    }
  }
  out
}

/// 归一化提供商类型，决定请求协议。
///   openai    → /chat/completions（OpenAI / ChatGPT / DeepSeek / 通义 / 月之暗面等兼容接口）
///   anthropic → /messages（Claude）
///   gemini    → /models/{model}:generateContent（Google Gemini）
fn provider_kind(p: &str) -> &'static str {
  let p = p.trim().to_lowercase();
  if p.contains("claude") || p.contains("anthropic") {
    "anthropic"
  } else if p.contains("gemini") || p.contains("google") || p.contains("vertex") {
    "gemini"
  } else {
    "openai"
  }
}

/// 拼接错误完整原因链（reqwest 顶层 Display 不含底层 cause，排查网络问题必需）
fn err_chain(e: &impl std::error::Error) -> String {
  let mut msg = e.to_string();
  let mut src = std::error::Error::source(e);
  while let Some(s) = src {
    msg.push_str(&format!("｜原因：{s}"));
    src = s.source();
  }
  msg
}

/// 多提供商 LLM 调用：按 provider 自动适配 Claude / ChatGPT(OpenAI) / Gemini 等协议。
async fn llm_chat(
  cfg: &AiEndpointConfig,
  system_prompt: &str,
  user_content: &str,
) -> Result<(String, Value), String> {
  if cfg.base_url.trim().is_empty() || cfg.api_key.trim().is_empty() {
    return Err("服务器未配置 LLM 密钥".to_string());
  }
  let kind = provider_kind(&cfg.provider);
  let base = cfg.base_url.trim().trim_end_matches('/');

  // Claude 官方 API 对数据中心 IP 风控极严：未配置专用代理时禁止直连，避免封号
  if kind == "anthropic" && cfg.proxy.host.trim().is_empty() {
    return Err(
      "Claude API 必须走指定代理线路（直连可能封号），请在后台「AI 接口配置」或本程序「接口设置」填写 LLM 代理"
        .to_string(),
    );
  }

  let mut builder = Client::builder().timeout(std::time::Duration::from_secs(300));
  if let Some(px) = resolve_proxy_url(&cfg.proxy).await? {
    builder = builder.proxy(reqwest::Proxy::all(&px).map_err(|e| format!("代理配置无效：{e}"))?);
  } else {
    // 直连 = 真直连：屏蔽系统/环境变量代理（TUN 等透明代理不受影响）
    builder = builder.no_proxy();
  }
  let client = builder
    .build()
    .map_err(|e| format!("HTTP 客户端创建失败：{e}"))?;

  let resp = match kind {
    "anthropic" => {
      let url = if base.ends_with("/messages") {
        base.to_string()
      } else {
        format!("{base}/messages")
      };
      let body = json!({
          "model": cfg.model,
          "max_tokens": 8192,
          "temperature": 0.7,
          "system": system_prompt,
          "messages": [{ "role": "user", "content": user_content }],
      });
      client
        .post(&url)
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
    }
    "gemini" => {
      // base 形如 https://generativelanguage.googleapis.com/v1beta
      let url = format!(
        "{base}/models/{}:generateContent?key={}",
        cfg.model, cfg.api_key
      );
      let body = json!({
          "system_instruction": { "parts": [{ "text": system_prompt }] },
          "contents": [{ "role": "user", "parts": [{ "text": user_content }] }],
          "generationConfig": { "temperature": 0.7, "maxOutputTokens": 8192 },
      });
      client.post(&url).json(&body).send().await
    }
    _ => {
      let url = if base.ends_with("/chat/completions") {
        base.to_string()
      } else {
        format!("{base}/chat/completions")
      };
      let body = json!({
          "model": cfg.model,
          "temperature": 0.7,
          "max_tokens": 8192,
          "messages": [
              { "role": "system", "content": system_prompt },
              { "role": "user", "content": user_content },
          ],
      });
      client
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await
    }
  }
  .map_err(|e| format!("LLM 请求失败：{}", err_chain(&e)))?;

  let status = resp.status();
  let text = resp
    .text()
    .await
    .map_err(|e| format!("读取 LLM 响应失败：{e}"))?;
  let v: Value = serde_json::from_str(&text).map_err(|_| {
    format!(
      "LLM 响应异常（HTTP {}）：{}",
      status.as_u16(),
      truncate(&text, 200)
    )
  })?;

  let content = match kind {
    "anthropic" => v
      .pointer("/content/0/text")
      .and_then(|c| c.as_str())
      .unwrap_or(""),
    "gemini" => v
      .pointer("/candidates/0/content/parts/0/text")
      .and_then(|c| c.as_str())
      .unwrap_or(""),
    _ => v
      .pointer("/choices/0/message/content")
      .and_then(|c| c.as_str())
      .unwrap_or(""),
  }
  .to_string();
  if content.trim().is_empty() {
    return Err(format_api_error("LLM", &v, &text, status.as_u16()));
  }

  let usage = match kind {
    "anthropic" => v.get("usage").cloned().unwrap_or(json!({})),
    "gemini" => v.get("usageMetadata").cloned().unwrap_or(json!({})),
    _ => v.get("usage").cloned().unwrap_or(json!({})),
  };
  Ok((content, usage))
}

fn now_hms() -> String {
  chrono::Local::now().format("%H:%M:%S").to_string()
}

/// 从视频行提取上报所需的基础字段。
fn video_identity(v: &Value) -> Value {
  json!({
      "video_id": v.get("video_id").cloned().unwrap_or(Value::Null),
      // 优先使用稳定页面链接（source_url）；play_url 是带签名的 CDN 直链、有时效，仅作兜底
      "source_url": v
          .get("source_url")
          .or_else(|| v.get("play_url"))
          .cloned()
          .unwrap_or(Value::Null),
      "account_id": v.get("account_id").cloned().unwrap_or(json!(0)),
      "account_type": v.get("account_type").cloned().unwrap_or(Value::Null),
      "platform": v.get("platform").cloned().unwrap_or(Value::Null),
      "title": v.get("title").cloned().unwrap_or(Value::Null),
      "cover_url": v.get("cover_url").cloned().unwrap_or(Value::Null),
      "play_count": v.get("play_count").cloned().unwrap_or(json!(0)),
      "digg_count": v.get("digg_count").cloned().unwrap_or(json!(0)),
      "comment_count": v.get("comment_count").cloned().unwrap_or(json!(0)),
      "video_duration": v.get("video_duration").cloned().unwrap_or(json!(0)),
  })
}

fn merge_identity(identity: &Value, extra: Value) -> Value {
  let mut base = identity.clone();
  if let (Value::Object(b), Value::Object(e)) = (&mut base, extra) {
    for (k, val) in e {
      b.insert(k, val);
    }
  }
  base
}

/// 字节数转可读文本（用于下载进度展示）。
fn fmt_bytes(b: u64) -> String {
  const KB: f64 = 1024.0;
  const MB: f64 = 1024.0 * 1024.0;
  const GB: f64 = 1024.0 * 1024.0 * 1024.0;
  let f = b as f64;
  if f >= GB {
    format!("{:.2}GB", f / GB)
  } else if f >= MB {
    format!("{:.1}MB", f / MB)
  } else if f >= KB {
    format!("{:.0}KB", f / KB)
  } else {
    format!("{b}B")
  }
}

/// 提供商可读名称（用于日志）。
fn provider_label(cfg: &AiEndpointConfig) -> String {
  let name = match provider_kind(&cfg.provider) {
    "anthropic" => "Claude",
    "gemini" => "Gemini",
    _ => "OpenAI/ChatGPT 兼容",
  };
  if cfg.model.trim().is_empty() {
    name.to_string()
  } else {
    format!("{name}（{}）", cfg.model)
  }
}

/// 测试接口配置连通性：从本机（走配置的代理）发一条最小请求。
/// 成功返回模型回复摘要；失败返回带 HTTP 状态码与响应体的详细错误，便于定位配置问题。
#[tauri::command]
pub async fn ai_rewrite_test_endpoint(endpoint: AiEndpointConfig) -> Result<String, String> {
  let base = endpoint.base_url.trim().trim_end_matches('/');
  let key = endpoint.api_key.trim();
  let model = endpoint.model.trim();
  if base.is_empty() || key.is_empty() || model.is_empty() {
    return Err("接口地址 / API Key / 模型 三项都必须填写".to_string());
  }
  let kind = provider_kind(&endpoint.provider);

  // 与运行时 llm_chat 保持一致：Claude 官方 API 未配代理直接提示（直连可能封号）
  if kind == "anthropic" && endpoint.proxy.host.trim().is_empty() {
    return Err(
      "Claude API 必须走指定代理线路（直连可能封号），请先在下方选择代理线路".to_string(),
    );
  }

  let mut builder = Client::builder().timeout(std::time::Duration::from_secs(60));
  if let Some(px) = resolve_proxy_url(&endpoint.proxy).await? {
    builder = builder.proxy(reqwest::Proxy::all(&px).map_err(|e| format!("代理配置无效：{e}"))?);
  } else {
    // 与运行时 llm_chat 一致：直连时屏蔽系统/环境变量代理
    builder = builder.no_proxy();
  }
  let client = builder
    .build()
    .map_err(|e| format!("HTTP 客户端创建失败：{e}"))?;

  let resp = match kind {
    "anthropic" => {
      let url = format!("{base}/messages");
      let body = json!({
          "model": model,
          "max_tokens": 16,
          "messages": [{ "role": "user", "content": "只回复 ok" }],
      });
      client
        .post(&url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
    }
    "gemini" => {
      let url = format!("{base}/models/{model}:generateContent?key={key}");
      let body = json!({
          "contents": [{ "role": "user", "parts": [{ "text": "只回复 ok" }] }],
          "generationConfig": { "maxOutputTokens": 16 },
      });
      client.post(&url).json(&body).send().await
    }
    _ => {
      let url = format!("{base}/chat/completions");
      let body = json!({
          "model": model,
          "max_tokens": 16,
          "messages": [{ "role": "user", "content": "只回复 ok" }],
      });
      client.post(&url).bearer_auth(key).json(&body).send().await
    }
  }
  .map_err(|e| {
    format!(
      "连接失败（未收到响应）：{} —— 检查接口地址是否正确、本机网络/代理能否访问该地址",
      err_chain(&e)
    )
  })?;

  let status = resp.status().as_u16();
  let text = resp.text().await.unwrap_or_default();
  if status != 200 {
    let compact: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    return Err(format!(
      "HTTP {status}：{} —— 401/403=Key 无效，404=接口地址路径不对，400=模型名或参数不被接受",
      truncate(&compact, 300)
    ));
  }
  let j: Value = serde_json::from_str(&text)
    .map_err(|_| format!("HTTP 200 但响应不是 JSON：{}", truncate(&text, 200)))?;
  let reply = match kind {
    "anthropic" => j["content"][0]["text"].as_str().unwrap_or("").to_string(),
    "gemini" => j["candidates"][0]["content"]["parts"][0]["text"]
      .as_str()
      .unwrap_or("")
      .to_string(),
    _ => j["choices"][0]["message"]["content"]
      .as_str()
      .unwrap_or("")
      .to_string(),
  };
  let reply = reply.trim().to_string();
  Ok(format!(
    "✅ 连接成功，模型「{model}」回复：{}",
    if reply.is_empty() { "(空)" } else { &reply }
  ))
}

/// 处理单个视频，返回该视频的结果摘要。
/// 全程通过 `ai-rewrite:progress`（阶段/百分比/速度）与 `ai-rewrite:log`（日志）实时上报。
async fn process_video(
  app: &tauri::AppHandle<impl tauri::Runtime>,
  video: &Value,
  creds: &AiCredentials,
  index: usize,
  total: usize,
) -> Result<Value, String> {
  use tauri::Emitter;

  let video_id = video
    .get("video_id")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_string();
  let title = video
    .get("title")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_string();
  let mut identity = video_identity(video);
  let source_url = identity["source_url"].as_str().unwrap_or("").to_string();
  // 粘贴链接等来源不带 platform：按链接兜底推断并写回，否则补拉/上报都会因平台未知而跳过
  if identity["platform"].as_str().unwrap_or("").is_empty() && !source_url.is_empty() {
    identity["platform"] = json!(infer_platform_from_url(&source_url));
  }
  let platform = identity["platform"].as_str().unwrap_or("").to_string();
  let task_key = if video_id.is_empty() {
    url_task_key(&source_url)
  } else {
    video_id.clone()
  };

  let emit = |stage: &str, percent: u32, message: &str, speed: &str| {
    let payload = json!({
        "task_key": task_key,
        "index": index,
        "total": total,
        "video_id": video_id,
        "title": title,
        "source_url": source_url,
        "platform": platform,
        "stage": stage,
        "percent": percent,
        "message": message,
        "speed_text": speed,
    });
    // 运行表与事件流同源：助手的进度查询工具读这份状态。
    tokio::spawn(track_running(payload.clone()));
    let _ = app.emit("ai-rewrite:progress", payload);
  };
  let log = |level: &str, message: &str| {
    let _ = app.emit(
      "ai-rewrite:log",
      json!({
          "task_key": task_key,
          "index": index,
          "total": total,
          "video_id": video_id,
          "title": title,
          "source_url": source_url,
          "level": level,
          "message": message,
          "time": now_hms(),
      }),
    );
  };
  // 实时把各环节产物推给 UI（转写文案 / 爆点分析 / 洗稿文案 / 打分）
  let content = |field: &str, value: Value| {
    let _ = app.emit(
      "ai-rewrite:content",
      json!({
          "task_key": task_key,
          "index": index,
          "total": total,
          "video_id": video_id,
          "title": title,
          "source_url": source_url,
          "platform": platform,
          "field": field,
          "value": value,
      }),
    );
  };

  emit("start", 0, "准备处理", "");
  log(
    "info",
    &format!(
      "开始处理：{}",
      if title.is_empty() {
        source_url.clone()
      } else {
        title.clone()
      }
    ),
  );
  if !source_url.is_empty() {
    log("info", &format!("链接：{source_url}"));
  }
  // transcript 模式：只提取文案（下载 + 转写），不做补拉/分析/洗稿/评分
  let transcript_only = video.get("mode").and_then(|v| v.as_str()) == Some("transcript");
  // 重新洗稿：跳过「已完成」整体复用，强制重跑分析/洗稿/打分（转写仍复用），最终结果覆盖旧记录
  let force_rerun = video
    .get("force_rerun")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
  // 本地视频文件：不经下载，ffmpeg 直接抽音轨；跳过云端查重与播放量补拉
  let is_local = crate::video_downloader::is_local_video(&source_url);
  if transcript_only {
    log("info", "本次为「提取文案」模式：仅下载转写，不洗稿");
  }

  // 1) 查重：命中历史转写/结果则复用，避免重复下载与调用（本地文件跳过）
  let mut asr_text: Option<String> = None;
  let mut asr_segs: Vec<Value> = Vec::new();
  let mut done_task_id: i64 = 0;
  if is_local {
    log("info", "本地视频文件：跳过云端查重，直接处理");
  } else {
    match AI_REWRITE.check(&source_url, &video_id).await {
      Ok(chk) => {
        if let Some(t) = chk.get("task") {
          if t.get("done").and_then(|b| b.as_bool()) == Some(true) {
            done_task_id = t.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
          }
          if t.get("asr_status").and_then(|s| s.as_i64()) == Some(2) {
            let text = t.get("asr_text").and_then(|s| s.as_str()).unwrap_or("");
            if !text.trim().is_empty() {
              asr_text = Some(text.to_string());
              emit("asr", 40, "复用历史转写", "");
              log("info", "命中历史转写，跳过下载与 ASR");
            }
          }
        }
        if let Some(c) = chk.get("cache") {
          if c.get("asr_status").and_then(|s| s.as_i64()) == Some(2) {
            let text = c.get("asr_text").and_then(|s| s.as_str()).unwrap_or("");
            if !text.trim().is_empty() {
              asr_text = Some(text.to_string());
              emit("asr", 40, "复用同链接历史转写", "");
              log("info", "命中同链接历史转写，跳过下载与 ASR");
            }
          }
        }
      }
      Err(e) => log("warn", &format!("查重失败（继续处理）：{e}")),
    }
  }

  // 1.5) 命中「已完成」任务 → 直接复用全部结果，跳过下载与所有 API 调用
  //      force_rerun（重新洗稿）时跳过整体复用：转写沿用查重复用值，重跑分析/洗稿/打分并覆盖
  if done_task_id > 0 && !force_rerun {
    match AI_REWRITE.fetch_result(done_task_id).await {
      Ok(det) => {
        emit("done", 100, "复用历史结果", "");
        log(
          "info",
          &format!("复用历史成果 #{done_task_id}（未重复计费）"),
        );
        let t = det.get("task").cloned().unwrap_or_else(|| json!({}));
        let s = det.get("script").cloned().unwrap_or(Value::Null);
        let tr = t.get("transcript").cloned().unwrap_or(json!(""));
        let an = t.get("analyze").cloned().unwrap_or(Value::Null);
        let rw = t.get("rewrite").cloned().unwrap_or(Value::Null);
        let sc = s.get("score_json").cloned().unwrap_or(Value::Null);
        content("transcript", tr.clone());
        content("analyze", an.clone());
        content("rewrite", rw.clone());
        content("score", sc.clone());
        return Ok(json!({
            "video_id": video_id.clone(),
            "title": title.clone(),
            "source_url": source_url.clone(),
            "platform": platform.clone(),
            "task_id": done_task_id,
            "script_id": s.get("id").cloned().unwrap_or(json!(0)),
            "score": s.get("score").cloned().unwrap_or(json!(0)),
            "transcript": tr,
            "analyze": an,
            "rewrite": rw,
            "score_json": sc,
            "reused": true,
            "transcript_only": transcript_only,
            "success": true,
        }));
      }
      Err(e) => log("warn", &format!("读取历史成果失败，改为重新处理：{e}")),
    }
  }
  if force_rerun {
    log(
      "info",
      if done_task_id > 0 {
        "重新洗稿模式：复用转写，重跑爆点分析 / 洗稿 / 打分（结果覆盖旧记录）"
      } else {
        "重新洗稿模式：重跑爆点分析 / 洗稿 / 打分"
      },
    );
  }

  // 1.8) 播放量缺失时实时补拉（按次计费的 fetch_video_data）：拉到的播放量 /
  //      发布时间并入爆点分析；上游明确「视频不存在」则直接跳过该视频，
  //      其余失败（key 未配 / 余额 / 网络 / 超出最近列表）不误伤，继续正常流程。
  let mut live_stats: Option<(i64, String)> = None;
  {
    let play = video
      .get("play_count")
      .and_then(|v| v.as_i64())
      .or_else(|| {
        video
          .get("play_count")
          .and_then(|v| v.as_str())
          .and_then(|s| s.parse::<i64>().ok())
      })
      .unwrap_or(0);
    if play <= 0 && !platform.is_empty() && platform != "local" && !is_local && !transcript_only {
      emit("start", 2, "补取实时播放量", "");
      let mut params = json!({ "platform": platform });
      if video_id.is_empty() {
        params["video_url"] = json!(source_url);
      } else {
        params["video_id"] = json!(video_id);
      }
      if let Some(aid) = video.get("account_id").and_then(|v| v.as_i64()) {
        params["account_id"] = json!(aid);
      }
      if let (Some(aid), at) = (
        params.get("account_id").and_then(|v| v.as_i64()),
        video
          .get("account_type")
          .and_then(|v| v.as_str())
          .unwrap_or(""),
      ) {
        if aid > 0 && !at.is_empty() {
          params["account_type"] = json!(at);
        }
      }
      if platform == "tiktok" {
        // tikwm 无单视频端点，上游按账号主页拉取：从链接解析 @用户名
        if let Some(name) = source_url
          .split('@')
          .nth(1)
          .map(|rest| rest.split('/').next().unwrap_or("").to_string())
          .filter(|n| !n.is_empty())
        {
          params["username"] = json!(name);
        }
      }
      match AI_REWRITE.cron_call("video_data", params).await {
        Ok(v) => {
          let st = v.get("video").cloned().unwrap_or_else(|| json!({}));
          let pc = st
            .get("play_count")
            .and_then(|x| {
              x.as_i64().or_else(|| {
                x.as_str()
                  .and_then(|s| s.replace(',', "").parse::<i64>().ok())
              })
            })
            .unwrap_or(0);
          let ct = match st.get("create_time") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
          };
          log(
            "info",
            &format!(
              "实时数据已归档：播放量 {pc}，发布时间 {}",
              if ct.is_empty() { "未知" } else { &ct }
            ),
          );
          live_stats = Some((pc, ct));
        }
        Err(e) => {
          let lower = e.to_lowercase();
          let missing = ["不存在", "已删除", "invalid", "not found"]
            .iter()
            .any(|k| lower.contains(k));
          if missing {
            return Err(format!("视频不存在或已删除，已跳过（{e}）"));
          }
          log("warn", &format!("实时播放量补拉失败（继续处理）：{e}"));
        }
      }
    }
  }

  // 2) 无历史则本地下载 + ASR（下载复用「视频下载」的 yt-dlp / 代理 / Cookie 逻辑）
  let workdir = task_workdir(&task_key);
  if asr_text.is_none() {
    // 0) 优先拉平台原生字幕（TikTok/YouTube 自带 CC，时间轴与画面严格对齐）；
    //    未命中再走下载 + 语音转写
    emit("download", 5, "尝试获取平台原生字幕", "");
    let native = if is_local {
      Vec::new()
    } else {
      crate::video_downloader::fetch_native_subtitles(app, &source_url, &workdir).await
    };
    if !native.is_empty() {
      let joined = native
        .iter()
        .filter_map(|s| s.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
      log(
        "info",
        &format!(
          "命中平台原生字幕（{} 段，时间轴与视频对齐），跳过下载与语音转写",
          native.len()
        ),
      );
      asr_text = Some(joined.clone());
      asr_segs = native;
      emit("asr", 40, "使用平台原生字幕", "");
      content("transcript", json!(joined.clone()));
      content("transcript_segments", json!(asr_segs.clone()));
      let body = merge_identity(
        &identity,
        json!({
          "step": "asr",
          "asr_text": joined,
          "usage": { "source": "native_subs" }
        }),
      );
      AI_REWRITE.report(body).await?;
    } else {
      let audio = if is_local {
        // 本地视频/音频文件：ffmpeg 直接抽音轨，不经 yt-dlp
        emit("download", 8, "本地视频抽取音频", "");
        log("info", "本地文件：ffmpeg 直接抽取音频");
        let src_path = std::path::PathBuf::from(source_url.trim());
        if matches!(
          src_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
          Some("mp3" | "m4a" | "wav")
        ) {
          src_path
        } else {
          let out = workdir.join("audio.mp3");
          crate::video_downloader::extract_audio_with_ffmpeg(&src_path, &out)
            .await
            .map_err(|e| format!("本地视频抽取音频失败：{e}"))?;
          out
        }
      } else {
        emit("download", 5, "准备下载音频", "");
        log(
          "info",
          "调用 yt-dlp 下载音频（复用视频下载的工具/代理/Cookie）",
        );
        log::info!(
          "[ai_rewrite] 开始下载 video_id={:?} url={source_url}",
          identity.get("video_id")
        );
        crate::video_downloader::download_audio_for_asr(app, &source_url, &workdir, |p| {
          if p.percent >= 0.0 {
            // 下载阶段占 5% - 20%
            let pct = (5.0 + (p.percent / 100.0) * 15.0).round() as u32;
            let size = if p.total > 0 {
              format!("{}/{}", fmt_bytes(p.downloaded), fmt_bytes(p.total))
            } else {
              String::new()
            };
            let msg = if size.is_empty() {
              format!("下载中 {}", p.speed_text)
            } else {
              format!("下载中 {} · {}", p.speed_text, size)
            };
            emit("download", pct, &msg, &p.speed_text);
          } else if !p.log.is_empty() {
            log("debug", &p.log);
          }
        })
        .await?
      };
      let name = audio
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio")
        .to_string();
      log("info", &format!("音频下载完成：{name}"));
      emit("download", 20, "下载完成", "");

      emit("asr", 22, "正在转写（本地 ASR）", "");
      log("info", "开始 ASR 转写…");
      let (text, usage, segs) = asr_transcribe(&creds.asr, &audio).await?;
      let _ = std::fs::remove_file(&audio);
      log(
        "info",
        &format!(
          "转写完成，共 {} 字，{} 段真实时间轴",
          text.chars().count(),
          segs.len()
        ),
      );
      emit("asr", 40, "转写完成", "");
      content("transcript", json!(text.clone()));
      if !segs.is_empty() {
        content("transcript_segments", json!(segs.clone()));
      }
      asr_segs = segs;

      let body = merge_identity(
        &identity,
        json!({ "step": "asr", "asr_text": text, "usage": usage }),
      );
      AI_REWRITE.report(body).await?;
      asr_text = Some(text);
    }
  }

  let transcript = asr_text.unwrap_or_default();

  // 2.2) 生成原文中文翻译（对照阅读用；失败不影响后续流程）
  let mut transcript_zh = String::new();
  if !transcript.trim().is_empty() {
    emit("asr", 42, "生成原文中文翻译", "");
    let sys = "你是字幕翻译。将用户提供的视频解说文案逐段翻译成简体中文：保持原有分段与顺序，不增删内容、不加注释，只输出译文。";
    match llm_chat(&creds.llm, sys, &transcript).await {
      Ok((raw, _)) => {
        let zh = raw
          .trim()
          .trim_start_matches("```")
          .trim_end_matches("```")
          .trim()
          .to_string();
        if !zh.is_empty() {
          content("transcript_zh", json!(zh.clone()));
          transcript_zh = zh;
          log("info", "原文中文翻译完成");
        }
      }
      Err(e) => log("warn", &format!("原文翻译失败（不影响洗稿）：{e}")),
    }
  }

  // 提取文案模式：拿到转写即完成，不进入分析/洗稿/评分
  if transcript_only {
    emit("done", 100, "文案提取完成", "");
    log("info", "文案提取完成（仅转录，未做洗稿）");
    return Ok(json!({
        "video_id": video_id.clone(),
        "title": title.clone(),
        "source_url": source_url.clone(),
        "platform": platform.clone(),
        "transcript": transcript,
        "transcript_zh": transcript_zh,
        "transcript_segments": json!(asr_segs),
        "transcript_only": true,
        "success": true,
    }));
  }

  // 3) 爆点分析（实时补拉到的播放量/发布时间一并交给模型做重点判断）
  emit("analyze", 45, "正在分析爆点", "");
  log(
    "info",
    &format!("调用 {} 做爆点分析", provider_label(&creds.llm)),
  );
  let analyze_input = match &live_stats {
    Some((pc, ct)) => format!(
      "【视频文案（来源：平台原生字幕/语音识别）】\n{transcript}\n\n【实时数据（来源：平台数据接口）】播放量：{pc}{}",
      if ct.is_empty() {
        String::new()
      } else {
        format!("，发布时间：{ct}（分析时以此判断内容发布阶段与数据表现是否匹配）")
      }
    ),
    None => format!("【视频文案（来源：平台原生字幕/语音识别）】\n{transcript}"),
  };
  let ep_hint = extract_episode_hint(&title);
  let ep_note = if ep_hint > 0 {
    format!(
      "\n\n【标题集数线索】系统从标题中检测到集数：第 {ep_hint} 集。若与文案内容不矛盾，\"episode\" 请直接采用该值。"
    )
  } else {
    String::new()
  };
  let analyze_input = format!(
    "{analyze_input}\n\n【输出要求补充】除按提示词要求的 JSON 结构输出外，请额外包含两个字段：\"work\"（作品名，从标题与文案识别，如动漫/影视剧名；无法判断则为空字符串）与 \"episode\"（集数，数字）。集数识别要点：从标题、话题标签、文案开头找 EP12、Episode 5、S3E5、第12集、第十二话、12集 等写法；系列连载的续集解说（如「接着上一集」）但未标明具体集数时填 0。这两个字段用于同作品同集的历史记录归档，务必尽力识别。{ep_note}"
  );
  let (analyze_raw, analyze_usage) =
    llm_chat(&creds.llm, &creds.prompts.analyze, &analyze_input).await?;
  let mut analyze_json = extract_json(&analyze_raw)
    .ok_or_else(|| format!("分析结果不是合法 JSON：{}", truncate(&analyze_raw, 200)))?;
  // LLM 没识别出集数而标题里有明确线索时，用标题提取值兜底（归档与展示都依赖它）
  if ep_hint > 0
    && analyze_json
      .get("episode")
      .and_then(|v| v.as_i64())
      .unwrap_or(0)
      == 0
  {
    if let Value::Object(ref mut m) = analyze_json {
      m.insert("episode".to_string(), json!(ep_hint));
    }
    log(
      "info",
      &format!("标题提取到集数线索，采用：第 {ep_hint} 集"),
    );
  }
  log("info", "爆点分析完成");
  emit("analyze", 58, "分析完成", "");
  content("analyze", analyze_json.clone());
  // 补拉到的实时数据随 analyze 上报回传，云端回写任务表后归档进爆点库/作品集数记录
  let mut analyze_extra =
    json!({ "step": "analyze", "analyze_json": analyze_json, "usage": analyze_usage });
  if let Some((pc, ct)) = &live_stats {
    analyze_extra["live_play_count"] = json!(pc);
    if !ct.is_empty() {
      analyze_extra["live_create_time"] = json!(ct);
    }
  }
  let body = merge_identity(&identity, analyze_extra);
  AI_REWRITE.report(body).await?;

  // 4) 洗稿
  emit("rewrite", 62, "正在洗稿", "");
  log("info", "开始生成洗稿文案…");

  // 洗稿前拉取「爆款网感参考」（同作品同集 → 作品记忆 → 同赛道 → 跨赛道 → 兜底）；失败降级不阻塞
  let (ref_label, ref_text, ref_detail) = fetch_points_reference(&analyze_json)
    .await
    .unwrap_or_default();
  if ref_label.is_empty() {
    log("info", "未注入参考爆点（拉取失败或爆点库为空）");
  } else {
    log("info", &format!("已注入参考爆点（{ref_label}）"));
    content(
      "rewrite_ref",
      json!({ "label": ref_label, "points": ref_detail }),
    );
  }

  let analysis_text = serde_json::to_string_pretty(&analyze_json).unwrap_or_default();
  let borrowed_note = if ref_text.is_empty() {
    String::new()
  } else {
    "\n\n【输出要求补充】除按提示词要求的 JSON 结构输出外，请额外包含字段 \"borrowed\"（字符串数组，3-6 条）：逐条说明本文借鉴了哪些网感点、用在哪里，格式如「借鉴同赛道第2条的开头悬念钩子，用于本文前3秒」。仅依据上方【爆款网感参考】作答，禁止编造。".to_string()
  };
  let rewrite_input = if ref_text.is_empty() {
    format!("【原始文案】\n{transcript}\n\n【爆点分析】\n{analysis_text}")
  } else {
    format!("{ref_text}\n\n【原始文案】\n{transcript}\n\n【爆点分析】\n{analysis_text}")
  } + borrowed_note.as_str();
  let (rewrite_raw, rewrite_usage) =
    llm_chat(&creds.llm, &creds.prompts.rewrite, &rewrite_input).await?;
  let rewrite_json = extract_json(&rewrite_raw)
    .ok_or_else(|| format!("洗稿结果不是合法 JSON：{}", truncate(&rewrite_raw, 200)))?;
  log("info", "洗稿文案生成完成");
  emit("rewrite", 80, "洗稿完成", "");
  content("rewrite", rewrite_json.clone());
  let body = merge_identity(
    &identity,
    json!({ "step": "rewrite", "rewrite_json": rewrite_json, "usage": rewrite_usage }),
  );
  let rewrite_resp = AI_REWRITE.report(body).await?;
  let script_id = rewrite_resp
    .pointer("/result/script_id")
    .and_then(|s| s.as_i64())
    .unwrap_or(0);

  // 5) 打分
  emit("score", 84, "正在打分", "");
  log("info", "开始文案打分…");
  let score_input = serde_json::to_string_pretty(&rewrite_json).unwrap_or_default();
  let (score_raw, score_usage) = llm_chat(&creds.llm, &creds.prompts.score, &score_input).await?;
  let score_json = extract_json(&score_raw)
    .ok_or_else(|| format!("打分结果不是合法 JSON：{}", truncate(&score_raw, 200)))?;
  let score_val = score_json.get("score").cloned().unwrap_or(json!(0));
  log("info", &format!("打分完成：{score_val} 分"));
  emit("score", 98, "打分完成", "");
  content("score", score_json.clone());
  let body = merge_identity(
    &identity,
    json!({
        "step": "score",
        "score_json": score_json,
        "script_id": script_id,
        "usage": score_usage,
    }),
  );
  let score_resp = AI_REWRITE.report(body).await?;

  // 清理临时目录
  let _ = std::fs::remove_dir_all(&workdir);

  emit("done", 100, "完成", "");
  log("info", "全流程完成");
  let task_id = rewrite_resp
    .pointer("/result/task/id")
    .and_then(|v| v.as_i64())
    .unwrap_or(0);
  let mut result = json!({
      "video_id": video_id.clone(),
      "title": title.clone(),
      "source_url": source_url.clone(),
      "platform": platform.clone(),
      "task_id": task_id,
      "script_id": script_id,
      "score": score_resp.pointer("/result/score").cloned().unwrap_or(json!(0)),
      "transcript": transcript,
      "transcript_zh": transcript_zh,
      "transcript_segments": json!(asr_segs),
      "analyze": analyze_json,
      "rewrite": rewrite_json,
      "score_json": score_json,
      "success": true,
  });
  // 实时补拉到的数据随成果归档（供 AI 助手与前端任务详情读取）
  if let Some((pc, ct)) = live_stats {
    if let Value::Object(ref mut m) = result {
      m.insert("live_play_count".to_string(), json!(pc));
      if !ct.is_empty() {
        m.insert("live_create_time".to_string(), json!(ct));
      }
    }
  }
  Ok(result)
}

/// 播放量格式化：2870000 → "287 万"，120000000 → "1.2 亿"。
fn fmt_plays(n: i64) -> String {
  if n >= 100_000_000 {
    format!("{:.1} 亿", n as f64 / 100_000_000.0)
  } else if n >= 10_000 {
    format!("{} 万", n / 10_000)
  } else {
    format!("{n}")
  }
}

fn str_field(v: &Value, key: &str) -> String {
  v.get(key)
    .and_then(|x| x.as_str())
    .unwrap_or("")
    .trim()
    .to_string()
}

/// keywords 字段兼容数组与字符串两种形态，取前 5 个逗号拼接。
fn join_keywords(v: Option<&Value>) -> String {
  match v {
    Some(Value::Array(a)) => a
      .iter()
      .filter_map(|x| x.as_str())
      .map(|s| s.trim().to_string())
      .filter(|s| !s.is_empty())
      .take(5)
      .collect::<Vec<_>>()
      .join(","),
    Some(Value::String(s)) => s.trim().to_string(),
    _ => String::new(),
  }
}

/// 把一条爆点记录格式化为一行参考文本；with_stages 时附带叙事结构（跨赛道层用）。
fn point_line(r: &Value, with_stages: bool) -> String {
  let hook = truncate(r.get("hook").and_then(|v| v.as_str()).unwrap_or(""), 100);
  let plays = fmt_plays(r.get("play_count").and_then(|v| v.as_i64()).unwrap_or(0));
  let emotion = r
    .get("emotion")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .trim();
  let episode = r.get("episode").and_then(|v| v.as_i64()).unwrap_or(0);
  let ep_tag = if episode > 0 {
    format!(" · 第{episode}集")
  } else {
    String::new()
  };
  let line = if emotion.is_empty() {
    format!("「{hook}」（{plays} 播放{ep_tag}）")
  } else {
    format!("「{hook}」（{plays} 播放{ep_tag} · {emotion}）")
  };
  if !with_stages {
    return line;
  }
  let stages: Vec<String> = r
    .get("stages")
    .and_then(|v| v.as_array())
    .map(|a| {
      a.iter()
        .filter_map(|s| s.as_str())
        .map(|s| s.to_string())
        .collect()
    })
    .unwrap_or_default();
  if stages.is_empty() {
    line
  } else {
    format!("{line} 结构：{}", stages.join(" → "))
  }
}

/// 洗稿前从爆点库拉取「爆款网感参考」（同作品同集 → 作品记忆 → 同赛道 → 跨赛道 → 兜底），
/// 格式化为提示词段落。任何失败或库为空都返回 None，洗稿照常进行。
/// 返回 (概要标签, 注入提示词的文本, 给用户看的各层明细)。
async fn fetch_points_reference(analyze_json: &Value) -> Option<(String, String, Value)> {
  let work = str_field(analyze_json, "work");
  let category = str_field(analyze_json, "category");
  let emotion = str_field(analyze_json, "emotion");
  let keywords = join_keywords(analyze_json.get("keywords"));
  let episode = analyze_json
    .get("episode")
    .and_then(|v| v.as_i64())
    .unwrap_or(0);
  if work.is_empty() && category.is_empty() {
    return None;
  }
  let resp = AI_REWRITE
    .call_auth(
      "points_sample",
      json!({
          "work": work,
          "episode": episode,
          "category": category,
          "emotion": emotion,
          "keywords": keywords,
      }),
    )
    .await
    .ok()?;

  let arr = |key: &str| -> Vec<Value> {
    resp
      .get(key)
      .and_then(|v| v.as_array())
      .cloned()
      .unwrap_or_default()
  };
  let work_records = arr("work_records");
  let same_work = arr("same_work");
  let same_track = arr("same_track");
  let cross_track = arr("cross_track");
  let fallback = arr("fallback");
  if work_records.is_empty()
    && same_work.is_empty()
    && same_track.is_empty()
    && cross_track.is_empty()
    && fallback.is_empty()
  {
    return None;
  }

  // 给用户看的各层明细（不包含防雷同等指令文本）
  let detail_lines = |rows: &[Value], with_stages: bool| -> Vec<String> {
    rows.iter().map(|r| point_line(r, with_stages)).collect()
  };
  let detail = json!({
      "work_records": work_records.iter().map(work_record_line).collect::<Vec<_>>(),
      "same_work": detail_lines(&same_work, false),
      "same_track": detail_lines(&same_track, false),
      "cross_track": detail_lines(&cross_track, true),
      "fallback": detail_lines(&fallback, false),
  });

  let mut text = String::from(
    "【爆款网感参考】（来自本公司爆点库的历史爆款，只借鉴其张力、结构与情绪，严禁照抄任何原句）\n",
  );
  let mut counts: Vec<String> = Vec::new();

  if !work_records.is_empty() {
    let mut block = String::from("— 同作品同集历史记录（本集此前处理过的视频与数据）—\n");
    for (i, r) in work_records.iter().enumerate() {
      block.push_str(&format!("{}. {}\n", i + 1, work_record_line(r)));
    }
    block.push_str("可参考此前稿件对该集的处理角度与数据表现，但必须换新的切入角度。\n");
    text.push_str(&block);
    text.push('\n');
    counts.push(format!("同作品同集 {}", work_records.len()));
  }

  if !same_work.is_empty() {
    let mut block = String::from("— 作品记忆：同一作品的历史高热爆点 —\n");
    if let Some(cnt) = resp
      .pointer("/work_summary/analyzed_count")
      .and_then(|c| c.as_i64())
    {
      block.push_str(&format!(
        "该作品已被分析过 {cnt} 次，以下是被验证过能带来播放量的桥段：\n"
      ));
    }
    for (i, r) in same_work.iter().enumerate() {
      block.push_str(&format!("{}. {}\n", i + 1, point_line(r, false)));
    }
    let used: Vec<String> = resp
      .pointer("/work_summary/used_hooks")
      .and_then(|v| v.as_array())
      .map(|a| {
        a.iter()
          .filter_map(|h| h.as_str())
          .map(|s| s.to_string())
          .collect()
      })
      .unwrap_or_default();
    if !used.is_empty() {
      block.push_str("以下开头已被本公司历史稿件使用过，本次必须换不同的切入角度，禁止雷同：\n");
      for (i, h) in used.iter().enumerate() {
        block.push_str(&format!("{}. {h}\n", i + 1));
      }
    }
    text.push_str(&block);
    text.push('\n');
    counts.push(format!("作品记忆 {}", same_work.len()));
  }

  if !same_track.is_empty() {
    let mut block = String::from("— 同赛道高热开头（借鉴其钩子写法）—\n");
    for (i, r) in same_track.iter().enumerate() {
      block.push_str(&format!("{}. {}\n", i + 1, point_line(r, false)));
    }
    text.push_str(&block);
    text.push('\n');
    counts.push(format!("同赛道 {}", same_track.len()));
  }

  if !cross_track.is_empty() {
    let mut block = String::from("— 跨赛道通用网感（借鉴其结构张力与情绪节奏）—\n");
    for (i, r) in cross_track.iter().enumerate() {
      block.push_str(&format!("{}. {}\n", i + 1, point_line(r, true)));
    }
    text.push_str(&block);
    text.push('\n');
    counts.push(format!("跨赛道 {}", cross_track.len()));
  }

  if !fallback.is_empty() {
    let mut block = String::from("— 全库最热参考 —\n");
    for (i, r) in fallback.iter().enumerate() {
      block.push_str(&format!("{}. {}\n", i + 1, point_line(r, false)));
    }
    text.push_str(&block);
    counts.push(format!("兜底 {}", fallback.len()));
  }

  Some((counts.join(" / "), text, detail))
}

/// 同作品同集历史记录的一行摘要：标题 · 播放量 · 发布时间。
fn work_record_line(r: &Value) -> String {
  let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("未命名");
  let play = r
    .get("play_count")
    .and_then(|v| v.as_i64())
    .or_else(|| {
      r.get("play_count")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
    })
    .unwrap_or(0);
  let ct = r
    .get("created_at")
    .or_else(|| r.get("create_time"))
    .and_then(|v| v.as_str())
    .unwrap_or("");
  let url = r.get("source_url").and_then(|v| v.as_str()).unwrap_or("");
  let mut line = format!("《{title}》 播放量 {play}");
  if !ct.is_empty() {
    line.push_str(&format!(" · 处理于 {ct}"));
  }
  if !url.is_empty() {
    line.push_str(&format!(" · {url}"));
  }
  line
}

/// 拉取公司可见视频列表（自有账号 + 对标账号），供 AI 洗稿页面勾选。
#[tauri::command]
pub async fn ai_rewrite_list_videos(
  platform: Option<String>,
  keyword: Option<String>,
  min_play: Option<i64>,
  page: Option<u32>,
  page_size: Option<u32>,
  skip_done: Option<i32>,
) -> Result<Value, String> {
  AI_REWRITE.ensure_login().await?;
  AI_REWRITE
    .fetch_videos(json!({
        "platform": platform.unwrap_or_default(),
        "keyword": keyword.unwrap_or_default(),
        "min_play": min_play.unwrap_or(0),
        "page": page.unwrap_or(1).max(1),
        "page_size": page_size.unwrap_or(20).clamp(1, 100),
        "skip_done": skip_done.unwrap_or(0),
    }))
    .await
}

/// 已入库的洗稿成果列表（供页面「历史记录」浏览）。
#[tauri::command]
pub async fn ai_rewrite_results(
  keyword: Option<String>,
  status: Option<String>,
  only_mine: Option<i32>,
  page: Option<u32>,
  page_size: Option<u32>,
) -> Result<Value, String> {
  AI_REWRITE.ensure_login().await?;
  AI_REWRITE
    .fetch_results(json!({
        "keyword": keyword.unwrap_or_default(),
        "status": status.unwrap_or_default(),
        "only_mine": only_mine.unwrap_or(0),
        "page": page.unwrap_or(1).max(1),
        "page_size": page_size.unwrap_or(20).clamp(1, 100),
    }))
    .await
}

/// 单条成果详情：转写 + 爆点分析 + 洗稿文案 + 打分。
#[tauri::command]
pub async fn ai_rewrite_result(task_id: i64) -> Result<Value, String> {
  AI_REWRITE.ensure_login().await?;
  AI_REWRITE.fetch_result(task_id).await
}

/// 按指定链接跑批：入参为前端勾选 / 粘贴的视频链接。
/// 每项至少包含 source_url（稳定页面链接，优先）或 play_url（CDN 直链，兜底），
/// 可附带 video_id/title/platform 等元数据。
/// `wait=false`（AI 助手入口）：校验通过即后台执行并立即返回任务清单，
/// 进度经 `ai_rewrite_running_tasks` / 事件流查询；默认等待全部完成后汇总。
#[tauri::command]
pub async fn ai_rewrite_run_links<R: tauri::Runtime>(
  app: tauri::AppHandle<R>,
  items: Vec<Value>,
  concurrency: Option<usize>,
  wait: Option<bool>,
) -> Result<Value, String> {
  use tauri::Emitter;

  AI_REWRITE.ensure_login().await?;

  // 余额预检
  let pre = AI_REWRITE.precheck().await?;
  if pre.get("config_ready").and_then(|b| b.as_bool()) == Some(false) {
    return Err("服务器尚未配置统一的 AI 密钥，请联系管理员".to_string());
  }

  // 本地文件/文件夹展开：source_url 是本地文件夹 → 展开为其中全部视频；
  // 本地文件 → 标记 platform=local（后续跳过云端查重与播放量补拉）
  let mut expanded: Vec<Value> = Vec::with_capacity(items.len());
  for it in items {
    let src = it
      .get("source_url")
      .or_else(|| it.get("play_url"))
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    if !src.is_empty() && std::path::Path::new(&src).is_dir() {
      for f in crate::video_downloader::list_local_videos(&src) {
        let mut obj = it.clone();
        if let Value::Object(ref mut m) = obj {
          m.insert("source_url".to_string(), json!(f));
          m.insert("platform".to_string(), json!("local"));
          let name = std::path::Path::new(&f)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("本地视频")
            .to_string();
          m.insert("title".to_string(), json!(name));
        }
        expanded.push(obj);
      }
    } else {
      let mut obj = it;
      if crate::video_downloader::is_local_video(&src) {
        if let Value::Object(ref mut m) = obj {
          if m
            .get("platform")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
          {
            m.insert("platform".to_string(), json!("local"));
          }
        }
      }
      expanded.push(obj);
    }
  }
  let items = expanded;

  // 归一化：统一以稳定页面链接（source_url）作为下载链接；
  // play_url 是带签名的 CDN 直链、有时效，仅在没有 source_url 时兜底
  let rows: Vec<Value> = items
    .into_iter()
    .filter_map(|it| {
      let url = it
        .get("source_url")
        .or_else(|| it.get("play_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
      if url.is_empty() {
        return None;
      }
      let mut obj = it;
      if let Value::Object(ref mut m) = obj {
        m.insert("source_url".to_string(), json!(url));
      }
      Some(obj)
    })
    .collect();

  if rows.is_empty() {
    return Err("请先勾选视频或粘贴链接".to_string());
  }

  // 工具检查（复用「视频下载」的工具）；全本地文件无需 yt-dlp
  let tools = crate::video_downloader::check_tools().await;
  let has_remote = rows.iter().any(|r| {
    let src = r.get("source_url").and_then(|v| v.as_str()).unwrap_or("");
    !crate::video_downloader::is_local_video(src)
  });
  if has_remote && !tools.yt_dlp {
    return Err("未找到 yt-dlp，请先在「视频下载」页安装下载工具".to_string());
  }
  if !tools.ffmpeg {
    return Err("未找到 ffmpeg，无法抽取音频，请先在「视频下载」页安装下载工具".to_string());
  }

  // 领取密钥 + 提示词
  let creds = AI_REWRITE.credentials().await?;

  let total = rows.len();
  // 助手入口的后台模式：任务条目先落盘（任务列表 Tab 立即可见并随事件流更新），
  // 再逐条 detach spawn，函数立即返回。
  let fire_and_forget = wait == Some(false);
  let started: Vec<Value> = rows
    .iter()
    .map(|row| {
      let vid = row.get("video_id").and_then(|v| v.as_str()).unwrap_or("");
      let src = row.get("source_url").and_then(|v| v.as_str()).unwrap_or("");
      json!({
          "task_key": if vid.is_empty() { url_task_key(src) } else { vid.to_string() },
          "video_id": vid,
          "title": row.get("title").and_then(|v| v.as_str()).unwrap_or(""),
          "source_url": src,
          "platform": row.get("platform").and_then(|v| v.as_str()).unwrap_or(""),
      })
    })
    .collect();
  if fire_and_forget {
    let mut stored = tasks_read();
    for (i, summary) in started.iter().enumerate() {
      let row = &rows[i];
      stored.push(json!({
          "key": summary.get("task_key").cloned().unwrap_or(Value::Null),
          "title": summary.get("title").cloned().unwrap_or(Value::Null),
          "url": summary.get("source_url").cloned().unwrap_or(Value::Null),
          "platform": summary.get("platform").cloned().unwrap_or(Value::Null),
          "stage": "pending",
          "percent": 0,
          "message": "",
          "speedText": "",
          "logs": [],
          "status": "pending",
          "via": "assistant",
          "_started_at": crate::proxy_manager::now_secs(),
          "_row": row,
      }));
    }
    tasks_write(&stored)?;
  }

  // 并发度：默认 3，最多 6，避免同时下载 / 调用过多
  let concurrency = concurrency.unwrap_or(3).clamp(1, 6).min(total.max(1));

  let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
  let run_one = |app: tauri::AppHandle<R>,
                 creds: AiCredentials,
                 sem: std::sync::Arc<tokio::sync::Semaphore>,
                 row: Value,
                 i: usize,
                 total: usize| async move {
    let _permit = sem.acquire_owned().await.ok();
    match process_video(&app, &row, &creds, i + 1, total).await {
      Ok(r) => r,
      Err(e) => {
        let vid = row.get("video_id").and_then(|v| v.as_str()).unwrap_or("");
        let src = row
          .get("source_url")
          .or_else(|| row.get("play_url"))
          .and_then(|v| v.as_str())
          .unwrap_or("");
        let key = if vid.is_empty() {
          url_task_key(src)
        } else {
          vid.to_string()
        };
        let progress = json!({
            "task_key": key,
            "index": i + 1,
            "total": total,
            "video_id": vid,
            "source_url": src,
            "stage": "error",
            "percent": 100,
            "message": e,
            "speed_text": "",
        });
        log::warn!("[ai_rewrite] 任务失败 video_id={vid} src={src} 错误={e}");
        tokio::spawn(track_running(progress.clone()));
        let _ = app.emit("ai-rewrite:progress", progress);
        let _ = app.emit(
          "ai-rewrite:log",
          json!({
              "task_key": key,
              "index": i + 1,
              "total": total,
              "video_id": vid,
              "source_url": src,
              "level": "error",
              "message": e,
              "time": now_hms(),
          }),
        );
        json!({ "video_id": vid, "source_url": src, "success": false, "error": e })
      }
    }
  };

  if fire_and_forget {
    for (i, row) in rows.into_iter().enumerate() {
      tokio::spawn(run_one(
        app.clone(),
        creds.clone(),
        sem.clone(),
        row,
        i,
        total,
      ));
    }
    return Ok(json!({
        "success": true,
        "async_started": true,
        "total": total,
        "tasks": started,
    }));
  }

  let mut set: tokio::task::JoinSet<Value> = tokio::task::JoinSet::new();
  for (i, row) in rows.into_iter().enumerate() {
    set.spawn(run_one(
      app.clone(),
      creds.clone(),
      sem.clone(),
      row,
      i,
      total,
    ));
  }

  let mut results: Vec<Value> = Vec::with_capacity(total);
  while let Some(joined) = set.join_next().await {
    if let Ok(v) = joined {
      results.push(v);
    }
  }

  let done = results
    .iter()
    .filter(|r| r.get("success").and_then(|b| b.as_bool()) == Some(true))
    .count();
  let failed = total.saturating_sub(done);

  Ok(json!({
      "success": true,
      "total": total,
      "done": done,
      "failed": failed,
      "results": results,
  }))
}

// ============================================================
// 导出剪映中英文字幕（SRT）
// ============================================================

/// 默认翻译提示词（与服务器 ai_rewrite_lib.php 保持一致）。
const DEFAULT_TRANSLATE_PROMPT: &str = "你是专业翻译。请把下面的短视频口播文案翻译成中文。\n\
   要求：\n\
   1. 只翻译，不改写、不增删内容、不总结。\n\
   2. 保持口语化，贴合短视频口播的语气与节奏。\n\
   3. 保留原文的段落与换行结构。\n\
   4. 专有名词（人名、片名、平台名）保留原文或通用译名，不要生造。\n\
   5. 只输出译文本身，不要任何解释、标题或引号。";

/// 把文案切成字幕条目：按行拆分；单行过长时再按句末标点切分。
fn split_into_cues(text: &str) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  for line in text.split('\n') {
    let line = line.trim();
    if line.is_empty() {
      continue;
    }
    if line.chars().count() <= 80 {
      out.push(line.to_string());
      continue;
    }
    let mut cur = String::new();
    for ch in line.chars() {
      cur.push(ch);
      if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；') {
        let t = cur.trim();
        if !t.is_empty() {
          out.push(t.to_string());
        }
        cur.clear();
      }
    }
    let t = cur.trim();
    if !t.is_empty() {
      out.push(t.to_string());
    }
  }
  out
}

/// 秒 → SRT 时间戳（HH:MM:SS,mmm）。
fn srt_time(secs: f64) -> String {
  let s = secs.max(0.0);
  let h = (s / 3600.0).floor() as u64;
  let m = ((s % 3600.0) / 60.0).floor() as u64;
  let sec = (s % 60.0).floor() as u64;
  let ms = ((s - s.floor()) * 1000.0).round() as u64;
  let ms = if ms >= 1000 { 999 } else { ms };
  format!("{h:02}:{m:02}:{sec:02},{ms:03}")
}

/// 单条文案翻译成中文。
async fn translate_text(
  cfg: &AiEndpointConfig,
  system_prompt: &str,
  text: &str,
) -> Result<String, String> {
  let (out, _usage) = llm_chat(cfg, system_prompt, text).await?;
  Ok(out.trim().to_string())
}

/// 从洗稿 JSON 中收集字幕条目（hook + body + cta），逐句一条。
fn cues_from_rewrite(rewrite: &Value) -> Vec<String> {
  let mut text = String::new();
  if let Some(hook) = rewrite.get("hook").and_then(|v| v.as_str()) {
    text.push_str(hook);
    text.push('\n');
  }
  match rewrite.get("body") {
    Some(Value::Array(arr)) => {
      for it in arr {
        if let Some(s) = it.as_str() {
          text.push_str(s);
          text.push('\n');
        }
      }
    }
    Some(Value::String(s)) => {
      text.push_str(s);
      text.push('\n');
    }
    _ => {}
  }
  if let Some(cta) = rewrite.get("cta").and_then(|v| v.as_str()) {
    text.push_str(cta);
  }
  split_cues_per_sentence(&text)
}

/// 把文案逐句切成字幕条目（洗稿导出用）：每句一条，不看行长。
/// 英文句末标点须后跟空白或行尾才切（避免 3.5、U.S. 被误切）；中文句末标点直接切。
fn split_cues_per_sentence(text: &str) -> Vec<String> {
  let chars: Vec<char> = text.chars().collect();
  let mut out: Vec<String> = Vec::new();
  let mut cur = String::new();
  for (i, &ch) in chars.iter().enumerate() {
    cur.push(ch);
    let is_end = matches!(ch, '。' | '！' | '？' | '；')
      || (matches!(ch, '.' | '!' | '?' | ';')
        && (i + 1 == chars.len() || chars[i + 1].is_whitespace()));
    if is_end {
      let t = cur.trim();
      if !t.is_empty() {
        out.push(t.to_string());
      }
      cur.clear();
    }
  }
  let t = cur.trim();
  if !t.is_empty() {
    out.push(t.to_string());
  }
  out
}

/// 始终生成两份独立 SRT（原文 + 中文译文），剪映可直接分别导入。
/// 未配置翻译模型时 srt_zh 为空。
#[tauri::command]
pub async fn ai_rewrite_build_srt(
  rewrite: Option<Value>,
  transcript: Option<String>,
  title: Option<String>,
  duration: Option<u32>,
  segments: Option<Value>,
) -> Result<Value, String> {
  // 真实时间轴：ASR verbose_json 的逐段 timestamps（仅转写字幕用；洗稿文稿没有可对齐的真实时间）
  let mut timed: Vec<(f64, f64, String)> = Vec::new();
  if rewrite.is_none() {
    if let Some(Value::Array(arr)) = segments.as_ref() {
      for s in arr {
        let st = s.get("start").and_then(|x| x.as_f64()).unwrap_or(-1.0);
        let en = s.get("end").and_then(|x| x.as_f64()).unwrap_or(-1.0);
        let tx = s
          .get("text")
          .and_then(|x| x.as_str())
          .unwrap_or("")
          .trim()
          .to_string();
        if st >= 0.0 && en > st && !tx.is_empty() {
          timed.push((st, en, tx));
        }
      }
    }
  }
  let use_real = !timed.is_empty();

  let mut cues: Vec<String> = if use_real {
    timed.iter().map(|(_, _, t)| t.clone()).collect()
  } else {
    match rewrite.as_ref() {
      Some(rw) => cues_from_rewrite(rw),
      None => Vec::new(),
    }
  };
  if cues.is_empty() {
    if let Some(tr) = transcript.as_ref() {
      cues = split_into_cues(tr);
    }
  }
  if cues.is_empty() {
    return Err("暂无可导出的文案".to_string());
  }

  // 翻译：复用服务器的翻译配置（免费翻译模型）。未配置时跳过。
  let mut zh: Vec<Option<String>> = vec![None; cues.len()];
  let mut translated = false;
  let mut note = String::new();
  match AI_REWRITE.credentials().await {
    Ok(creds) => {
      let cfg = creds.translate.clone();
      if cfg.base_url.trim().is_empty() || cfg.api_key.trim().is_empty() {
        note = "未配置翻译模型，仅导出原文".to_string();
      } else {
        let prompt = if creds.prompts.translate.trim().is_empty() {
          DEFAULT_TRANSLATE_PROMPT.to_string()
        } else {
          creds.prompts.translate.clone()
        };
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
        let mut set: tokio::task::JoinSet<(usize, Result<String, String>)> =
          tokio::task::JoinSet::new();
        for (i, c) in cues.iter().enumerate() {
          let cfg = cfg.clone();
          let prompt = prompt.clone();
          let c = c.clone();
          let sem = sem.clone();
          set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            (i, translate_text(&cfg, &prompt, &c).await)
          });
        }
        let mut first_err: Option<String> = None;
        while let Some(joined) = set.join_next().await {
          if let Ok((i, r)) = joined {
            match r {
              Ok(t) => zh[i] = Some(t),
              Err(e) => {
                if first_err.is_none() {
                  first_err = Some(e);
                }
              }
            }
          }
        }
        if zh.iter().all(|z| z.is_some()) {
          translated = true;
        } else if let Some(e) = first_err {
          note = format!("翻译未完成，仅导出原文：{e}");
        }
      }
    }
    Err(e) => note = format!("获取翻译配置失败，仅导出原文：{e}"),
  }

  // 时间轴：优先 ASR 真实逐段 timestamps；无则按字符数估算（约每秒 4 字符，最少 1.6s），
  // 并可选按视频总时长等比缩放
  let mut starts: Vec<f64> = Vec::with_capacity(cues.len());
  let mut ends: Vec<f64> = Vec::with_capacity(cues.len());
  if use_real {
    for (st, en, _) in timed.iter() {
      starts.push(*st);
      ends.push(*en);
    }
  } else {
    let raw: Vec<f64> = cues
      .iter()
      .map(|c| (c.chars().count() as f64 / 4.0).max(1.6))
      .collect();
    let raw_total: f64 = raw.iter().sum::<f64>() + 0.15 * (cues.len().max(1) - 1) as f64;
    let scale = match duration {
      Some(d) if d > 0 && raw_total > 0.0 => (d as f64) / raw_total,
      _ => 1.0,
    };
    let mut t = 0.0f64;
    for dur in raw {
      starts.push(t);
      t += dur * scale;
      ends.push(t);
      t += 0.15 * scale;
    }
  }

  // 同时构建两份独立 SRT
  let mut srt_en = String::new();
  let mut srt_zh = String::new();
  for (i, c) in cues.iter().enumerate() {
    let head = format!(
      "{}\n{} --> {}\n",
      i + 1,
      srt_time(starts[i]),
      srt_time(ends[i])
    );
    srt_en.push_str(&head);
    srt_en.push_str(c);
    srt_en.push_str("\n\n");
    if let Some(Some(z)) = zh.get(i) {
      srt_zh.push_str(&head);
      srt_zh.push_str(z);
      srt_zh.push_str("\n\n");
    }
  }

  Ok(json!({
      "success": true,
      "cue_count": cues.len(),
      "real_timing": use_real,
      "srt_en": srt_en,
      "srt_zh": srt_zh,
      "translated": translated,
      "title": title.unwrap_or_default(),
      "message": if note.is_empty() {
          format!("已生成 {} 条字幕", cues.len())
      } else {
          note
      },
  }))
}

// ============================================================
// 任务列表持久化（本地 tasks.json）
// ============================================================

/// 持久化任务列表文件路径：{data_dir}/ai-rewrite-tasks.json
fn tasks_store_path() -> PathBuf {
  crate::app_dirs::data_dir().join("ai-rewrite-tasks.json")
}

/// 读取已落盘的任务列表。文件不存在 / 损坏时返回空列表。
fn tasks_read() -> Vec<Value> {
  let path = tasks_store_path();
  let Ok(text) = std::fs::read_to_string(&path) else {
    return Vec::new();
  };
  let Ok(v) = serde_json::from_str::<Value>(&text) else {
    return Vec::new();
  };
  match v {
    Value::Array(arr) => arr,
    _ => Vec::new(),
  }
}

/// 原子写盘：先写临时文件再改名，避免断电/崩溃留下半个文件。
fn tasks_write(tasks: &[Value]) -> Result<(), String> {
  let path = tasks_store_path();
  let dir = path.parent().ok_or("无法确定数据目录")?;
  std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
  let tmp = dir.join(format!("ai-rewrite-tasks.json.tmp.{}", std::process::id()));
  let data = serde_json::to_vec_pretty(tasks).map_err(|e| format!("序列化失败：{e}"))?;
  std::fs::write(&tmp, &data).map_err(|e| format!("写入失败：{e}"))?;
  std::fs::rename(&tmp, &path).map_err(|e| format!("落盘失败：{e}"))?;
  Ok(())
}

/// 读取持久化任务列表（页面进入时调用）。
#[tauri::command]
pub fn ai_rewrite_load_tasks() -> Result<Vec<Value>, String> {
  Ok(tasks_read())
}

/// 全量保存任务列表（页面任务变化后调用：新增完成 / 删除 / 清空）。
#[tauri::command]
pub fn ai_rewrite_save_tasks(tasks: Vec<Value>) -> Result<Value, String> {
  tasks_write(&tasks)?;
  Ok(json!({ "success": true, "count": tasks.len() }))
}

/// 清空任务列表（覆盖为空数组）。
#[tauri::command]
pub fn ai_rewrite_clear_tasks() -> Result<Value, String> {
  tasks_write(&[])?;
  Ok(json!({ "success": true, "count": 0 }))
}

/// 读取接口配置（与云端后台「AI 接口配置」同一份数据；接口设置对话框打开时回显）。
#[tauri::command]
pub async fn ai_rewrite_get_api_config() -> Result<AiApiConfig, String> {
  AI_REWRITE.api_config().await
}

/// 保存接口配置到服务器（云端后台与本程序实时同步；下次洗稿立即生效）。
#[tauri::command]
pub async fn ai_rewrite_save_api_config(config: AiApiConfig) -> Result<Value, String> {
  AI_REWRITE.save_api_config(&config).await?;
  Ok(json!({ "success": true }))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn per_sentence_cues() {
    let cues = split_cues_per_sentence(
      "He painted it. No hesitation!\nWhat happened next? He sold 3.5 copies anyway",
    );
    assert_eq!(cues.len(), 4);
    assert_eq!(cues[0], "He painted it.");
    assert_eq!(cues[2], "What happened next?");
    assert_eq!(cues[3], "He sold 3.5 copies anyway");
    let zh = split_cues_per_sentence("你敢信吗？全校最凶的霸王，突然一见钟情！");
    assert_eq!(zh.len(), 2);
  }

  #[test]
  fn extract_plain_object() {
    let v = extract_json(r#"{"a":1}"#).unwrap();
    assert_eq!(v.get("a").unwrap().as_i64(), Some(1));
  }

  #[test]
  fn episode_hint_from_title() {
    assert_eq!(extract_episode_hint("Spongebob S3E12 full story"), 12);
    assert_eq!(extract_episode_hint("Doraemon EP45 讲解"), 45);
    assert_eq!(extract_episode_hint("Episode 5 was insane"), 5);
    assert_eq!(extract_episode_hint("哆啦A梦 第12集 解说"), 12);
    assert_eq!(extract_episode_hint("海贼王 第1024话"), 1024);
    assert_eq!(extract_episode_hint("蜡笔小新 第二十三集"), 23);
    assert_eq!(extract_episode_hint("火影 105集 精彩片段"), 105);
    assert_eq!(extract_episode_hint("哆啦A梦第十二话"), 12);
    // 没有线索时为 0
    assert_eq!(extract_episode_hint("Before he was even born..."), 0);
    assert_eq!(
      extract_episode_hint("This cartoon will ruin your childhood"),
      0
    );
    assert_eq!(extract_episode_hint(""), 0);
  }

  #[test]
  fn chinese_number_to_int() {
    assert_eq!(cn_num_to_int("十二"), 12);
    assert_eq!(cn_num_to_int("二十三"), 23);
    assert_eq!(cn_num_to_int("一百零五"), 105);
    assert_eq!(cn_num_to_int("两百"), 200);
    assert_eq!(cn_num_to_int("九"), 9);
    assert_eq!(cn_num_to_int("abc"), 0);
  }

  #[test]
  fn platform_inferred_from_url() {
    assert_eq!(
      infer_platform_from_url("https://www.youtube.com/shorts/jFmOE3Dl-CQ"),
      "youtube"
    );
    assert_eq!(
      infer_platform_from_url("https://youtu.be/jFmOE3Dl-CQ"),
      "youtube"
    );
    assert_eq!(
      infer_platform_from_url("https://www.tiktok.com/@user/video/123"),
      "tiktok"
    );
    assert_eq!(
      infer_platform_from_url("https://v.douyin.com/iabc123/"),
      "douyin"
    );
    assert_eq!(infer_platform_from_url("D:\\videos\\movie.mp4"), "");
  }

  #[test]
  fn extract_fenced_object() {
    let raw = "```json\n{\"title\":\"x\",\"body\":[\"a\",\"b\"]}\n```";
    let v = extract_json(raw).unwrap();
    assert_eq!(v.get("title").unwrap().as_str(), Some("x"));
    assert_eq!(v.get("body").unwrap().as_array().unwrap().len(), 2);
  }

  #[test]
  fn extract_fenced_with_trailing_prose() {
    let raw = "好的，结果如下：\n```json\n{\"score\":88}\n```\n希望对你有帮助。";
    let v = extract_json(raw).unwrap();
    assert_eq!(v.get("score").unwrap().as_i64(), Some(88));
  }

  #[test]
  fn extract_truncated_fenced_object() {
    // 模拟被 max_tokens 截断：围栏未闭合、字符串/数组/对象都没收尾
    let raw = "```json\n{\"title\":\"标题\",\"hook\":\"开场\",\"body\":[\"第一段\",\"第二段";
    let v = extract_json(raw).unwrap();
    assert_eq!(v.get("title").unwrap().as_str(), Some("标题"));
    assert_eq!(v.get("body").unwrap().as_array().unwrap().len(), 2);
  }

  #[test]
  fn extract_truncated_dangling_key() {
    // 截断在“只有键、没有值”的位置，应丢弃该键并闭合
    let v = extract_json("{\"title\":\"标题\",\"cta\":").unwrap();
    assert_eq!(v.get("title").unwrap().as_str(), Some("标题"));
    assert!(v.get("cta").is_none());
  }

  #[test]
  fn extract_bare_newline_in_string() {
    // 字符串内含裸换行（严格 JSON 非法）应被自动转义
    let v = extract_json("{\"body\":[\"第一行\n第二行\"]}").unwrap();
    let body = v.get("body").unwrap().as_array().unwrap();
    assert_eq!(body.len(), 1);
    assert!(body[0].as_str().unwrap().contains("第二行"));
  }

  #[test]
  fn extract_rejects_non_json() {
    assert!(extract_json("这是一段纯文本，没有任何 JSON").is_none());
  }

  #[test]
  fn api_config_json_roundtrip() {
    // 服务器 ai_config_get 下发的形状（proxy.type 带改名、snake_case 字段）
    let v = json!({
        "asr": {
            "provider": "openai_compatible",
            "base_url": "https://api.siliconflow.cn/v1",
            "api_key": "sk-asr",
            "model": "SenseVoice",
            "proxy": { "host": "1.2.3.4:8080", "type": "http", "user": "", "pass": "" }
        },
        "llm": {
            "provider": "anthropic",
            "base_url": "https://api.anthropic.com",
            "api_key": "sk-ant",
            "model": "claude-x",
            "proxy": { "host": "5.6.7.8:1080", "type": "socks5", "user": "u", "pass": "p" }
        },
        "translate": {
            "provider": "openai_compatible",
            "base_url": "",
            "api_key": "",
            "model": "",
            "proxy": { "host": "1.2.3.4:8080", "type": "http", "user": "", "pass": "" }
        }
    });
    let cfg: AiApiConfig = serde_json::from_value(v).unwrap();
    assert_eq!(cfg.llm.provider, "anthropic");
    assert_eq!(cfg.llm.proxy.kind, "socks5");
    assert_eq!(cfg.llm.proxy.host, "5.6.7.8:1080");
    assert_eq!(cfg.asr.proxy.host, "1.2.3.4:8080");
    // 保存时的序列化形状能原样读回
    let back: AiApiConfig = serde_json::from_value(serde_json::to_value(&cfg).unwrap()).unwrap();
    assert_eq!(back.llm.api_key, "sk-ant");
    // 缺省字段反序列化不报错
    let empty: AiApiConfig = serde_json::from_value(json!({})).unwrap();
    assert!(empty.llm.base_url.is_empty());
    assert!(empty.llm.proxy.host.is_empty());
  }

  #[test]
  fn classify_proxy_empty_host_is_direct() {
    assert_eq!(
      classify_proxy(&AiProxyConfig::default()),
      ProxyTarget::Direct
    );
    assert_eq!(
      classify_proxy(&AiProxyConfig {
        host: "   ".to_string(),
        kind: "socks5".to_string(),
        ..Default::default()
      }),
      ProxyTarget::Direct
    );
  }

  #[test]
  fn classify_proxy_plain_schemes() {
    let socks = AiProxyConfig {
      host: "1.2.3.4:1080".to_string(),
      kind: "socks5".to_string(),
      user: "u".to_string(),
      pass: "p".to_string(),
    };
    assert_eq!(
      classify_proxy(&socks),
      ProxyTarget::Plain {
        url: "socks5://u:p@1.2.3.4:1080".to_string()
      }
    );
    let http = AiProxyConfig {
      host: "proxy.example.com:8080".to_string(),
      kind: "HTTPS".to_string(),
      ..Default::default()
    };
    assert_eq!(
      classify_proxy(&http),
      ProxyTarget::Plain {
        url: "http://proxy.example.com:8080".to_string()
      }
    );
  }

  #[test]
  fn classify_proxy_tunnel_protocols_use_xray() {
    // kind 指明高级协议：host 补全成 URI
    let by_kind = AiProxyConfig {
      host: "example.com:443".to_string(),
      kind: "VLESS".to_string(),
      ..Default::default()
    };
    assert_eq!(
      classify_proxy(&by_kind),
      ProxyTarget::Xray {
        uri: "vless://example.com:443".to_string()
      }
    );
    // host 自带完整 URI（代理中心保存的形态）
    let uri = "vless://uuid@example.com:443?security=reality&type=tcp#AI线路";
    let by_uri = AiProxyConfig {
      host: uri.to_string(),
      kind: String::new(),
      ..Default::default()
    };
    assert_eq!(
      classify_proxy(&by_uri),
      ProxyTarget::Xray {
        uri: uri.to_string()
      }
    );
    let trojan = AiProxyConfig {
      host: "trojan://pass@example.com:443?sni=a.com".to_string(),
      kind: "trojan".to_string(),
      ..Default::default()
    };
    assert_eq!(
      classify_proxy(&trojan),
      ProxyTarget::Xray {
        uri: "trojan://pass@example.com:443?sni=a.com".to_string()
      }
    );
  }
}
