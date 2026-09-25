#![allow(dead_code, clippy::too_many_arguments)]

//! Bwbrowser Cloud Authentication
//! 对接 bwbrowser_sync.php 的账号密码登录系统
//!
//! API 文档见：https://www.yacm.xin/tk/bwbrowser_sync.php
//! 认证方式：username + password (POST action=login)

use chrono::Utc;
use lazy_static::lazy_static;
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Runtime};

use crate::cloud_auth::{CloudAuthState, CloudUser, Entitlements};
use crate::settings_manager::SettingsManager;

/// Deserialize either an integer or a string into Option<String>.
/// PHP APIs often return numeric fields as integers even when they are
/// conceptually strings (status codes, IDs, timestamps, etc.).
fn deserialize_int_or_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
  D: Deserializer<'de>,
{
  use serde::de::Error;

  let value = serde_json::Value::deserialize(deserializer)?;
  match value {
    serde_json::Value::String(s) => Ok(Some(s)),
    serde_json::Value::Number(n) => Ok(Some(n.to_string())),
    serde_json::Value::Null => Ok(None),
    serde_json::Value::Bool(b) => Ok(Some(b.to_string())),
    _ => Err(D::Error::custom("expected string, number, bool, or null")),
  }
}

/// Same as above but for Vec<String> elements that might be integers.
fn deserialize_int_or_string_vec<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
  D: Deserializer<'de>,
{
  use serde::de::Error;

  let value = serde_json::Value::deserialize(deserializer)?;
  match value {
    serde_json::Value::Array(arr) => {
      let mut result = Vec::new();
      for v in arr {
        match v {
          serde_json::Value::String(s) => result.push(s),
          serde_json::Value::Number(n) => result.push(n.to_string()),
          _ => return Err(D::Error::custom("array element must be string or number")),
        }
      }
      Ok(Some(result))
    }
    serde_json::Value::Null => Ok(None),
    _ => Err(D::Error::custom("expected array or null")),
  }
}

/// 反序列化 start_urls，兼容字符串数组 ["url1"] 和对象数组 [{"url":"url1","sort_order":0}]
fn deserialize_start_urls<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
  match value {
    None | Some(serde_json::Value::Null) => Ok(None),
    Some(serde_json::Value::Array(arr)) => {
      let mut result = Vec::new();
      for item in arr {
        match item {
          serde_json::Value::String(s) => result.push(s),
          serde_json::Value::Object(obj) => {
            if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
              result.push(url.to_string());
            }
          }
          _ => {}
        }
      }
      Ok(Some(result))
    }
    _ => Ok(None),
  }
}

// ========== 配置 ==========

/// Bwbrowser 云端 API 地址（登录/同步用）
pub const BWBROWSER_API_URL: &str = "http://yacm.xin/tk/bwbrowser_sync.php";

/// 平台配置 API 地址（获取平台 creator_url）
pub const PLATFORM_CONFIG_API_URL: &str = "http://yacm.xin/tk/api_get_platform_config.php";

/// Wayfern Token 接口地址
pub const WAYFERN_TOKEN_API_URL: &str = "http://yacm.xin/tk/api_auth_wayfern_start.php";

/// Simprint 云端账号 API 地址列表（带 fallback，与 Simprint 保持一致）
const SIMPRINT_ACCOUNTS_URLS: &[&str] = &[
  "http://47.93.197.114/tk/simprint_accounts.php",
  "http://www.yacm.xin/tk/simprint_accounts.php",
  "http://www.baowenku.com/tk/simprint_accounts.php",
];

// ========== 响应类型 ==========

#[derive(Debug, Deserialize)]
struct BwbrowserLoginResponse {
  success: bool,
  message: Option<String>,
  user: Option<BwbrowserUser>,
  #[serde(default)]
  #[allow(dead_code)]
  snapshot_stats: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BwbrowserUser {
  user_id: i64,
  username: String,
  #[serde(default)]
  real_name: Option<String>,
  #[serde(default)]
  role: Option<String>,
  #[serde(
    default,
    rename = "avatar_url",
    alias = "dingtalk_avatar",
    alias = "avatar"
  )]
  avatar: Option<String>,
  #[serde(default)]
  company_id: Option<i64>,
  #[serde(default)]
  company_name: Option<String>,
  #[serde(default)]
  company_code: Option<String>,
  #[serde(default)]
  is_super_admin: Option<bool>,
  #[serde(default)]
  email: Option<String>,
  #[serde(default)]
  phone: Option<String>,
  #[serde(default, rename = "is_pro")]
  is_pro: Option<bool>,
  #[serde(default, rename = "plan_name")]
  plan_name: Option<String>,
  #[serde(default)]
  stats: Option<serde_json::Value>,
}

// ========== 本地存储的认证状态 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BwbrowserAuthState {
  /// Bwbrowser 用户名
  pub username: String,
  /// Bwbrowser 密码（bwbrowser_sync.php 每次请求都需要）
  pub password: String,
  /// Bwbrowser 用户信息
  pub bwbrowser_user: BwbrowserUser,
  /// 映射到 Bwbrowser 格式的用户信息
  pub cloud_user: CloudUser,
  /// 登录时间（ISO 8601）
  pub logged_in_at: String,
}

// ========== 管理器 ==========

pub struct BwbrowserAuthManager {
  client: Client,
  state: Mutex<Option<BwbrowserAuthState>>,
  /// 环境列表内存缓存（登录会话内有效）
  env_cache: Mutex<Option<(Vec<BwbrowserEnvironment>, std::time::Instant)>>,
  /// 代理列表内存缓存（登录会话内有效）
  proxy_cache: Mutex<Option<(Vec<BwbrowserProxy>, std::time::Instant)>>,
}

lazy_static! {
  pub static ref BWBROWSER_AUTH: BwbrowserAuthManager = BwbrowserAuthManager::new();
}

impl Default for BwbrowserAuthManager {
  fn default() -> Self {
    Self::new()
  }
}

impl BwbrowserAuthManager {
  pub fn new() -> Self {
    let state = Self::load_auth_state_from_disk();
    let client = Client::builder()
      .timeout(std::time::Duration::from_secs(15))
      .connect_timeout(std::time::Duration::from_secs(5))
      .danger_accept_invalid_certs(true) // 跳过 SSL 证书验证（调试用）
      .build()
      .unwrap_or_else(|_| Client::new());
    Self {
      client,
      state: Mutex::new(state),
      env_cache: Mutex::new(None),
      proxy_cache: Mutex::new(None),
    }
  }

  // --- 磁盘存储 ---

  fn get_settings_dir() -> PathBuf {
    SettingsManager::instance().get_settings_dir()
  }

  fn auth_state_file() -> PathBuf {
    Self::get_settings_dir().join("bwbrowser_auth.json")
  }

  fn load_auth_state_from_disk() -> Option<BwbrowserAuthState> {
    let path = Self::auth_state_file();
    if path.exists() {
      if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(state) = serde_json::from_str::<BwbrowserAuthState>(&content) {
          return Some(state);
        }
      }
    }
    None
  }

  fn save_auth_state_to_disk(state: &BwbrowserAuthState) -> Result<(), String> {
    let path = Self::auth_state_file();
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(())
  }

  fn clear_auth_state_from_disk() {
    let path = Self::auth_state_file();
    if path.exists() {
      let _ = fs::remove_file(&path);
    }
  }

  // --- 公共 API ---

  /// 是否已登录
  pub fn is_logged_in(&self) -> bool {
    self.state.lock().unwrap().is_some()
  }

  /// 获取当前用户（Bwbrowser 格式）
  pub fn get_user(&self) -> Option<CloudAuthState> {
    self.state.lock().unwrap().as_ref().map(|s| CloudAuthState {
      user: s.cloud_user.clone(),
      logged_in_at: s.logged_in_at.clone(),
    })
  }

  /// 获取 Bwbrowser 凭证（用于后续 API 调用）
  pub fn get_credentials(&self) -> Option<(String, String)> {
    self
      .state
      .lock()
      .unwrap()
      .as_ref()
      .map(|s| (s.username.clone(), s.password.clone()))
  }

  /// 获取当前登录用户的 ID
  pub fn get_user_id(&self) -> Option<i64> {
    self
      .state
      .lock()
      .unwrap()
      .as_ref()
      .map(|s| s.bwbrowser_user.user_id)
  }

  /// 当前用户是否为管理角色（manager/supervisor/leader/super_admin）
  pub fn is_manager_role(&self) -> bool {
    self.state.lock().unwrap().as_ref().is_some_and(|s| {
      let u = &s.bwbrowser_user;
      u.is_super_admin.unwrap_or(false)
        || matches!(
          u.role.as_deref(),
          Some("manager")
            | Some("supervisor")
            | Some("leader")
            | Some("admin")
            | Some("super_admin")
        )
    })
  }

  /// 登录
  pub async fn login<R: Runtime>(
    &self,
    app: &tauri::AppHandle<R>,
    username: &str,
    password: &str,
  ) -> Result<CloudAuthState, String> {
    log_bwbrowser(
      "login",
      &format!(
        "→ 请求登录: user={}, pwd={}",
        username,
        mask_password(password)
      ),
    );
    log_bwbrowser("login", &format!("  API URL: {}", BWBROWSER_API_URL));

    // 调用登录接口（form 编码）
    let form_data = format!(
      "action=login&username={}&password={}&client_version=BwBrowser {}",
      urlencode(username),
      urlencode(password),
      env!("BUILD_VERSION")
        .trim_start_matches('v')
        .trim_start_matches("dev-"),
    );

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("login", &format!("网络请求失败: {}", e));
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
      log_bwbrowser_error("login", &format!("读取响应失败: {}", e));
      format!("读取响应失败: {}", e)
    })?;

    log_bwbrowser(
      "login",
      &format!("← 响应状态: {}, 内容长度: {} bytes", status, body.len()),
    );
    log_bwbrowser("login", &format!("  响应内容: {}", body));

    if !status.is_success() {
      if body.trim().is_empty() {
        let msg = format!("登录失败 (HTTP {})：服务器返回空响应", status);
        log_bwbrowser_error("login", &msg);
        return Err(msg);
      }
      // 尝试从可能包含 PHP 警告的响应中提取 JSON
      if let Ok(err_resp) = parse_body::<BwbrowserLoginResponse>("login_error", &body) {
        let msg = err_resp
          .message
          .unwrap_or_else(|| format!("登录失败 (HTTP {})", status));
        log_bwbrowser_error("login", &msg);
        return Err(msg);
      }
      let preview = if body.len() > 200 {
        format!("{}", trunc(&body, 200))
      } else {
        body.to_string()
      };
      let msg = format!("登录失败 (HTTP {}): {}", status, preview);
      log_bwbrowser_error("login", &msg);
      return Err(msg);
    }

    let login_resp: BwbrowserLoginResponse = parse_body("login", &body)?;

    if !login_resp.success {
      let msg = login_resp.message.unwrap_or_else(|| "登录失败".to_string());
      log_bwbrowser_error("login", &msg);
      return Err(msg);
    }

    let bwbrowser_user = login_resp
      .user
      .ok_or_else(|| "服务器未返回用户信息".to_string())?;

    log_bwbrowser(
      "login",
      &format!(
        "✓ 登录成功: user={}, company={}",
        bwbrowser_user.username,
        bwbrowser_user.company_name.as_deref().unwrap_or("未知")
      ),
    );

    // 映射到 Bwbrowser CloudUser 格式
    let cloud_user = map_bwbrowser_to_cloud_user(&bwbrowser_user);

    let state = BwbrowserAuthState {
      username: username.to_string(),
      password: password.to_string(),
      bwbrowser_user,
      cloud_user: cloud_user.clone(),
      logged_in_at: Utc::now().to_rfc3339(),
    };

    // 保存状态
    if let Err(e) = Self::save_auth_state_to_disk(&state) {
      log::warn!("Failed to save bwbrowser auth state: {}", e);
    }
    *self.state.lock().unwrap() = Some(state);

    // 发送事件通知前端
    let _ = app.emit("cloud-auth-changed", ());

    Ok(CloudAuthState {
      user: cloud_user,
      logged_in_at: Utc::now().to_rfc3339(),
    })
  }

  /// 获取 Wayfern token（从爆文库服务器）
  pub async fn get_wayfern_token(&self) -> Option<String> {
    let state = self.state.lock().unwrap().clone();
    let (username, password) = match state {
      Some(s) => (s.username, s.password),
      None => return None,
    };

    let form_data = format!(
      "action=wayfern_start&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );

    let resp = match self
      .client
      .post(WAYFERN_TOKEN_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .timeout(std::time::Duration::from_secs(8))
      .send()
      .await
    {
      Ok(r) => r,
      Err(e) => {
        log_bwbrowser_error("wayfern_token", &format!("请求失败: {}", e));
        return None;
      }
    };

    if !resp.status().is_success() {
      log_bwbrowser_error("wayfern_token", &format!("HTTP 错误: {}", resp.status()));
      return None;
    }

    let body = match resp.text().await {
      Ok(b) => b,
      Err(e) => {
        log_bwbrowser_error("wayfern_token", &format!("读取响应失败: {}", e));
        return None;
      }
    };

    // 尝试解析 JSON: {"success": true, "token": "xxx"}
    #[derive(Deserialize)]
    struct WayfernResp {
      success: bool,
      token: Option<String>,
    }

    match serde_json::from_str::<WayfernResp>(&body) {
      Ok(r) if r.success => r.token,
      Ok(_) => {
        log_bwbrowser_error("wayfern_token", "服务器返回失败");
        None
      }
      Err(e) => {
        log_bwbrowser_error("wayfern_token", &format!("解析响应失败: {}", e));
        None
      }
    }
  }

  /// 登出
  pub async fn logout<R: Runtime>(&self, app: &tauri::AppHandle<R>) {
    *self.state.lock().unwrap() = None;
    self.invalidate_all_cache();
    Self::clear_auth_state_from_disk();
    let _ = app.emit("cloud-auth-changed", ());
  }

  /// 刷新用户信息
  pub fn refresh_profile_sync(&self) -> Result<CloudUser, String> {
    self
      .state
      .lock()
      .unwrap()
      .as_ref()
      .map(|s| s.cloud_user.clone())
      .ok_or_else(|| "未登录".to_string())
  }
}

// ========== 类型映射 ==========

/// 将 Bwbrowser 用户映射为 Bwbrowser CloudUser 格式
fn map_bwbrowser_to_cloud_user(bwbrowser: &BwbrowserUser) -> CloudUser {
  let is_paid = bwbrowser.role.as_deref() == Some("manager")
    || bwbrowser.role.as_deref() == Some("supervisor")
    || bwbrowser.is_super_admin.unwrap_or(false);

  let plan = if bwbrowser.is_super_admin.unwrap_or(false) {
    "enterprise".to_string()
  } else if bwbrowser.role.as_deref() == Some("manager") {
    "team".to_string()
  } else {
    "pro".to_string() // 默认给 pro 权限
  };

  let subscription_status = "active".to_string();

  // 构建 entitlements — 全部解锁
  let entitlements = Entitlements {
    active: true,
    browser_automation: true,
    cross_os_fingerprints: true,
    cloud_backup: true,
    team_collaboration: is_paid,
    cookie_bot: true,
    remote_interactive: true,
    remote_control: bwbrowser.is_super_admin.unwrap_or(false),
    agent_automation: true,
    profile_limit: 9999,
    requests_per_hour: 9999,
    remote_browser_hours: 9999,
  };

  CloudUser {
    id: bwbrowser.user_id.to_string(),
    email: bwbrowser.username.clone(),
    plan: plan.clone(),
    plan_period: Some("lifetime".to_string()),
    subscription_status,
    profile_limit: 9999,
    cloud_profiles_used: 0,
    proxy_bandwidth_limit_mb: 999999,
    proxy_bandwidth_used_mb: 0,
    proxy_bandwidth_extra_mb: 0,
    effective_plan: Some(plan),
    team_id: bwbrowser.company_id.map(|id| id.to_string()),
    team_name: bwbrowser.company_name.clone(),
    team_role: bwbrowser.role.clone(),
    device_ordinal: Some(1),
    device_count: Some(1),
    is_primary_device: Some(true),
    real_name: bwbrowser.real_name.clone(),
    avatar: bwbrowser.avatar.clone(),
    plan_name: bwbrowser.plan_name.clone(),
    stats: bwbrowser.stats.clone(),
    entitlements: Some(entitlements),
  }
}

// ========== 工具函数 ==========

fn urlencode(s: &str) -> String {
  s.bytes()
    .map(|b| match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
        (b as char).to_string()
      }
      b' ' => "+".to_string(),
      _ => format!("%{:02X}", b),
    })
    .collect()
}

/// 打印 Bwbrowser API 日志
pub fn log_bwbrowser(action: &str, msg: &str) {
  let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
  let line = format!("[{}][Bwbrowser][{}] {}", timestamp, action, msg);
  println!("{}", line);
  log::info!("{}", line);
  write_log_file(&line);
}

/// 解析 JSON 响应体，对空响应和非 JSON 内容给出友好错误。
/// 当服务器输出 PHP 警告/HTML 时，尝试从中提取 JSON。
/// 安全截断预览文本，避免按字节切 slice 命中多字节 UTF-8 边界导致 panic
fn trunc(s: &str, max: usize) -> String {
  if s.len() <= max {
    return s.to_string();
  }
  let mut end = max;
  while end > 0 && !s.is_char_boundary(end) {
    end -= 1;
  }
  format!("{}...", &s[..end])
}

/// 从账号 proxy_node 提取代理主机，用于列表补充时区。
/// 兼容 node: 前缀、vless:// trojan:// URI、以及 socks5:host:port 等 type:host 形式。
fn extract_proxy_host_from_node(node: &str) -> Option<String> {
  if node.is_empty() {
    return None;
  }
  let n = node
    .strip_prefix(crate::cloud_proxy_manager::NODE_PREFIX)
    .unwrap_or(node);
  if n.starts_with("trojan://") {
    return crate::xray::parse_trojan_uri(n)
      .ok()
      .map(|p| p.config.address.clone())
      .filter(|a| !a.is_empty());
  }
  if n.starts_with("vless://") {
    return crate::xray::parse_vless_uri(n)
      .ok()
      .map(|p| p.config.address.clone())
      .filter(|a| !a.is_empty());
  }
  crate::cloud_proxy_manager::parse_proxy_node(n)
    .map(|settings| settings.host)
    .filter(|h| !h.is_empty())
}

fn parse_body<T: serde::de::DeserializeOwned>(action: &str, body: &str) -> Result<T, String> {
  if body.trim().is_empty() {
    let msg = "服务器返回了空响应".to_string();
    log_bwbrowser_error(action, &msg);
    return Err(msg);
  }

  // 先解析为 serde_json::Value（重复键自动取最后一个），再转目标结构体，
  // 避免服务端偶尔返回重复字段（如 owner_id）触发 serde "duplicate field" 解析失败，
  // 导致 get_account_detail 等接口在启动关键路径上出错、被迫回落用旧参数。
  let from_value = |v: serde_json::Value| -> Result<T, String> {
    serde_json::from_value::<T>(v).map_err(|e| {
      let preview = if body.len() > 200 {
        format!("{}", trunc(&body, 200))
      } else {
        body.to_string()
      };
      let err_msg = format!("解析响应失败: {} | body={}", e, preview);
      log_bwbrowser_error(action, &err_msg);
      format!("解析响应失败: {}", e)
    })
  };

  match serde_json::from_str::<serde_json::Value>(body) {
    Ok(v) => from_value(v),
    Err(_) => {
      // 服务器可能输出了 PHP 警告/HTML，尝试从中提取 JSON
      // 找到第一个 '{' 或 '[' 的位置
      let json_start = body.find('{').or_else(|| body.find('['));
      if let Some(start) = json_start {
        let json_candidate = &body[start..];
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_candidate) {
          if let Ok(val) = from_value(v) {
            log_bwbrowser(action, "直接解析失败，但从 HTML/PHP 警告中提取 JSON 成功");
            return Ok(val);
          }
        }
      }

      let preview = if body.len() > 200 {
        format!("{}", trunc(&body, 200))
      } else {
        body.to_string()
      };
      let err_msg = format!(
        "解析响应失败: 响应不是有效 JSON（可能是 PHP 警告或空响应）| body={}",
        preview
      );
      log_bwbrowser_error(action, &err_msg);
      Err("解析响应失败: 服务器响应不是有效 JSON".to_string())
    }
  }
}

/// 打印日志（含错误）
pub fn log_bwbrowser_error(action: &str, err: &str) {
  let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
  let line = format!("[{}][Bwbrowser][{}][ERROR] {}", timestamp, action, err);
  eprintln!("{}", line);
  log::error!("{}", line);
  write_log_file(&line);
}

/// 统一解析云端 API 响应，附带 HTTP 状态码校验和友好错误提示
fn parse_api_response<T: serde::de::DeserializeOwned>(
  action: &str,
  status: reqwest::StatusCode,
  body: &str,
) -> Result<T, String> {
  if !status.is_success() {
    let preview = if body.is_empty() {
      "(空响应)".to_string()
    } else if body.len() > 200 {
      format!("{}", trunc(&body, 200))
    } else {
      body.to_string()
    };
    let err = format!("HTTP {}: {}", status.as_u16(), preview);
    log_bwbrowser_error(action, &err);
    return Err(format!(
      "服务器返回错误 (HTTP {}): {}",
      status.as_u16(),
      preview
    ));
  }

  if body.trim().is_empty() {
    let err = format!("HTTP {}: 响应体为空", status.as_u16());
    log_bwbrowser_error(action, &err);
    return Err("服务器返回了空响应，请检查网络或稍后重试".to_string());
  }

  // 先经 Value 去重再转结构体，规避服务端返回重复字段导致的 "duplicate field" 解析失败
  match serde_json::from_str::<serde_json::Value>(body) {
    Ok(v) => serde_json::from_value::<T>(v).map_err(|e| {
      let preview = if body.len() > 200 {
        format!("{}", trunc(&body, 200))
      } else {
        body.to_string()
      };
      let err_msg = format!("解析响应失败: {} | body={}", e, preview);
      log_bwbrowser_error(action, &err_msg);
      format!("解析响应失败: {}", e)
    }),
    Err(_) => {
      let preview = if body.len() > 200 {
        format!("{}", trunc(&body, 200))
      } else {
        body.to_string()
      };
      let err_msg = format!("解析响应失败: 响应不是有效 JSON | body={}", preview);
      log_bwbrowser_error(action, &err_msg);
      Err("解析响应失败: 服务器响应不是有效 JSON".to_string())
    }
  }
}

/// 直接写入日志文件（后备，不依赖 tauri-plugin-log）
fn write_log_file(line: &str) {
  use std::io::Write;
  let log_path = bwbrowser_log_path();
  if let Some(path) = log_path {
    if let Some(parent) = path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&path)
    {
      let _ = writeln!(file, "{}", line);
    }
  }
}

/// 获取 bwbrowser 专用日志文件路径
fn bwbrowser_log_path() -> Option<std::path::PathBuf> {
  Some(crate::app_dirs::data_dir().join("bwbrowser_debug.log"))
}

/// 隐藏密码，用于日志显示
fn mask_password(pwd: &str) -> String {
  if pwd.is_empty() {
    return "(empty)".to_string();
  }
  if pwd.len() <= 2 {
    return "***".to_string();
  }
  format!(
    "{}***{}",
    pwd.chars().next().unwrap_or('#'),
    pwd.chars().last().unwrap_or('#')
  )
}

// ========== Tauri Commands ==========

#[tauri::command]
pub async fn bwbrowser_login<R: Runtime>(
  app: tauri::AppHandle<R>,
  username: String,
  password: String,
) -> Result<CloudAuthState, String> {
  BWBROWSER_AUTH.login(&app, &username, &password).await
}

#[tauri::command]
pub fn bwbrowser_get_user() -> Option<CloudAuthState> {
  BWBROWSER_AUTH.get_user()
}

#[tauri::command]
pub async fn bwbrowser_logout<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
  BWBROWSER_AUTH.logout(&app).await;
  Ok(())
}

#[tauri::command]
pub fn bwbrowser_refresh_profile() -> Result<CloudUser, String> {
  BWBROWSER_AUTH.refresh_profile_sync()
}

/// 在 VPS 登录浏览器中打开爆文库账号详情页（已运行则新建标签，未运行则拉起 VPS 登录并跳转详情）
#[tauri::command]
pub async fn bwbrowser_open_account_detail_in_vps(
  app_handle: tauri::AppHandle,
  account_name: String,
  platform: Option<String>,
) -> Result<String, String> {
  let plat = platform.as_deref().unwrap_or("tiktok");
  let detail_url = build_baowenku_account_detail_url(&account_name, plat);
  log_bwbrowser(
    "open_account_in_vps",
    &format!("  账号详情 URL: {}", &detail_url),
  );

  let profiles = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("获取本地 profile 列表失败: {}", e))?;
  let vps_profile = profiles
    .into_iter()
    .find(|p| p.name.to_lowercase() == "vps登录");

  let runner = crate::browser_runner::BrowserRunner::instance();
  match vps_profile {
    Some(profile) => {
      let is_running = runner
        .check_browser_status(app_handle.clone(), &profile)
        .await
        .map_err(|e| format!("检查 VPS 浏览器状态失败: {}", e))?;
      if is_running {
        runner
          .open_url_in_existing_browser(app_handle.clone(), &profile, &detail_url, None)
          .await
          .map_err(|e| format!("在 VPS 浏览器中打开账号详情失败: {}", e))?;
        log_bwbrowser(
          "open_account_in_vps",
          "  ✓ 已在 VPS 浏览器中新建标签打开账号详情",
        );
        return Ok("opened_tab".to_string());
      }
      bwbrowser_open_vps_login(app_handle, Some(detail_url.clone()))
        .await
        .map_err(|e| format!("启动 VPS 登录浏览器失败: {}", e))?;
      log_bwbrowser(
        "open_account_in_vps",
        "  ✓ 已拉起 VPS 登录浏览器并跳转账号详情",
      );
      Ok("started_browser".to_string())
    }
    None => {
      bwbrowser_open_vps_login(app_handle, Some(detail_url.clone()))
        .await
        .map_err(|e| format!("启动 VPS 登录浏览器失败: {}", e))?;
      Ok("started_browser".to_string())
    }
  }
}

/// 构造爆文库账号详情页 URL
fn build_baowenku_account_detail_url(account_name: &str, platform: &str) -> String {
  // 过滤昵称中的 @，避免被 urlencode 成 %40 导致搜索失败
  let clean_name = account_name.replace('@', "");
  format!(
    "https://yacm.xin/tk/baowenku.php?tab=account&video_sort=play_desc&visibility=all&platform={}&owner_id=0&search={}",
    urlencode(&platform.to_lowercase()),
    urlencode(&clean_name),
  )
}

#[tauri::command]
pub async fn bwbrowser_open_vps_login(
  app_handle: tauri::AppHandle,
  redirect: Option<String>,
) -> Result<(), String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录云端账号".to_string())?;

  // 真实启动进度：按步骤向前端广播百分比，替代前端的模拟动画
  let emit_app = app_handle.clone();
  let emit_progress = |pct: u32, label: &str| {
    let _ = emit_app.emit(
      "vps-launch-progress",
      serde_json::json!({
        "pct": pct,
        "label": label,
      }),
    );
  };
  emit_progress(5, "正在准备 VPS 登录...");

  let redirect = redirect.unwrap_or_else(|| "bao_wen_ku.php".to_string());
  let url = format!(
    "http://yacm.xin/tk/login.php?auto_login=1&username={}&password={}&redirect={}",
    urlencode(&username),
    urlencode(&password),
    urlencode(&redirect),
  );

  log_bwbrowser("open_vps_login", &format!("opening with Wayfern: {}", url));

  // 查找或创建 VPS 登录专用 profile
  let profile_name = "VPS登录".to_string();
  let existing = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("获取本地 profile 列表失败: {}", e))?;

  let existing_profile = existing
    .into_iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase());

  // 构建本机真机指纹配置（无代理，使用本机信息）
  // 设置 fingerprint = "{}" → 跳过 Wayfern 指纹生成，使用本机真实设备指纹/时区/语言
  // 不设置 identity_id → 不创建 identity，不消耗指纹配额
  let mut wayfern_config = default_wayfern_config_for_host();
  wayfern_config.fingerprint = Some("{}".to_string());

  let profile = match existing_profile {
    Some(mut p) => {
      log_bwbrowser("open_vps_login", &format!("复用已有 profile: id={}", p.id));
      // 检查是否需要切换为真机模式
      let needs_switch = p
        .wayfern_config
        .as_ref()
        .map(|c| c.identity_id.is_some())
        .unwrap_or(false);

      if needs_switch {
        log_bwbrowser("open_vps_login", "  切换为真机指纹模式（不消耗配额）");
      }

      // 直接设置 wayfern_config，确保 identity_id 为空，使用真机指纹
      // 注意：不调用 update_wayfern_config，因为它会自动携带旧的 identity_id
      let pm = crate::profile::manager::ProfileManager::instance();
      match pm.check_browser_status(app_handle.clone(), &p).await {
        Ok(false) => {
          let mut new_config = wayfern_config.clone();
          // 确保清除 identity 相关字段，使用纯真机模式
          new_config.identity_id = None;
          new_config.identity_baseline = None;
          new_config.identity_overrides = None;
          new_config.location = None;
          new_config.geo_proxy_signature = None;
          p.wayfern_config = Some(new_config.clone());
          match pm.save_profile(&p) {
            Ok(_) => {
              log_bwbrowser("open_vps_login", "  ✓ 已切换为真机指纹模式");
            }
            Err(e) => {
              log_bwbrowser_error("open_vps_login", &format!("  保存配置失败: {}", e));
            }
          }
        }
        Ok(true) => {
          log_bwbrowser("open_vps_login", "  浏览器运行中，跳过配置更新");
        }
        Err(e) => {
          log_bwbrowser_error("open_vps_login", &format!("  检查浏览器状态失败: {}", e));
        }
      }
      p
    }
    None => {
      // 查找已安装的 Wayfern 版本
      let registry = crate::downloaded_browsers_registry::DownloadedBrowsersRegistry::instance();
      let mut versions = registry.get_downloaded_versions("wayfern");
      versions.sort_by(|a, b| crate::api_client::compare_versions(b, a));
      let version = versions
        .into_iter()
        .next()
        .ok_or_else(|| "Wayfern 内核未安装，请先下载浏览器内核".to_string())?;
      log_bwbrowser("open_vps_login", &format!("使用 Wayfern 版本: {}", version));

      let pm = crate::profile::manager::ProfileManager::instance();
      pm.create_profile_with_group(
        &app_handle,
        &profile_name,
        "wayfern",
        &version,
        "stable",
        None, // 无代理
        None,
        Some(wayfern_config.clone()),
        None,
        false,
        None,
        None,
      )
      .await
      .map_err(|e| format!("创建 profile 失败: {}", e))?
    }
  };
  emit_progress(30, "profile 就绪");
  emit_progress(40, "同步云端书签...");

  // 本地解析 VPS 登录代理时区（仅本地解析，不回传云端）
  // 有代理 → 按代理解析时区并在启动后注入浏览器；无代理 → 保持本机真实时区
  let vps_geo_info = if profile
    .proxy_id
    .as_deref()
    .is_some_and(|p| !p.trim().is_empty())
  {
    resolve_geo_from_proxy(profile.proxy_id.as_deref()).await
  } else {
    None
  };
  if let Some(ref geo) = vps_geo_info {
    log_bwbrowser(
      "open_vps_login",
      &format!(
        "  ✓ VPS 代理时区（本地解析）: timezone={}, language={}",
        geo.timezone, geo.language
      ),
    );
  } else {
    log_bwbrowser(
      "open_vps_login",
      "  VPS 登录无代理或无法解析时区，保持本机真实时区",
    );
  }

  // ---- 书签同步：启动前下载云端书签到本地 ----
  // 以本地为准：只有本地没有书签时才从云端下载注入
  let profile_id_str = profile.id.to_string();
  let local_bookmarks = BwbrowserAuthManager::read_local_bookmarks(&profile_id_str).unwrap_or(None);

  if local_bookmarks.is_none() {
    match BWBROWSER_AUTH.get_bwbrowser_bookmarks().await {
      Ok(Some(cloud_bookmarks)) if !cloud_bookmarks.is_empty() && cloud_bookmarks != "null" => {
        log_bwbrowser(
          "vps_bookmark_sync",
          &format!("本地无书签，从云端下载: {} 字节", cloud_bookmarks.len()),
        );
        match BwbrowserAuthManager::write_local_bookmarks(&profile_id_str, &cloud_bookmarks) {
          Ok(_) => {
            log_bwbrowser("vps_bookmark_sync", "✓ 云端书签已写入本地");
          }
          Err(e) => {
            log_bwbrowser_error("vps_bookmark_sync", &format!("写入书签失败: {}", e));
          }
        }
      }
      Ok(_) => {
        log_bwbrowser("vps_bookmark_sync", "云端无书签，跳过");
      }
      Err(e) => {
        log_bwbrowser_error("vps_bookmark_sync", &format!("拉取云端书签失败: {}", e));
      }
    }
  } else {
    log_bwbrowser(
      "vps_bookmark_sync",
      "本地已有书签，跳过云端注入（以本地为准）",
    );
  }
  emit_progress(60, "书签已就绪");
  emit_progress(70, "启动浏览器内核...");

  // 启动 Wayfern 浏览器，直接打开 VPS 登录 URL
  let options = crate::browser_runner::LaunchOptions {
    gate: crate::launch_gate::FingerprintGate::Advisory,
    ..Default::default()
  };
  let launched_profile = crate::browser_runner::launch_browser_profile_impl(
    app_handle.clone(),
    profile.clone(),
    Some(url),
    options,
  )
  .await
  .map_err(|e| format!("启动浏览器失败: {}", e))?;

  log_bwbrowser("open_vps_login", "  ✓ Wayfern 浏览器已启动");

  // 注入 VPS 代理时区（Wayfern 内核级设置 + CDP 回退），无代理时保持本机时区
  if let Some(ref geo) = vps_geo_info {
    if !geo.timezone.is_empty() {
      let tz = geo.timezone.clone();
      let lang = geo.language.clone();
      log_bwbrowser(
        "open_vps_login",
        &format!("  [VPS-TZ] 注入代理时区: {} 语言: {}", tz, lang),
      );
      let wayfern_tz_set = match crate::wayfern_manager::WayfernManager::instance()
        .set_wayfern_timezone(
          &launched_profile,
          &tz,
          Some(lang.as_str()),
          geo.latitude,
          geo.longitude,
        )
        .await
      {
        Ok(true) => {
          log_bwbrowser(
            "open_vps_login",
            &format!("  [VPS-TZ] ✓ Wayfern 内核时区已设置: {}", tz),
          );
          true
        }
        Ok(false) => {
          log_bwbrowser(
            "open_vps_login",
            "  [VPS-TZ] Wayfern 不支持时区设置，回退到 CDP 注入",
          );
          false
        }
        Err(e) => {
          log_bwbrowser_error(
            "open_vps_login",
            &format!("  [VPS-TZ] ✗ Wayfern 时区设置失败: {}", e),
          );
          false
        }
      };
      if !wayfern_tz_set {
        match crate::cookie_sync::inject_timezone_via_cdp(&launched_profile, &tz).await {
          Ok(_) => {
            log_bwbrowser(
              "open_vps_login",
              &format!("  [VPS-TZ] ✓ CDP 时区注入成功: {}", tz),
            );
          }
          Err(e) => {
            log_bwbrowser_error(
              "open_vps_login",
              &format!("  [VPS-TZ] ✗ CDP 时区注入失败: {}", e),
            );
          }
        }
      }
    }
  }

  emit_progress(100, "爆文库已启动");

  // 后台任务：注入云端 cookie + 关闭时回传 cookie
  tokio::spawn(async move {
    // 重新加载最新的 profile（启动后 process_id 才写入，旧 profile 里没有）
    let pm = crate::profile::manager::ProfileManager::instance();
    let profile = match pm.list_profiles() {
      Ok(list) => match list.into_iter().find(|p| p.id == profile.id) {
        Some(p) => p,
        None => {
          log_bwbrowser_error("vps_cookie_sync", "找不到 VPS 登录 profile，退出同步");
          return;
        }
      },
      Err(e) => {
        log_bwbrowser_error("vps_cookie_sync", &format!("加载 profile 失败: {}", e));
        return;
      }
    };
    log_bwbrowser(
      "vps_cookie_sync",
      &format!("已加载最新 profile: process_id={:?}", profile.process_id),
    );

    // ---- 阶段 1: 检查本地 cookie 数量，决定是否注入云端 cookie ----
    // 以本地为准：只有本地 cookie 很少（<5个）或没有时，才从云端注入
    // 浏览器刚启动 CDP 可能没就绪，重试几次
    let mut local_cookie_count = 0;
    for attempt in 1..=10 {
      match crate::cookie_sync::export_cookies_via_cdp(&profile).await {
        Ok(ref s) if !s.is_empty() && s != "[]" => {
          match serde_json::from_str::<Vec<serde_json::Value>>(s) {
            Ok(arr) => {
              local_cookie_count = arr.len();
              break;
            }
            Err(_) => {
              local_cookie_count = 0;
              break;
            }
          }
        }
        Ok(_) => {
          local_cookie_count = 0;
          break;
        }
        Err(_) => {
          // CDP 还没连上，等 2 秒再试
          tokio::time::sleep(std::time::Duration::from_secs(2)).await;
          log_bwbrowser(
            "vps_cookie_sync",
            &format!("检查本地 cookie: 第{}次 CDP 未就绪，重试中...", attempt),
          );
        }
      }
    }

    log_bwbrowser(
      "vps_cookie_sync",
      &format!("本地 cookie 数量: {}", local_cookie_count),
    );

    let should_inject_cloud = if local_cookie_count < 5 {
      log_bwbrowser("vps_cookie_sync", "本地 cookie 很少，尝试从云端拉取");
      true
    } else {
      log_bwbrowser(
        "vps_cookie_sync",
        "本地 cookie 充足，跳过云端注入（以本地为准）",
      );
      false
    };

    let _cloud_has_cookies = if should_inject_cloud {
      match BWBROWSER_AUTH.get_bwbrowser_cookies().await {
        Ok(Some(cookie_str)) if !cookie_str.is_empty() && cookie_str != "null" => {
          log_bwbrowser(
            "vps_cookie_sync",
            &format!("拉取云端 cookie: {} 字节，准备注入", cookie_str.len()),
          );
          match serde_json::from_str::<Vec<serde_json::Value>>(&cookie_str) {
            Ok(cookies) => {
              match crate::cookie_sync::inject_cookies_via_cdp(&profile, &cookies, None).await {
                Ok(n) => {
                  log_bwbrowser(
                    "vps_cookie_sync",
                    &format!("✓ 云端 cookie 注入成功: {} 个", n),
                  );
                }
                Err(e) => {
                  log_bwbrowser_error("vps_cookie_sync", &format!("注入 cookie 失败: {}", e));
                }
              }
            }
            Err(e) => {
              log_bwbrowser_error("vps_cookie_sync", &format!("解析云端 cookie 失败: {}", e));
            }
          }
          true
        }
        Ok(None) => {
          log_bwbrowser("vps_cookie_sync", "云端无 cookie");
          false
        }
        Ok(Some(_)) => {
          log_bwbrowser("vps_cookie_sync", "云端 cookie 为空");
          false
        }
        Err(e) => {
          log_bwbrowser_error("vps_cookie_sync", &format!("拉取云端 cookie 失败: {}", e));
          false
        }
      }
    } else {
      false
    };

    // ---- 阶段 1.5: 主动上传本地 cookie 到云端（以本地为准）----
    // 浏览器刚启动时 CDP 可能还没就绪，重试多次直到连上
    let mut upload_tried = false;
    for attempt in 1..=15 {
      tokio::time::sleep(std::time::Duration::from_secs(2)).await;
      match crate::cookie_sync::export_cookies_via_cdp(&profile).await {
        Ok(cookie_str) if !cookie_str.is_empty() && cookie_str != "[]" => {
          log_bwbrowser(
            "vps_cookie_sync",
            &format!(
              "第{}次尝试成功，上传本地 cookie: {} 字节",
              attempt,
              cookie_str.len()
            ),
          );
          match BWBROWSER_AUTH.update_bwbrowser_cookies(&cookie_str).await {
            Ok(_) => {
              log_bwbrowser("vps_cookie_sync", "✓ cookie 已同步到云端");
            }
            Err(e) => {
              log_bwbrowser_error("vps_cookie_sync", &format!("上传 cookie 失败: {}", e));
            }
          }
          upload_tried = true;
          break;
        }
        Ok(_) => {
          // 浏览器起来了但 cookie 为空（可能还在加载），继续等
          log_bwbrowser(
            "vps_cookie_sync",
            &format!("第{}次: 浏览器已就绪但 cookie 为空，继续等待...", attempt),
          );
        }
        Err(e) => {
          // CDP 还没连上，继续重试
          log_bwbrowser(
            "vps_cookie_sync",
            &format!("第{}次: CDP 未就绪 ({})，继续等待...", attempt, e),
          );
        }
      }
    }
    if !upload_tried {
      log_bwbrowser(
        "vps_cookie_sync",
        "多次尝试后仍无法获取本地 cookie，跳过上传",
      );
    }

    // ---- 阶段 2: 等待浏览器关闭，回传 cookie ----
    // 每 3 秒检查一次浏览器状态，退出时导出并上传
    loop {
      tokio::time::sleep(std::time::Duration::from_secs(3)).await;
      let pm = crate::profile::manager::ProfileManager::instance();
      let is_running = pm
        .check_browser_status(app_handle.clone(), &profile)
        .await
        .unwrap_or(false);
      if !is_running {
        log_bwbrowser("vps_cookie_sync", "浏览器已关闭，准备回传 cookie");
        // 浏览器已关闭，CDP 不可用，仅用 SQLite 直读（不再尝试 CDP，避免无意义报错）
        let cookie_str = match crate::cookie_manager::CookieManager::export_cookies(
          &profile.id.to_string(),
          "json",
        ) {
          Ok(s) if !s.is_empty() && s != "[]" => {
            log_bwbrowser(
              "vps_cookie_sync",
              &format!("SQLite 导出 cookie: {} 字节", s.len()),
            );
            s
          }
          Ok(_) => {
            log_bwbrowser("vps_cookie_sync", "SQLite cookie 为空，跳过回传");
            String::new()
          }
          Err(e) => {
            log_bwbrowser(
              "vps_cookie_sync",
              &format!("SQLite 导出失败 ({}), 跳过回传", e),
            );
            String::new()
          }
        };
        if !cookie_str.is_empty() {
          match BWBROWSER_AUTH.update_bwbrowser_cookies(&cookie_str).await {
            Ok(_) => {
              log_bwbrowser("vps_cookie_sync", "✓ cookie 已同步到云端");
            }
            Err(e) => {
              log_bwbrowser_error("vps_cookie_sync", &format!("上传 cookie 失败: {}", e));
            }
          }
        }

        // ---- 上传书签到云端 ----
        log_bwbrowser("vps_bookmark_sync", "浏览器已关闭，准备上传书签");
        match BwbrowserAuthManager::read_local_bookmarks(&profile.id.to_string()) {
          Ok(Some(bookmarks)) if !bookmarks.is_empty() => {
            log_bwbrowser(
              "vps_bookmark_sync",
              &format!("读取本地书签: {} 字节", bookmarks.len()),
            );
            match BWBROWSER_AUTH.update_bwbrowser_bookmarks(&bookmarks).await {
              Ok(_) => {
                log_bwbrowser("vps_bookmark_sync", "✓ 书签已同步到云端");
              }
              Err(e) => {
                log_bwbrowser_error("vps_bookmark_sync", &format!("上传书签失败: {}", e));
              }
            }
          }
          Ok(_) => {
            log_bwbrowser("vps_bookmark_sync", "本地无书签，跳过上传");
          }
          Err(e) => {
            log_bwbrowser_error("vps_bookmark_sync", &format!("读取本地书签失败: {}", e));
          }
        }

        break;
      }
    }
  });

  Ok(())
}

/// 获取 VPS 登录 profile 信息（用于右键菜单显示当前代理等）
#[tauri::command]
pub fn bwbrowser_get_vps_profile_info() -> Result<Option<serde_json::Value>, String> {
  let profile_name = "VPS登录";
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().map_err(|e| e.to_string())?;
  let profile = profiles
    .into_iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase());

  match profile {
    Some(p) => Ok(Some(serde_json::json!({
      "id": p.id,
      "name": p.name,
      "proxy_id": p.proxy_id,
      "browser": p.browser,
    }))),
    None => Ok(None),
  }
}

/// 设置 VPS 登录 profile 的代理
/// 如果 profile 不存在，自动创建一个（基于本机真机指纹）
#[tauri::command]
pub async fn bwbrowser_set_vps_proxy(
  app_handle: tauri::AppHandle,
  proxy_id: Option<String>,
) -> Result<(), String> {
  let profile_name = "VPS登录";
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().map_err(|e| e.to_string())?;
  let existing = profiles
    .into_iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase());

  let profile_id = match existing {
    Some(p) => p.id.to_string(),
    None => {
      // profile 不存在，自动创建（基于本机真机指纹）
      log_bwbrowser("set_vps_proxy", "VPS登录 profile 不存在，自动创建中...");
      // 设置 fingerprint = "{}" → 跳过指纹生成，用本机真实设备指纹，不消耗配额
      let mut wayfern_config = default_wayfern_config_for_host();
      wayfern_config.fingerprint = Some("{}".to_string());

      let registry = crate::downloaded_browsers_registry::DownloadedBrowsersRegistry::instance();
      let mut versions = registry.get_downloaded_versions("wayfern");
      versions.sort_by(|a, b| crate::api_client::compare_versions(b, a));
      let version = versions
        .into_iter()
        .next()
        .ok_or_else(|| "Wayfern 内核未安装，请先下载浏览器内核".to_string())?;

      let new_profile = pm
        .create_profile_with_group(
          &app_handle,
          profile_name,
          "wayfern",
          &version,
          "stable",
          None,
          None,
          Some(wayfern_config),
          None,
          false,
          None,
          None,
        )
        .await
        .map_err(|e| format!("创建 profile 失败: {}", e))?;

      log_bwbrowser(
        "set_vps_proxy",
        &format!("  ✓ 已自动创建 VPS登录 profile: id={}", new_profile.id),
      );
      new_profile.id.to_string()
    }
  };

  pm.update_profile_proxy(app_handle, &profile_id, proxy_id.clone())
    .await
    .map_err(|e| e.to_string())?;

  log_bwbrowser(
    "set_vps_proxy",
    &format!("  ✓ VPS登录 profile 代理已更新: {:?}", proxy_id),
  );

  // 本地解析代理时区（不回传云端）：设置代理时立即解析并写日志，
  // 结果缓存在本地代理库，启动爆文库时会命中缓存并注入浏览器
  if let Some(pid) = proxy_id.as_deref() {
    match resolve_geo_from_proxy(Some(pid)).await {
      Some(geo) => {
        log_bwbrowser(
          "set_vps_proxy",
          &format!(
            "  ✓ 代理时区已本地解析: timezone={}, language={}（启动时注入浏览器）",
            geo.timezone, geo.language
          ),
        );
      }
      None => {
        log_bwbrowser(
          "set_vps_proxy",
          "  ⚠ 无法解析代理时区，启动时将保持本机真实时区",
        );
      }
    }
  } else {
    log_bwbrowser("set_vps_proxy", "  已清除代理，启动时使用本机真实时区");
  }
  Ok(())
}

/// 删除 VPS 登录 profile 的本地数据（删除 profile，下次打开重新创建）
#[tauri::command]
pub fn bwbrowser_delete_vps_data(app_handle: tauri::AppHandle) -> Result<(), String> {
  let profile_name = "VPS登录";
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().map_err(|e| e.to_string())?;
  let profile = profiles
    .into_iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase())
    .ok_or_else(|| "VPS登录 profile 不存在，无需删除".to_string())?;

  pm.delete_profile_permanently(&app_handle, &profile.id.to_string())
    .map_err(|e| e.to_string())?;

  log_bwbrowser(
    "delete_vps_data",
    &format!("  ✓ VPS登录 profile 已删除: id={}", profile.id),
  );
  Ok(())
}

// ========== 代理同步 ==========

/// Bwbrowser 云端代理数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BwbrowserProxy {
  #[serde(rename = "id", alias = "proxy_id")]
  pub proxy_id: i64,
  #[serde(default, rename = "name", alias = "proxy_name")]
  pub proxy_name: Option<String>,
  pub proxy_type: String,
  pub host: String,
  pub port: i64,
  #[serde(default)]
  pub username: Option<String>,
  #[serde(default)]
  pub password: Option<String>,
  #[serde(default)]
  pub country: Option<String>,
  #[serde(default)]
  pub city: Option<String>,
  #[serde(default)]
  pub timezone: Option<String>,
  #[serde(default)]
  pub provider: Option<String>,
  #[serde(default)]
  pub protocol_config: Option<String>,
  #[serde(default)]
  pub proxy_uuid: Option<String>,
  #[serde(deserialize_with = "deserialize_int_or_string", default)]
  pub status: Option<String>,
  #[serde(deserialize_with = "deserialize_int_or_string", default)]
  pub created_at: Option<String>,
  #[serde(deserialize_with = "deserialize_int_or_string", default)]
  pub updated_at: Option<String>,
}

/// list_proxies 响应
#[derive(Debug, Deserialize)]
struct BwbrowserProxyListResponse {
  success: bool,
  message: Option<String>,
  #[serde(default)]
  proxies: Option<Vec<BwbrowserProxy>>,
}

/// sync_proxy 响应
#[derive(Debug, Deserialize)]
struct BwbrowserProxySyncResponse {
  success: bool,
  message: Option<String>,
  #[serde(default)]
  proxy_id: Option<i64>,
}

impl BwbrowserAuthManager {
  /// 获取云端代理列表（带内存缓存，同一会话内复用）
  pub async fn list_cloud_proxies(
    &self,
    company_id: Option<i64>,
  ) -> Result<Vec<BwbrowserProxy>, String> {
    // 先查缓存：5 分钟内复用（切换公司时跳过缓存）
    if company_id.is_none() {
      if let Ok(cache) = self.proxy_cache.lock() {
        if let Some((ref proxies, ref time)) = *cache {
          if time.elapsed() < std::time::Duration::from_secs(300) {
            log_bwbrowser(
              "list_proxies",
              &format!(
                "✓ 使用缓存，共 {} 条代理（缓存年龄: {}s）",
                proxies.len(),
                time.elapsed().as_secs()
              ),
            );
            return Ok(proxies.clone());
          }
        }
      }
    }

    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "list_proxies",
      &format!("→ 请求代理列表: user={}", username),
    );

    let mut form_data = format!(
      "action=list_proxies&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );
    if let Some(cid) = company_id {
      form_data.push_str(&format!("&company_id={}", cid));
    }

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("list_proxies", &format!("网络请求失败: {}", e));
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
      log_bwbrowser_error("list_proxies", &format!("读取响应失败: {}", e));
      format!("读取响应失败: {}", e)
    })?;

    log_bwbrowser(
      "list_proxies",
      &format!("← 响应状态: {}, 长度: {} bytes", status, body.len()),
    );
    log_bwbrowser("list_proxies", &format!("  响应内容: {}", body));

    let result: BwbrowserProxyListResponse = parse_api_response("list_proxies", status, &body)?;

    if !result.success {
      let msg = result
        .message
        .unwrap_or_else(|| "获取代理列表失败".to_string());
      log_bwbrowser_error("list_proxies", &msg);
      return Err(msg);
    }

    let proxies = result.proxies.unwrap_or_default();
    let count = proxies.len();
    log_bwbrowser("list_proxies", &format!("✓ 获取成功，共 {} 条代理", count));

    // 写入缓存（切换公司时不缓存）
    if company_id.is_none() {
      if let Ok(mut cache) = self.proxy_cache.lock() {
        *cache = Some((proxies.clone(), std::time::Instant::now()));
      }
    }

    Ok(proxies)
  }

  /// 获取云端代理列表（强制刷新，跳过缓存）
  pub async fn list_cloud_proxies_force(&self) -> Result<Vec<BwbrowserProxy>, String> {
    self.invalidate_proxy_cache();
    self.list_cloud_proxies(None).await
  }

  /// 同步代理到云端（新增或更新）
  #[allow(clippy::too_many_arguments)]
  pub async fn sync_cloud_proxy(
    &self,
    proxy_id: Option<i64>,
    proxy_name: &str,
    proxy_type: &str,
    host: &str,
    port: i64,
    username: Option<&str>,
    password: Option<&str>,
    country: Option<&str>,
    city: Option<&str>,
    timezone: Option<&str>,
    protocol_config: Option<&str>,
  ) -> Result<i64, String> {
    let (user, pass) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let mut form_data = format!(
      "action=sync_proxy&username={}&password={}&proxy_type={}&host={}&port={}",
      urlencode(&user),
      urlencode(&pass),
      urlencode(proxy_type),
      urlencode(host),
      port
    );

    if let Some(pid) = proxy_id {
      form_data.push_str(&format!("&proxy_id={}", pid));
    }
    form_data.push_str(&format!("&proxy_name={}", urlencode(proxy_name)));
    if let Some(u) = username {
      form_data.push_str(&format!("&username_proxy={}", urlencode(u)));
    }
    if let Some(p) = password {
      form_data.push_str(&format!("&password_proxy={}", urlencode(p)));
    }
    if let Some(c) = country {
      form_data.push_str(&format!("&country={}", urlencode(c)));
    }
    if let Some(ci) = city {
      form_data.push_str(&format!("&city={}", urlencode(ci)));
    }
    if let Some(tz) = timezone {
      form_data.push_str(&format!("&timezone={}", urlencode(tz)));
    }
    if let Some(pc) = protocol_config {
      form_data.push_str(&format!("&protocol_config={}", urlencode(pc)));
    }

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("网络请求失败: {}", e))?;

    let body = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败: {}", e))?;

    let result: BwbrowserProxySyncResponse = parse_body("api", &body)?;

    if !result.success {
      return Err(result.message.unwrap_or_else(|| "同步代理失败".to_string()));
    }

    self.invalidate_proxy_cache();
    result
      .proxy_id
      .ok_or_else(|| "服务器未返回 proxy_id".to_string())
  }

  /// 删除云端代理
  pub async fn delete_cloud_proxy(&self, proxy_id: i64) -> Result<(), String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let form_data = format!(
      "action=delete_proxy&username={}&password={}&proxy_id={}",
      urlencode(&username),
      urlencode(&password),
      proxy_id
    );

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("网络请求失败: {}", e))?;

    let body = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败: {}", e))?;

    let result: serde_json::Value = parse_body("api", &body)?;

    let success = result
      .get("success")
      .and_then(|v| v.as_bool())
      .unwrap_or(false);
    if !success {
      let msg = result
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("删除代理失败");
      return Err(msg.to_string());
    }

    self.invalidate_proxy_cache();
    Ok(())
  }

  /// 仅同步代理的地理信息（国家/城市/时区）到云端，不动其它代理配置。
  /// 用于「测试代理」成功后自动回传探测到的国家。
  pub async fn sync_proxy_geo(
    &self,
    proxy_id: i64,
    country: Option<&str>,
    city: Option<&str>,
    timezone: Option<&str>,
  ) -> Result<(), String> {
    let (user, pass) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "sync_proxy_geo",
      &format!(
        "  → 回传国家到云端: proxy_id={}, country={:?}, city={:?}, timezone={:?}",
        proxy_id, country, city, timezone
      ),
    );

    let mut form_data = format!(
      "action=update_proxy_geo&username={}&password={}&proxy_id={}",
      urlencode(&user),
      urlencode(&pass),
      proxy_id
    );
    if let Some(c) = country {
      form_data.push_str(&format!("&country={}", urlencode(c)));
    }
    if let Some(ci) = city {
      form_data.push_str(&format!("&city={}", urlencode(ci)));
    }
    if let Some(tz) = timezone {
      form_data.push_str(&format!("&timezone={}", urlencode(tz)));
    }

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("网络请求失败: {}", e))?;

    let body = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败: {}", e))?;

    let result: BwbrowserProxySyncResponse = parse_body("api", &body)?;
    if !result.success {
      let msg = result
        .message
        .unwrap_or_else(|| "同步代理地理信息失败".to_string());
      log_bwbrowser("sync_proxy_geo", &format!("  ✗ 回传失败: {}", msg));
      return Err(msg);
    }
    self.invalidate_proxy_cache();
    log_bwbrowser("sync_proxy_geo", "  ✓ 回传成功");
    Ok(())
  }

  /// 获取云端账号列表（使用 Simprint 账号 API，与 Simprint 客户端保持一致）
  pub async fn list_cloud_accounts(
    &self,
    page: i32,
    page_size: i32,
    platform: Option<String>,
    keyword: Option<String>,
    owner_id: Option<i64>,
    company_id: Option<i64>,
  ) -> Result<BwbrowserAccountListResponse, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "list_accounts",
      &format!(
        "→ 请求账号列表: user={}, page={}, page_size={}",
        username, page, page_size
      ),
    );

    let mut form_data = format!(
      "action=list&username={}&password={}&page={}&page_size={}&account_type=tiktok",
      urlencode(&username),
      urlencode(&password),
      page,
      page_size
    );
    if let Some(p) = platform {
      form_data.push_str(&format!("&platform={}", urlencode(&p)));
    }
    if let Some(k) = keyword {
      form_data.push_str(&format!("&keyword={}", urlencode(&k)));
    }
    // 普通成员只能看到自己名下的账号：用 SQL 侧过滤，
    // 而不是拉到全部后在前端过滤（否则非自己的账号会显示成 0 条）
    let effective_owner_id = if !self.is_manager_role() {
      self.get_user_id()
    } else {
      owner_id
    };
    if let Some(oid) = effective_owner_id {
      form_data.push_str(&format!("&owner_id={}", oid));
    }
    if let Some(cid) = company_id {
      form_data.push_str(&format!("&company_id={}", cid));
    }

    let bwbrowser_form_data = form_data
      .replacen("action=list&", "action=list_accounts&", 1)
      .replace("&account_type=tiktok", "");

    // 优先用 simprint_accounts.php（有正确的 JOIN 和筛选逻辑），
    // 全部失败时才回退到 bwbrowser_sync.php
    let endpoints: [(&str, &str); 4] = [
      (SIMPRINT_ACCOUNTS_URLS[0], form_data.as_str()),
      (SIMPRINT_ACCOUNTS_URLS[1], form_data.as_str()),
      (SIMPRINT_ACCOUNTS_URLS[2], form_data.as_str()),
      (BWBROWSER_API_URL, bwbrowser_form_data.as_str()),
    ];

    let mut last_err: Option<String> = None;
    for (url, data) in endpoints {
      log_bwbrowser("list_accounts", &format!("  尝试 URL: {}", url));

      let resp = match self
        .client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(data.to_string())
        .send()
        .await
      {
        Ok(r) => r,
        Err(e) => {
          last_err = Some(format!("网络请求失败 ({}): {}", url, e));
          log_bwbrowser_error("list_accounts", &last_err.clone().unwrap());
          continue;
        }
      };

      let status = resp.status();
      let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
          last_err = Some(format!("读取响应失败 ({}): {}", url, e));
          log_bwbrowser_error("list_accounts", &last_err.clone().unwrap());
          continue;
        }
      };

      log_bwbrowser(
        "list_accounts",
        &format!("← 响应状态: {}, 长度: {} bytes", status, body.len()),
      );
      log_bwbrowser("list_accounts", &format!("  响应内容: {}", body));

      let body = body.trim_start_matches('\u{feff}');
      match serde_json::from_str::<BwbrowserAccountListResponse>(body) {
        Ok(mut result) => {
          if !result.success {
            let msg = result
              .message
              .clone()
              .unwrap_or_else(|| "获取账号列表失败".to_string());
            log_bwbrowser_error("list_accounts", &msg);
            return Err(msg);
          }

          log_bwbrowser(
            "list_accounts",
            &format!(
              "  ✓ 获取成功: total={}, 当前页={} 条",
              result.total.unwrap_or(0),
              result.accounts.as_ref().map(|a| a.len()).unwrap_or(0)
            ),
          );
          log_bwbrowser(
            "list_accounts",
            &format!(
              "  权限: can_view_password={:?}, can_view_2fa={:?}, can_view_sms={:?}, is_manager={:?}, is_super_admin={:?}",
              result.can_view_password, result.can_view_2fa, result.can_view_sms, result.is_manager, result.is_super_admin
            ),
          );
          if let Some(ref accts) = result.accounts {
            if let Some(first) = accts.first() {
              log_bwbrowser(
                "list_accounts",
                &format!(
                  "  第一条账号: id={}, phone_id={:?}, owner_id={:?}, safe_link={:?}, bind_phone={:?}",
                  first.id, first.phone_id, first.owner_id, first.safe_link, first.bind_phone
                ),
              );
            }
          }

          // 补充账号时区：账号列表本身若无 timezone，从关联代理(proxy_id)读取刚解析的时区
          if let Some(accts) = result.accounts.as_mut() {
            if let Ok(cloud_proxies) = self.list_cloud_proxies(None).await {
              let mut tz_by_id: std::collections::HashMap<i64, String> =
                std::collections::HashMap::new();
              let mut tz_by_host: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
              for p in cloud_proxies {
                if let Some(tz) = p.timezone.as_deref() {
                  if !tz.trim().is_empty() {
                    tz_by_id.insert(p.proxy_id, tz.trim().to_string());
                    tz_by_host
                      .entry(p.host.clone())
                      .or_insert_with(|| tz.trim().to_string());
                  }
                }
              }
              for acc in accts.iter_mut() {
                if acc
                  .timezone
                  .as_deref()
                  .map(|tt| !tt.trim().is_empty())
                  .unwrap_or(false)
                {
                  continue;
                }
                let tz = acc
                  .proxy_id
                  .and_then(|pid| tz_by_id.get(&pid).cloned())
                  .or_else(|| {
                    acc
                      .proxy_node
                      .as_deref()
                      .and_then(extract_proxy_host_from_node)
                      .and_then(|h| tz_by_host.get(&h).cloned())
                  })
                  .or_else(|| acc.proxy_timezone.clone());
                if let Some(tz) = tz {
                  acc.timezone = Some(tz);
                }
              }
            }
          }

          return Ok(result);
        }
        Err(e) => {
          last_err = Some(format!("解析响应失败 ({}): {}", url, e));
          log_bwbrowser_error("list_accounts", &last_err.clone().unwrap());
          continue;
        }
      }
    }

    Err(last_err.unwrap_or_else(|| "所有服务器均请求失败".to_string()))
  }

  /// 获取账号汇总统计（平台分布、人员分布）
  pub async fn get_account_summary(
    &self,
    owner_id: Option<i64>,
    company_id: Option<i64>,
  ) -> Result<BwbrowserAccountSummaryResponse, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "account_summary",
      &format!("→ 请求账号汇总: user={}", username),
    );

    let mut form_data = format!(
      "action=summary&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );
    // 普通成员强制只看自己的汇总
    let effective_owner_id = if !self.is_manager_role() {
      self.get_user_id()
    } else {
      owner_id
    };
    if let Some(oid) = effective_owner_id {
      form_data.push_str(&format!("&owner_id={}", oid));
    }
    if let Some(cid) = company_id {
      form_data.push_str(&format!("&company_id={}", cid));
    }

    let mut last_err: Option<String> = None;
    for url in SIMPRINT_ACCOUNTS_URLS {
      let resp = match self
        .client
        .post(*url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_data.clone())
        .send()
        .await
      {
        Ok(r) => r,
        Err(e) => {
          last_err = Some(format!("网络请求失败 ({}): {}", url, e));
          continue;
        }
      };

      let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
          last_err = Some(format!("读取响应失败 ({}): {}", url, e));
          continue;
        }
      };

      log_bwbrowser("account_summary", &format!("  响应: {}", body));

      match serde_json::from_str::<BwbrowserAccountSummaryResponse>(&body) {
        Ok(result) => {
          if !result.success {
            let msg = result
              .message
              .clone()
              .unwrap_or_else(|| "获取汇总失败".to_string());
            return Err(msg);
          }
          return Ok(result);
        }
        Err(e) => {
          last_err = Some(format!("解析响应失败 ({}): {}", url, e));
          continue;
        }
      }
    }

    Err(last_err.unwrap_or_else(|| "所有服务器均请求失败".to_string()))
  }

  /// 通过 account_id 获取账号详情（最新的 env_uuid 等）
  pub async fn get_account_detail(&self, account_id: i64) -> Result<BwbrowserAccount, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "account_detail",
      &format!("→ 请求账号详情: account_id={}", account_id),
    );

    let form_data = format!(
      "action=get_account_detail&username={}&password={}&account_id={}",
      urlencode(&username),
      urlencode(&password),
      account_id
    );

    // 使用 bwbrowser_sync.php（get_account_detail 接口在那里）
    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .timeout(std::time::Duration::from_secs(10))
      .send()
      .await
      .map_err(|e| format!("网络请求失败: {}", e))?;
    let status = resp.status();
    let body = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败: {}", e))?;

    log_bwbrowser(
      "account_detail",
      &format!(
        "  响应: HTTP {} body={}",
        status.as_u16(),
        if body.len() > 200 {
          format!("{}", trunc(&body, 200))
        } else {
          body.clone()
        }
      ),
    );

    match parse_body::<BwbrowserAccountDetailResponse>("account_detail", &body) {
      Ok(result) => {
        if !result.success {
          let msg = result
            .message
            .clone()
            .unwrap_or_else(|| "获取账号详情失败".to_string());
          return Err(msg);
        }
        let account = result.account.ok_or_else(|| "账号数据为空".to_string())?;
        log_bwbrowser(
          "account_detail",
          &format!("  ✓ 获取成功: env_uuid={:?}, phone_id={:?}, owner_id={:?}, safe_link={:?}, bind_phone={:?}", account.env_uuid, account.phone_id, account.owner_id, account.safe_link, account.bind_phone),
        );
        Ok(account)
      }
      Err(e) => Err(e),
    }
  }
}

// ==================== 云端账号数据结构（与 Simprint API 对齐）====================

/// 反序列化 tags 字段：兼容字符串（逗号分隔）和数组两种格式
fn deserialize_tags_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let val: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
  match val {
    serde_json::Value::Null => Ok(None),
    serde_json::Value::String(s) => Ok(Some(s)),
    serde_json::Value::Array(arr) => {
      let tags: Vec<String> = arr
        .into_iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
      if tags.is_empty() {
        Ok(None)
      } else {
        Ok(Some(tags.join(",")))
      }
    }
    _ => Ok(None),
  }
}

/// 反序列化布尔字段：兼容整数（0/1）和布尔（true/false）
/// PHP 接口经常返回 int 而不是 bool
fn deserialize_int_or_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
  D: serde::Deserializer<'de>,
{
  let val: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
  match val {
    serde_json::Value::Null => Ok(None),
    serde_json::Value::Bool(b) => Ok(Some(b)),
    serde_json::Value::Number(n) => {
      if let Some(i) = n.as_i64() {
        Ok(Some(i != 0))
      } else if let Some(f) = n.as_f64() {
        Ok(Some(f != 0.0))
      } else {
        Ok(None)
      }
    }
    serde_json::Value::String(s) => {
      let lower = s.to_lowercase();
      match lower.as_str() {
        "true" | "1" | "yes" | "on" => Ok(Some(true)),
        "false" | "0" | "no" | "off" | "" => Ok(Some(false)),
        _ => Ok(None),
      }
    }
    _ => Ok(None),
  }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BwbrowserAccount {
  pub id: i64,
  pub account_name: String,
  #[serde(default)]
  pub login_account: Option<String>,
  #[serde(default)]
  pub login_password: Option<String>,
  #[serde(default)]
  pub device_id: Option<String>,
  #[serde(default)]
  pub safe_link: Option<String>,
  #[serde(default)]
  pub bind_phone: Option<String>,
  #[serde(default)]
  pub sms_url: Option<String>,
  #[serde(default)]
  pub pure_username: Option<String>,
  #[serde(default)]
  pub nickname: Option<String>,
  #[serde(default)]
  pub platform: Option<String>,
  #[serde(default, deserialize_with = "deserialize_tags_string")]
  pub tags: Option<String>,
  #[serde(default)]
  pub category: Option<String>,
  #[serde(default)]
  pub remark: Option<String>,
  // 不用 alias = "user_id"：接口响应同时包含 owner_id 与 user_id 两列，
  // alias 会让 serde 把两个键都映射到 owner_id 字段，触发 "duplicate field owner_id" 解析失败。
  #[serde(default)]
  pub owner_id: Option<i64>,
  #[serde(default)]
  pub owner_name: Option<String>,
  #[serde(default)]
  pub status: Option<i32>,
  #[serde(default)]
  pub followers: Option<i64>,
  #[serde(default)]
  pub likes: Option<i64>,
  #[serde(default)]
  pub video_count: Option<i64>,
  #[serde(default)]
  pub total_views: Option<i64>,
  #[serde(default)]
  pub visibility: Option<String>,
  #[serde(default)]
  pub avatar_url: Option<String>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub verified: Option<bool>,
  #[serde(default)]
  pub cookie_updated_at: Option<String>,
  #[serde(default)]
  pub last_logged_in_at: Option<String>,
  #[serde(default)]
  pub proxy_node: Option<String>,
  #[serde(default)]
  pub proxy_country: Option<String>,
  #[serde(default)]
  pub proxy_city: Option<String>,
  #[serde(default)]
  pub timezone: Option<String>,
  #[serde(default)]
  pub proxy_timezone: Option<String>,
  #[serde(default)]
  pub proxy_id: Option<i64>,
  #[serde(default)]
  pub fingerprint_updated_at: Option<String>,
  #[serde(default)]
  pub bind_person_id: Option<i64>,
  #[serde(default)]
  pub account_type: Option<String>,
  #[serde(default)]
  pub created_at: Option<String>,
  #[serde(default)]
  pub updated_at: Option<String>,
  #[serde(default)]
  pub env_uuid: Option<String>,
  #[serde(default)]
  pub phone_id: Option<String>,
  #[serde(default)]
  pub account_nickname: Option<String>,
  #[serde(default)]
  pub last_login_ip: Option<String>,
  #[serde(default)]
  pub tags_list: Option<Vec<String>>,
  #[serde(default)]
  pub backup_email: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BwbrowserAccountListResponse {
  pub success: bool,
  #[serde(default)]
  pub accounts: Option<Vec<BwbrowserAccount>>,
  #[serde(default)]
  pub total: Option<i64>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub can_view_password: Option<bool>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub can_view_2fa: Option<bool>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub can_view_sms: Option<bool>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_manager: Option<bool>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_super_admin: Option<bool>,
  #[serde(default)]
  pub message: Option<String>,
}

// ==================== 账号汇总数据结构 ====================

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PlatformStat {
  pub platform: String,
  pub count: i64,
  #[serde(default)]
  pub followers: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OwnerStat {
  pub owner_id: i64,
  pub owner_name: String,
  pub count: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BwbrowserAccountSummaryResponse {
  pub success: bool,
  #[serde(default)]
  pub total: Option<i64>,
  #[serde(default)]
  pub platforms: Option<Vec<PlatformStat>>,
  #[serde(default)]
  pub owners: Option<Vec<OwnerStat>>,
  #[serde(default)]
  pub company_name: Option<String>,
  #[serde(default)]
  pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BwbrowserAccountDetailResponse {
  pub success: bool,
  #[serde(default)]
  pub account: Option<BwbrowserAccount>,
  #[serde(default)]
  pub message: Option<String>,
}

// ==================== 用户列表（含考勤状态）数据结构 ====================

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CloudUserItem {
  pub id: i64,
  #[serde(default)]
  pub username: Option<String>,
  #[serde(default)]
  pub real_name: Option<String>,
  #[serde(default)]
  pub company_name: Option<String>,
  #[serde(default)]
  pub sector: Option<String>,
  #[serde(default)]
  pub leave_status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CloudUserListResponse {
  pub success: bool,
  #[serde(default)]
  pub users: Option<Vec<CloudUserItem>>,
  #[serde(default)]
  pub message: Option<String>,
}

// ==================== 公司列表（超级管理员） ====================

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CompanyItem {
  pub id: i64,
  pub name: String,
  #[serde(default)]
  pub code: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CompanyListResponse {
  pub success: bool,
  #[serde(default)]
  pub companies: Option<Vec<CompanyItem>>,
  #[serde(default)]
  pub message: Option<String>,
}

// ==================== Tauri Commands ====================

#[tauri::command]
pub async fn bwbrowser_list_accounts(
  page: Option<i32>,
  page_size: Option<i32>,
  platform: Option<String>,
  keyword: Option<String>,
  owner_id: Option<i64>,
  company_id: Option<i64>,
) -> Result<BwbrowserAccountListResponse, String> {
  let mut result = BWBROWSER_AUTH
    .list_cloud_accounts(
      page.unwrap_or(1),
      page_size.unwrap_or(20),
      platform,
      keyword,
      owner_id,
      company_id,
    )
    .await?;

  // 权限过滤：普通成员只能看到自己的账号
  let is_manager = result.is_manager.unwrap_or(false) || BWBROWSER_AUTH.is_manager_role();
  let is_super_admin = result.is_super_admin.unwrap_or(false);
  if !is_manager && !is_super_admin {
    if let Some(uid) = BWBROWSER_AUTH.get_user_id() {
      if let Some(ref mut accounts) = result.accounts {
        let before = accounts.len();
        accounts.retain(|a| a.owner_id == Some(uid));
        log_bwbrowser(
          "list_accounts",
          &format!(
            "权限过滤: 普通成员 uid={}，{} -> {} 条账号",
            uid,
            before,
            accounts.len()
          ),
        );
      }
      result.total = Some(
        result
          .accounts
          .as_ref()
          .map(|a| a.len() as i64)
          .unwrap_or(0),
      );
    }
  }

  Ok(result)
}

#[tauri::command]
pub async fn bwbrowser_get_account_summary(
  owner_id: Option<i64>,
  company_id: Option<i64>,
) -> Result<BwbrowserAccountSummaryResponse, String> {
  let mut result = BWBROWSER_AUTH
    .get_account_summary(owner_id, company_id)
    .await?;

  // 权限过滤：普通成员只能看到自己的 owner 统计
  if !BWBROWSER_AUTH.is_manager_role() {
    if let Some(uid) = BWBROWSER_AUTH.get_user_id() {
      if let Some(ref mut owners) = result.owners {
        owners.retain(|o| o.owner_id == uid);
      }
    }
  }

  Ok(result)
}

#[tauri::command]
pub async fn bwbrowser_list_cloud_users(
  company_id: Option<i64>,
) -> Result<CloudUserListResponse, String> {
  let mut result = BWBROWSER_AUTH.list_cloud_users(company_id).await?;

  // 权限过滤：普通成员只能看到自己
  if !BWBROWSER_AUTH.is_manager_role() {
    if let Some(uid) = BWBROWSER_AUTH.get_user_id() {
      if let Some(ref mut users) = result.users {
        users.retain(|u| u.id == uid);
      }
    }
  }

  Ok(result)
}

#[tauri::command]
pub async fn bwbrowser_list_companies() -> Result<CompanyListResponse, String> {
  BWBROWSER_AUTH.list_cloud_companies().await
}

#[tauri::command]
pub async fn bwbrowser_get_account_detail(account_id: i64) -> Result<BwbrowserAccount, String> {
  BWBROWSER_AUTH.get_account_detail(account_id).await
}

/// 后台代理有效性检测 + 时区解析 + 回写本地/云端代理。
/// 由 bwbrowser_update_account_proxy 异步 spawn 调用，不阻塞保存与 UI。
async fn resolve_and_sync_proxy_geo(
  app_handle: tauri::AppHandle,
  account_id: i64,
  account_name: String,
  proxy_node: String,
  host: String,
  port: i64,
  proxy_type: String,
  settings: crate::browser::ProxySettings,
) {
  if host.is_empty() {
    return;
  }
  // Cloud-only: directly use the proxy_node settings for validity check.
  let geo_info: ProxyGeoInfo = async {
    let check_id = format!("{}{}", crate::cloud_proxy_manager::NODE_PREFIX, proxy_node);
    log_bwbrowser(
      "update_account_proxy:bg",
      "  → 直接检测代理出口 IP 时区（cloud-only）",
    );
    match crate::proxy_manager::PROXY_MANAGER
      .check_proxy_validity(&check_id, &settings)
      .await
    {
      Ok(result) if result.is_valid && !result.ip.is_empty() => {
        log_bwbrowser(
          "update_account_proxy:bg",
          &format!(
            "  ✓ 代理可用: IP={}, country={}",
            result.ip,
            result.country.as_deref().unwrap_or("")
          ),
        );
        if let Some(exit_geo) = resolve_timezone_online(&result.ip).await {
          log_bwbrowser(
            "update_account_proxy:bg",
            &format!(
              "  ✓ 代理出口 IP 时区（在线）: exit_ip={}, timezone={}",
              result.ip, exit_geo.timezone
            ),
          );
          return exit_geo;
        }
        if let Some(tz) = result.timezone {
          log_bwbrowser(
            "update_account_proxy:bg",
            &format!(
              "  ✓ 代理出口 IP 时区（MaxMind）: exit_ip={}, timezone={}",
              result.ip, tz
            ),
          );
          return ProxyGeoInfo {
            timezone: tz,
            language: "en-US".to_string(),
            latitude: None,
            longitude: None,
          };
        }
      }
      Ok(result) => {
        log_bwbrowser_error(
          "update_account_proxy:bg",
          &format!(
            "  ⚠ 代理检测无效: valid={}, ip={}",
            result.is_valid, result.ip
          ),
        );
      }
      Err(e) => {
        log_bwbrowser_error(
          "update_account_proxy:bg",
          &format!("  ⚠ 代理连接检测失败: {}", e),
        );
      }
    }
    // 方案2：在线 API 查代理服务器 IP
    if let Some(g) = resolve_timezone_online(&host).await {
      log_bwbrowser(
        "update_account_proxy:bg",
        &format!(
          "  ✓ 回退: 在线 API 查代理服务器 IP 时区: host={}, timezone={}",
          host, g.timezone
        ),
      );
      return g;
    }
    // 方案3：本地 MaxMind 数据库兜底
    match crate::geolocation::get_geolocation(&host) {
      Ok(geo) => ProxyGeoInfo {
        timezone: geo.timezone.clone(),
        language: geo.locale.language.clone(),
        latitude: Some(geo.latitude),
        longitude: Some(geo.longitude),
      },
      Err(e) => {
        log_bwbrowser_error(
          "update_account_proxy:bg",
          &format!("  ✗ 代理时区解析失败: host={}, err={}", host, e),
        );
        ProxyGeoInfo {
          timezone: "America/Los_Angeles".to_string(),
          language: "en-US".to_string(),
          latitude: None,
          longitude: None,
        }
      }
    }
  }
  .await;
  log_bwbrowser(
    "update_account_proxy:bg",
    &format!(
      "  ✓ 代理时区解析: host={}, timezone={}, language={}",
      host, geo_info.timezone, geo_info.language
    ),
  );

  // 更新本地代理的 geo_timezone（启动时 resolve_geo_from_proxy 优先读本地）
  let all_local = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
  for local_p in &all_local {
    if local_p.proxy_settings.host == host {
      crate::proxy_manager::PROXY_MANAGER
        .update_proxy_geo(&local_p.id, Some(geo_info.timezone.clone()));
      log_bwbrowser(
        "update_account_proxy:bg",
        &format!(
          "  ✓ 本地代理时区已更新: id={}, timezone={}",
          local_p.id, geo_info.timezone
        ),
      );
      break;
    }
  }

  // 尝试同步到服务器代理表
  if let Ok(cloud_proxies) = BWBROWSER_AUTH.list_cloud_proxies(None).await {
    let matching = cloud_proxies.iter().find(|p| p.host == host);
    if let Some(cp) = matching {
      log_bwbrowser(
        "update_account_proxy:bg",
        &format!(
          "  → 回传时区到云端代理: cloud_id={}, timezone={}",
          cp.proxy_id, geo_info.timezone
        ),
      );
      match BWBROWSER_AUTH
        .sync_cloud_proxy(
          Some(cp.proxy_id),
          &cp.proxy_name.clone().unwrap_or_default(),
          &cp.proxy_type,
          &cp.host,
          cp.port,
          cp.username.as_deref(),
          cp.password.as_deref(),
          cp.country.as_deref(),
          cp.city.as_deref(),
          Some(&geo_info.timezone),
          cp.protocol_config.as_deref(),
        )
        .await
      {
        Ok(_) => {
          log_bwbrowser(
            "update_account_proxy:bg",
            &format!("  ✓ 时区已回传到服务器: timezone={}", geo_info.timezone),
          );
        }
        Err(e) => {
          log_bwbrowser(
            "update_account_proxy:bg",
            &format!("  ⚠ 时区回传失败（不影响本地使用）: {}", e),
          );
        }
      }
    } else {
      let proxy_name = format!("云_{}", proxy_node);
      log_bwbrowser(
        "update_account_proxy:bg",
        &format!(
          "  → 云端无此代理，新建并回传时区: host={}, timezone={}",
          host, geo_info.timezone
        ),
      );
      match BWBROWSER_AUTH
        .sync_cloud_proxy(
          None,
          &proxy_name,
          &proxy_type,
          &host,
          port,
          None,
          None,
          None,
          None,
          Some(&geo_info.timezone),
          None,
        )
        .await
      {
        Ok(cloud_id) => {
          log_bwbrowser(
            "update_account_proxy:bg",
            &format!(
              "  ✓ 代理已创建并回传时区: cloud_id={}, timezone={}",
              cloud_id, geo_info.timezone
            ),
          );
        }
        Err(e) => {
          log_bwbrowser(
            "update_account_proxy:bg",
            &format!("  ⚠ 代理创建/时区回传失败（不影响本地使用）: {}", e),
          );
        }
      }
    }
  }
  _ = account_name; // 保留字段，便于日志/后续扩展

  // 后台完成后通知前端刷新
  let _ = app_handle.emit(
    "proxy-geo-updated",
    serde_json::json!({
      "account_id": account_id,
      "proxy_node": proxy_node,
      "timezone": geo_info.timezone,
      "language": geo_info.language,
    }),
  );
  log_bwbrowser(
    "update_account_proxy:bg",
    "  ✓ 后台时区解析完成，已通知前端",
  );
}

#[tauri::command]
pub async fn bwbrowser_update_account_proxy(
  app_handle: tauri::AppHandle,
  account_id: i64,
  account_name: String,
  proxy_node: String,
) -> Result<serde_json::Value, String> {
  log_bwbrowser(
    "update_account_proxy",
    &format!(
      "====== 开始设置代理 ======\n  account_id={}\n  proxy_node={} (len={})\n  proxy_node 前缀={}",
      account_id,
      proxy_node,
      proxy_node.len(),
      trunc(&proxy_node, 20)
    ),
  );

  // Step 1: 更新云端账号代理
  log_bwbrowser("update_account_proxy", "Step 1: 更新云端账号代理...");
  BWBROWSER_AUTH
    .update_account_proxy(account_id, &proxy_node)
    .await?;
  log_bwbrowser("update_account_proxy", "  ✓ 云端代理更新成功");

  // Step 2: 同步更新本地 profile 的 proxy_id（直接用前端传入的 account_name）
  let new_proxy_id = format!("{}{}", crate::cloud_proxy_manager::NODE_PREFIX, proxy_node);
  log_bwbrowser(
    "update_account_proxy",
    &format!("Step 2: 同步本地 profile proxy_id={}", new_proxy_id),
  );

  {
    let profile_name = account_profile_name(&account_name, account_id);
    log_bwbrowser(
      "update_account_proxy",
      &format!("  account_name+id={}", profile_name),
    );

    let pm = crate::profile::manager::ProfileManager::instance();
    let profiles = pm.list_profiles().unwrap_or_default();
    log_bwbrowser(
      "update_account_proxy",
      &format!(
        "  本地 profile 总数={}, 查找 name={} (大小写不敏感)",
        profiles.len(),
        profile_name
      ),
    );

    let all_names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    log_bwbrowser(
      "update_account_proxy",
      &format!("  所有 profile 名称: {:?}", all_names),
    );

    let matching_profiles: Vec<_> = profiles
      .into_iter()
      .filter(|p| p.name.to_lowercase() == profile_name.to_lowercase())
      .collect();
    let matching_profiles_was_empty = matching_profiles.is_empty();

    log_bwbrowser(
      "update_account_proxy",
      &format!("  匹配到 {} 个同名 profile", matching_profiles.len()),
    );

    for profile in &matching_profiles {
      let profile_id = profile.id.to_string();
      let old_proxy_id = profile.proxy_id.clone();
      log_bwbrowser(
        "update_account_proxy",
        &format!(
          "  更新 profile: id={}, name={}, 当前 proxy_id={:?}",
          profile_id, profile.name, old_proxy_id
        ),
      );

      match pm
        .update_profile_proxy(app_handle.clone(), &profile_id, Some(new_proxy_id.clone()))
        .await
      {
        Ok(updated) => {
          log_bwbrowser(
            "update_account_proxy",
            &format!(
              "  update_profile_proxy Ok: id={}, proxy_id={:?} (之前={:?})",
              profile_id, updated.proxy_id, old_proxy_id
            ),
          );
        }
        Err(e) => {
          log_bwbrowser_error(
            "update_account_proxy",
            &format!("  更新 profile {} 失败: {}", profile_id, e),
          );
        }
      }
    }

    // 对每个匹配的 profile 直接写入 metadata.json 确保持久化
    for profile in &matching_profiles {
      let profile_id = profile.id.to_string();
      let profiles_dir = pm.get_profiles_dir();
      let metadata_path = profiles_dir.join(&profile_id).join("metadata.json");
      match std::fs::read_to_string(&metadata_path) {
        Ok(content) => {
          if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) {
            let current_pid = json
              .get("proxy_id")
              .and_then(|v| v.as_str())
              .map(|s| s.to_string());
            if current_pid.as_deref() != Some(new_proxy_id.as_str()) {
              log_bwbrowser(
                "update_account_proxy",
                &format!(
                  "  直接写入 metadata.json: id={}, old={:?}, new={:?}",
                  profile_id, current_pid, new_proxy_id
                ),
              );
              json["proxy_id"] = serde_json::Value::String(new_proxy_id.clone());
              json["vpn_id"] = serde_json::Value::Null;
              json["updated_at"] = serde_json::Value::Number(serde_json::Number::from(
                crate::proxy_manager::now_secs(),
              ));
              if let Ok(new_json) = serde_json::to_string_pretty(&json) {
                if let Err(e) = std::fs::write(&metadata_path, new_json.as_bytes()) {
                  log_bwbrowser_error("update_account_proxy", &format!("  写入失败: {}", e));
                } else {
                  log_bwbrowser("update_account_proxy", "  ✓ 直接写入成功");
                }
              }
            }
          }
        }
        Err(e) => {
          log_bwbrowser_error(
            "update_account_proxy",
            &format!("  读取 metadata.json 失败: {}", e),
          );
        }
      }
    }

    if matching_profiles_was_empty {
      log_bwbrowser(
        "update_account_proxy",
        &format!(
          "  ℹ 未找到匹配的本地 profile: name={}（正常，启动时会从云端拉取代理配置）",
          profile_name
        ),
      );
    }
  }

  // 设置代理成功后，解析代理主机 IP 的时区并同步到服务器
  // proxy_node 格式:
  // - VLESS/Trojan: 完整 URI（vless://... 或 trojan://...）
  // - HTTP/SOCKS5: type:host:port:user:pass 或旧格式 host:port:user:pass
  let (proxy_type, host, port): (String, String, i64) =
    if proxy_node.starts_with("vless://") || proxy_node.starts_with("trojan://") {
      let pt = if proxy_node.starts_with("vless://") {
        "vless"
      } else {
        "trojan"
      };
      let (h, p) = if proxy_node.starts_with("trojan://") {
        let parsed = crate::xray::parse_trojan_uri(&proxy_node).ok();
        (
          parsed
            .as_ref()
            .map(|p| p.config.address.clone())
            .unwrap_or_default(),
          parsed.as_ref().map(|p| p.config.port as i64).unwrap_or(0),
        )
      } else {
        let parsed = crate::xray::parse_vless_uri(&proxy_node).ok();
        (
          parsed
            .as_ref()
            .map(|p| p.config.address.clone())
            .unwrap_or_default(),
          parsed.as_ref().map(|p| p.config.port as i64).unwrap_or(0),
        )
      };
      (pt.to_string(), h, p)
    } else {
      let parts: Vec<&str> = proxy_node.split(':').collect();
      const KNOWN_TYPES: &[&str] = &["http", "socks5"];
      if parts.len() >= 2 {
        let first = parts[0].to_lowercase();
        if KNOWN_TYPES.contains(&first.as_str()) {
          (
            first,
            parts[1].to_string(),
            parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0),
          )
        } else {
          (
            "http".to_string(),
            parts[0].to_string(),
            parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0),
          )
        }
      } else {
        (String::new(), String::new(), 0)
      }
    };

  let settings = crate::cloud_proxy_manager::parse_proxy_node(&proxy_node).unwrap_or(
    crate::browser::ProxySettings {
      proxy_type: if proxy_type.is_empty() {
        "http".to_string()
      } else {
        proxy_type.clone()
      },
      host: host.clone(),
      port: port as u16,
      username: None,
      password: None,
      vless_uri: if proxy_node.starts_with("vless://") || proxy_node.starts_with("trojan://") {
        Some(proxy_node.clone())
      } else {
        None
      },
    },
  );

  if !host.is_empty() {
    // 快速保存：云端代理 + 本地 profile 已在上方持久化完成。
    // 耗时较长的代理有效性检测 + 时区解析放后台执行，避免阻塞 UI，
    // 用户可立即继续操作下一个账号；完成后通过 proxy-geo-updated 事件通知前端。
    let app_handle = app_handle.clone();
    let account_name = account_name.clone();
    let proxy_node = proxy_node.clone();
    let host = host.clone();
    let settings = settings.clone();
    tauri::async_runtime::spawn(async move {
      resolve_and_sync_proxy_geo(
        app_handle,
        account_id,
        account_name,
        proxy_node,
        host,
        port,
        proxy_type,
        settings,
      )
      .await;
    });
    return Ok(serde_json::json!({
      "success": true,
      "geo_pending": true,
      "timezone": null,
      "language": null,
    }));
  }

  Ok(serde_json::json!({ "success": true }))
}

// ========== list_cloud_users 方法实现 ==========

impl BwbrowserAuthManager {
  /// 获取云端用户列表（含钉钉考勤状态）
  pub async fn list_cloud_users(
    &self,
    company_id: Option<i64>,
  ) -> Result<CloudUserListResponse, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser("list_users", &format!("→ 请求用户列表: user={}", username));

    let mut form_data = format!(
      "action=list_users&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );
    if let Some(cid) = company_id {
      form_data.push_str(&format!("&company_id={}", cid));
    }

    let mut last_err: Option<String> = None;
    for url in SIMPRINT_ACCOUNTS_URLS {
      let resp = match self
        .client
        .post(*url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_data.clone())
        .send()
        .await
      {
        Ok(r) => r,
        Err(e) => {
          last_err = Some(format!("网络请求失败 ({}): {}", url, e));
          continue;
        }
      };

      let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
          last_err = Some(format!("读取响应失败 ({}): {}", url, e));
          continue;
        }
      };

      log_bwbrowser("list_users", &format!("  响应: {}", body));

      match serde_json::from_str::<CloudUserListResponse>(&body) {
        Ok(result) => {
          if !result.success {
            let msg = result
              .message
              .clone()
              .unwrap_or_else(|| "获取用户列表失败".to_string());
            return Err(msg);
          }
          return Ok(result);
        }
        Err(e) => {
          last_err = Some(format!("解析响应失败 ({}): {}", url, e));
          continue;
        }
      }
    }

    Err(last_err.unwrap_or_else(|| "所有服务器均请求失败".to_string()))
  }

  /// 获取公司列表（仅超级管理员可用）
  pub async fn list_cloud_companies(&self) -> Result<CompanyListResponse, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "list_companies",
      &format!("→ 请求公司列表: user={}", username),
    );

    let form_data = format!(
      "action=list_companies&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("网络请求失败: {}", e))?;

    let status = resp.status();
    let body = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败: {}", e))?;

    log_bwbrowser(
      "list_companies",
      &format!("← 响应状态: {}, 长度: {} bytes", status, body.len()),
    );

    let result: CompanyListResponse = parse_body("list_companies", &body)?;

    if !result.success {
      let msg = result
        .message
        .clone()
        .unwrap_or_else(|| "获取公司列表失败".to_string());
      return Err(msg);
    }

    log_bwbrowser(
      "list_companies",
      &format!(
        "✓ 获取成功，共 {} 家公司",
        result.companies.as_ref().map(|c| c.len()).unwrap_or(0)
      ),
    );

    Ok(result)
  }

  /// 设置云端账号的代理节点
  pub async fn update_account_proxy(
    &self,
    account_id: i64,
    proxy_node: &str,
  ) -> Result<(), String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "update_account_proxy",
      &format!(
        "→ 设置账号代理: account_id={}, proxy={}",
        account_id, proxy_node
      ),
    );

    // 先确保 proxy_node 列为 TEXT 类型（兼容旧库 VARCHAR 限制）
    let alter_form = format!(
      "action=ensure_proxy_text&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );
    for url in SIMPRINT_ACCOUNTS_URLS {
      let _ = self
        .client
        .post(*url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(alter_form.clone())
        .send()
        .await;
    }

    let form_data = format!(
      "action=update_proxy&username={}&password={}&account_id={}&proxy_node={}",
      urlencode(&username),
      urlencode(&password),
      account_id,
      urlencode(proxy_node)
    );

    let mut last_err: Option<String> = None;
    for url in SIMPRINT_ACCOUNTS_URLS {
      let resp = match self
        .client
        .post(*url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_data.clone())
        .send()
        .await
      {
        Ok(r) => r,
        Err(e) => {
          last_err = Some(format!("网络请求失败 ({}): {}", url, e));
          continue;
        }
      };

      let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
          last_err = Some(format!("读取响应失败 ({}): {}", url, e));
          continue;
        }
      };

      log_bwbrowser("update_account_proxy", &format!("  响应: {}", body));

      // 解析响应，success 或 code === 1 都算成功
      #[derive(Deserialize)]
      struct UpdateProxyResponse {
        success: Option<bool>,
        code: Option<i32>,
        message: Option<String>,
      }

      match serde_json::from_str::<UpdateProxyResponse>(&body) {
        Ok(result) => {
          let is_success = result.success.unwrap_or(false) || result.code == Some(1);
          if !is_success {
            let msg = result
              .message
              .clone()
              .unwrap_or_else(|| "设置代理失败".to_string());
            return Err(msg);
          }
          log_bwbrowser("update_account_proxy", "  ✓ 设置成功");
          return Ok(());
        }
        Err(e) => {
          last_err = Some(format!("解析响应失败 ({}): {}", url, e));
          continue;
        }
      }
    }

    Err(last_err.unwrap_or_else(|| "所有服务器均请求失败".to_string()))
  }
}

// ========== 代理格式转换 ==========

use crate::browser::ProxySettings;

/// 将 Bwbrowser 代理转换为 Bwbrowser ProxySettings
pub fn bwbrowser_to_proxy_settings(sp: &BwbrowserProxy) -> ProxySettings {
  // 处理 VLESS/Trojan 等高级协议的 protocol_config
  let vless_uri = if sp.proxy_type == "vless" || sp.proxy_type == "trojan" || sp.proxy_type == "ss"
  {
    if let Some(pc) = &sp.protocol_config {
      let pc_trimmed = pc.trim();
      // 先尝试当作 JSON 解析
      if let Ok(config) = serde_json::from_str::<serde_json::Value>(pc_trimmed) {
        let key = if sp.proxy_type == "vless" {
          "vless_url"
        } else if sp.proxy_type == "trojan" {
          "trojan_url"
        } else {
          "ss_url"
        };
        let extracted = config
          .get(key)
          .and_then(|v| v.as_str())
          .map(|s| s.to_string());
        if extracted.is_none() {
          log_bwbrowser_error(
            "bwbrowser_to_proxy_settings",
            &format!(
              "  ⚠ JSON protocol_config 缺少 key={}: {}",
              key,
              trunc(&pc_trimmed, 200)
            ),
          );
        }
        extracted
      } else {
        // 不是 JSON，直接当作原始 URI 使用
        let prefix = format!("{}://", sp.proxy_type);
        if pc_trimmed.starts_with(&prefix) {
          Some(pc_trimmed.to_string())
        } else {
          // 尝试不区分大小写
          if pc_trimmed
            .to_lowercase()
            .starts_with(&prefix.to_lowercase())
          {
            Some(pc_trimmed.to_string())
          } else {
            log_bwbrowser_error(
              "bwbrowser_to_proxy_settings",
              &format!(
                "  ⚠ protocol_config 不以 {} 开头: {}",
                prefix,
                trunc(&pc_trimmed, 200)
              ),
            );
            None
          }
        }
      }
    } else {
      log_bwbrowser_error(
        "bwbrowser_to_proxy_settings",
        &format!(
          "  ⚠ {} 代理缺少 protocol_config 字段, proxy_id={}",
          sp.proxy_type, sp.proxy_id
        ),
      );
      None
    }
  } else {
    None
  };

  ProxySettings {
    proxy_type: sp.proxy_type.clone(),
    host: sp.host.clone(),
    port: sp.port as u16,
    username: sp.username.clone(),
    password: sp.password.clone(),
    vless_uri,
  }
}

/// 同步结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
  pub cloud_total: usize,
  pub created: usize,
  pub skipped: usize,
  pub removed: usize,
}

/// 前端使用的代理数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BwbrowserProxyItem {
  pub id: String,         // "bwbrowser_{proxy_id}"
  pub name: String,       // 代理名称
  pub proxy_type: String, // 协议类型
  pub host: String,
  pub port: u16,
  pub username: Option<String>,
  pub password: Option<String>,
  pub vless_uri: Option<String>,
  pub country: Option<String>,
  pub city: Option<String>,
  pub provider: Option<String>,
  pub is_cloud_managed: bool,
}

impl BwbrowserProxyItem {
  fn from_bwbrowser(sp: &BwbrowserProxy) -> Self {
    let settings = bwbrowser_to_proxy_settings(sp);
    Self {
      id: format!("bwbrowser_{}", sp.proxy_id),
      name: sp
        .proxy_name
        .clone()
        .unwrap_or_else(|| format!("{}:{}", sp.host, sp.port)),
      proxy_type: settings.proxy_type,
      host: settings.host,
      port: settings.port,
      username: settings.username,
      password: settings.password,
      vless_uri: settings.vless_uri,
      country: sp.country.clone(),
      city: sp.city.clone(),
      provider: sp.provider.clone(),
      is_cloud_managed: true,
    }
  }
}

// ========== Tauri Commands - 代理同步 ==========

#[tauri::command]
pub async fn bwbrowser_list_proxies(
  company_id: Option<i64>,
) -> Result<Vec<BwbrowserProxyItem>, String> {
  let proxies = BWBROWSER_AUTH.list_cloud_proxies(company_id).await?;
  Ok(
    proxies
      .iter()
      .map(BwbrowserProxyItem::from_bwbrowser)
      .collect(),
  )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn bwbrowser_sync_proxy(
  proxy_id: Option<i64>,
  proxy_name: String,
  proxy_type: String,
  host: String,
  port: u16,
  username: Option<String>,
  password: Option<String>,
  country: Option<String>,
  city: Option<String>,
  timezone: Option<String>,
  protocol_config: Option<String>,
) -> Result<i64, String> {
  BWBROWSER_AUTH
    .sync_cloud_proxy(
      proxy_id,
      &proxy_name,
      &proxy_type,
      &host,
      port as i64,
      username.as_deref(),
      password.as_deref(),
      country.as_deref(),
      city.as_deref(),
      timezone.as_deref(),
      protocol_config.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn bwbrowser_delete_proxy(proxy_id: i64) -> Result<(), String> {
  BWBROWSER_AUTH.delete_cloud_proxy(proxy_id).await
}

/// 测试代理成功后自动回传探测到的国家/城市到云端
#[tauri::command]
pub async fn bwbrowser_sync_proxy_geo(
  proxy_id: i64,
  country: Option<String>,
  city: Option<String>,
  timezone: Option<String>,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .sync_proxy_geo(
      proxy_id,
      country.as_deref(),
      city.as_deref(),
      timezone.as_deref(),
    )
    .await
}

/// 批量同步云端代理到本地缓存（云端 → 本地）
/// 同步后本地代理与云端保持一致：新增的创建、已有的跳过、云端已删的清理本地
#[tauri::command]
pub async fn bwbrowser_sync_proxies_to_local(
  app_handle: tauri::AppHandle,
) -> Result<SyncResult, String> {
  let cloud_proxies = BWBROWSER_AUTH.list_cloud_proxies_force().await?;
  let local_proxies = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();

  let mut created = 0;
  let mut skipped = 0;
  let mut removed = 0;

  // 云端有、本地没有 → 创建
  for cp in &cloud_proxies {
    if cp.proxy_type == "vless" || cp.proxy_type == "trojan" || cp.proxy_type == "ss" {
      log_bwbrowser(
        "sync_proxies_to_local",
        &format!(
          "  高级协议代理: id={}, type={}, host={}, port={}, protocol_config={:?}",
          cp.proxy_id,
          cp.proxy_type,
          cp.host,
          cp.port,
          cp.protocol_config.as_deref().map(|s| trunc(s, 100))
        ),
      );
    }
    // 提取 URI（和 bwbrowser_to_proxy_settings 同逻辑），用于匹配和创建
    let settings = bwbrowser_to_proxy_settings(cp);
    let cp_uri_extracted = settings.vless_uri.as_deref().map(|s| s.trim().to_string());

    let name = cp
      .proxy_name
      .clone()
      .unwrap_or_else(|| format!("云_{}:{}", cp.host, cp.port));

    let exists = if cp.proxy_type == "vless" || cp.proxy_type == "trojan" || cp.proxy_type == "ss" {
      // 高级协议：先按 URI 匹配，匹配不到按 host:port 兜底
      // （本地旧格式代理可能缺 vless_uri，但有相同的 host:port）
      let by_uri = if cp_uri_extracted.is_some() {
        local_proxies.iter().any(|lp| {
          lp.proxy_settings.proxy_type == cp.proxy_type
            && lp.proxy_settings.vless_uri.as_deref() == cp_uri_extracted.as_deref()
        })
      } else {
        false
      };
      by_uri
        || local_proxies.iter().any(|lp| {
          lp.proxy_settings.proxy_type == cp.proxy_type
            && lp.proxy_settings.host == cp.host
            && lp.proxy_settings.port as i64 == cp.port
        })
        || local_proxies.iter().any(|lp| lp.name == name)
    } else {
      local_proxies.iter().any(|lp| {
        (lp.proxy_settings.host == cp.host && lp.proxy_settings.port as i64 == cp.port)
          || lp.name == name
      })
    };
    if !exists {
      // 本机 xray 无法解析的高级协议节点（如 VLESS security=tls 等非 reality）:
      // 仍按原始配置存入本地“仅显示”代理，保证它出现在设置代理下拉框，也不刷错误日志。
      if cp.proxy_type == "vless" || cp.proxy_type == "trojan" {
        let unparsable = match &cp_uri_extracted {
          Some(uri) => match cp.proxy_type.as_str() {
            "trojan" => crate::xray::parse_trojan_uri(uri).is_err(),
            _ => crate::xray::parse_vless_uri(uri).is_err(),
          },
          None => true,
        };
        if unparsable {
          match crate::proxy_manager::PROXY_MANAGER.insert_lenient_cloud_proxy(name, settings) {
            Ok(stored) => {
              log_bwbrowser(
                "sync_proxies",
                &format!(
                  "  ✓ 以原始配置存入本机不支持的{}节点(仅显示): {}:{} -> {}",
                  cp.proxy_type, cp.host, cp.port, stored.id
                ),
              );
              created += 1;
            }
            Err(e) => {
              log_bwbrowser_error(
                "sync_proxies",
                &format!("  ✗ 存入失败: {}:{} - {}", cp.host, cp.port, e),
              );
            }
          }
          continue;
        }
      }
      match crate::proxy_manager::PROXY_MANAGER.create_stored_proxy(&app_handle, name, settings) {
        Ok(mut stored) => {
          // 如果云端代理有时区信息，直接写入本地，省去重复解析
          if let Some(ref tz) = cp.timezone {
            if !tz.is_empty() {
              crate::proxy_manager::PROXY_MANAGER.update_proxy_geo(&stored.id, Some(tz.clone()));
              stored.geo_timezone = Some(tz.clone());
              log_bwbrowser(
                "sync_proxies",
                &format!(
                  "  ✓ 创建本地缓存: {} -> {} (timezone={} from cloud)",
                  cp.host, stored.id, tz
                ),
              );
            } else {
              log_bwbrowser(
                "sync_proxies",
                &format!("  ✓ 创建本地缓存: {} -> {}", cp.host, stored.id),
              );
            }
          } else {
            log_bwbrowser(
              "sync_proxies",
              &format!("  ✓ 创建本地缓存: {} -> {}", cp.host, stored.id),
            );
          }
          created += 1;
        }
        Err(e) => {
          log_bwbrowser_error(
            "sync_proxies",
            &format!("  ✗ 创建失败: {}:{} - {}", cp.host, cp.port, e),
          );
        }
      }
    } else {
      // 已存在的代理：补全缺失字段
      let local_match =
        if cp.proxy_type == "vless" || cp.proxy_type == "trojan" || cp.proxy_type == "ss" {
          // 先按 URI 匹配
          let by_uri = if cp_uri_extracted.is_some() {
            local_proxies.iter().find(|lp| {
              lp.proxy_settings.proxy_type == cp.proxy_type
                && lp.proxy_settings.vless_uri.as_deref() == cp_uri_extracted.as_deref()
            })
          } else {
            None
          };
          // 如果 URI 匹配不到，按 host:port 兜底（本地可能缺 vless_uri）
          by_uri.or_else(|| {
            local_proxies.iter().find(|lp| {
              lp.proxy_settings.proxy_type == cp.proxy_type
                && lp.proxy_settings.host == cp.host
                && lp.proxy_settings.port as i64 == cp.port
            })
          })
        } else {
          local_proxies.iter().find(|lp| {
            lp.proxy_settings.host == cp.host && lp.proxy_settings.port as i64 == cp.port
          })
        };
      if let Some(local) = local_match {
        let cloud_settings = bwbrowser_to_proxy_settings(cp);
        let local_settings = &local.proxy_settings;
        let needs_update = cloud_settings.proxy_type != local_settings.proxy_type
          || cloud_settings.host != local_settings.host
          || cloud_settings.port != local_settings.port
          || cloud_settings.username != local_settings.username
          || cloud_settings.password != local_settings.password
          || cloud_settings.vless_uri != local_settings.vless_uri;

        if needs_update {
          let _ = crate::proxy_manager::PROXY_MANAGER.update_stored_proxy(
            &app_handle,
            &local.id,
            cp.proxy_name.clone(),
            Some(cloud_settings),
          );
          log_bwbrowser(
            "sync_proxies",
            &format!(
              "  ✓ 覆盖更新本地代理: {} (host={}, port={})",
              local.id, cp.host, cp.port
            ),
          );
          created += 1;
        } else {
          let local_tz_empty = local.geo_timezone.as_ref().is_none_or(|tz| tz.is_empty());
          if local_tz_empty {
            if let Some(ref tz) = cp.timezone {
              if !tz.is_empty() {
                crate::proxy_manager::PROXY_MANAGER.update_proxy_geo(&local.id, Some(tz.clone()));
                log_bwbrowser(
                  "sync_proxies",
                  &format!("  ✓ 补全本地代理时区: {} -> {}", local.id, tz),
                );
              }
            }
          }
          skipped += 1;
        }
      } else {
        skipped += 1;
      }
    }
  }

  // 本地有、云端没有 → 删除本地缓存
  // 高级协议按 URI 匹配，普通协议按 host:port 匹配
  // 注意：如果云端 protocol_config 为空（服务器未更新字段），
  // 不要删除本地有 URI 的代理，避免丢失配置
  let cloud_keys: std::collections::HashSet<(String, i64)> = cloud_proxies
    .iter()
    .filter(|cp| !["vless", "trojan", "ss"].contains(&cp.proxy_type.as_str()))
    .map(|cp| (cp.host.clone(), cp.port))
    .collect();
  // 提取高级协议的 URI（和 bwbrowser_to_proxy_settings 同逻辑）
  let cloud_uris: std::collections::HashSet<String> = cloud_proxies
    .iter()
    .filter(|cp| ["vless", "trojan", "ss"].contains(&cp.proxy_type.as_str()))
    .filter_map(|cp| {
      let s = bwbrowser_to_proxy_settings(cp);
      s.vless_uri.map(|u| u.trim().to_string())
    })
    .filter(|s| !s.is_empty())
    .collect();
  let cloud_advanced_hostport: std::collections::HashSet<(String, i64)> = cloud_proxies
    .iter()
    .filter(|cp| ["vless", "trojan", "ss"].contains(&cp.proxy_type.as_str()))
    .map(|cp| (cp.host.clone(), cp.port))
    .collect();
  for lp in &local_proxies {
    let is_advanced = ["vless", "trojan", "ss"]
      .iter()
      .any(|t| lp.proxy_settings.proxy_type.eq_ignore_ascii_case(t));
    let should_delete = if is_advanced {
      let lp_uri = lp
        .proxy_settings
        .vless_uri
        .as_deref()
        .map(|s| s.trim().to_string());
      match lp_uri.as_deref() {
        None | Some("") => {
          let key = (
            lp.proxy_settings.host.clone(),
            lp.proxy_settings.port as i64,
          );
          !cloud_advanced_hostport.contains(&key)
        }
        Some(_uri) if cloud_uris.is_empty() => {
          let key = (
            lp.proxy_settings.host.clone(),
            lp.proxy_settings.port as i64,
          );
          !cloud_advanced_hostport.contains(&key)
        }
        Some(uri) => !cloud_uris.contains(uri),
      }
    } else {
      let key = (
        lp.proxy_settings.host.clone(),
        lp.proxy_settings.port as i64,
      );
      !cloud_keys.contains(&key)
    };
    if should_delete && lp.name.starts_with("云_") {
      if let Err(e) = crate::proxy_manager::PROXY_MANAGER.delete_stored_proxy(&app_handle, &lp.id) {
        log_bwbrowser_error(
          "sync_proxies",
          &format!("  ✗ 删除本地缓存失败: {} - {}", lp.id, e),
        );
      } else {
        log_bwbrowser("sync_proxies", &format!("  ✓ 删除本地缓存: {}", lp.id));
        removed += 1;
      }
    }
  }

  log_bwbrowser(
    "sync_proxies",
    &format!(
      "✓ 同步完成: 云端={}, 新建={}, 跳过={}, 清理={}",
      cloud_proxies.len(),
      created,
      skipped,
      removed
    ),
  );

  Ok(SyncResult {
    cloud_total: cloud_proxies.len(),
    created,
    skipped,
    removed,
  })
}

// ========== 代理双向同步 ==========

/// 将云端代理导入到本地存储（云端 → 本地）
#[tauri::command]
pub async fn bwbrowser_pull_proxy_to_local(
  app_handle: tauri::AppHandle,
  proxy_id: i64,
) -> Result<String, String> {
  let proxies = BWBROWSER_AUTH.list_cloud_proxies(None).await?;
  let proxy = proxies
    .iter()
    .find(|p| p.proxy_id == proxy_id)
    .ok_or_else(|| "云端代理不存在".to_string())?;

  let settings = bwbrowser_to_proxy_settings(proxy);
  let name = proxy
    .proxy_name
    .clone()
    .unwrap_or_else(|| format!("云_{}:{}", proxy.host, proxy.port));

  let stored = crate::proxy_manager::PROXY_MANAGER
    .create_stored_proxy(&app_handle, name, settings)
    .map_err(|e| format!("创建本地代理失败: {}", e))?;

  log_bwbrowser(
    "pull_proxy",
    &format!(
      "✓ 云端代理已导入本地: proxy_id={}, local_id={}",
      proxy_id, stored.id
    ),
  );

  Ok(stored.id)
}

/// 将本地代理上传到云端（本地 → 云端）
#[tauri::command]
pub async fn bwbrowser_push_local_proxy_to_cloud(local_proxy_id: String) -> Result<i64, String> {
  let all_local = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
  let stored = all_local
    .iter()
    .find(|p| p.id == local_proxy_id)
    .ok_or_else(|| "本地代理不存在".to_string())?;

  let ps = &stored.proxy_settings;

  let cloud_id = BWBROWSER_AUTH
    .sync_cloud_proxy(
      None,
      &stored.name,
      &ps.proxy_type,
      &ps.host,
      ps.port as i64,
      ps.username.as_deref(),
      ps.password.as_deref(),
      stored.geo_country.as_deref(),
      stored.geo_city.as_deref(),
      stored.geo_timezone.as_deref(),
      None,
    )
    .await?;

  log_bwbrowser(
    "push_proxy",
    &format!(
      "✓ 本地代理已上传云端: local_id={}, cloud_id={}",
      local_proxy_id, cloud_id
    ),
  );

  Ok(cloud_id)
}

// ========== 环境同步 ==========

/// Bwbrowser 云端环境数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BwbrowserEnvironment {
  pub env_uuid: String,
  pub name: String,
  pub description: Option<String>,
  pub group_uuid: Option<String>,
  pub group_name: Option<String>,
  pub proxy_uuid: Option<String>,
  pub browser_type: String,
  /// 指纹配置，兼容字符串和对象两种格式
  pub fingerprint_config: Option<serde_json::Value>,
  /// start_urls 兼容字符串数组和对象数组两种格式
  #[serde(default, deserialize_with = "deserialize_start_urls")]
  pub start_urls: Option<Vec<String>>,
  pub tag_uuids: Option<Vec<String>>,
  #[serde(deserialize_with = "deserialize_int_or_string", default)]
  pub status: Option<String>,
  pub remark: Option<String>,
  pub owner_name: Option<String>,
  #[serde(deserialize_with = "deserialize_int_or_string", default)]
  pub updated_at: Option<String>,
  #[serde(deserialize_with = "deserialize_int_or_string", default)]
  pub created_at: Option<String>,
}

/// 前端使用的环境数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BwbrowserEnvItem {
  pub id: String,
  pub name: String,
  pub env_uuid: String,
  pub browser_type: String,
  pub status: String,
  pub description: Option<String>,
  pub remark: Option<String>,
  pub owner_name: Option<String>,
  pub fingerprint_config: Option<String>,
  pub start_urls: Option<Vec<String>>,
  pub updated_at: Option<String>,
  pub created_at: Option<String>,
}

impl BwbrowserEnvItem {
  pub fn from_bwbrowser(env: &BwbrowserEnvironment) -> Self {
    // 将 fingerprint_config 统一转换为 JSON 字符串
    let fingerprint_str = env
      .fingerprint_config
      .as_ref()
      .and_then(|v| serde_json::to_string(v).ok());

    Self {
      id: format!("bwbrowser_env_{}", env.env_uuid),
      name: env.name.clone(),
      env_uuid: env.env_uuid.clone(),
      browser_type: env.browser_type.clone(),
      status: env.status.clone().unwrap_or_default(),
      description: env.description.clone(),
      remark: env.remark.clone(),
      owner_name: env.owner_name.clone(),
      fingerprint_config: fingerprint_str,
      start_urls: env.start_urls.clone(),
      updated_at: env.updated_at.clone(),
      created_at: env.created_at.clone(),
    }
  }
}

/// list_envs 响应
#[derive(Debug, Deserialize)]
struct ListEnvsResponse {
  success: bool,
  environments: Option<Vec<BwbrowserEnvironment>>,
  message: Option<String>,
}

/// sync_env 响应
#[derive(Debug, Deserialize)]
struct SyncEnvResponse {
  success: bool,
  message: Option<String>,
}

impl BwbrowserAuthManager {
  /// 清除环境列表缓存（增删改环境后调用）
  pub fn invalidate_env_cache(&self) {
    if let Ok(mut cache) = self.env_cache.lock() {
      *cache = None;
      log_bwbrowser("env_cache", "已清除环境列表缓存");
    }
  }

  /// 清除代理列表缓存（增删改代理后调用）
  pub fn invalidate_proxy_cache(&self) {
    if let Ok(mut cache) = self.proxy_cache.lock() {
      *cache = None;
      log_bwbrowser("proxy_cache", "已清除代理列表缓存");
    }
  }

  /// 清除所有缓存（退出登录时调用）
  pub fn invalidate_all_cache(&self) {
    self.invalidate_env_cache();
    self.invalidate_proxy_cache();
  }

  /// 列出云端环境（带内存缓存，同一会话内复用）
  pub async fn list_cloud_envs(
    &self,
    company_id: Option<i64>,
  ) -> Result<Vec<BwbrowserEnvironment>, String> {
    // 先查缓存：5 分钟内复用（切换公司时跳过缓存）
    if company_id.is_none() {
      if let Ok(cache) = self.env_cache.lock() {
        if let Some((ref envs, ref time)) = *cache {
          if time.elapsed() < std::time::Duration::from_secs(300) {
            log_bwbrowser(
              "list_envs",
              &format!(
                "✓ 使用缓存，共 {} 条环境（缓存年龄: {}s）",
                envs.len(),
                time.elapsed().as_secs()
              ),
            );
            return Ok(envs.clone());
          }
        }
      }
    }

    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser("list_envs", &format!("→ 请求环境列表: user={}", username));

    let mut form_data = format!(
      "action=list_envs&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );
    if let Some(cid) = company_id {
      form_data.push_str(&format!("&company_id={}", cid));
    }

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("list_envs", &format!("网络请求失败: {}", e));
        format!("请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
      log_bwbrowser_error("list_envs", &format!("读取响应失败: {}", e));
      format!("读取响应失败: {}", e)
    })?;

    log_bwbrowser(
      "list_envs",
      &format!("← 响应状态: {}, 长度: {} bytes", status, body.len()),
    );
    log_bwbrowser("list_envs", &format!("  响应内容: {}", body));

    let result: ListEnvsResponse = parse_body("list_envs", &body)?;

    if !result.success {
      let msg = result
        .message
        .unwrap_or_else(|| "获取环境列表失败".to_string());
      log_bwbrowser_error("list_envs", &msg);
      return Err(msg);
    }

    let envs = result.environments.unwrap_or_default();
    let count = envs.len();
    log_bwbrowser("list_envs", &format!("✓ 获取成功，共 {} 条环境", count));

    // 写入缓存（切换公司时不缓存）
    if company_id.is_none() {
      if let Ok(mut cache) = self.env_cache.lock() {
        *cache = Some((envs.clone(), std::time::Instant::now()));
      }
    }

    Ok(envs)
  }

  /// 按UUID直接获取环境（不受用户过滤，用于跨用户共享指纹）
  pub async fn get_cloud_env_by_uuid(
    &self,
    env_uuid: &str,
  ) -> Result<BwbrowserEnvironment, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "get_env_by_uuid",
      &format!("→ 请求环境: uuid={}, user={}", env_uuid, username),
    );

    let form_data = format!(
      "action=get_env_by_uuid&username={}&password={}&env_uuid={}",
      urlencode(&username),
      urlencode(&password),
      urlencode(env_uuid),
    );

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("请求失败: {}", e))?;

    let status = resp.status();
    let body = resp
      .text()
      .await
      .map_err(|e| format!("读取响应失败: {}", e))?;

    log_bwbrowser(
      "get_env_by_uuid",
      &format!("← 响应状态: {}, 长度: {} bytes", status, body.len()),
    );

    if !status.is_success() {
      return Err(format!("服务器返回错误: {} - {}", status, body));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body)
      .map_err(|e| format!("解析JSON失败: {} - body: {}", e, trunc(&body, 200)))?;

    if !parsed
      .get("success")
      .and_then(|v| v.as_bool())
      .unwrap_or(false)
    {
      let msg = parsed
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("未知错误");
      return Err(msg.to_string());
    }

    let env_json = parsed
      .get("environment")
      .ok_or("响应中缺少 environment 字段")?;

    let env: BwbrowserEnvironment =
      serde_json::from_value(env_json.clone()).map_err(|e| format!("解析环境失败: {}", e))?;

    log_bwbrowser(
      "get_env_by_uuid",
      &format!("✓ 获取环境成功: uuid={}, name={}", env.env_uuid, env.name),
    );

    Ok(env)
  }

  /// 同步环境到云端（新增或更新）
  #[allow(clippy::too_many_arguments)]
  pub async fn sync_cloud_env(
    &self,
    env_uuid: &str,
    name: &str,
    description: Option<&str>,
    browser_type: &str,
    fingerprint_config: Option<&str>,
    start_urls: Option<&str>,
    remark: Option<&str>,
    status: &str,
  ) -> Result<String, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let mut form_data = format!(
      "action=sync_env&username={}&password={}&env_uuid={}&name={}&browser_type={}&status={}",
      urlencode(&username),
      urlencode(&password),
      urlencode(env_uuid),
      urlencode(name),
      urlencode(browser_type),
      urlencode(status)
    );

    if let Some(desc) = description {
      form_data.push_str(&format!("&description={}", urlencode(desc)));
    }
    if let Some(fp) = fingerprint_config {
      form_data.push_str(&format!("&fingerprint_config={}", urlencode(fp)));
    }
    if let Some(urls) = start_urls {
      form_data.push_str(&format!("&start_urls={}", urlencode(urls)));
    }
    if let Some(r) = remark {
      form_data.push_str(&format!("&remark={}", urlencode(r)));
    }

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("请求失败: {}", e))?;

    let result: SyncEnvResponse = resp
      .json()
      .await
      .map_err(|e| format!("解析响应失败: {}", e))?;

    if !result.success {
      return Err(result.message.unwrap_or_else(|| "同步环境失败".to_string()));
    }

    self.invalidate_env_cache();
    Ok(env_uuid.to_string())
  }

  /// 删除云端环境
  pub async fn delete_cloud_env(&self, env_uuid: &str) -> Result<(), String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let form_data = format!(
      "action=delete_env&username={}&password={}&env_uuid={}",
      urlencode(&username),
      urlencode(&password),
      urlencode(env_uuid)
    );

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| format!("请求失败: {}", e))?;

    let result: SyncEnvResponse = resp
      .json()
      .await
      .map_err(|e| format!("解析响应失败: {}", e))?;

    if !result.success {
      return Err(result.message.unwrap_or_else(|| "删除环境失败".to_string()));
    }

    self.invalidate_env_cache();
    Ok(())
  }
}

#[tauri::command]
pub async fn bwbrowser_list_envs(company_id: Option<i64>) -> Result<Vec<BwbrowserEnvItem>, String> {
  let envs = BWBROWSER_AUTH.list_cloud_envs(company_id).await?;
  Ok(envs.iter().map(BwbrowserEnvItem::from_bwbrowser).collect())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn bwbrowser_sync_env(
  env_uuid: String,
  name: String,
  description: Option<String>,
  browser_type: String,
  fingerprint_config: Option<String>,
  start_urls: Option<String>,
  remark: Option<String>,
  status: Option<String>,
) -> Result<String, String> {
  BWBROWSER_AUTH
    .sync_cloud_env(
      &env_uuid,
      &name,
      description.as_deref(),
      &browser_type,
      fingerprint_config.as_deref(),
      start_urls.as_deref(),
      remark.as_deref(),
      &status.unwrap_or_else(|| "ready".to_string()),
    )
    .await
}

#[tauri::command]
pub async fn bwbrowser_delete_env(env_uuid: String) -> Result<(), String> {
  BWBROWSER_AUTH.delete_cloud_env(&env_uuid).await
}

/// 更新账号的 2FA 链接和短信验证链接
#[tauri::command]
pub async fn bwbrowser_update_account_codes(
  account_id: i64,
  safe_link: Option<String>,
  bind_phone: Option<String>,
) -> Result<(), String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let form_data = format!(
    "action=update_account_codes&username={}&password={}&account_id={}&safe_link={}&bind_phone={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
    urlencode(safe_link.as_deref().unwrap_or("")),
    urlencode(bind_phone.as_deref().unwrap_or("")),
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("更新失败").to_string());
  }

  log_bwbrowser(
    "update_account_codes",
    &format!("✓ 账号 {} 的 2FA/短信链接已更新", account_id),
  );
  Ok(())
}

/// 更新账号的环境绑定
#[tauri::command]
pub async fn bwbrowser_update_account_env(
  account_id: i64,
  env_uuid: Option<String>,
) -> Result<(), String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let form_data = format!(
    "action=update_account_env&username={}&password={}&account_id={}&env_uuid={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
    urlencode(env_uuid.as_deref().unwrap_or("")),
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("更新失败").to_string());
  }

  log_bwbrowser(
    "update_account_env",
    &format!("✓ 账号 {} 的环境已绑定: {:?}", account_id, env_uuid),
  );
  Ok(())
}

/// 更新账号基本信息
#[tauri::command]
pub async fn bwbrowser_update_account_info(
  account_id: i64,
  account_name: Option<String>,
  login_account: Option<String>,
  login_password: Option<String>,
  remark: Option<String>,
  tags: Option<String>,
  category: Option<String>,
  nickname: Option<String>,
  owner_id: Option<i64>,
  status: Option<i32>,
  phone_id: Option<String>,
  bind_phone: Option<String>,
  safe_link: Option<String>,
  backup_email: Option<String>,
) -> Result<(), String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let mut params: Vec<String> = vec![
    "action=update_account_info".to_string(),
    format!("username={}", urlencode(&username)),
    format!("password={}", urlencode(&password)),
    format!("account_id={}", account_id),
  ];

  if let Some(v) = account_name {
    params.push(format!("account_name={}", urlencode(&v)));
  }
  if let Some(v) = login_account {
    params.push(format!("login_account={}", urlencode(&v)));
  }
  if let Some(v) = login_password {
    params.push(format!("login_password={}", urlencode(&v)));
  }
  if let Some(v) = remark {
    params.push(format!("remark={}", urlencode(&v)));
  }
  if let Some(v) = tags {
    params.push(format!("tags={}", urlencode(&v)));
  }
  if let Some(v) = category {
    params.push(format!("category={}", urlencode(&v)));
  }
  if let Some(v) = nickname {
    params.push(format!("nickname={}", urlencode(&v)));
  }
  if let Some(v) = owner_id {
    params.push(format!("owner_id={}", v));
  }
  if let Some(v) = status {
    params.push(format!("status={}", v));
  }
  if let Some(v) = phone_id {
    params.push(format!("phone_id={}", urlencode(&v)));
  }
  if let Some(v) = bind_phone {
    params.push(format!("bind_phone={}", urlencode(&v)));
  }
  if let Some(v) = safe_link {
    params.push(format!("safe_link={}", urlencode(&v)));
  }
  if let Some(v) = backup_email {
    params.push(format!("backup_email={}", urlencode(&v)));
  }

  let form_data = params.join("&");

  log_bwbrowser(
    "update_account_info",
    &format!(
      "→ 发送请求: {}",
      form_data.replace(
        &format!("password={}", urlencode(&password)),
        "password=***"
      )
    ),
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;

  log_bwbrowser(
    "update_account_info",
    &format!(
      "← 响应: {}",
      if body.len() > 500 {
        format!("{}", trunc(&body, 500))
      } else {
        body.clone()
      }
    ),
  );

  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("更新失败").to_string());
  }

  if let Some(account) = result.get("account") {
    log_bwbrowser(
      "update_account_info",
      &format!(
        "✓ 账号 {} 已更新: login_account={:?}, phone_id={:?}, bind_phone={:?}, safe_link={:?}, owner_id={:?}, backup_email={:?}",
        account_id,
        account["login_account"].as_str(),
        account["phone_id"].as_str(),
        account["bind_phone"].as_str(),
        account["safe_link"].as_str(),
        account["owner_id"].as_i64(),
        account["backup_email"].as_str(),
      ),
    );
  } else {
    log_bwbrowser(
      "update_account_info",
      &format!("✓ 账号 {} 基本信息已更新 (无返回数据)", account_id),
    );
  }
  Ok(())
}

/// 获取账号详情（含account_details表数据，返回完整JSON）
#[tauri::command]
pub async fn bwbrowser_get_account_detail_full(
  account_id: i64,
) -> Result<serde_json::Value, String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let form_data = format!(
    "action=get_account_detail&username={}&password={}&account_id={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(
      result["message"]
        .as_str()
        .unwrap_or("获取详情失败")
        .to_string(),
    );
  }

  Ok(result["account"].clone())
}

/// 创建云端账号
#[tauri::command]
pub async fn bwbrowser_create_account(
  phone_id: Option<String>,
  account_name: String,
  platform: Option<String>,
  login_account: Option<String>,
  login_password: Option<String>,
  bind_phone: Option<String>,
  safe_link: Option<String>,
  backup_email: Option<String>,
  owner_id: Option<i64>,
  remark: Option<String>,
  tags: Option<String>,
) -> Result<i64, String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let mut params: Vec<String> = vec![
    "action=create_account".to_string(),
    format!("username={}", urlencode(&username)),
    format!("password={}", urlencode(&password)),
    format!("account_name={}", urlencode(&account_name)),
  ];

  if let Some(v) = phone_id {
    params.push(format!("phone_id={}", urlencode(&v)));
  }
  if let Some(v) = platform {
    params.push(format!("platform={}", urlencode(&v)));
  }
  if let Some(v) = login_account {
    params.push(format!("login_account={}", urlencode(&v)));
  }
  if let Some(v) = login_password {
    params.push(format!("login_password={}", urlencode(&v)));
  }
  if let Some(v) = bind_phone {
    params.push(format!("bind_phone={}", urlencode(&v)));
  }
  if let Some(v) = safe_link {
    params.push(format!("safe_link={}", urlencode(&v)));
  }
  if let Some(v) = backup_email {
    params.push(format!("backup_email={}", urlencode(&v)));
  }
  if let Some(v) = owner_id {
    params.push(format!("owner_id={}", v));
  }
  if let Some(v) = remark {
    params.push(format!("remark={}", urlencode(&v)));
  }
  if let Some(v) = tags {
    params.push(format!("tags={}", urlencode(&v)));
  }

  let form_data = params.join("&");

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("创建失败").to_string());
  }

  let id = result["account_id"].as_i64().unwrap_or(0);
  log_bwbrowser(
    "create_account",
    &format!("✓ 新账号已创建: id={}, name={}", id, account_name),
  );
  Ok(id)
}

// ========== 用户 → 账号同步 ==========

/// 用户账号同步结果
#[derive(Debug, Serialize)]
pub struct UserAccountSyncResult {
  pub total_users: usize,
  pub created: usize,
  pub already_existed: usize,
  pub failed: usize,
  pub message: String,
}

/// 同步团队用户到账号列表：
/// - 每个用户自动对应一个 TikTok 账号
/// - 账号昵称 = 姓名(用户名)
/// - 已存在的账号跳过（按 owner_id 匹配）
/// - Cookies 通过现有机制正常同步
impl BwbrowserAuthManager {
  pub async fn sync_users_to_accounts(&self) -> Result<UserAccountSyncResult, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    // 1. 获取用户列表
    let users_resp = self.list_cloud_users(None).await?;
    let users = users_resp.users.unwrap_or_default();
    if users.is_empty() {
      return Ok(UserAccountSyncResult {
        total_users: 0,
        created: 0,
        already_existed: 0,
        failed: 0,
        message: "用户列表为空".to_string(),
      });
    }

    // 2. 获取全部账号（第一页，page_size 尽量大）
    let accounts_resp = self
      .list_cloud_accounts(1, 500, None, None, None, None)
      .await?;
    let accounts = accounts_resp.accounts.unwrap_or_default();

    // 3. 按 owner_id 建立已有账号索引
    use std::collections::HashSet;
    let existing_owner_ids: HashSet<i64> = accounts.iter().filter_map(|a| a.owner_id).collect();

    let mut created = 0usize;
    let mut already_existed = 0usize;
    let mut failed = 0usize;

    // 4. 为每个用户检查并创建账号
    for user in &users {
      if existing_owner_ids.contains(&user.id) {
        already_existed += 1;
        continue;
      }

      // 构造账号名称：姓名(用户名)
      let real_name = user.real_name.clone().unwrap_or_default();
      let user_name = user.username.clone().unwrap_or_default();
      let account_name = if !real_name.is_empty() && !user_name.is_empty() {
        format!("{}({})", real_name, user_name)
      } else if !real_name.is_empty() {
        real_name
      } else {
        user_name
      };

      if account_name.is_empty() {
        failed += 1;
        continue;
      }

      // 创建账号
      let params: Vec<String> = vec![
        "action=create_account".to_string(),
        format!("username={}", urlencode(&username)),
        format!("password={}", urlencode(&password)),
        format!("account_name={}", urlencode(&account_name)),
        "platform=tiktok".to_string(),
        format!("owner_id={}", user.id),
        format!("nickname={}", urlencode(&account_name)),
      ];
      let form_data = params.join("&");

      match self
        .client
        .post(BWBROWSER_API_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_data)
        .send()
        .await
      {
        Ok(resp) => match resp.text().await {
          Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(json) => {
              if json["success"].as_bool().unwrap_or(false) {
                created += 1;
                log_bwbrowser(
                  "sync_users_to_accounts",
                  &format!("  ✓ 已创建账号: {} (owner_id={})", account_name, user.id),
                );
              } else {
                failed += 1;
                let msg = json["message"].as_str().unwrap_or("未知错误").to_string();
                log_bwbrowser(
                  "sync_users_to_accounts",
                  &format!("  ✗ 创建失败: {} - {}", account_name, msg),
                );
              }
            }
            Err(_) => {
              failed += 1;
            }
          },
          Err(_) => {
            failed += 1;
          }
        },
        Err(_) => {
          failed += 1;
        }
      }
    }

    let message = format!(
      "同步完成：共 {} 个用户，新建 {} 个账号，已存在 {} 个，失败 {} 个",
      users.len(),
      created,
      already_existed,
      failed
    );
    log_bwbrowser("sync_users_to_accounts", &message);

    Ok(UserAccountSyncResult {
      total_users: users.len(),
      created,
      already_existed,
      failed,
      message,
    })
  }
}

#[tauri::command]
pub async fn bwbrowser_sync_users_to_accounts() -> Result<UserAccountSyncResult, String> {
  BWBROWSER_AUTH.sync_users_to_accounts().await
}

// ========== Cookie 管理 ==========

/// 查看服务器端 Cookie
#[tauri::command]
pub async fn bwbrowser_get_account_cookies(account_id: i64) -> Result<serde_json::Value, String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let form_data = format!(
    "action=get_account_cookies&username={}&password={}&account_id={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("获取失败").to_string());
  }

  log_bwbrowser(
    "get_account_cookies",
    &format!("✓ 获取账号 {} 服务器 Cookie 成功", account_id),
  );
  Ok(result)
}

/// 删除服务器端 Cookie
#[tauri::command]
pub async fn bwbrowser_delete_cloud_cookies(account_id: i64) -> Result<(), String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

  let form_data = format!(
    "action=delete_account_cookies&username={}&password={}&account_id={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("请求失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("删除失败").to_string());
  }

  log_bwbrowser(
    "delete_cloud_cookies",
    &format!("✓ 账号 {} 服务器 Cookie 已删除", account_id),
  );
  Ok(())
}

/// 删除本地 Cookie（清除指定账号的所有 Cookie 和浏览数据）
#[tauri::command]
pub async fn bwbrowser_delete_local_cookies(
  account_name: String,
  account_id: Option<i64>,
) -> Result<(), String> {
  use crate::profile::manager::ProfileManager;

  let pm = ProfileManager::instance();
  let profiles_dir = pm.get_profiles_dir();
  let profiles = pm
    .list_profiles()
    .map_err(|e| format!("Failed to list profiles: {e}"))?;

  // 有 account_id 时按「账号名+id」的唯一派生名匹配，同名不同账号互不干扰；
  // 兼容旧调用：没有 id 时退回纯账号名匹配。
  let lookup_name = match account_id {
    Some(id) => account_profile_name(&account_name, id),
    None => account_name.clone(),
  };
  let matching: Vec<_> = profiles
    .into_iter()
    .filter(|p| p.name.to_lowercase() == lookup_name.to_lowercase())
    .collect();

  if matching.is_empty() {
    return Err(format!("未找到本地账号: {account_name}"));
  }

  let mut total_deleted = 0;
  for profile in &matching {
    match profile.browser.as_str() {
      "wayfern" => {
        let profile_data_path = profile.get_profile_data_path(&profiles_dir);
        let default_dir = profile_data_path.join("Default");

        log_bwbrowser(
          "delete_local_cookies",
          &format!(
            "  profile id={}, data_path={}",
            profile.id,
            profile_data_path.display()
          ),
        );

        if default_dir.exists() {
          match std::fs::remove_dir_all(&default_dir) {
            Ok(_) => {
              total_deleted += 1;
              log_bwbrowser(
                "delete_local_cookies",
                &format!("  ✓ 已删除 Default 目录 ({})", profile.id),
              );
            }
            Err(e) => {
              log_bwbrowser_error(
                "delete_local_cookies",
                &format!(
                  "  ⚠ 删除 Default 目录失败 ({}): {}，尝试逐项删除",
                  profile.id, e
                ),
              );
              // 逐项删除
              if default_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&default_dir) {
                  for entry in entries.flatten() {
                    let p = entry.path();
                    let _ = if p.is_dir() {
                      std::fs::remove_dir_all(&p)
                    } else {
                      std::fs::remove_file(&p)
                    };
                  }
                  total_deleted += 1;
                }
              }
            }
          }
        } else {
          log_bwbrowser(
            "delete_local_cookies",
            &format!("  Default 目录不存在 ({})", profile.id),
          );
        }
      }
      _ => {
        log_bwbrowser_error(
          "delete_local_cookies",
          &format!("不支持的浏览器类型 ({}): {}", profile.id, profile.browser),
        );
      }
    }
  }

  log_bwbrowser(
    "delete_local_cookies",
    &format!(
      "✓ 已删除本地 Cookie（账号: {}，{} 个 profile），清理了 {} 项数据",
      account_name,
      matching.len(),
      total_deleted
    ),
  );
  Ok(())
}

// ========== 用户权限 ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BwbrowserPermissions {
  pub user_id: i64,
  pub username: String,
  pub real_name: String,
  pub role: String,
  pub company_id: i64,
  pub is_super_admin: bool,
  pub is_manager: bool,
  pub allow_view_password: bool,
  pub allow_proxy_management: bool,
  pub allow_env_management: bool,
  pub allow_view_revenue: bool,
  pub allow_manage_users: bool,
  pub allow_cloud_accounts: bool,
  #[serde(default)]
  pub allow_2fa: bool,
  #[serde(default)]
  pub allow_sms_management: bool,
  pub module_permissions: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GetPermissionsResponse {
  success: bool,
  permissions: Option<BwbrowserPermissions>,
  message: Option<String>,
}

impl BwbrowserAuthManager {
  /// 获取当前用户权限
  pub async fn get_permissions(&self) -> Result<BwbrowserPermissions, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    log_bwbrowser(
      "get_permissions",
      &format!("→ 请求用户权限: user={}", username),
    );

    let form_data = format!(
      "action=get_permissions&username={}&password={}",
      urlencode(&username),
      urlencode(&password)
    );

    let resp = self
      .client
      .post(BWBROWSER_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("get_permissions", &format!("网络请求失败: {}", e));
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
      log_bwbrowser_error("get_permissions", &format!("读取响应失败: {}", e));
      format!("读取响应失败: {}", e)
    })?;

    log_bwbrowser(
      "get_permissions",
      &format!("← 响应状态: {}, 长度: {} bytes", status, body.len()),
    );
    log_bwbrowser("get_permissions", &format!("  响应内容: {}", body));

    let result: GetPermissionsResponse = parse_body("get_permissions", &body)?;

    if !result.success {
      let msg = result.message.unwrap_or_else(|| "获取权限失败".to_string());
      log_bwbrowser_error("get_permissions", &msg);
      return Err(msg);
    }

    let perms = result
      .permissions
      .ok_or_else(|| "服务器未返回权限信息".to_string())?;

    log_bwbrowser(
      "get_permissions",
      &format!(
        "✓ 获取成功: role={}, super_admin={}, manager={}",
        perms.role, perms.is_super_admin, perms.is_manager
      ),
    );

    Ok(perms)
  }
}

#[tauri::command]
pub async fn bwbrowser_get_permissions() -> Result<BwbrowserPermissions, String> {
  BWBROWSER_AUTH.get_permissions().await
}

// ========== 启动云端账号浏览器 ==========

/// 计算时区的 UTC 偏移量（分钟）
/// 浏览器 `Date.getTimezoneOffset()` 返回 UTC - local（分钟）
/// 例如 America/Los_Angeles PDT (UTC-7) → 返回 420
/// Europe/London BST (UTC+1) → 返回 -60
fn calc_timezone_offset_minutes(timezone: &str) -> i32 {
  use chrono::Utc;
  use chrono_tz::Tz;
  let tz: Option<Tz> = timezone.parse().ok();
  if let Some(tz) = tz {
    let now = Utc::now();
    let local_time = now.with_timezone(&tz);
    let utc_time = now.naive_utc();
    let local_naive = local_time.naive_local();
    let diff = utc_time - local_naive;
    diff.num_minutes() as i32
  } else {
    log::warn!("无法解析时区: {}，timezoneOffset 设为 0", timezone);
    0
  }
}

/// 将云端环境的 fingerprint_config 转换为 WayfernConfig
fn env_fingerprint_to_wayfern_config(
  fingerprint_config: &Option<serde_json::Value>,
) -> crate::wayfern_manager::WayfernConfig {
  use crate::wayfern_manager::WayfernConfig;

  let mut config = WayfernConfig::default();

  let fc = match fingerprint_config {
    Some(serde_json::Value::String(s)) => match serde_json::from_str::<serde_json::Value>(s) {
      Ok(v) => Some(v),
      Err(_) => {
        config.fingerprint = Some(s.clone());
        return config;
      }
    },
    Some(v) => Some(v.clone()),
    None => return config,
  };

  if let Some(fc) = fc {
    if let Some(identity_id) = fc.get("identity_id").and_then(|v| v.as_str()) {
      config.identity_id = Some(identity_id.to_string());
    }

    if let Some(location) = fc.get("location") {
      config.location = Some(serde_json::to_string(location).unwrap_or_default());
    }

    if let Some(os) = fc.get("os").and_then(|v| v.as_str()) {
      config.os = Some(os.to_string());
    }

    // fingerprint 子对象处理：
    // 云端 fingerprint_settings 的字段名（如 audioContext）和值格式（如 "random"）
    // 与 Wayfern.setFingerprint 原生格式不兼容，直接发送会导致 CDP 拒绝。
    // 因此有 fingerprint 但无 identity_id 的环境，暂不应用指纹（让 Wayfern 自
    // 行生成），只应用 location（时区/语言/经纬度）。
    // TODO: 建立云端 fingerprint_settings 到 Wayfern 原生字段的映射关系后再启用
    if fc.get("fingerprint").is_some() && config.identity_id.is_none() {
      log::debug!(
        "Cloud env has fingerprint_settings but no identity_id; skipping fingerprint apply (Wayfern will auto-generate)"
      );
    }

    // identity 模式优先：有 identity_id 时清除 fingerprint
    // （无 identity_id 时也不应用 fingerprint，见上方注释）
    if config.identity_id.is_some() {
      config.fingerprint = None;
    }
  }

  config
}

/// 根据主机系统创建默认 WayfernConfig
fn default_wayfern_config_for_host() -> crate::wayfern_manager::WayfernConfig {
  use crate::wayfern_manager::WayfernConfig;

  let mut config = WayfernConfig::default();
  #[cfg(target_os = "windows")]
  {
    config.os = Some("windows".to_string());
  }
  #[cfg(target_os = "macos")]
  {
    config.os = Some("macos".to_string());
  }
  #[cfg(target_os = "linux")]
  {
    config.os = Some("linux".to_string());
  }
  config
}

/// 将 WayfernConfig 转换为 fingerprint_config JSON 字符串（用于 sync_cloud_env）
fn wayfern_config_to_fingerprint_json(config: &crate::wayfern_manager::WayfernConfig) -> String {
  let mut obj = serde_json::json!({
    "os": config.os.as_deref().unwrap_or("windows"),
    "timezone": "ip",
    "language": "ip",
    "interfaceLanguage": "ip",
    "geolocation": "ip",
    "geolocationPrompt": "allow",
  });

  if let Some(ref id) = config.identity_id {
    obj["identity_id"] = serde_json::Value::String(id.clone());
  }

  if let Some(ref fp) = config.fingerprint {
    if let Ok(fp_val) = serde_json::from_str::<serde_json::Value>(fp) {
      obj["fingerprint"] = fp_val;
    }
  }

  if let Some(ref loc) = config.location {
    if let Ok(loc_val) = serde_json::from_str::<serde_json::Value>(loc) {
      obj["location"] = loc_val.clone();
    }
  }

  serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
}

/// Cloud-only: store proxy_node string directly as proxy_id with "node:" prefix.
/// No local proxy creation or lookup. The proxy_node is parsed at launch time
/// by cloud_proxy_manager::get_proxy_cloud_only.
async fn process_proxy_node(
  _app_handle: &tauri::AppHandle,
  proxy_node: Option<String>,
) -> Result<Option<String>, String> {
  let node = match proxy_node {
    Some(n) => n.trim().to_string(),
    None => return Ok(None),
  };
  if node.is_empty() {
    return Ok(None);
  }

  log_bwbrowser(
    "launch_account",
    &format!(
      "  process_proxy_node: using node: prefix, length={}",
      node.len()
    ),
  );
  Ok(Some(format!(
    "{}{}",
    crate::cloud_proxy_manager::NODE_PREFIX,
    node
  )))
}

#[allow(dead_code)]
async fn process_proxy_node_legacy(
  app_handle: &tauri::AppHandle,
  proxy_node: Option<String>,
) -> Result<Option<String>, String> {
  let node = match proxy_node {
    Some(n) => n.trim().to_string(),
    None => return Ok(None),
  };
  if node.is_empty() {
    return Ok(None);
  }

  // VLESS/Trojan: proxy_node 就是完整 URI（vless://... 或 trojan://...）
  if node.starts_with("vless://") || node.starts_with("trojan://") {
    let proxy_type = if node.starts_with("vless://") {
      "vless"
    } else {
      "trojan"
    };
    let vless_uri = node.clone();
    log_bwbrowser(
      "launch_account",
      &format!(
        "  process_proxy_node: {} URI 长度={}",
        proxy_type,
        vless_uri.len()
      ),
    );

    // 尝试从已有代理中匹配（按 URI 匹配）
    let all_proxies = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
    if let Some(existing) = all_proxies.iter().find(|p| {
      p.proxy_settings.proxy_type.eq_ignore_ascii_case(proxy_type)
        && p
          .proxy_settings
          .vless_uri
          .as_deref()
          .is_some_and(|uri| uri == vless_uri)
    }) {
      log_bwbrowser(
        "launch_account",
        &format!("  复用已有 {} 代理: id={}", proxy_type, existing.id),
      );
      return Ok(Some(existing.id.clone()));
    }

    // 新建 VLESS/Trojan 代理
    let proxy_name = format!("云_{}_{}", proxy_type, trunc(&vless_uri, 40));
    let settings = crate::browser::ProxySettings {
      proxy_type: proxy_type.to_string(),
      host: String::new(),
      port: 0,
      username: None,
      password: None,
      vless_uri: Some(vless_uri.clone()),
    };

    let stored = crate::proxy_manager::PROXY_MANAGER
      .create_stored_proxy(app_handle, proxy_name.clone(), settings)
      .map_err(|e| format!("创建 {} 代理失败: {}", proxy_type, e))?;

    log_bwbrowser(
      "launch_account",
      &format!(
        "  新建本地 {} 代理: id={}, name={}",
        proxy_type, stored.id, stored.name
      ),
    );

    // VLESS/Trojan 代理的时区在设置代理时检测，启动时不检测。
    let host = if vless_uri.starts_with("trojan://") {
      crate::xray::parse_trojan_uri(&vless_uri)
        .ok()
        .map(|p| p.config.address.clone())
    } else {
      crate::xray::parse_vless_uri(&vless_uri)
        .ok()
        .map(|p| p.config.address.clone())
    };
    if let Some(host) = host {
      log_bwbrowser(
        "launch_account",
        "  新建 VLESS/Trojan 代理跳过时区检测（仅在设置代理时检测）",
      );
      // 同步到云端（不带时区）
      let _ = BWBROWSER_AUTH
        .sync_cloud_proxy(
          None,
          &proxy_name,
          proxy_type,
          &host,
          0,
          None,
          None,
          Some(&vless_uri),
          None,
          None,
          None,
        )
        .await;
    }

    return Ok(Some(stored.id));
  }

  // HTTP/SOCKS5: 格式为 type:host:port:user:pass 或旧格式 host:port:user:pass
  let parts: Vec<&str> = node.split(':').collect();
  if parts.len() < 2 {
    return Ok(None);
  }

  const KNOWN_PROXY_TYPES: &[&str] = &["http", "socks5"];
  let (proxy_type, host_idx) = {
    let first = parts[0].to_lowercase();
    if KNOWN_PROXY_TYPES.contains(&first.as_str()) {
      (first, 1usize)
    } else {
      ("http".to_string(), 0usize)
    }
  };

  let remaining = &parts[host_idx..];
  if remaining.len() < 2 {
    return Ok(None);
  }

  let host = remaining[0].to_string();
  let port: u16 = remaining[1]
    .parse()
    .map_err(|_| format!("代理端口无效: {}", remaining[1]))?;
  let username = if remaining.len() >= 3 {
    Some(remaining[2].to_string())
  } else {
    None
  };
  let password = if remaining.len() >= 4 {
    Some(remaining[3..].join(":"))
  } else {
    None
  };

  let all_proxies = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
  if let Some(existing) = all_proxies
    .iter()
    .find(|p| p.proxy_settings.host == host && p.proxy_settings.port == port)
  {
    log_bwbrowser(
      "launch_account",
      &format!("  复用已有代理: id={}, {}:{}", existing.id, host, port),
    );
    // 启动时不检测时区，只读本地DB缓存。
    // 时区检测只在设置代理时（bwbrowser_update_account_proxy）做一次。
    if let Some(ref tz) = existing.geo_timezone {
      if !tz.is_empty() {
        log_bwbrowser(
          "launch_account",
          &format!("  ✓ 代理使用本地缓存时区: {}", tz),
        );
      } else {
        log_bwbrowser(
          "launch_account",
          "  ⚠ 代理本地无时区，请在账号设置中重新设置代理以检测时区",
        );
      }
    } else {
      log_bwbrowser(
        "launch_account",
        "  ⚠ 代理本地无时区，请在账号设置中重新设置代理以检测时区",
      );
    }
    return Ok(Some(existing.id.clone()));
  }

  let proxy_name = format!("云_{}:{}", host, port);
  let settings = crate::browser::ProxySettings {
    proxy_type: proxy_type.clone(),
    host: host.clone(),
    port,
    username: username.clone(),
    password: password.clone(),
    vless_uri: None,
  };

  let stored = crate::proxy_manager::PROXY_MANAGER
    .create_stored_proxy(app_handle, proxy_name.clone(), settings)
    .map_err(|e| format!("创建代理失败: {}", e))?;

  log_bwbrowser(
    "launch_account",
    &format!("  新建本地代理: id={}, name={}", stored.id, stored.name),
  );

  // 启动时不检测时区。时区在设置代理时（bwbrowser_update_account_proxy）检测。
  // 新建代理时留空，后续设置代理时会自动检测并写入。
  log_bwbrowser(
    "launch_account",
    "  新建代理跳过时区检测（仅在设置代理时检测）",
  );

  match BWBROWSER_AUTH
    .sync_cloud_proxy(
      None,
      &proxy_name,
      &proxy_type,
      &host,
      port as i64,
      username.as_deref(),
      password.as_deref(),
      None,
      None,
      None,
      None,
    )
    .await
  {
    Ok(cloud_id) => {
      log_bwbrowser(
        "launch_account",
        &format!("  ✓ 代理已同步到云端: cloud_id={}", cloud_id),
      );
    }
    Err(e) => {
      log_bwbrowser_error("launch_account", &format!("  同步到云端失败: {}", e));
    }
  }

  Ok(Some(stored.id))
}

/// 通过代理获取出口 IP 对应的完整地理位置信息
/// 包括 timezone、language、latitude、longitude
/// 用于写入 Wayfern identity document，使浏览器启动时就应用正确的指纹
#[derive(Clone)]
pub struct ProxyGeoInfo {
  pub timezone: String,
  pub language: String,
  pub latitude: Option<f64>,
  pub longitude: Option<f64>,
}

/// 通过多个在线 API 交叉验证 IP 的时区信息。
/// 查询 ipwho.is、ip-api.com、ip2location.io 三个源，
/// 多数投票确定时区。ipwho.is 和 ip-api.com 返回时区名，
/// ip2location.io 返回 offset，用坐标推时区名。
pub async fn resolve_timezone_online(ip: &str) -> Option<ProxyGeoInfo> {
  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(10))
    .build()
    .ok()?;

  // 并行查 3 个源
  let (r1, r2, r3) = tokio::join!(
    query_ipwho_is(&client, ip),
    query_ip_api_com(&client, ip),
    query_ip2location_io(&client, ip),
  );

  let mut sources: Vec<(String, String, Option<f64>, Option<f64>)> = Vec::new();
  // (source_name, timezone, lat, lon)

  if let Some(ref g) = r1 {
    sources.push((
      "ipwho.is".into(),
      g.timezone.clone(),
      g.latitude,
      g.longitude,
    ));
  }
  if let Some(ref g) = r2 {
    sources.push((
      "ip-api.com".into(),
      g.timezone.clone(),
      g.latitude,
      g.longitude,
    ));
  }
  if let Some(ref g) = r3 {
    sources.push((
      "ip2location.io".into(),
      g.timezone.clone(),
      g.latitude,
      g.longitude,
    ));
  }

  if sources.is_empty() {
    log::warn!(
      "resolve_timezone_online: all 3 API sources failed for ip={}",
      ip
    );
    return None;
  }

  // 打印各源结果
  for (name, tz, lat, lon) in &sources {
    log::info!("  [{}] tz={}, lat={:?}, lon={:?}", name, tz, lat, lon);
  }

  // 如果有 ip2location.io 的结果，检查它的坐标和另外两个源是否一致
  // ip2location.io 和 BrowserScan 用同一数据库，坐标更可靠
  // 当 ip2location.io 的坐标和 ipwho.is/ip-api.com 坐标差距 > 5度时，
  // 说明在线 API 坐标不准，以 ip2location.io 为准
  if let Some(ref ip2) = r3 {
    let ip2_lat = ip2.latitude.unwrap_or(0.0);
    let ip2_lon = ip2.longitude.unwrap_or(0.0);

    let mut coords_disagree = false;
    for (name, _, lat, lon) in &sources {
      if name == "ip2location.io" {
        continue;
      }
      if let (Some(la), Some(lo)) = (lat, lon) {
        let dist = ((la - ip2_lat).powi(2) + (lo - ip2_lon).powi(2)).sqrt();
        if dist > 5.0 {
          log::info!(
            "  ⚠ {} coords ({},{}) vs ip2location ({},{}) → distance={}, using ip2location",
            name,
            la,
            lo,
            ip2_lat,
            ip2_lon,
            dist
          );
          coords_disagree = true;
        }
      }
    }

    if coords_disagree {
      log::info!(
        "resolve_timezone_online: ip={} → timezone={} (ip2location.io override, coords disagree)",
        ip,
        ip2.timezone
      );
      return Some(ip2.clone());
    }
  }

  // 多数投票
  let mut vote: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
  for (_, tz, _, _) in &sources {
    *vote.entry(tz.clone()).or_default() += 1;
  }
  let winner = vote
    .iter()
    .max_by_key(|(_, n)| *n)
    .map(|(tz, _)| tz.clone());

  // 多数票
  if let Some(tz) = winner {
    let (win_lat, win_lon) = sources
      .iter()
      .find(|(_, t, _, _)| t == &tz)
      .map(|(_, _, lat, lon)| (*lat, *lon))
      .unwrap_or((None, None));
    log::info!(
      "resolve_timezone_online: ip={} → timezone={} (majority vote)",
      ip,
      tz
    );
    return Some(ProxyGeoInfo {
      timezone: tz,
      language: "en-US".to_string(),
      latitude: win_lat,
      longitude: win_lon,
    });
  }

  // 无多数票（三方各不同），用 ipwho.is 的结果
  log::warn!("resolve_timezone_online: no majority, using first source");
  r1.or(r2).or(r3)
}

/// 查 ipwho.is — 返回时区名
async fn query_ipwho_is(client: &reqwest::Client, ip: &str) -> Option<ProxyGeoInfo> {
  let resp = client
    .get(format!("https://ipwho.is/{}", ip))
    .send()
    .await
    .ok()?;
  if !resp.status().is_success() {
    return None;
  }
  let json: serde_json::Value = resp.json().await.ok()?;
  let timezone = json
    .get("timezone")
    .and_then(|t| t.get("id"))
    .and_then(|t| t.as_str())?;
  let country = json
    .get("country_code")
    .and_then(|c| c.as_str())
    .unwrap_or("US");
  let latitude = json.get("latitude").and_then(|v| v.as_f64());
  let longitude = json.get("longitude").and_then(|v| v.as_f64());
  let language = if country == "US" {
    "en-US".to_string()
  } else if country == "GB" {
    "en-GB".to_string()
  } else {
    "en".to_string()
  };
  Some(ProxyGeoInfo {
    timezone: timezone.to_string(),
    language,
    latitude,
    longitude,
  })
}

/// 查 ip-api.com — 返回时区名
async fn query_ip_api_com(client: &reqwest::Client, ip: &str) -> Option<ProxyGeoInfo> {
  let resp = client
    .get(format!("http://ip-api.com/json/{}", ip))
    .send()
    .await
    .ok()?;
  if !resp.status().is_success() {
    return None;
  }
  let json: serde_json::Value = resp.json().await.ok()?;
  let timezone = json.get("timezone").and_then(|t| t.as_str())?;
  let country = json
    .get("countryCode")
    .and_then(|c| c.as_str())
    .unwrap_or("US");
  let latitude = json.get("lat").and_then(|v| v.as_f64());
  let longitude = json.get("lon").and_then(|v| v.as_f64());
  let language = if country == "US" {
    "en-US".to_string()
  } else if country == "GB" {
    "en-GB".to_string()
  } else {
    "en".to_string()
  };
  Some(ProxyGeoInfo {
    timezone: timezone.to_string(),
    language,
    latitude,
    longitude,
  })
}

/// 查 ip2location.io — 返回 offset + 坐标，用坐标推时区名
async fn query_ip2location_io(client: &reqwest::Client, ip: &str) -> Option<ProxyGeoInfo> {
  let resp = client
    .get(format!("https://api.ip2location.io/?ip={}", ip))
    .send()
    .await
    .ok()?;
  if !resp.status().is_success() {
    return None;
  }
  let json: serde_json::Value = resp.json().await.ok()?;
  let latitude = json.get("latitude").and_then(|v| v.as_f64());
  let longitude = json.get("longitude").and_then(|v| v.as_f64());
  let country = json
    .get("country_code")
    .and_then(|c| c.as_str())
    .unwrap_or("US");

  // ip2location.io 返回 offset 如 "-04:00"，不是时区名
  // 用坐标推时区（比 offset 更准，offset 受 DST 影响）
  let timezone = offset_and_coords_to_timezone(
    json.get("time_zone").and_then(|t| t.as_str()),
    latitude,
    longitude,
    country,
  )?;
  let language = if country == "US" {
    "en-US".to_string()
  } else if country == "GB" {
    "en-GB".to_string()
  } else {
    "en".to_string()
  };
  Some(ProxyGeoInfo {
    timezone,
    language,
    latitude,
    longitude,
  })
}

/// 用 offset 和坐标推时区名。
/// offset 只能区分 UTC 偏移，坐标能区分东西海岸。
/// US 时区分界（粗略，基于经度）：
///   lon > -83.5  → America/New_York (Eastern)
///   -97 > lon >= -83.5 → America/Chicago (Central)
///   -115 > lon >= -97 → America/Denver (Mountain)
///   lon >= -115 → America/Los_Angeles (Pacific)
fn offset_and_coords_to_timezone(
  offset: Option<&str>,
  lat: Option<f64>,
  lon: Option<f64>,
  country: &str,
) -> Option<String> {
  // 非 US 用 offset 直接映射常见时区
  if country != "US" {
    if let Some(off) = offset {
      return Some(offset_to_timezone(off, country));
    }
    // 无 offset，用坐标兜底
    if let (Some(_), Some(lon_val)) = (lat, lon) {
      return Some(non_us_coords_to_timezone(lon_val, country));
    }
    return None;
  }

  // US：用经度推时区（更准，不受 DST 影响）
  if let Some(lon_val) = lon {
    let tz = if lon_val > -83.5 {
      "America/New_York"
    } else if lon_val > -97.0 {
      "America/Chicago"
    } else if lon_val > -115.0 {
      "America/Denver"
    } else {
      "America/Los_Angeles"
    };
    log::info!("  ip2location.io: lon={} → {}", lon_val, tz);
    return Some(tz.to_string());
  }

  // 无坐标，用 offset 推
  if let Some(off) = offset {
    return Some(offset_to_timezone(off, country));
  }

  None
}

/// 用 UTC offset 推时区（仅 US，粗略，仅坐标缺失时用）
fn offset_to_timezone(offset: &str, country: &str) -> String {
  if country == "US" {
    match offset {
      "-04:00" | "-05:00" => "America/New_York".to_string(),
      "-06:00" => "America/Chicago".to_string(),
      "-07:00" => "America/Denver".to_string(),
      "-08:00" => "America/Los_Angeles".to_string(),
      _ => "America/New_York".to_string(),
    }
  } else if country == "GB" {
    "Europe/London".to_string()
  } else if country == "JP" {
    "Asia/Tokyo".to_string()
  } else if country == "DE" || country == "FR" || country == "IT" || country == "ES" {
    "Europe/Berlin".to_string()
  } else {
    "America/New_York".to_string()
  }
}

/// 非 US 坐标推时区（非常粗略）
fn non_us_coords_to_timezone(lon: f64, country: &str) -> String {
  if country == "GB" {
    "Europe/London".to_string()
  } else if country == "JP" {
    "Asia/Tokyo".to_string()
  } else if lon > -10.0 && lon < 40.0 {
    "Europe/Berlin".to_string()
  } else if lon >= 40.0 {
    "Asia/Tokyo".to_string()
  } else {
    "America/New_York".to_string()
  }
}

/// 启动浏览器时读取代理时区。
/// 对于 node: 前缀的 proxy_id，直接解析 proxy_node 提取 host，从云端查找时区。
/// 对于本地 UUID，优先读本地DB；本地DB无时区时从云端同步。
/// 云端也无时区时，自动通过代理 IP 在线查询 geoip。
async fn resolve_geo_from_proxy(proxy_id: Option<&str>) -> Option<ProxyGeoInfo> {
  let proxy_id = proxy_id?;
  log_bwbrowser(
    "proxy_geo",
    &format!("  → 开始解析代理时区: proxy_id={}", proxy_id),
  );

  // node: 前缀 — 直接从 proxy_node 解析 host，查云端时区
  if let Some(node) = proxy_id.strip_prefix(crate::cloud_proxy_manager::NODE_PREFIX) {
    let settings = crate::cloud_proxy_manager::parse_proxy_node(node)?;
    let host = if !settings.host.is_empty() {
      settings.host.clone()
    } else if let Some(ref uri) = settings.vless_uri {
      // VLESS/Trojan: parse host from URI
      if uri.starts_with("trojan://") {
        crate::xray::parse_trojan_uri(uri)
          .ok()
          .map(|p| p.config.address.clone())
          .unwrap_or_default()
      } else {
        crate::xray::parse_vless_uri(uri)
          .ok()
          .map(|p| p.config.address.clone())
          .unwrap_or_default()
      }
    } else {
      String::new()
    };
    if host.is_empty() {
      return None;
    }
    log_bwbrowser("proxy_geo", &format!("  → node: 代理, host={}", host));
    if let Ok(cloud_proxies) = BWBROWSER_AUTH.list_cloud_proxies(None).await {
      for cp in &cloud_proxies {
        if cp.host == host {
          if let Some(ref tz) = cp.timezone {
            if !tz.is_empty() {
              log_bwbrowser(
                "proxy_geo",
                &format!("  ✓ 命中云端代理时区: host={}, timezone={}", host, tz),
              );
              return Some(ProxyGeoInfo {
                timezone: tz.clone(),
                language: "en-US".to_string(),
                latitude: None,
                longitude: None,
              });
            }
          }
        }
      }
    }
    // 云端无时区 → 在线 geoip 查询
    log_bwbrowser(
      "proxy_geo",
      &format!("  ⚠ 云端无时区，在线 geoip 查询: host={}", host),
    );
    if let Some(geo) = resolve_timezone_online(&host).await {
      log_bwbrowser(
        "proxy_geo",
        &format!(
          "  ✓ 在线 geoip 解析成功: host={}, timezone={}, language={}",
          host, geo.timezone, geo.language
        ),
      );
      return Some(geo);
    }
    log_bwbrowser_error(
      "proxy_geo",
      &format!("  ✗ 未找到代理时区（云端+在线均无）: host={}", host),
    );
    return None;
  }

  // Legacy local UUID
  let local_proxy = crate::proxy_manager::PROXY_MANAGER
    .get_stored_proxies()
    .into_iter()
    .find(|p| p.id == proxy_id);

  if let Some(stored) = local_proxy.as_ref() {
    if let Some(ref tz) = stored.geo_timezone {
      if !tz.is_empty() {
        log_bwbrowser(
          "proxy_geo",
          &format!(
            "  ✓ 命中本地代理时区: host={}, timezone={}",
            stored.proxy_settings.host, tz
          ),
        );
        return Some(ProxyGeoInfo {
          timezone: tz.clone(),
          language: "en-US".to_string(),
          latitude: None,
          longitude: None,
        });
      }
    }

    log_bwbrowser(
      "proxy_geo",
      &format!(
        "  ⚠ 本地无时区，查云端: host={}",
        stored.proxy_settings.host
      ),
    );
    if let Ok(cloud_proxies) = BWBROWSER_AUTH.list_cloud_proxies(None).await {
      for cp in &cloud_proxies {
        if cp.host == stored.proxy_settings.host {
          if let Some(ref tz) = cp.timezone {
            if !tz.is_empty() {
              log_bwbrowser(
                "proxy_geo",
                &format!(
                  "  ✓ 从云端取得时区: host={}, timezone={}",
                  stored.proxy_settings.host, tz
                ),
              );
              crate::proxy_manager::PROXY_MANAGER.update_proxy_geo(proxy_id, Some(tz.clone()));
              return Some(ProxyGeoInfo {
                timezone: tz.clone(),
                language: "en-US".to_string(),
                latitude: None,
                longitude: None,
              });
            }
          }
        }
      }
    }

    // 本地和云端都无时区 → 在线 geoip 查询
    let proxy_host = &stored.proxy_settings.host;
    log_bwbrowser(
      "proxy_geo",
      &format!(
        "  ⚠ 本地/云端均无时区，在线 geoip 查询: host={}",
        proxy_host
      ),
    );
    if let Some(geo) = resolve_timezone_online(proxy_host).await {
      log_bwbrowser(
        "proxy_geo",
        &format!(
          "  ✓ 在线 geoip 解析成功: host={}, timezone={}, language={}",
          proxy_host, geo.timezone, geo.language
        ),
      );
      crate::proxy_manager::PROXY_MANAGER.update_proxy_geo(proxy_id, Some(geo.timezone.clone()));
      return Some(geo);
    }
    log_bwbrowser_error(
      "proxy_geo",
      &format!(
        "  ✗ 未找到代理时区（本地/云端/在线均无）: host={}",
        proxy_host
      ),
    );
  } else {
    log_bwbrowser_error(
      "proxy_geo",
      &format!("  ✗ 本地代理库中找不到: proxy_id={}", proxy_id),
    );
  }

  None
}

/// 根据 env_uuid 从云端环境构建 WayfernConfig
/// 返回 (config, new_env_uuid)
/// 如果环境在服务器上找到，new_env_uuid 为 None
/// 如果创建了新环境，new_env_uuid 为新的 UUID
/// geo_info 用于创建新环境时写入时区/语言（从代理 IP 解析）
async fn build_wayfern_config(
  env_uuid: Option<&str>,
  account_name: Option<&str>,
  platform: Option<&str>,
  account_id: i64,
  geo_info: Option<&ProxyGeoInfo>,
) -> (
  Option<crate::wayfern_manager::WayfernConfig>,
  Option<String>,
) {
  let env_name = match (platform, account_name) {
    (Some(p), Some(n)) => format!("[{}] {} (cloud)", p, n),
    (Some(p), None) => format!("[{}] cloud_env", p),
    (None, Some(n)) => format!("{} (cloud)", n),
    (None, None) => "Cloud Environment".to_string(),
  };

  // 账号没有 env_uuid，自动创建新环境
  let uuid = match env_uuid {
    Some(u) if !u.is_empty() => u.to_string(),
    _ => {
      log_bwbrowser("launch_account", "  账号无环境，自动创建...");
      let mut config = default_wayfern_config_for_host();
      if let Some(geo) = geo_info {
        let lang = &geo.language;
        let langs_arr = if lang.contains('-') {
          vec![
            lang.clone(),
            lang.split('-').next().unwrap_or("en").to_string(),
          ]
        } else {
          vec![lang.clone()]
        };
        let tz_offset = calc_timezone_offset_minutes(&geo.timezone);
        let loc = serde_json::json!({
          "timezone": geo.timezone,
          "language": geo.language,
          "languages": langs_arr,
          "latitude": geo.latitude,
          "longitude": geo.longitude,
          "timezoneOffset": tz_offset,
          "accuracy": 100,
        });
        config.location = Some(serde_json::to_string(&loc).unwrap_or_default());
        log_bwbrowser(
          "launch_account",
          &format!(
            "  创建环境时写入时区: {}, language={}, languages={:?}, offset={}",
            geo.timezone, geo.language, langs_arr, tz_offset
          ),
        );
      } else {
        log_bwbrowser("launch_account", "  ⚠️ 无代理 geoip，环境将不带时区创建");
      }
      let fp_json = wayfern_config_to_fingerprint_json(&config);
      let new_uuid = uuid::Uuid::new_v4().to_string();
      match BWBROWSER_AUTH
        .sync_cloud_env(
          &new_uuid,
          &env_name,
          Some(&format!("Cloud account: {}", account_name.unwrap_or(""))),
          "chromium",
          Some(&fp_json),
          None,
          None,
          "ready",
        )
        .await
      {
        Ok(_) => {
          log_bwbrowser(
            "launch_account",
            &format!("  ✓ 新环境已创建: uuid={}", new_uuid),
          );
          match bwbrowser_update_account_env(account_id, Some(new_uuid.clone())).await {
            Ok(_) => log_bwbrowser("launch_account", "  ✓ 账号 env_uuid 已更新"),
            Err(e) => log_bwbrowser_error(
              "launch_account",
              &format!("  更新账号 env_uuid 失败: {}", e),
            ),
          }
          return (Some(config), Some(new_uuid));
        }
        Err(e) => {
          log_bwbrowser_error("launch_account", &format!("  创建新环境失败: {}", e));
          return (Some(config), None);
        }
      }
    }
  };

  match BWBROWSER_AUTH.list_cloud_envs(None).await {
    Ok(envs) => {
      if let Some(env) = envs.iter().find(|e| e.env_uuid == uuid) {
        log_bwbrowser(
          "launch_account",
          &format!("  找到云端环境: uuid={}, name={}", env.env_uuid, env.name),
        );
        let mut config = env_fingerprint_to_wayfern_config(&env.fingerprint_config);
        if config.os.is_none() {
          config.os = default_wayfern_config_for_host().os;
        }
        // 始终用代理 geoip 覆盖时区
        // 服务器环境中的 timezone 通常是 Wayfern 默认值（Europe/London），不是真实代理时区
        if let Some(geo) = geo_info {
          let mut loc: serde_json::Map<String, serde_json::Value> = config
            .location
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
          let old_tz = loc
            .get("timezone")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
          loc.insert("timezone".to_string(), serde_json::json!(geo.timezone));
          loc.insert("language".to_string(), serde_json::json!(geo.language));
          // languages 数组：[en-US, en] 防止中文泄露
          let lang = &geo.language;
          let langs_arr = if lang.contains('-') {
            vec![
              lang.clone(),
              lang.split('-').next().unwrap_or("en").to_string(),
            ]
          } else {
            vec![lang.clone()]
          };
          loc.insert("languages".to_string(), serde_json::json!(langs_arr));
          if let Some(lat) = geo.latitude {
            loc.insert("latitude".to_string(), serde_json::json!(lat));
          }
          if let Some(lon) = geo.longitude {
            loc.insert("longitude".to_string(), serde_json::json!(lon));
          }
          // 计算正确的 timezoneOffset（分钟）
          // 浏览器中 timezoneOffset 是 UTC 相对于本地的偏移（分钟），即 UTC - local
          // 例如 America/Los_Angeles PDT = UTC-7 → offset = +420
          let tz_offset = calc_timezone_offset_minutes(&geo.timezone);
          loc.insert("timezoneOffset".to_string(), serde_json::json!(tz_offset));
          config.location = Some(serde_json::to_string(&loc).unwrap_or_default());
          log_bwbrowser(
            "launch_account",
            &format!(
              "  地理信息覆盖: timezone={}, language={}, languages={:?}, lat={:?}, lon={:?}, offset={}",
              geo.timezone, geo.language, langs_arr, geo.latitude, geo.longitude, tz_offset
            ),
          );
          log_bwbrowser(
            "launch_account",
            &format!("  时区覆盖: {} → {}", old_tz, geo.timezone),
          );
          // 同步更新到服务器
          let fp_json = wayfern_config_to_fingerprint_json(&config);
          let _ = BWBROWSER_AUTH
            .sync_cloud_env(
              &uuid,
              &env_name,
              Some(&format!("Cloud account: {}", account_name.unwrap_or(""))),
              "chromium",
              Some(&fp_json),
              None,
              None,
              "ready",
            )
            .await;
          log_bwbrowser("launch_account", "  ✓ 时区已同步到云端环境");
        }
        (Some(config), None)
      } else {
        log_bwbrowser(
          "launch_account",
          &format!("  环境 uuid={} 不在当前用户列表中，尝试直接获取...", uuid),
        );
        match BWBROWSER_AUTH.get_cloud_env_by_uuid(&uuid).await {
          Ok(env) => {
            log_bwbrowser(
              "launch_account",
              &format!("  ✓ 通过UUID找到环境: name={}", env.name),
            );
            let mut config = env_fingerprint_to_wayfern_config(&env.fingerprint_config);
            if config.os.is_none() {
              config.os = default_wayfern_config_for_host().os;
            }
            if let Some(geo) = geo_info {
              let mut loc: serde_json::Map<String, serde_json::Value> = config
                .location
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
              loc.insert("timezone".to_string(), serde_json::json!(geo.timezone));
              loc.insert("language".to_string(), serde_json::json!(geo.language));
              let lang = &geo.language;
              let langs_arr = if lang.contains('-') {
                vec![
                  lang.clone(),
                  lang.split('-').next().unwrap_or("en").to_string(),
                ]
              } else {
                vec![lang.clone()]
              };
              loc.insert("languages".to_string(), serde_json::json!(langs_arr));
              if let Some(lat) = geo.latitude {
                loc.insert("latitude".to_string(), serde_json::json!(lat));
              }
              if let Some(lon) = geo.longitude {
                loc.insert("longitude".to_string(), serde_json::json!(lon));
              }
              let tz_offset = calc_timezone_offset_minutes(&geo.timezone);
              loc.insert("timezoneOffset".to_string(), serde_json::json!(tz_offset));
              config.location = Some(serde_json::to_string(&loc).unwrap_or_default());
              log_bwbrowser(
                "launch_account",
                &format!("  地理信息覆盖: timezone={}", geo.timezone),
              );
            }
            return (Some(config), None);
          }
          Err(e) => {
            log_bwbrowser(
              "launch_account",
              &format!("  直接获取环境失败: {}，创建新环境", e),
            );
          }
        }
        let mut config = default_wayfern_config_for_host();
        if let Some(geo) = geo_info {
          let lang = &geo.language;
          let langs_arr = if lang.contains('-') {
            vec![
              lang.clone(),
              lang.split('-').next().unwrap_or("en").to_string(),
            ]
          } else {
            vec![lang.clone()]
          };
          let tz_offset = calc_timezone_offset_minutes(&geo.timezone);
          let loc = serde_json::json!({
            "timezone": geo.timezone,
            "language": geo.language,
            "languages": langs_arr,
            "latitude": geo.latitude,
            "longitude": geo.longitude,
            "timezoneOffset": tz_offset,
            "accuracy": 100,
          });
          config.location = Some(serde_json::to_string(&loc).unwrap_or_default());
          log_bwbrowser(
            "launch_account",
            &format!(
              "  创建环境时写入: timezone={}, language={}, languages={:?}, offset={}",
              geo.timezone, geo.language, langs_arr, tz_offset
            ),
          );
        }
        let fp_json = wayfern_config_to_fingerprint_json(&config);

        // 先用原 UUID 尝试创建（环境真的不存在的情况）
        log_bwbrowser("launch_account", "  尝试用原 UUID 创建环境...");
        let sync_result = BWBROWSER_AUTH
          .sync_cloud_env(
            &uuid,
            &env_name,
            Some(&format!("Cloud account: {}", account_name.unwrap_or(""))),
            "chromium",
            Some(&fp_json),
            None,
            None,
            "ready",
          )
          .await;

        match sync_result {
          Ok(_) => {
            log_bwbrowser(
              "launch_account",
              &format!("  ✓ 环境已创建（使用原 UUID）: {}", uuid),
            );
            (Some(config), None)
          }
          Err(e) => {
            // 原 UUID 创建失败（可能环境已存在但属于别人），生成新 UUID 创建
            log_bwbrowser(
              "launch_account",
              &format!("  原 UUID 创建失败: {}，生成新环境并更新账号绑定", e),
            );
            let new_uuid = uuid::Uuid::new_v4().to_string();
            match BWBROWSER_AUTH
              .sync_cloud_env(
                &new_uuid,
                &env_name,
                Some(&format!("Cloud account: {}", account_name.unwrap_or(""))),
                "chromium",
                Some(&fp_json),
                None,
                None,
                "ready",
              )
              .await
            {
              Ok(_) => {
                log_bwbrowser(
                  "launch_account",
                  &format!("  ✓ 新环境已创建: uuid={}", new_uuid),
                );
                match bwbrowser_update_account_env(account_id, Some(new_uuid.clone())).await {
                  Ok(_) => log_bwbrowser("launch_account", "  ✓ 账号 env_uuid 已更新"),
                  Err(e) => log_bwbrowser_error(
                    "launch_account",
                    &format!("  更新账号 env_uuid 失败: {}", e),
                  ),
                }
                (Some(config), Some(new_uuid))
              }
              Err(e) => {
                log_bwbrowser_error("launch_account", &format!("  创建新环境失败: {}", e));
                (Some(config), None)
              }
            }
          }
        }
      }
    }
    Err(e) => {
      log_bwbrowser_error(
        "launch_account",
        &format!("  获取云端环境失败: {}，使用默认配置", e),
      );
      (Some(default_wayfern_config_for_host()), None)
    }
  }
}

/// 由「账号名 + 数据库唯一 id」派生本地 profile 名。
/// 不同账号即使账号名相同（例如不同平台登记同一手机号）也各自独立，
/// 不会命中同一个浏览器环境。账号名为空时退回纯 id 派生名。
fn account_profile_name(account_name: &str, account_id: i64) -> String {
  let name = account_name.trim();
  if name.is_empty() {
    format!("account_{}", account_id)
  } else {
    format!("{}_{}", name, account_id)
  }
}

/// 启动云端账号浏览器：自动查找/创建 profile、设置代理、绑定环境
#[tauri::command]
pub async fn bwbrowser_launch_account(
  app_handle: tauri::AppHandle,
  account_id: i64,
  account_name: String,
  env_uuid: Option<String>,
  proxy_node: Option<String>,
  platform: Option<String>,
) -> Result<String, String> {
  // 整体启动超时兜底：防止某个联网/启动步骤永久挂起，导致进度卡在 95%
  tokio::time::timeout(
    std::time::Duration::from_secs(70),
    launch_account_impl(
      app_handle,
      account_id,
      account_name,
      env_uuid,
      proxy_node,
      platform,
    ),
  )
  .await
  .map_err(|_| "账号启动超时：启动流程超过 70 秒未完成，请重试".to_string())
  .and_then(|r| r)
}

async fn launch_account_impl(
  app_handle: tauri::AppHandle,
  account_id: i64,
  account_name: String,
  env_uuid: Option<String>,
  proxy_node: Option<String>,
  platform: Option<String>,
) -> Result<String, String> {
  log_bwbrowser(
    "launch_account",
    &format!(
      "→ 启动账号: id={}, name={}, env_uuid={:?}, proxy={:?}",
      account_id, account_name, env_uuid, proxy_node
    ),
  );

  // 真实启动进度：按步骤向后端前端广播百分比，替代前端的模拟动画
  let emit_app = app_handle.clone();
  let emit_progress = |pct: u32, label: &str| {
    let _ = emit_app.emit(
      "account-launch-progress",
      serde_json::json!({
        "account_id": account_id,
        "pct": pct,
        "label": label,
      }),
    );
  };
  emit_progress(2, "正在拉取账号信息...");

  // 0. 通过 account_id 从服务器获取最新的账号详情（确保 env_uuid 是最新的）
  let (server_env_uuid, server_account_name, server_platform, server_proxy_node) =
    match BWBROWSER_AUTH.get_account_detail(account_id).await {
      Ok(detail) => {
        let env = detail.env_uuid.clone();
        let name = if detail.account_name.is_empty() {
          account_name.clone()
        } else {
          detail.account_name.clone()
        };
        let plat = detail.platform.clone().or(platform.clone());
        let proxy = detail
          .proxy_node
          .clone()
          .filter(|s| !s.is_empty())
          .or(proxy_node.clone());
        log_bwbrowser(
          "launch_account",
          &format!("  ✓ 从服务器获取最新账号详情: env_uuid={:?}", env),
        );
        (env, name, plat, proxy)
      }
      Err(e) => {
        log_bwbrowser_error(
          "launch_account",
          &format!("  获取账号详情失败，使用传入参数: {}", e),
        );
        (
          env_uuid,
          account_name.clone(),
          platform.clone(),
          proxy_node.clone(),
        )
      }
    };

  // 0.5 优先使用本地 profile 已有的 proxy_id（由 bwbrowser_update_account_proxy 设置）
  emit_progress(8, "已获取账号信息"); //     云端 API 可能因服务器同步延迟返回旧值，导致覆盖本地正确配置。
                                      //     如果本地 profile 有 node: 前缀的 proxy_id，直接用它，不从云端取。
  let server_proxy_node = {
    let pm = crate::profile::manager::ProfileManager::instance();
    let profiles = pm.list_profiles().unwrap_or_default();
    let lookup_name = account_profile_name(&server_account_name, account_id);
    let existing = profiles
      .into_iter()
      .find(|p| p.name.to_lowercase() == lookup_name.to_lowercase());
    if let Some(ref profile) = existing {
      if let Some(ref pid) = profile.proxy_id {
        if let Some(node) = pid.strip_prefix(crate::cloud_proxy_manager::NODE_PREFIX) {
          log_bwbrowser(
            "launch_account",
            &format!(
              "  使用本地 profile 的 proxy_id (node: 前缀, {} 字符), 不使用云端值 {:?}",
              node.len(),
              server_proxy_node
            ),
          );
          Some(node.to_string())
        } else {
          log_bwbrowser(
            "launch_account",
            &format!(
              "  本地 profile 有 proxy_id={:?} 但非 node: 前缀, 使用云端值",
              pid
            ),
          );
          server_proxy_node.clone()
        }
      } else {
        log_bwbrowser("launch_account", "  本地 profile 无 proxy_id, 使用云端值");
        server_proxy_node.clone()
      }
    } else {
      log_bwbrowser("launch_account", "  本地无已有 profile, 使用云端值");
      server_proxy_node.clone()
    }
  };

  // 1. 处理代理
  emit_progress(15, "正在配置代理...");
  log_bwbrowser(
    "launch_account",
    &format!("  effective_proxy_node={:?}", server_proxy_node),
  );
  let has_proxy_configured = server_proxy_node
    .as_ref()
    .is_some_and(|s| !s.trim().is_empty());
  let local_proxy_id = match process_proxy_node(&app_handle, server_proxy_node.clone()).await {
    Ok(Some(id)) => {
      log_bwbrowser("launch_account", &format!("  代理: proxy_id={}", id));
      let settings = if let Some(node) = id.strip_prefix(crate::cloud_proxy_manager::NODE_PREFIX) {
        crate::cloud_proxy_manager::parse_proxy_node(node)
      } else {
        crate::proxy_manager::PROXY_MANAGER.get_proxy_settings_by_id(&id)
      };
      if let Some(settings) = settings {
        log_bwbrowser(
          "launch_account",
          &format!(
            "  正在验证代理可用性: {}:{}...",
            settings.host, settings.port
          ),
        );
        match crate::proxy_manager::PROXY_MANAGER
          .check_proxy_validity(&id, &settings)
          .await
        {
          Ok(result) if result.is_valid => {
            log_bwbrowser(
              "launch_account",
              &format!(
                "  ✓ 代理可用: IP={}, country={}",
                result.ip,
                result.country.as_deref().unwrap_or("")
              ),
            );
          }
          Ok(_) => {
            log_bwbrowser_error("launch_account", "  ✗ 代理不可用，拒绝启动浏览器");
            return Err(serde_json::json!({ "code": "PROXY_NOT_WORKING" }).to_string());
          }
          Err(e) => {
            let err_str = e.to_string();
            log_bwbrowser_error(
              "launch_account",
              &format!("  ✗ 代理验证失败: {}，拒绝启动浏览器", err_str),
            );
            if err_str.contains("402") {
              return Err(serde_json::json!({ "code": "PROXY_PAYMENT_REQUIRED" }).to_string());
            }
            return Err(serde_json::json!({ "code": "PROXY_NOT_WORKING" }).to_string());
          }
        }
      }
      Some(id)
    }
    Ok(None) => {
      if has_proxy_configured {
        log_bwbrowser_error(
          "launch_account",
          "  ✗ 代理配置无效（解析为空），拒绝启动浏览器",
        );
        return Err(serde_json::json!({ "code": "PROXY_NOT_WORKING" }).to_string());
      }
      log_bwbrowser("launch_account", "  无代理");
      None
    }
    Err(e) => {
      log_bwbrowser_error(
        "launch_account",
        &format!("  ✗ 代理处理失败: {}，拒绝启动浏览器", e),
      );
      return Err(serde_json::json!({ "code": "PROXY_NOT_WORKING" }).to_string());
    }
  };

  // 1.5 提前解析代理 geoip（创建环境时需要时区）
  emit_progress(28, "解析代理地理位置...");
  let geo_info = resolve_geo_from_proxy(local_proxy_id.as_deref()).await;
  if let Some(ref geo) = geo_info {
    log_bwbrowser(
      "launch_account",
      &format!(
        "  ✓ 代理 geoip: timezone={}, language={}",
        geo.timezone, geo.language
      ),
    );
  } else {
    log_bwbrowser("launch_account", "  ⚠️ 无法获取代理 geoip");
  }

  // 2. 构建 WayfernConfig（有环境用环境，无环境自动创建并写入时区）
  emit_progress(42, "构建指纹配置...");
  let (wayfern_config, new_env_uuid) = build_wayfern_config(
    server_env_uuid.as_deref(),
    Some(&server_account_name),
    server_platform.as_deref(),
    account_id,
    geo_info.as_ref(),
  )
  .await;
  if let Some(ref c) = wayfern_config {
    log_bwbrowser(
      "launch_account",
      &format!(
        "  WayfernConfig: identity_id={:?}, os={:?}, has_fingerprint={}",
        c.identity_id.as_deref(),
        c.os.as_deref(),
        c.fingerprint.is_some()
      ),
    );
  }

  // 2.1 始终用代理 geoip 覆盖 wayfern_config 的 location
  //     服务器环境的 timezone 通常是 Wayfern 默认值（Europe/London），不是真实代理时区
  let wayfern_config = match wayfern_config {
    Some(mut wc) => {
      if let Some(geo) = geo_info.as_ref() {
        let old_tz = wc
          .location
          .as_ref()
          .and_then(|loc_str| {
            serde_json::from_str::<serde_json::Value>(loc_str)
              .ok()
              .and_then(|v| {
                v.get("timezone")
                  .and_then(|t| t.as_str())
                  .map(|s| s.to_string())
              })
          })
          .unwrap_or_default();
        let mut loc: serde_json::Map<String, serde_json::Value> = wc
          .location
          .as_ref()
          .and_then(|s| serde_json::from_str(s).ok())
          .unwrap_or_default();
        loc.insert("timezone".to_string(), serde_json::json!(geo.timezone));
        loc.insert("language".to_string(), serde_json::json!(geo.language));
        // languages 数组：[en-US, en] 防止中文泄露
        let lang = &geo.language;
        let langs_arr = if lang.contains('-') {
          vec![
            lang.clone(),
            lang.split('-').next().unwrap_or("en").to_string(),
          ]
        } else {
          vec![lang.clone()]
        };
        loc.insert("languages".to_string(), serde_json::json!(langs_arr));
        if let Some(lat) = geo.latitude {
          loc.insert("latitude".to_string(), serde_json::json!(lat));
        }
        if let Some(lon) = geo.longitude {
          loc.insert("longitude".to_string(), serde_json::json!(lon));
        }
        // 计算正确的 timezoneOffset（分钟）
        let tz_offset = calc_timezone_offset_minutes(&geo.timezone);
        loc.insert("timezoneOffset".to_string(), serde_json::json!(tz_offset));
        loc.insert("accuracy".to_string(), serde_json::json!(100));
        wc.location = Some(serde_json::to_string(&loc).unwrap_or_default());
        log_bwbrowser(
          "launch_account",
          &format!(
            "  时区覆盖 (step 2.1): {} → {}, languages={:?}, offset={}",
            old_tz, geo.timezone, langs_arr, tz_offset
          ),
        );
      } else {
        let current_tz = wc.location.as_ref().and_then(|loc_str| {
          serde_json::from_str::<serde_json::Value>(loc_str)
            .ok()
            .and_then(|v| {
              v.get("timezone")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
            })
        });
        log_bwbrowser(
          "launch_account",
          &format!("  配置时区（无 geoip）: {}", current_tz.unwrap_or_default()),
        );
      }
      Some(wc)
    }
    None => None,
  };
  let has_fingerprint_config = wayfern_config
    .as_ref()
    .is_some_and(|c| c.fingerprint.is_some() || c.identity_id.is_some());

  // Wayfern 浏览器且 location 里有时区 → Wayfern 内核会处理时区（identity/setFingerprint/默认指纹+location）
  // 不需要 CDP 表层注入（CDP 注入会被 browserscan 等检测）
  let _has_wayfern_location_tz = wayfern_config.as_ref().is_some_and(|c| {
    c.location
      .as_deref()
      .and_then(|loc_str| {
        serde_json::from_str::<serde_json::Value>(loc_str)
          .ok()
          .and_then(|v| {
            v.get("timezone")
              .and_then(|t| t.as_str())
              .map(|s| s.to_string())
          })
      })
      .is_some_and(|s| !s.is_empty())
  });

  // 记录启动前的 identity_id，用于判断是否新生成了指纹
  let identity_id_before = wayfern_config.as_ref().and_then(|c| c.identity_id.clone());

  // 提前提取 timezone（后面 wayfern_config 会被 move，所以这里先取出来）
  let timezone_for_cdp: Option<String> = {
    let from_config = wayfern_config.as_ref().and_then(|wc| {
      wc.location.as_ref().and_then(|loc_str| {
        serde_json::from_str::<serde_json::Value>(loc_str)
          .ok()
          .and_then(|v| {
            v.get("timezone")
              .and_then(|t| t.as_str())
              .map(|s| s.to_string())
          })
      })
    });

    if from_config.as_ref().is_none_or(|tz| tz.is_empty()) {
      geo_info.as_ref().map(|geo| geo.timezone.clone())
    } else {
      from_config
    }
  };

  // 3. 查找已有 profile（用账号名作为 profile 名称）
  emit_progress(58, "准备本地配置...");
  // 用「账号名 + account_id」派生唯一 profile 名：同名但不同账号（如不同平台同一
  // 手机号）各自拥有独立浏览器，不再共用同一个本地环境。
  let profile_name = account_profile_name(&server_account_name, account_id);
  log_bwbrowser(
    "launch_account",
    &format!("  profile 名称（含账号 id）: {}", profile_name),
  );
  let existing = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("获取本地 profile 列表失败: {}", e))?;

  // 优先按派生名精确匹配。
  let existing_profile = existing
    .iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase())
    .cloned();

  // 兜底：旧版本以纯账号名命名。若该账号全局唯一，则原地重命名为派生名以保留
  // 已有浏览器指纹与登录态；若同名冲突（正是要修复的场景）则不迁移，走新建独立环境。
  let existing_profile = match existing_profile {
    Some(p) => Some(p),
    None => {
      let legacy: Vec<_> = existing
        .iter()
        .filter(|p| p.name.to_lowercase() == server_account_name.to_lowercase())
        .collect();
      if legacy.len() == 1 {
        match crate::profile::manager::ProfileManager::instance().rename_profile(
          &app_handle,
          &legacy[0].id.to_string(),
          &profile_name,
        ) {
          Ok(renamed) => {
            log_bwbrowser(
              "launch_account",
              &format!(
                "  迁移旧 profile: 重命名为 {}（保留浏览器环境）",
                profile_name
              ),
            );
            Some(renamed)
          }
          Err(e) => {
            log_bwbrowser(
              "launch_account",
              &format!("  旧 profile 重命名失败，走新建独立环境: {}", e),
            );
            None
          }
        }
      } else {
        None
      }
    }
  };

  let profile = match existing_profile {
    Some(mut p) => {
      log_bwbrowser(
        "launch_account",
        &format!("  复用已有 profile: id={}", p.id),
      );
      // 如果代理变了，更新 profile 的代理
      let current_proxy = p.proxy_id.as_deref();
      log_bwbrowser(
        "launch_account",
        &format!(
          "  代理对比: local_proxy_id={:?} vs profile.proxy_id={:?}",
          local_proxy_id, current_proxy
        ),
      );
      let need_update = match (&local_proxy_id, current_proxy) {
        (Some(new_id), Some(old_id)) => {
          let same = new_id == old_id;
          log_bwbrowser("launch_account", &format!("  代理相同={}", same));
          !same
        }
        (Some(_), None) => {
          log_bwbrowser("launch_account", "  需更新: 本地有代理, profile 无");
          true
        }
        (None, Some(_)) => {
          log_bwbrowser("launch_account", "  需更新: 本地无代理, profile 有");
          true
        }
        (None, None) => false,
      };
      if need_update {
        log_bwbrowser(
          "launch_account",
          &format!("  更新代理: {:?} -> {:?}", current_proxy, local_proxy_id),
        );
        match crate::profile::manager::ProfileManager::instance()
          .update_profile_proxy(
            app_handle.clone(),
            &p.id.to_string(),
            local_proxy_id.clone(),
          )
          .await
        {
          Ok(updated) => {
            p.proxy_id = updated.proxy_id.clone();
            log_bwbrowser("launch_account", "  代理已更新");
          }
          Err(e) => {
            log_bwbrowser_error("launch_account", &format!("  更新代理失败: {}", e));
          }
        }
      }

      // 始终用最新的 wayfern_config（包含 geoip 时区覆盖）更新 profile
      if let Some(ref wc) = wayfern_config {
        let mut config_to_apply = wc.clone();

        // 保护已有指纹：爆文库云端环境的 fingerprint_config 格式与 Wayfern 原生不兼容，
        // env_fingerprint_to_wayfern_config 不会生成 fingerprint 或生成空 "{}"。
        // 如果直接覆盖，会把本地已有的指纹清空，导致每次启动都随机生成新指纹。
        // 规则：新配置没有有效 fingerprint/identity，但本地有 → 保留旧 fingerprint
        let new_fp_is_empty = config_to_apply
          .fingerprint
          .as_deref()
          .map(|f| f.is_empty() || f == "{}")
          .unwrap_or(true);
        if new_fp_is_empty && config_to_apply.identity_id.is_none() {
          if let Some(ref stored) = p.wayfern_config {
            if let Some(ref stored_fp) = stored.fingerprint {
              if !stored_fp.is_empty() && stored_fp != "{}" {
                config_to_apply.fingerprint = Some(stored_fp.clone());
                // Preserve randomize_fingerprint_on_launch from stored config
                // so browser_runner.rs doesn't trigger migrating_payload
                config_to_apply.randomize_fingerprint_on_launch =
                  stored.randomize_fingerprint_on_launch.or(Some(false));
                log_bwbrowser(
                  "launch_account",
                  &format!(
                    "  保留本地已有指纹（{} chars），避免覆盖丢失",
                    stored_fp.len()
                  ),
                );
              }
            }
          }
        }

        log_bwbrowser(
          "launch_account",
          &format!(
            "  [FP-DEBUG] before update_wayfern_config: fingerprint={} chars, identity_id={:?}",
            config_to_apply
              .fingerprint
              .as_deref()
              .map(|f| f.len().to_string())
              .unwrap_or("None".to_string()),
            config_to_apply.identity_id
          ),
        );

        match crate::profile::manager::ProfileManager::instance()
          .update_wayfern_config(
            app_handle.clone(),
            &p.id.to_string(),
            config_to_apply.clone(),
          )
          .await
        {
          Ok(_) => {
            p.wayfern_config = Some(config_to_apply);
            log_bwbrowser(
              "launch_account",
              "  ✓ 地理位置已写入 profile（timezone 生效）",
            );
          }
          Err(e) => log_bwbrowser_error("launch_account", &format!("  写入地理位置失败: {}", e)),
        }
      }

      // update_wayfern_config 内部调用 list_profiles() + save_profile()，
      // 可能用旧 proxy_id 覆盖 update_profile_proxy 刚写入的新值。
      // 直接修改 metadata.json 确保 proxy_id 正确。
      if let Some(ref new_pid) = local_proxy_id {
        let pm = crate::profile::manager::ProfileManager::instance();
        let profiles_dir = pm.get_profiles_dir();
        let metadata_path = profiles_dir.join(p.id.to_string()).join("metadata.json");
        if let Ok(content) = std::fs::read_to_string(&metadata_path) {
          if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) {
            let current_pid = json
              .get("proxy_id")
              .and_then(|v| v.as_str())
              .map(|s| s.to_string());
            if current_pid.as_deref() != Some(new_pid.as_str()) {
              log_bwbrowser(
                "launch_account",
                &format!(
                  "  ⚠ update_wayfern_config 覆盖了 proxy_id: {:?} → 直接修复为 {:?}",
                  current_pid, new_pid
                ),
              );
              json["proxy_id"] = serde_json::Value::String(new_pid.clone());
              json["vpn_id"] = serde_json::Value::Null;
              json["updated_at"] = serde_json::Value::Number(serde_json::Number::from(
                crate::proxy_manager::now_secs(),
              ));
              if let Ok(new_json) = serde_json::to_string_pretty(&json) {
                if let Err(e) = std::fs::write(&metadata_path, new_json.as_bytes()) {
                  log_bwbrowser_error(
                    "launch_account",
                    &format!("  ⚠ 直接修复 proxy_id 写入失败: {}", e),
                  );
                } else {
                  log_bwbrowser("launch_account", "  ✓ proxy_id 已直接修复");
                }
              }
            }
          }
        }
        p.proxy_id = Some(new_pid.clone());
      }

      p
    }
    None => {
      // 解析已安装的 Wayfern 版本（REST API 会自动解析，Tauri 命令不会）
      let registry = crate::downloaded_browsers_registry::DownloadedBrowsersRegistry::instance();
      let mut versions = registry.get_downloaded_versions("wayfern");
      versions.sort_by(|a, b| crate::api_client::compare_versions(b, a));
      let version = if let Some(v) = versions.into_iter().next() {
        v
      } else {
        // Wayfern 未安装，自动获取最新版本并下载
        log_bwbrowser("launch_account", "  Wayfern 内核未安装，自动下载中...");
        let api_client = crate::api_client::ApiClient::new();
        let version_info = api_client
          .fetch_wayfern_version_with_caching(true)
          .await
          .map_err(|e| format!("获取 Wayfern 版本信息失败: {}", e))?;
        let latest_version = version_info.version.clone();
        log_bwbrowser(
          "launch_account",
          &format!("  最新 Wayfern 版本: {}，开始下载...", latest_version),
        );
        crate::downloader::Downloader::instance()
          .download_browser_full(&app_handle, "wayfern".to_string(), latest_version.clone())
          .await
          .map_err(|e| format!("Wayfern 内核下载失败: {}", e))?;
        log_bwbrowser("launch_account", "  ✓ Wayfern 内核下载完成");
        latest_version
      };
      log_bwbrowser(
        "launch_account",
        &format!("  使用 Wayfern 版本: {}", version),
      );

      let pm = crate::profile::manager::ProfileManager::instance();
      pm.create_profile_with_group(
        &app_handle,
        &profile_name,
        "wayfern",
        &version,
        "stable",
        local_proxy_id.clone(),
        None,
        wayfern_config,
        None,
        false,
        None,
        None,
      )
      .await
      .map_err(|e| format!("创建 profile 失败: {}", e))?
    }
  };

  let profile_id_str = profile.id.to_string();
  let app_handle_clone = app_handle.clone();

  // 3.5 检查本地是否有 Cookie 文件（判断是否全新 profile）
  emit_progress(66, "检查登录状态..."); //     - 全新 profile（无 Cookie 文件）：可以安全注入云端 Cookie
                                        //     - 有 Cookie 文件：用 CDP 导出做对比（避免 SQLite 解密失败的问题）
  let is_fresh_profile = {
    use crate::cookie_manager::CookieManager;
    match CookieManager::export_cookies(&profile_id_str, "json") {
      Ok(json) => {
        log_bwbrowser(
          "launch_account",
          &format!(
            "  本地有 Cookie 文件: {} 字节（启动后用 CDP 导出做对比）",
            json.len()
          ),
        );
        false
      }
      Err(e) => {
        let is_not_found = e.contains("not found")
          || e.contains("No such file")
          || e.contains("找不到")
          || e.contains("cannot find the file");
        if is_not_found {
          log_bwbrowser("launch_account", "  全新 profile，无本地 Cookie 文件");
          true
        } else {
          log_bwbrowser(
            "launch_account",
            &format!(
              "  本地 Cookie 文件存在但读取失败（用 CDP 导出做对比）: {}",
              e
            ),
          );
          false
        }
      }
    }
  };

  // 4. 启动浏览器（使用 Advisory gate，不阻止指纹不匹配的启动）
  emit_progress(75, "启动浏览器内核..."); //    云端账号的指纹在启动时通过代理出口 IP 自动解析时区/语言
                                          //    提前获取平台 URL，作为启动 URL 直接传入（比 CDP 导航更可靠）
  let launch_url = if let Some(ref p) = server_platform {
    fetch_platform_url(p).await
  } else {
    None
  };
  if let Some(ref url) = launch_url {
    log_bwbrowser(
      "launch_account",
      &format!(
        "  启动 URL: {} -> {}",
        server_platform.as_deref().unwrap_or(""),
        url
      ),
    );
  }

  let launch_url_for_cookie = launch_url.clone();
  let launched_profile = {
    let options = crate::browser_runner::LaunchOptions {
      gate: crate::launch_gate::FingerprintGate::Advisory,
      ..Default::default()
    };
    crate::browser_runner::launch_browser_profile_impl(app_handle, profile, launch_url, options)
      .await
      .map_err(|e| format!("启动浏览器失败: {}", e))?
  };

  // browser_runner 的 save_process_info 会覆盖 proxy_id，
  // 启动后直接修复 metadata.json
  if let Some(ref new_pid) = local_proxy_id {
    let pm = crate::profile::manager::ProfileManager::instance();
    let profiles_dir = pm.get_profiles_dir();
    let metadata_path = profiles_dir
      .join(launched_profile.id.to_string())
      .join("metadata.json");
    if let Ok(content) = std::fs::read_to_string(&metadata_path) {
      if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) {
        let current_pid = json
          .get("proxy_id")
          .and_then(|v| v.as_str())
          .map(|s| s.to_string());
        if current_pid.as_deref() != Some(new_pid.as_str()) {
          log_bwbrowser(
            "launch_account",
            &format!(
              "  ⚠ 启动后 proxy_id 被覆盖: {:?} → 直接修复为 {:?}",
              current_pid, new_pid
            ),
          );
          json["proxy_id"] = serde_json::Value::String(new_pid.clone());
          json["vpn_id"] = serde_json::Value::Null;
          if let Ok(new_json) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&metadata_path, new_json.as_bytes());
            log_bwbrowser("launch_account", "  ✓ 启动后 proxy_id 已修复");
          }
        }
      }
    }
  }

  // 5. Wayfern 内核级时区设置：getFingerprint → 改时区 → setFingerprint
  //    这比 CDP Emulation 更可靠，因为是 Wayfern 内核自己设置的，所有 tab 都生效。
  //    Emulation.setTimezoneOverride 只对当前 tab 生效，新 tab 不继承。
  let skip_cdp_tz = has_fingerprint_config;
  if !skip_cdp_tz {
    let process_alive = launched_profile.process_id.is_none_or(|pid| {
      use sysinfo::{ProcessRefreshKind, RefreshKind, System};
      let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
      );
      let alive = system.process(sysinfo::Pid::from_u32(pid)).is_some();
      if !alive {
        log_bwbrowser_error(
          "launch_account",
          &format!("  [Wayfern-TZ] 浏览器进程 {} 已退出，跳过时区设置", pid),
        );
      }
      alive
    });

    if process_alive {
      let tz_for_inject = if timezone_for_cdp.as_ref().is_none_or(|tz| tz.is_empty()) {
        geo_info.as_ref().map(|geo| geo.timezone.clone())
      } else {
        timezone_for_cdp.clone()
      };

      if let Some(ref tz) = tz_for_inject {
        if !tz.is_empty() {
          // 第一步：用 Wayfern 自己的 CDP 命令设置时区 + 语言
          //   getFingerprint → 修改 timezone + language → setFingerprint
          //   这样时区和语言都是 Wayfern 内核级设置，所有 tab 都生效
          let lang = geo_info.as_ref().map(|geo| geo.language.as_str());
          let lat = geo_info.as_ref().and_then(|geo| geo.latitude);
          let lng = geo_info.as_ref().and_then(|geo| geo.longitude);
          log_bwbrowser(
            "launch_account",
            &format!(
              "  [Wayfern-TZ] 开始设置内核时区: {} 语言: {:?}...",
              tz, lang
            ),
          );

          let wayfern_tz_set = match crate::wayfern_manager::WayfernManager::instance()
            .set_wayfern_timezone(&launched_profile, tz, lang, lat, lng)
            .await
          {
            Ok(true) => {
              log_bwbrowser(
                "launch_account",
                &format!("  [Wayfern-TZ] ✓ Wayfern 内核时区已设置: {}", tz),
              );
              true
            }
            Ok(false) => {
              log_bwbrowser(
                "launch_account",
                "  [Wayfern-TZ] Wayfern 不支持时区设置，回退到 CDP 注入",
              );
              false
            }
            Err(e) => {
              log_bwbrowser_error(
                "launch_account",
                &format!("  [Wayfern-TZ] ✗ Wayfern 时区设置失败: {}", e),
              );
              false
            }
          };

          // 第二步：如果 Wayfern 内核级设置失败，回退到 CDP 注入
          if !wayfern_tz_set {
            log_bwbrowser(
              "launch_account",
              "  [CDP-TZ] 开始 CDP 时区注入（fallback）...",
            );
            match crate::cookie_sync::inject_timezone_via_cdp(&launched_profile, tz).await {
              Ok(_) => {
                log_bwbrowser(
                  "launch_account",
                  &format!("  [CDP-TZ] ✓ CDP 时区注入成功: {}", tz),
                );
              }
              Err(e) => {
                log_bwbrowser_error(
                  "launch_account",
                  &format!("  [CDP-TZ] ✗ CDP 时区注入失败: {}", e),
                );
              }
            }
          }
        }
      }
    } else {
      log_bwbrowser(
        "launch_account",
        "  [CDP-TZ] 跳过：Wayfern 内核级指纹已生效（identity/setFingerprint），无需 <script> 注入",
      );
    }
  }

  // 5.1 启动后注入云端 Cookie（后台异步执行，不阻塞启动）
  //     以前是同步等待，CDP 重试最多 20 秒会导致启动感觉很慢。
  //     现在放到 tokio 后台任务，浏览器先返回启动成功，cookie 在后台注入。
  let app_handle_spawn = app_handle_clone.clone();
  let profile_spawn = launched_profile.clone();
  let server_platform_spawn = server_platform.clone();
  let launch_url_spawn = launch_url_for_cookie.clone();
  tokio::spawn(async move {
    if let Err(e) = inject_cloud_cookies_after_launch(
      &app_handle_spawn,
      account_id,
      &profile_id_str,
      &profile_spawn,
      is_fresh_profile,
      server_platform_spawn.as_deref(),
      launch_url_spawn.as_deref(),
    )
    .await
    {
      log_bwbrowser_error("launch_account", &format!("  Cookie 注入失败: {}", e));
    }
  });

  // 5.5 验证指纹是否成功注入（通过 CDP 读取实际指纹信息）
  if has_fingerprint_config {
    match verify_fingerprint_via_cdp(&launched_profile).await {
      Ok(info) => {
        log_bwbrowser(
          "launch_account",
          &format!(
            "  ✓ 指纹验证成功: ua={}, timezone={}, language={}, platform={}",
            info.user_agent.chars().take(50).collect::<String>(),
            info.timezone,
            info.language,
            info.platform
          ),
        );
      }
      Err(e) => log_bwbrowser_error("launch_account", &format!("  指纹验证失败: {}", e)),
    }
  }

  // 5.8 如果新生成了 identity_id，同步回云端环境（确保下次启动使用相同指纹）
  let identity_id_after = launched_profile
    .wayfern_config
    .as_ref()
    .and_then(|c| c.identity_id.clone());
  if identity_id_before.is_none() {
    if let Some(new_identity_id) = identity_id_after.as_ref() {
      log_bwbrowser(
        "launch_account",
        &format!("  新生成 identity_id={}，同步到云端环境", new_identity_id),
      );
      let env_uuid_for_sync = new_env_uuid
        .as_deref()
        .or_else(|| server_env_uuid.as_deref().filter(|s| !s.is_empty()));
      if let Some(env_uuid) = env_uuid_for_sync {
        // 从 launched_profile 构建新的 fingerprint_config
        let fp_json = launched_profile
          .wayfern_config
          .as_ref()
          .map(wayfern_config_to_fingerprint_json)
          .unwrap_or_default();
        let env_name = format!(
          "[{}] {}",
          server_platform.as_deref().unwrap_or(""),
          server_account_name
        );
        match BWBROWSER_AUTH
          .sync_cloud_env(
            env_uuid,
            &env_name,
            Some(&format!("Cloud account: {}", server_account_name)),
            "chromium",
            Some(&fp_json),
            None,
            None,
            "ready",
          )
          .await
        {
          Ok(_) => log_bwbrowser("launch_account", "  ✓ 指纹已同步到云端环境"),
          Err(e) => log_bwbrowser_error(
            "launch_account",
            &format!("  指纹同步到云端环境失败: {}", e),
          ),
        }
      }
    }
  }

  // 6. Cookie 回传在 inject_cloud_cookies_after_launch 中完成（启动后立即通过 CDP 导出回传）
  //    浏览器关闭时不再回传，避免 SQLite 锁定问题和重复上传

  // 启动完成前校验浏览器进程仍存活：若已退出，视为启动失败并提示
  {
    let alive = launched_profile.process_id.is_none_or(|pid| {
      use sysinfo::{ProcessRefreshKind, RefreshKind, System};
      let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
      );
      system.process(sysinfo::Pid::from_u32(pid)).is_some()
    });
    if !alive {
      log_bwbrowser_error("launch_account", "  浏览器进程已退出，启动失败");
      let _ = emit_app.emit(
        "account-launch-failed",
        serde_json::json!({ "account_id": account_id, "reason": "浏览器启动后进程未保持运行" }),
      );
      return Err("浏览器启动后进程未保持运行，未能正常打开".to_string());
    }
  }

  emit_progress(100, "浏览器已启动");
  log_bwbrowser("launch_account", "✓ 启动成功");
  Ok("ok".to_string())
}

/// 从平台配置 API 获取指定平台的创作者主页 URL
async fn fetch_platform_url(platform: &str) -> Option<String> {
  log_bwbrowser(
    "platform_url",
    &format!(
      "请求平台配置: platform={}, url={}",
      platform, PLATFORM_CONFIG_API_URL
    ),
  );

  // GET 请求获取全量平台列表，然后本地匹配
  let resp = match BWBROWSER_AUTH
    .client
    .get(PLATFORM_CONFIG_API_URL)
    .send()
    .await
  {
    Ok(r) => r,
    Err(e) => {
      log_bwbrowser_error("platform_url", &format!("请求失败: {}", e));
      return None;
    }
  };

  let body = match resp.text().await {
    Ok(b) => b,
    Err(e) => {
      log_bwbrowser_error("platform_url", &format!("读取响应失败: {}", e));
      return None;
    }
  };

  // 调试：打印原始响应（截断前 500 字符）
  let debug_body = if body.len() > 500 {
    format!("{}(共 {} 字节)", trunc(&body, 500), body.len())
  } else {
    body.clone()
  };
  log_bwbrowser("platform_url", &format!("原始响应: {}", debug_body));

  let result: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));

  // 尝试从 data.platforms 获取（API 包装格式）
  let platforms_value = result
    .get("data")
    .and_then(|d| d.get("platforms"))
    .or_else(|| result.get("platforms"));

  if let Some(platforms) = platforms_value {
    // 支持两种格式：对象（key 是平台 id）或数组（每项有 id/platform 字段）
    if let Some(map) = platforms.as_object() {
      // 格式 1: platforms 是对象 { "youtube": { creator_url: "..." }, ... }
      let platform_keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
      log_bwbrowser(
        "platform_url",
        &format!(
          "API 返回平台列表 (对象格式, {}个): [{}]",
          platform_keys.len(),
          platform_keys.join(", ")
        ),
      );

      let platform_lower = platform.to_lowercase();
      for (key, value) in map {
        if key.to_lowercase() == platform_lower {
          if let Some(url) = value.get("creator_url").and_then(|u| u.as_str()) {
            if !url.is_empty() {
              log_bwbrowser(
                "platform_url",
                &format!("✓ API 返回: {} -> {}", platform, url),
              );
              return Some(url.to_string());
            }
          }
        }
      }
    } else if let Some(arr) = platforms.as_array() {
      // 格式 2: platforms 是数组 [{ id/platform: "youtube", creator_url: "..." }, ...]
      // 先看看第一个元素有哪些字段（调试用）
      if let Some(first) = arr.first() {
        let keys: Vec<&str> = first
          .as_object()
          .map(|o| o.keys().map(|k| k.as_str()).collect())
          .unwrap_or_default();
        log_bwbrowser(
          "platform_url",
          &format!(
            "API 返回平台列表 (数组格式, {}个), 首项字段: [{}]",
            arr.len(),
            keys.join(", ")
          ),
        );
      }

      let platform_lower = platform.to_lowercase();
      for item in arr {
        // 尝试 id、platform、name 字段匹配
        let matched = ["id", "platform", "name"]
          .iter()
          .filter_map(|field| item.get(field).and_then(|v| v.as_str()))
          .any(|val| val.to_lowercase() == platform_lower);

        if matched {
          if let Some(url) = item.get("creator_url").and_then(|u| u.as_str()) {
            if !url.is_empty() {
              log_bwbrowser(
                "platform_url",
                &format!("✓ API 返回: {} -> {}", platform, url),
              );
              return Some(url.to_string());
            }
          }
        }
      }
    }
  }

  log_bwbrowser(
    "platform_url",
    &format!("未找到平台 {} 的 creator_url", platform),
  );
  None
}

/// 指纹验证结果
#[derive(Debug, Clone)]
struct FingerprintInfo {
  user_agent: String,
  timezone: String,
  language: String,
  platform: String,
}

/// 通过 CDP 读取浏览器实际指纹信息，验证指纹注入是否成功
async fn verify_fingerprint_via_cdp(
  profile: &crate::profile::BrowserProfile,
) -> Result<FingerprintInfo, String> {
  let target = crate::cdp_target::resolve(profile)
    .await
    .map_err(|e| format!("CDP 连接失败: {}", e))?;

  let eval_js = r#"
    JSON.stringify({
      userAgent: navigator.userAgent,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      language: navigator.language,
      platform: navigator.platform
    })
  "#;

  let result = crate::cdp_target::run_command(
    &target,
    "Runtime.evaluate",
    serde_json::json!({
      "expression": eval_js,
      "returnByValue": true,
    }),
  )
  .await
  .map_err(|e| format!("CDP 执行失败: {}", e))?;

  // 检查异常
  if result.get("exceptionDetails").is_some() {
    return Err("CDP 脚本执行异常".to_string());
  }

  let value_str = result
    .get("result")
    .and_then(|r| r.get("value"))
    .and_then(|v| v.as_str())
    .ok_or_else(|| "无法获取指纹信息".to_string())?;

  let info: serde_json::Value =
    serde_json::from_str(value_str).map_err(|e| format!("解析指纹 JSON 失败: {}", e))?;

  Ok(FingerprintInfo {
    user_agent: info
      .get("userAgent")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
    timezone: info
      .get("timezone")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
    language: info
      .get("language")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
    platform: info
      .get("platform")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string(),
  })
}

/// 启动后注入云端 Cookie（后台异步执行）
///
/// 流程：
/// 1. 从云端拉取 Cookie
/// 2. 等待 CDP 端口就绪
/// 3. 通过 CDP 导出本地 Cookie 并评分
/// 4. 对比云端与本地评分，决定是否注入
/// 5. 注入后立即通过 CDP 导出回传云端
async fn inject_cloud_cookies_after_launch(
  app_handle: &tauri::AppHandle,
  account_id: i64,
  _profile_id_str: &str,
  profile: &crate::profile::BrowserProfile,
  is_fresh_profile: bool,
  platform: Option<&str>,
  launch_url: Option<&str>,
) -> Result<(), String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录".to_string())?;

  let default_platform = platform.unwrap_or("toutiao").to_string();

  // 1. 从云端拉取 Cookie
  let form_data = format!(
    "action=get_account_cookies&username={}&password={}&account_id={}&platform={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
    urlencode(&default_platform),
  );

  log_bwbrowser(
    "inject_cookies",
    &format!(
      "请求云端 Cookie: account_id={}, platform={}, url={}",
      account_id, default_platform, BWBROWSER_API_URL
    ),
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("获取云端 Cookie 失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;

  log_bwbrowser(
    "inject_cookies",
    &format!("API 响应长度: {} 字节", body.len()),
  );
  if !body.is_empty() {
    let preview = if body.len() > 200 {
      format!("{}", trunc(&body, 200))
    } else {
      body.clone()
    };
    log_bwbrowser("inject_cookies", &format!("响应前 200 字符: {}", preview));
  }

  let result: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));

  let success = result["success"].as_bool().unwrap_or(false);
  let cloud_platform = result
    .get("platform")
    .and_then(|v| v.as_str())
    .unwrap_or(&default_platform)
    .to_string();

  log_bwbrowser("inject_cookies", &format!("平台字段: {}", cloud_platform));

  // 2. 解析云端 Cookie
  let cloud_cookie_text = if success {
    result["cookie"].as_str().map(|s| s.to_string())
  } else {
    None
  };

  let has_cloud_cookies = cloud_cookie_text
    .as_ref()
    .map(|s| !s.is_empty() && s != "[]")
    .unwrap_or(false);

  // 调试：列出云端 Cookie 的所有 domain
  if has_cloud_cookies {
    if let Ok(cookies_val) =
      serde_json::from_str::<serde_json::Value>(cloud_cookie_text.as_deref().unwrap_or(""))
    {
      if let Some(arr) = cookies_val.as_array() {
        let mut domains: Vec<String> = arr
          .iter()
          .filter_map(|c| {
            c.get("domain")
              .and_then(|v| v.as_str())
              .map(|s| s.to_string())
          })
          .collect();
        domains.sort();
        domains.dedup();
        log_bwbrowser(
          "inject_cookies",
          &format!(
            "云端 Cookie 共 {} 条，涉及 domain: [{}]",
            arr.len(),
            domains.join(", ")
          ),
        );
      }
    }
  }

  if !has_cloud_cookies {
    log_bwbrowser("inject_cookies", "无云端 Cookie，将尝试导出本地并上传");
  }

  // 计算云端 Cookie 评分
  let cloud_score = if has_cloud_cookies {
    let cloud_val =
      serde_json::from_str::<serde_json::Value>(cloud_cookie_text.as_deref().unwrap_or(""))
        .unwrap_or(serde_json::Value::Array(vec![]));
    let cloud_cookies =
      crate::cookie_sync::prepare_cookies_for_injection(&cloud_val, &cloud_platform);
    let score = crate::cookie_sync::score_cookies(&cloud_cookies, &cloud_platform);
    log_bwbrowser(
      "inject_cookies",
      &format!(
        "云端 Cookie 评分: {} 分 (可用={}, 确定登录={})",
        score.total, score.usable, score.definite_logged_in
      ),
    );
    Some(score)
  } else {
    None
  };

  // 3. 等待 CDP 端口就绪（最多 20 秒）
  log_bwbrowser("inject_cookies", "等待 CDP 端口就绪（最多 20 秒）...");
  let mut cdp_ready = false;
  for i in 0..20 {
    match crate::cdp_target::resolve(profile).await {
      Ok(_) => {
        cdp_ready = true;
        log_bwbrowser("inject_cookies", &format!("CDP 端口就绪（等待 {} 秒）", i));
        break;
      }
      Err(_) => {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
      }
    }
  }
  if !cdp_ready {
    return Err("CDP 端口 20 秒内未就绪".to_string());
  }

  // 4. 通过 CDP 导出本地 Cookie 并评分
  let local_cookie_json = match crate::cookie_sync::export_cookies_via_cdp(profile).await {
    Ok(s) => s,
    Err(e) => {
      log_bwbrowser_error(
        "inject_cookies",
        &format!("CDP 导出本地 Cookie 失败: {}", e),
      );
      return Err(format!("CDP 导出本地 Cookie 失败: {}", e));
    }
  };

  log_bwbrowser(
    "inject_cookies",
    &format!("CDP 导出本地 Cookie 成功: {} 字节", local_cookie_json.len()),
  );

  let local_val = serde_json::from_str::<serde_json::Value>(&local_cookie_json)
    .unwrap_or(serde_json::Value::Array(vec![]));
  let local_cookies =
    crate::cookie_sync::prepare_cookies_for_injection(&local_val, &cloud_platform);
  let local_score = crate::cookie_sync::score_cookies(&local_cookies, &cloud_platform);

  // 调试：列出本地 Cookie 的所有 domain
  if let Some(arr) = local_val.as_array() {
    let mut domains: Vec<String> = arr
      .iter()
      .filter_map(|c| {
        c.get("domain")
          .and_then(|v| v.as_str())
          .map(|s| s.to_string())
      })
      .collect();
    domains.sort();
    domains.dedup();
    log_bwbrowser(
      "inject_cookies",
      &format!(
        "本地 CDP Cookie 共 {} 条，涉及 domain: [{}]",
        arr.len(),
        domains.join(", ")
      ),
    );
  }

  log_bwbrowser(
    "inject_cookies",
    &format!(
      "本地 Cookie 评分 (CDP): {} 分 (可用={}, 确定登录={})",
      local_score.total, local_score.usable, local_score.definite_logged_in
    ),
  );

  // 5. 决定是否注入云端 Cookie（以本地为主）
  //    - 首次创建 profile（is_fresh_profile）：服务器有就注入
  //    - 本地 cookie 分数低于服务器 20 分以上：注入服务器的
  //    - 否则：不注入，保留本地
  let has_cloud = cloud_cookie_text
    .as_deref()
    .map(|s| !s.is_empty() && s != "[]")
    .unwrap_or(false);

  let should_inject = if is_fresh_profile && has_cloud {
    log_bwbrowser("inject_cookies", "首次创建 profile，注入云端 Cookie");
    true
  } else if !has_cloud {
    log_bwbrowser("inject_cookies", "无云端 Cookie，保留本地");
    false
  } else {
    // 都有 cookie，比较分数
    let cloud_total = cloud_score.as_ref().map(|s| s.total).unwrap_or(0);
    let diff = cloud_total - local_score.total;
    if diff > 20 {
      log_bwbrowser(
        "inject_cookies",
        &format!(
          "云端 Cookie 分数比本地高 {} 分 ({} vs {})，注入云端",
          diff, cloud_total, local_score.total
        ),
      );
      true
    } else {
      log_bwbrowser(
        "inject_cookies",
        &format!(
          "本地 Cookie 分数足够 (本地={} 云端={} 差={}≤20)，保留本地",
          local_score.total, cloud_total, diff
        ),
      );
      false
    }
  };

  if should_inject {
    if let Some(ref cloud_text) = cloud_cookie_text {
      let cloud_val = serde_json::from_str::<serde_json::Value>(cloud_text)
        .unwrap_or(serde_json::Value::Array(vec![]));
      let cdp_cookies =
        crate::cookie_sync::prepare_cookies_for_injection(&cloud_val, &cloud_platform);

      if !cdp_cookies.is_empty() {
        match crate::cookie_sync::inject_cookies_via_cdp(profile, &cdp_cookies, launch_url).await {
          Ok(count) => {
            log_bwbrowser(
              "inject_cookies",
              &format!("✓ 云端 Cookie 注入成功: {} 个", count),
            );
          }
          Err(e) => {
            log_bwbrowser_error("inject_cookies", &format!("✗ 云端 Cookie 注入失败: {}", e));
          }
        }
      }
    }
  } else if let Some(url) = launch_url {
    log_bwbrowser("inject_cookies", &format!("保留本地，导航到 {}", url));
    if let Err(e) = crate::cookie_sync::navigate_to_url(profile, url).await {
      log_bwbrowser_error("inject_cookies", &format!("导航失败: {}", e));
    }
  }

  // 7. 通过页面 DOM 检测真实登录状态（替代 cookie 判断）
  let mut page_logged_in = false;
  if let Some(url) = launch_url {
    if let Some(lc_config) = crate::cookie_sync::fetch_login_check_config(&default_platform).await {
      log_bwbrowser(
        "inject_cookies",
        &format!(
          "开始页面登录检测: platform={}, url={}",
          default_platform, url
        ),
      );
      match crate::cookie_sync::check_login_via_page(profile, url, &lc_config).await {
        Ok(logged_in) => {
          page_logged_in = logged_in;
          if logged_in {
            log_bwbrowser("inject_cookies", "✓ 页面登录检测: 已登录");
          } else {
            log_bwbrowser(
              "inject_cookies",
              "✗ 页面登录检测: 未登录，可能 Cookie 无效或已过期",
            );
          }
        }
        Err(e) => {
          log_bwbrowser_error("inject_cookies", &format!("页面登录检测失败: {}", e));
        }
      }
    } else {
      log_bwbrowser(
        "inject_cookies",
        &format!(
          "平台 {} 无 login_check 配置，跳过页面检测",
          default_platform
        ),
      );
    }
  }

  // 8. 只有页面检测确认登录，才回传 Cookie 到云端
  if !page_logged_in {
    log_bwbrowser("inject_cookies", "页面检测未登录，跳过回传云端");
    return Ok(());
  }

  log_bwbrowser("inject_cookies", "页面检测已登录，回传 Cookie 到云端...");
  let final_cookie_json = match crate::cookie_sync::export_cookies_via_cdp(profile).await {
    Ok(s) => s,
    Err(e) => {
      log_bwbrowser_error(
        "inject_cookies",
        &format!("CDP 导出回传 Cookie 失败: {}", e),
      );
      return Err(format!("CDP 导出回传 Cookie 失败: {}", e));
    }
  };

  log_bwbrowser(
    "inject_cookies",
    &format!("CDP 导出 Cookie: {} 字节", final_cookie_json.len()),
  );

  match sync_cookies_to_cloud(app_handle, account_id, &final_cookie_json, &cloud_platform).await {
    Ok((uploaded, local_total, cloud_total)) => {
      if uploaded {
        log_bwbrowser("inject_cookies", "✓ Cookie 已立即回传云端");
      } else {
        log_bwbrowser(
          "inject_cookies",
          &format!(
            "Cookie 未回传 (本地={}分, 云端={}分)",
            local_total,
            cloud_total.unwrap_or(0)
          ),
        );
      }
    }
    Err(e) => {
      log_bwbrowser_error("inject_cookies", &format!("Cookie 回传失败: {}", e));
    }
  }

  Ok(())
}

/// 等待进程退出（轮询方式）
async fn wait_for_process_exit(pid: u32) {
  loop {
    // 检查进程是否还在运行
    #[cfg(target_os = "windows")]
    {
      use std::process::Command;
      let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH"])
        .output();
      if let Ok(out) = output {
        let s = String::from_utf8_lossy(&out.stdout);
        if !s.contains(&pid.to_string()) {
          break;
        }
      }
    }
    #[cfg(not(target_os = "windows"))]
    {
      // Unix 系统：检查 /proc/{pid} 是否存在
      if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
        break;
      }
    }
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
  }
}

/// 对比本地 Cookie 与云端 Cookie 评分，本地更高则上传到云端
/// 返回 (是否上传, 本地评分, 云端评分)
async fn sync_cookies_to_cloud(
  _app_handle: &tauri::AppHandle,
  account_id: i64,
  cookie_json: &str,
  platform: &str,
) -> Result<(bool, i32, Option<i32>), String> {
  if cookie_json.trim().is_empty() || cookie_json == "[]" {
    log_bwbrowser("cookie_sync", "  无本地 Cookie 数据，跳过上传");
    return Ok((false, 0, None));
  }

  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录".to_string())?;

  // 先获取云端 Cookie 用于评分对比
  let check_form = format!(
    "action=get_account_cookies&username={}&password={}&account_id={}",
    urlencode(&username),
    urlencode(&password),
    account_id
  );

  let check_resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(check_form)
    .send()
    .await
    .map_err(|e| format!("获取云端 Cookie 失败: {}", e))?;

  let check_body = check_resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;
  let check_result: serde_json::Value =
    serde_json::from_str(&check_body).unwrap_or(serde_json::json!({}));

  let cloud_platform = check_result
    .get("platform")
    .and_then(|v| v.as_str())
    .unwrap_or(platform);

  // 计算本地 Cookie 评分
  let local_val = serde_json::from_str::<serde_json::Value>(cookie_json)
    .unwrap_or(serde_json::Value::Array(vec![]));
  let local_cookies = crate::cookie_sync::prepare_cookies_for_injection(&local_val, cloud_platform);
  let local_score = crate::cookie_sync::score_cookies(&local_cookies, cloud_platform);

  // 计算云端 Cookie 评分
  let cloud_score = if check_result["success"].as_bool().unwrap_or(false) {
    if let Some(cloud_cookie_text) = check_result["cookie"].as_str() {
      if !cloud_cookie_text.is_empty() {
        let cloud_val = serde_json::from_str::<serde_json::Value>(cloud_cookie_text)
          .unwrap_or(serde_json::Value::String(cloud_cookie_text.to_string()));
        let cloud_cookies =
          crate::cookie_sync::prepare_cookies_for_injection(&cloud_val, cloud_platform);
        let score = crate::cookie_sync::score_cookies(&cloud_cookies, cloud_platform);
        Some(score)
      } else {
        None
      }
    } else {
      None
    }
  } else {
    None
  };

  log_bwbrowser(
    "cookie_sync",
    &format!(
      "  本地评分: {} 分 (可用={}, 确定登录={}), 云端评分: {} 分 (平台: {})",
      local_score.total,
      local_score.usable,
      local_score.definite_logged_in,
      cloud_score
        .as_ref()
        .map(|s| s.total.to_string())
        .unwrap_or_else(|| "无".to_string()),
      cloud_platform
    ),
  );

  // 始终上传（页面检测已确认登录状态，cookie 评分不再控制流程）
  log_bwbrowser(
    "cookie_sync",
    "  始终上传本地 Cookie 到云端（页面检测为准）",
  );

  // 上传到服务器
  let form_data = format!(
    "action=update_account_cookies&username={}&password={}&account_id={}&cookie={}",
    urlencode(&username),
    urlencode(&password),
    account_id,
    urlencode(cookie_json)
  );

  let resp = BWBROWSER_AUTH
    .client
    .post(BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("上传 Cookie 失败: {}", e))?;

  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取响应失败: {}", e))?;

  let result: serde_json::Value = parse_body("api", &body)?;

  if !result["success"].as_bool().unwrap_or(false) {
    let msg = result["message"].as_str().unwrap_or("未知错误").to_string();
    return Err(msg);
  }

  log_bwbrowser("cookie_sync", "  ✓ Cookie 已上传到云端");
  Ok((
    true,
    local_score.total,
    cloud_score.as_ref().map(|s| s.total),
  ))
}

// ==================== 用户管理（users.php API）====================

const BWBROWSER_USERS_API_URL: &str = "http://yacm.xin/tk/users.php";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserManagementUser {
  pub id: i64,
  #[serde(default)]
  pub username: String,
  #[serde(default)]
  pub real_name: String,
  #[serde(default)]
  pub role: String,
  #[serde(default)]
  pub role_label: String,
  #[serde(default)]
  pub status: String,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_super_admin: Option<bool>,
  #[serde(default)]
  pub phone: Option<String>,
  #[serde(default)]
  pub email: Option<String>,
  #[serde(default)]
  pub created_at: Option<String>,
  #[serde(default)]
  pub last_login_at: Option<String>,
  #[serde(default)]
  pub permissions: Option<std::collections::BTreeMap<String, bool>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserManagementListResponse {
  pub success: bool,
  #[serde(default)]
  pub users: Option<Vec<UserManagementUser>>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_super_admin: Option<bool>,
  #[serde(default)]
  pub permission_labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserManagementRoleOption {
  pub value: String,
  pub label: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserManagementRolesResponse {
  pub success: bool,
  #[serde(default)]
  pub roles: Option<Vec<UserManagementRoleOption>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CurrentManagementUser {
  #[serde(default)]
  pub success: bool,
  #[serde(default)]
  pub id: i64,
  #[serde(default)]
  pub username: String,
  #[serde(default)]
  pub real_name: String,
  #[serde(default)]
  pub role: String,
  #[serde(default)]
  pub role_label: String,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_super_admin: Option<bool>,
  #[serde(default)]
  pub permissions: Option<std::collections::BTreeMap<String, bool>>,
}

#[derive(Debug, Deserialize)]
struct MeApiResponse {
  success: bool,
  #[serde(default)]
  user: serde_json::Value,
}

impl BwbrowserAuthManager {
  /// 调用 users.php 的 JSON API
  async fn call_users_api(
    &self,
    action: &str,
    extra_params: &[(&str, &str)],
  ) -> Result<String, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let mut form_parts: Vec<String> = vec![
      format!("action={}", urlencode(action)),
      format!("username={}", urlencode(&username)),
      format!("password={}", urlencode(&password)),
    ];
    for (k, v) in extra_params {
      form_parts.push(format!("{}={}", k, urlencode(v)));
    }
    let form_data = form_parts.join("&");

    log_bwbrowser("users", &format!("→ action={}", action));

    let resp = self
      .client
      .post(BWBROWSER_USERS_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("users", &format!("网络请求失败: {}", e));
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
      log_bwbrowser_error("users", &format!("读取响应失败: {}", e));
      format!("读取响应失败: {}", e)
    })?;

    log_bwbrowser(
      "users",
      &format!(
        "← HTTP {}, {} bytes: {}",
        status,
        body.len(),
        body.chars().take(200).collect::<String>()
      ),
    );

    Ok(body)
  }

  /// 获取当前登录的管理用户信息（含权限）
  pub async fn get_current_management_user(&self) -> Result<CurrentManagementUser, String> {
    let body = self.call_users_api("me", &[]).await?;
    let api_resp: MeApiResponse = parse_body("users_me", &body)?;
    let user_val = if api_resp.user.is_object() {
      api_resp.user
    } else {
      return Ok(CurrentManagementUser {
        success: false,
        id: 0,
        username: String::new(),
        real_name: String::new(),
        role: String::new(),
        role_label: String::new(),
        is_super_admin: None,
        permissions: None,
      });
    };
    let mut result: CurrentManagementUser =
      serde_json::from_value(user_val).map_err(|e| format!("解析用户信息失败: {}", e))?;
    result.success = true;
    Ok(result)
  }

  /// 获取用户列表
  pub async fn list_management_users(&self) -> Result<UserManagementListResponse, String> {
    let body = self.call_users_api("list", &[]).await?;
    let result: UserManagementListResponse = parse_body("users_list", &body)?;
    Ok(result)
  }

  /// 获取角色列表
  pub async fn list_management_roles(&self) -> Result<Vec<UserManagementRoleOption>, String> {
    let body = self.call_users_api("roles", &[]).await?;
    let result: UserManagementRolesResponse = parse_body("users_roles", &body)?;
    Ok(result.roles.unwrap_or_default())
  }

  /// 获取当前用户的爆文库浏览器 cookies
  pub async fn get_bwbrowser_cookies(&self) -> Result<Option<String>, String> {
    let body = self.call_users_api("get_bwbrowser_cookies", &[]).await?;
    let result: serde_json::Value = parse_body("get_bwbrowser_cookies", &body)?;
    let cookies = result["cookies"].as_str().map(|s| s.to_string());
    Ok(cookies)
  }

  /// 更新当前用户的爆文库浏览器 cookies
  pub async fn update_bwbrowser_cookies(&self, cookies: &str) -> Result<(), String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let form_data = format!(
      "action=update_bwbrowser_cookies&username={}&password={}&cookies={}",
      urlencode(&username),
      urlencode(&password),
      urlencode(cookies),
    );

    let resp = self
      .client
      .post(BWBROWSER_USERS_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("update_bwbrowser_cookies", &format!("网络请求失败: {}", e));
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    log_bwbrowser(
      "update_bwbrowser_cookies",
      &format!("← HTTP {}, {} bytes", status, body.len()),
    );

    let result: serde_json::Value =
      serde_json::from_str(&body).map_err(|e| format!("解析响应失败: {}", e))?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("保存失败");
      return Err(msg.to_string());
    }
    Ok(())
  }

  /// 获取当前用户的爆文库浏览器书签
  pub async fn get_bwbrowser_bookmarks(&self) -> Result<Option<String>, String> {
    let body = self.call_users_api("get_bwbrowser_bookmarks", &[]).await?;
    let result: serde_json::Value = parse_body("get_bwbrowser_bookmarks", &body)?;
    let bookmarks = result["bookmarks"].as_str().map(|s| s.to_string());
    Ok(bookmarks)
  }

  /// 更新当前用户的爆文库浏览器书签
  pub async fn update_bwbrowser_bookmarks(&self, bookmarks: &str) -> Result<(), String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let form_data = format!(
      "action=update_bwbrowser_bookmarks&username={}&password={}&bookmarks={}",
      urlencode(&username),
      urlencode(&password),
      urlencode(bookmarks),
    );

    let resp = self
      .client
      .post(BWBROWSER_USERS_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error(
          "update_bwbrowser_bookmarks",
          &format!("网络请求失败: {}", e),
        );
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    log_bwbrowser(
      "update_bwbrowser_bookmarks",
      &format!("← HTTP {}, {} bytes", status, body.len()),
    );

    let result: serde_json::Value =
      serde_json::from_str(&body).map_err(|e| format!("解析响应失败: {}", e))?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("保存失败");
      return Err(msg.to_string());
    }
    Ok(())
  }

  /// 从本地 profile 读取 Bookmarks 文件内容
  pub fn read_local_bookmarks(profile_id: &str) -> Result<Option<String>, String> {
    let pm = crate::profile::manager::ProfileManager::instance();
    let profiles_dir = pm.get_profiles_dir();
    let profile_uuid = uuid::Uuid::parse_str(profile_id).map_err(|e| e.to_string())?;
    let profile_path = profiles_dir.join(profile_uuid.to_string()).join("profile");
    let bookmarks_path = profile_path.join("Default").join("Bookmarks");

    if !bookmarks_path.exists() {
      return Ok(None);
    }

    let content = std::fs::read_to_string(&bookmarks_path).map_err(|e| {
      log_bwbrowser_error("bookmarks", &format!("读取 Bookmarks 文件失败: {}", e));
      format!("读取书签失败: {}", e)
    })?;

    if content.is_empty() {
      Ok(None)
    } else {
      Ok(Some(content))
    }
  }

  /// 将书签写入本地 profile 的 Bookmarks 文件
  pub fn write_local_bookmarks(profile_id: &str, bookmarks: &str) -> Result<(), String> {
    let pm = crate::profile::manager::ProfileManager::instance();
    let profiles_dir = pm.get_profiles_dir();
    let profile_uuid = uuid::Uuid::parse_str(profile_id).map_err(|e| e.to_string())?;
    let profile_path = profiles_dir.join(profile_uuid.to_string()).join("profile");

    if !profile_path.exists() {
      return Err("profile 目录不存在".to_string());
    }

    let bookmarks_path = profile_path.join("Default").join("Bookmarks");
    std::fs::write(&bookmarks_path, bookmarks).map_err(|e| {
      log_bwbrowser_error("bookmarks", &format!("写入 Bookmarks 文件失败: {}", e));
      format!("写入书签失败: {}", e)
    })?;

    log_bwbrowser(
      "bookmarks",
      &format!("已写入 Bookmarks 文件: {} 字节", bookmarks.len()),
    );
    Ok(())
  }

  /// 添加用户
  pub async fn add_management_user(
    &self,
    username: &str,
    password: &str,
    real_name: &str,
    role: &str,
  ) -> Result<i64, String> {
    let body = self
      .call_users_api(
        "add",
        &[
          ("new_username", username),
          ("new_password", password),
          ("real_name", real_name),
          ("role", role),
        ],
      )
      .await?;
    let result: serde_json::Value = parse_body("users_add", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("添加失败").to_string();
      return Err(msg);
    }
    let user_id = result["user_id"].as_i64().unwrap_or(0);
    Ok(user_id)
  }

  /// 更新用户信息
  pub async fn update_management_user(
    &self,
    user_id: i64,
    real_name: &str,
    role: &str,
    status: &str,
    password: Option<&str>,
  ) -> Result<(), String> {
    let mut params: Vec<(&str, String)> = vec![
      ("user_id", user_id.to_string()),
      ("real_name", real_name.to_string()),
      ("role", role.to_string()),
      ("status", status.to_string()),
    ];
    if let Some(pw) = password {
      if !pw.is_empty() {
        params.push(("password", pw.to_string()));
      }
    }
    let param_refs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let body = self.call_users_api("update", &param_refs).await?;
    let result: serde_json::Value = parse_body("users_update", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("更新失败").to_string();
      return Err(msg);
    }
    Ok(())
  }

  /// 切换用户权限
  pub async fn toggle_management_user_permission(
    &self,
    user_id: i64,
    permission: &str,
    value: bool,
  ) -> Result<(), String> {
    let body = self
      .call_users_api(
        "toggle_permission",
        &[
          ("user_id", &user_id.to_string()),
          ("permission", permission),
          ("value", if value { "1" } else { "0" }),
        ],
      )
      .await?;
    let result: serde_json::Value = parse_body("users_toggle_perm", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("操作失败").to_string();
      return Err(msg);
    }
    Ok(())
  }

  /// 切换用户状态（启用/禁用）
  pub async fn toggle_management_user_status(
    &self,
    user_id: i64,
    status: &str,
  ) -> Result<(), String> {
    let body = self
      .call_users_api(
        "toggle_status",
        &[("user_id", &user_id.to_string()), ("status", status)],
      )
      .await?;
    let result: serde_json::Value = parse_body("users_toggle_status", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("操作失败").to_string();
      return Err(msg);
    }
    Ok(())
  }

  /// 删除用户
  pub async fn delete_management_user(&self, user_id: i64) -> Result<(), String> {
    let body = self
      .call_users_api("delete", &[("user_id", &user_id.to_string())])
      .await?;
    let result: serde_json::Value = parse_body("users_delete", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"].as_str().unwrap_or("删除失败").to_string();
      return Err(msg);
    }
    Ok(())
  }
}

// ========== Tauri Commands - 用户管理 ==========

#[tauri::command]
pub async fn bwbrowser_get_current_management_user() -> Result<CurrentManagementUser, String> {
  let result = BWBROWSER_AUTH.get_current_management_user().await?;
  Ok(result)
}

#[tauri::command]
pub async fn bwbrowser_get_bwbrowser_cookies() -> Result<Option<String>, String> {
  let cookies = BWBROWSER_AUTH.get_bwbrowser_cookies().await?;
  Ok(cookies)
}

#[tauri::command]
pub async fn bwbrowser_update_bwbrowser_cookies(cookies: String) -> Result<(), String> {
  BWBROWSER_AUTH.update_bwbrowser_cookies(&cookies).await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_get_bwbrowser_bookmarks() -> Result<Option<String>, String> {
  let bookmarks = BWBROWSER_AUTH.get_bwbrowser_bookmarks().await?;
  Ok(bookmarks)
}

#[tauri::command]
pub async fn bwbrowser_update_bwbrowser_bookmarks(bookmarks: String) -> Result<(), String> {
  BWBROWSER_AUTH
    .update_bwbrowser_bookmarks(&bookmarks)
    .await?;
  Ok(())
}

#[tauri::command]
pub fn bwbrowser_read_local_bookmarks(profile_id: String) -> Result<Option<String>, String> {
  BwbrowserAuthManager::read_local_bookmarks(&profile_id)
}

#[tauri::command]
pub fn bwbrowser_write_local_bookmarks(
  profile_id: String,
  bookmarks: String,
) -> Result<(), String> {
  BwbrowserAuthManager::write_local_bookmarks(&profile_id, &bookmarks)
}

#[tauri::command]
pub async fn bwbrowser_list_management_users() -> Result<UserManagementListResponse, String> {
  let result = BWBROWSER_AUTH.list_management_users().await?;
  Ok(result)
}

#[tauri::command]
pub async fn bwbrowser_list_management_roles() -> Result<Vec<UserManagementRoleOption>, String> {
  let roles = BWBROWSER_AUTH.list_management_roles().await?;
  Ok(roles)
}

#[tauri::command]
pub async fn bwbrowser_add_management_user(
  username: String,
  password: String,
  real_name: String,
  role: String,
) -> Result<i64, String> {
  let user_id = BWBROWSER_AUTH
    .add_management_user(&username, &password, &real_name, &role)
    .await?;
  Ok(user_id)
}

#[tauri::command]
pub async fn bwbrowser_update_management_user(
  user_id: i64,
  real_name: String,
  role: String,
  status: String,
  password: Option<String>,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .update_management_user(user_id, &real_name, &role, &status, password.as_deref())
    .await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_toggle_management_user_permission(
  user_id: i64,
  permission: String,
  value: bool,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .toggle_management_user_permission(user_id, &permission, value)
    .await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_toggle_management_user_status(
  user_id: i64,
  status: String,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .toggle_management_user_status(user_id, &status)
    .await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_delete_management_user(user_id: i64) -> Result<(), String> {
  BWBROWSER_AUTH.delete_management_user(user_id).await?;
  Ok(())
}
