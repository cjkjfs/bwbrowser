//! 远程桌面被控端（Rust 原生实现，替代 Python server.py）
//!
//! 协议与 `remote/remote-tool/server.py` v3 + `remote/php/relay_server.py` 兼容：
//! - relay 模式：wss://yacm.xin/relay?role=client&uuid=..&hostname=..，等 viewer 连接
//! - 指令为 JSON 文本（auth/start_stream/mouse_*/key_*/..），视频帧为 >III 头 + JPEG 二进制
//! - VPS 注册：client_login/register/heartbeat/unregister（remote_api.php）
//!
//! 权限：云端 users.php 权限组的 allow_remote_desktop 决定 client_login 是否放行，
//! 无权限账号注册被拒 → 被控端无法上线（远程桌面关闭）。

mod capture;
mod encode;
mod input;
mod protocol;
mod relay;
mod vps;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::bwbrowser_cloud::BWBROWSER_AUTH;
use crate::settings_manager::SettingsManager;

/// 被控端配置（本地持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAgentConfig {
  pub uuid: String,
  pub port: u16,
  pub password: String,
  pub quality: u8,
  pub fps: u8,
  pub scale: f32,
}

impl Default for RemoteAgentConfig {
  fn default() -> Self {
    Self {
      uuid: String::new(),
      port: 8765,
      password: "remote123".to_string(),
      quality: 65,
      fps: 30,
      scale: 1.0,
    }
  }
}

/// 被控端运行状态（返回前端展示）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentStatus {
  pub running: bool,
  pub uuid: String,
  pub hostname: String,
  pub relay_connected: bool,
  pub vps_registered: bool,
  pub viewers: usize,
  pub last_error: Option<String>,
}

struct AgentRuntime {
  stop: Arc<AtomicBool>,
  status: Arc<Mutex<AgentStatus>>,
  relay_task: Option<tokio::task::JoinHandle<()>>,
  local_task: Option<tokio::task::JoinHandle<()>>,
}

static AGENT: Mutex<Option<AgentRuntime>> = Mutex::new(None);

fn config_file() -> std::path::PathBuf {
  SettingsManager::instance()
    .get_settings_dir()
    .join("remote_agent.json")
}

fn load_or_create_config() -> Result<RemoteAgentConfig, String> {
  let path = config_file();
  if path.exists() {
    if let Ok(content) = std::fs::read_to_string(&path) {
      if let Ok(cfg) = serde_json::from_str::<RemoteAgentConfig>(&content) {
        if !cfg.uuid.is_empty() {
          return Ok(cfg);
        }
      }
    }
  }
  let cfg = RemoteAgentConfig {
    uuid: uuid::Uuid::new_v4().to_string(),
    ..Default::default()
  };
  let dir = path
    .parent()
    .ok_or_else(|| serde_json::json!({"code": "REMOTE_DESKTOP_INTERNAL"}).to_string())?;
  std::fs::create_dir_all(dir).map_err(|e| {
    serde_json::json!({"code": "REMOTE_DESKTOP_INTERNAL", "params": {"detail": e.to_string()}})
      .to_string()
  })?;
  std::fs::write(
    &path,
    serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?,
  )
  .map_err(|e| {
    serde_json::json!({"code": "REMOTE_DESKTOP_INTERNAL", "params": {"detail": e.to_string()}})
      .to_string()
  })?;
  Ok(cfg)
}

fn save_config(cfg: &RemoteAgentConfig) -> Result<(), String> {
  std::fs::write(
    config_file(),
    serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?,
  )
  .map_err(|e| {
    serde_json::json!({"code": "REMOTE_DESKTOP_INTERNAL", "params": {"detail": e.to_string()}})
      .to_string()
  })
}

fn hostname() -> String {
  #[cfg(windows)]
  {
    std::env::var("COMPUTERNAME")
      .or_else(|_| std::env::var("HOSTNAME"))
      .unwrap_or_else(|_| "unknown".to_string())
  }
  #[cfg(not(windows))]
  {
    // 从 Finder/Dock 启动的 GUI 进程没有 HOSTNAME 环境变量，直接问内核。
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
      let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
      if let Ok(name) = std::str::from_utf8(&buf[..end]) {
        if !name.is_empty() {
          return name.to_string();
        }
      }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
  }
}

/// macOS 被控端所需的两个 TCC 权限：屏幕录制（截屏）与辅助功能（注入鼠标键盘）。
/// 仅在真正（重新）拉起被控端时请求，运行中不会反复弹窗。
#[cfg(target_os = "macos")]
async fn ensure_macos_permissions() {
  use tauri_plugin_macos_permissions as perms;
  if !perms::check_screen_recording_permission().await {
    log_remote("macOS 缺少屏幕录制权限，正在请求（被控端截屏需要）");
    perms::request_screen_recording_permission().await;
    // 屏幕录制是 macOS 里少数「授权后必须重启进程才生效」的 TCC 权限：
    // 不重启的话本次会话只会截到桌面壁纸，看起来像功能没生效。
    log_remote(
      "请在「系统设置 → 隐私与安全性 → 屏幕录制」中勾选 BW Browser，勾选后重启本应用再连接",
    );
  }
  if !perms::check_accessibility_permission().await {
    log_remote("macOS 缺少辅助功能权限，正在请求（被控端注入鼠标键盘需要）");
    perms::request_accessibility_permission().await;
  }
}

/// 启动被控端：VPS 注册 + relay 连接 + 本地 WS 服务
pub async fn start_agent(quality: Option<u8>, fps: Option<u8>) -> Result<AgentStatus, String> {
  {
    let guard = AGENT
      .lock()
      .map_err(|_| serde_json::json!({"code": "REMOTE_DESKTOP_INTERNAL"}).to_string())?;
    if let Some(rt) = guard.as_ref() {
      if rt.relay_task.is_some() || rt.local_task.is_some() {
        return Ok(rt.status.lock().unwrap().clone());
      }
    }
  }

  let creds = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| serde_json::json!({"code": "CLOUD_NOT_SIGNED_IN"}).to_string())?;

  let mut cfg = load_or_create_config()?;
  if let Some(q) = quality {
    cfg.quality = q.clamp(10, 100);
  }
  if let Some(f) = fps {
    cfg.fps = f.clamp(5, 60);
  }
  save_config(&cfg)?;

  #[cfg(target_os = "macos")]
  ensure_macos_permissions().await;

  let status = Arc::new(Mutex::new(AgentStatus {
    running: true,
    uuid: cfg.uuid.clone(),
    hostname: hostname(),
    ..Default::default()
  }));
  let stop = Arc::new(AtomicBool::new(false));

  let relay_handle = {
    let stop = stop.clone();
    let status = status.clone();
    let cfg = cfg.clone();
    let creds = creds.clone();
    tokio::spawn(async move {
      relay::run_agent_loop(cfg, creds, stop, status).await;
    })
  };

  let local_handle = {
    let stop = stop.clone();
    let status = status.clone();
    let cfg = cfg.clone();
    tokio::spawn(async move {
      relay::run_local_server(cfg, stop, status).await;
    })
  };

  let runtime = AgentRuntime {
    stop,
    status: status.clone(),
    relay_task: Some(relay_handle),
    local_task: Some(local_handle),
  };
  *AGENT.lock().unwrap() = Some(runtime);

  let snapshot = status.lock().unwrap().clone();
  Ok(snapshot)
}

/// 停止被控端：撤销注册并终止所有任务
pub async fn stop_agent() -> AgentStatus {
  let mut guard = AGENT.lock().unwrap();
  if let Some(rt) = guard.as_mut() {
    log_remote(&format!("用户 {} 关闭远程桌面", current_user()));
    rt.stop.store(true, Ordering::SeqCst);
    // 尽力通知服务端下线
    let uuid = rt.status.lock().unwrap().uuid.clone();
    tokio::spawn(async move {
      let _ = vps::unregister(&uuid).await;
    });
    if let Some(t) = rt.relay_task.take() {
      t.abort();
    }
    if let Some(t) = rt.local_task.take() {
      t.abort();
    }
    let mut st = rt.status.lock().unwrap();
    st.running = false;
    st.relay_connected = false;
    st.vps_registered = false;
    st.viewers = 0;
    let snapshot = st.clone();
    drop(st);
    drop(guard);
    *AGENT.lock().unwrap() = None;
    return snapshot;
  }
  AgentStatus::default()
}

/// 查询被控端状态
pub fn agent_status() -> AgentStatus {
  let guard = AGENT.lock().unwrap();
  if let Some(rt) = guard.as_ref() {
    return rt.status.lock().unwrap().clone();
  }
  let cfg = load_or_create_config().unwrap_or_default();
  AgentStatus {
    running: false,
    uuid: cfg.uuid,
    hostname: hostname(),
    ..Default::default()
  }
}

/// 从 JSON 错误串里提取 code（无则 None）
pub(crate) fn error_code(err: &str) -> Option<String> {
  serde_json::from_str::<serde_json::Value>(err)
    .ok()
    .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_string))
}

/// 被控端专用日志（写入 bwbrowser_debug.log，与登录日志同通道）
pub(crate) fn log_remote(msg: &str) {
  crate::bwbrowser_cloud::log_bwbrowser("remote_desktop", msg);
}

/// 当前登录的爆文库账号（用于日志标注用户身份）
pub(crate) fn current_user() -> String {
  BWBROWSER_AUTH
    .get_credentials()
    .map(|(u, _)| u)
    .unwrap_or_else(|| "?".to_string())
}

/// 当前是否已有被控端任务在跑
fn is_running() -> bool {
  AGENT
    .lock()
    .unwrap()
    .as_ref()
    .is_some_and(|rt| rt.relay_task.is_some() || rt.local_task.is_some())
}

/// 防刷屏：记住最近一次权限判定 (账号, 是否被拒)，只有状态变化才打日志
static LAST_PERM_STATE: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// 云端权限驱动启停（无 UI 开关，权限仅来自 users.php 的 allow_remote_desktop）：
/// 未登录或被拒 → 停止；放行 → 启动；其他网络类失败保持现状，交给 relay 循环重试
pub async fn reconcile() {
  if BWBROWSER_AUTH.get_credentials().is_none() {
    stop_agent().await;
    *LAST_PERM_STATE.lock().unwrap() = None;
    return;
  }
  let was_running = is_running();
  let user = current_user();
  match vps::client_login().await {
    Ok(_) => {
      {
        let mut last = LAST_PERM_STATE.lock().unwrap();
        if last
          .as_ref()
          .is_some_and(|(u, denied)| u == &user && *denied)
        {
          log_remote(&format!("用户 {user} 远程桌面权限已开通，被控端启动"));
        }
        *last = Some((user.clone(), false));
      }
      match start_agent(None, None).await {
        Ok(status) if status.running && !was_running => {
          log_remote(&format!("用户 {user} 开启远程桌面（权限放行）"));
        }
        Ok(_) => {}
        Err(e) => log::warn!("remote agent start failed: {e}"),
      }
    }
    Err(e) => {
      if error_code(&e).as_deref() == Some("NO_REMOTE_PERMISSION") {
        {
          let mut last = LAST_PERM_STATE.lock().unwrap();
          let changed = last
            .as_ref()
            .map(|(u, denied)| u != &user || !denied)
            .unwrap_or(true);
          if changed {
            if was_running {
              log_remote(&format!(
                "用户 {user} 远程桌面被拒绝（users.php 权限组未开通），被控端下线"
              ));
            } else {
              log_remote(&format!(
                "用户 {user} 远程桌面被拒绝（users.php 权限组未开通），被控端未启动"
              ));
            }
            *last = Some((user.clone(), true));
          }
        }
        log::info!("remote desktop permission revoked, stopping controlled-end");
        stop_agent().await;
      }
    }
  }
}

/// 权限守护任务：每 60 秒按云端权限重判启停（登录/登出另有即时触发）
pub fn spawn_permission_watcher() {
  tauri::async_runtime::spawn(async move {
    loop {
      reconcile().await;
      tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
  });
}
