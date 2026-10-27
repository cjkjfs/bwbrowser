#![allow(dead_code, clippy::too_many_arguments)]

//! 视频下载模块 - 基于 yt-dlp 的视频下载管理
//!
//! 功能：
//! - 多任务队列管理
//! - 实时进度/速度上报（通过 Tauri 事件）
//! - 支持从爆文浏览器导出 cookie
//! - 粘贴链接自动下载

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};
use tokio::sync::Mutex;

/// 下载任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
  Waiting,
  Downloading,
  Paused,
  Completed,
  Error,
  Cancelled,
}

/// 单个下载任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
  pub id: String,
  pub url: String,
  pub title: Option<String>,
  pub status: DownloadStatus,
  pub progress: f64,           // 0-100
  pub speed_text: String,      // 可读速度文本
  pub peak_speed_text: String, // 峰值速度文本
  pub downloaded: u64,         // 已下载字节
  pub total: u64,              // 总字节
  pub filename: Option<String>,
  pub resolution: Option<String>, // 视频分辨率
  pub error_msg: Option<String>,
  pub status_text: Option<String>, // 当前状态描述（解析阶段反馈）
  pub started_at: Option<i64>,
  pub finished_at: Option<i64>,
  pub thumbnail: Option<String>, // 视频缩略图 URL
}

impl DownloadTask {
  fn new(url: String) -> Self {
    Self {
      id: uuid::Uuid::new_v4().to_string(),
      url,
      title: None,
      status: DownloadStatus::Waiting,
      progress: 0.0,
      speed_text: String::new(),
      peak_speed_text: String::new(),
      downloaded: 0,
      total: 0,
      filename: None,
      resolution: None,
      error_msg: None,
      status_text: None,
      started_at: None,
      finished_at: None,
      thumbnail: None,
    }
  }
}

/// 下载设置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadSettings {
  pub download_dir: String,
  pub max_concurrent: u32, // 0 = 无限制
  pub max_height: u32,     // 0 = 最高
  pub auto_paste_download: bool,
  pub cookie_from_browser: bool,
  // VPS 登录 profile 的 Cookie 取不到时，回退到系统浏览器（Chrome/Edge/Firefox）提取
  #[serde(default = "default_true")]
  pub cookie_from_system_browser: bool,
  pub proxy_id: Option<String>, // None = 直连
}

fn default_true() -> bool {
  true
}

/// 默认下载目录：桌面/downloadvideo（不存在时自动创建）。
fn default_download_dir() -> PathBuf {
  let base = dirs::desktop_dir()
    .or_else(dirs::download_dir)
    .unwrap_or_else(|| PathBuf::from("."));
  let dir = base.join("downloadvideo");
  let _ = std::fs::create_dir_all(&dir);
  dir
}

impl Default for DownloadSettings {
  fn default() -> Self {
    Self {
      download_dir: default_download_dir().to_string_lossy().to_string(),
      max_concurrent: 10,
      max_height: 0,
      auto_paste_download: false,
      cookie_from_browser: true,
      cookie_from_system_browser: true,
      proxy_id: None,
    }
  }
}

/// 下载管理器内部状态
#[derive(Default)]
struct DownloaderInner {
  tasks: HashMap<String, DownloadTask>,
  settings: DownloadSettings,
  yt_dlp_path: Option<PathBuf>,
  last_cookie_time: i64,
  running_pids: HashMap<String, u32>, // task_id -> child pid
  tools_checked: bool,                // 是否已检查过工具
  tools_available: bool,              // 核心工具（yt-dlp + ffmpeg）是否可用
}

/// 根据 proxy_id 构建 yt-dlp 可用的代理 URL
/// 返回 None 表示直连（不设置代理）
fn build_proxy_url(proxy_id: &Option<String>) -> Option<String> {
  let pid = proxy_id.as_ref()?;
  if pid.is_empty() || pid == "direct" || pid == "none" {
    return None;
  }
  let stored = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
  let proxy = stored.iter().find(|p| p.id == *pid)?;
  let ps = &proxy.proxy_settings;

  // 构建 URL: scheme://[user:pass@]host:port
  let scheme = match ps.proxy_type.to_lowercase().as_str() {
    "socks5" | "socks" => "socks5",
    "http" | "https" => "http",
    _ => "http",
  };

  let auth = match (&ps.username, &ps.password) {
    (Some(u), Some(p)) if !u.is_empty() => {
      format!("{}:{}@", urlencode(u), urlencode(p))
    }
    (Some(u), None) if !u.is_empty() => format!("{}@", urlencode(u)),
    _ => String::new(),
  };

  Some(format!("{}://{}{}:{}", scheme, auth, ps.host, ps.port))
}

fn urlencode(s: &str) -> String {
  let mut encoded = String::new();
  for c in s.chars() {
    if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
      encoded.push(c);
    } else {
      for byte in c.to_string().as_bytes() {
        encoded.push_str(&format!("%{:02X}", byte));
      }
    }
  }
  encoded
}

/// 下载管理器
pub struct VideoDownloader {
  inner: Mutex<DownloaderInner>,
  clipboard_monitor_running: Arc<AtomicBool>,
  clipboard_monitor_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
  tools_downloading: Mutex<std::collections::HashSet<String>>,
}

impl VideoDownloader {
  pub fn new() -> Self {
    Self {
      inner: Mutex::new(DownloaderInner {
        tasks: HashMap::new(),
        settings: DownloadSettings::default(),
        yt_dlp_path: None,
        last_cookie_time: 0,
        running_pids: HashMap::new(),
        tools_checked: false,
        tools_available: false,
      }),
      clipboard_monitor_running: Arc::new(AtomicBool::new(false)),
      clipboard_monitor_handle: Mutex::new(None),
      tools_downloading: Mutex::new(std::collections::HashSet::new()),
    }
  }

  pub async fn is_downloading_tools(&self) -> bool {
    let set = self.tools_downloading.lock().await;
    !set.is_empty()
  }

  async fn begin_tool_download(&self, tool: &str) -> bool {
    let mut set = self.tools_downloading.lock().await;
    if set.contains(tool) {
      return false;
    }
    set.insert(tool.to_string());
    true
  }

  async fn end_tool_download(&self, tool: &str) {
    let mut set = self.tools_downloading.lock().await;
    set.remove(tool);
  }

  /// 从持久化文件加载任务和设置
  pub async fn load_from_disk(&self) {
    let data_dir = crate::app_dirs::data_dir();
    let tasks_file = data_dir.join("video_download_tasks.json");
    let settings_file = data_dir.join("video_download_settings.json");

    // 加载设置
    if let Ok(content) = std::fs::read_to_string(&settings_file) {
      if let Ok(saved) = serde_json::from_str::<DownloadSettings>(&content) {
        let mut inner = self.inner.lock().await;
        // 老配置里目录为空 → 升级为默认目录（桌面/downloadvideo，自动创建）
        if saved.download_dir.trim().is_empty() {
          inner.settings = DownloadSettings {
            download_dir: default_download_dir().to_string_lossy().to_string(),
            ..saved
          };
        } else {
          inner.settings = saved;
        }
      }
    }

    // 加载任务
    if let Ok(content) = std::fs::read_to_string(&tasks_file) {
      if let Ok(saved_tasks) = serde_json::from_str::<Vec<DownloadTask>>(&content) {
        let mut inner = self.inner.lock().await;
        for task in saved_tasks {
          // 之前正在下载的，重置为等待状态，下次自动重试
          let mut t = task;
          if t.status == DownloadStatus::Downloading {
            t.status = DownloadStatus::Paused;
          }
          inner.tasks.insert(t.id.clone(), t);
        }
      }
    }
  }

  /// 持久化任务和设置到磁盘
  async fn save_to_disk(&self) {
    let data_dir = crate::app_dirs::data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let tasks_file = data_dir.join("video_download_tasks.json");
    let settings_file = data_dir.join("video_download_settings.json");

    let inner = self.inner.lock().await;

    // 保存设置
    if let Ok(json) = serde_json::to_string_pretty(&inner.settings) {
      let _ = std::fs::write(&settings_file, json);
    }

    // 保存任务列表
    let tasks: Vec<&DownloadTask> = inner.tasks.values().collect();
    if let Ok(json) = serde_json::to_string_pretty(&tasks) {
      let _ = std::fs::write(&tasks_file, json);
    }
  }

  pub async fn get_last_cookie_time(&self) -> i64 {
    self.inner.lock().await.last_cookie_time
  }

  async fn set_last_cookie_time(&self, time: i64) {
    let mut inner = self.inner.lock().await;
    inner.last_cookie_time = time;
  }

  pub async fn set_yt_dlp_path(&self, path: PathBuf) {
    let mut inner = self.inner.lock().await;
    inner.yt_dlp_path = Some(path);
  }

  pub async fn get_settings(&self) -> DownloadSettings {
    self.inner.lock().await.settings.clone()
  }

  pub async fn update_settings(&self, new: DownloadSettings) {
    let mut inner = self.inner.lock().await;
    inner.settings = new;
    drop(inner);
    self.save_to_disk().await;
  }

  pub async fn list_tasks(&self) -> Vec<DownloadTask> {
    let inner = self.inner.lock().await;
    let mut list: Vec<DownloadTask> = inner.tasks.values().cloned().collect();
    list.sort_by_key(|t| std::cmp::Reverse(t.started_at));
    list
  }

  /// 添加下载任务，返回任务 ID
  pub async fn add_task<R: Runtime>(&self, url: String, app: &tauri::AppHandle<R>) -> String {
    // 去重：URL 已存在则返回现有任务 ID
    {
      let inner = self.inner.lock().await;
      if let Some(existing) = inner.tasks.values().find(|t| t.url == url) {
        return existing.id.clone();
      }
    }
    let task = DownloadTask::new(url);
    let id = task.id.clone();
    {
      let mut inner = self.inner.lock().await;
      inner.tasks.insert(id.clone(), task);
    }
    let _ = app.emit("video-download:task-added", &id);
    self.try_start_next(app).await;
    id
  }

  pub async fn cancel_task<R: Runtime>(&self, task_id: &str, app: &tauri::AppHandle<R>) {
    let pid = {
      let mut inner = self.inner.lock().await;
      if let Some(task) = inner.tasks.get_mut(task_id) {
        if matches!(
          task.status,
          DownloadStatus::Waiting | DownloadStatus::Paused | DownloadStatus::Error
        ) {
          task.status = DownloadStatus::Cancelled;
          task.finished_at = Some(now_secs());
        }
      }
      inner.running_pids.get(task_id).copied()
    };
    // 如果正在下载，kill 进程
    if let Some(pid) = pid {
      kill_process(pid);
    }
    self.save_to_disk().await;
    let _ = app.emit("video-download:task-updated", task_id);
  }

  /// 暂停任务
  pub async fn pause_task<R: Runtime>(&self, task_id: &str, app: &tauri::AppHandle<R>) {
    let pid = {
      let mut inner = self.inner.lock().await;
      if let Some(task) = inner.tasks.get_mut(task_id) {
        if task.status == DownloadStatus::Downloading || task.status == DownloadStatus::Waiting {
          task.status = DownloadStatus::Paused;
        }
      }
      inner.running_pids.remove(task_id)
    };
    if let Some(pid) = pid {
      kill_process(pid);
    }
    self.save_to_disk().await;
    let _ = app.emit("video-download:task-updated", task_id);
  }

  /// 重试任务
  pub async fn retry_task<R: Runtime>(&self, task_id: &str, app: &tauri::AppHandle<R>) -> bool {
    let has_task = {
      let mut inner = self.inner.lock().await;
      if let Some(task) = inner.tasks.get_mut(task_id) {
        if matches!(
          task.status,
          DownloadStatus::Error | DownloadStatus::Cancelled | DownloadStatus::Paused
        ) {
          task.status = DownloadStatus::Waiting;
          task.error_msg = None;
          task.progress = 0.0;
          task.finished_at = None;
          true
        } else {
          false
        }
      } else {
        false
      }
    };
    if has_task {
      self.save_to_disk().await;
      let _ = app.emit("video-download:task-updated", task_id);
      self.try_start_next(app).await;
    }
    has_task
  }

  /// 删除单个任务
  pub async fn delete_task<R: Runtime>(&self, task_id: &str, app: &tauri::AppHandle<R>) {
    let pid = {
      let mut inner = self.inner.lock().await;
      inner.tasks.remove(task_id);
      inner.running_pids.remove(task_id)
    };
    if let Some(pid) = pid {
      kill_process(pid);
    }
    self.save_to_disk().await;
    let _ = app.emit("video-download:task-deleted", task_id);
  }

  pub async fn clear_finished(&self) {
    let mut inner = self.inner.lock().await;
    inner.tasks.retain(|_, t| {
      matches!(
        t.status,
        DownloadStatus::Waiting | DownloadStatus::Downloading | DownloadStatus::Paused
      )
    });
    drop(inner);
    self.save_to_disk().await;
  }

  /// 暂停所有任务（正在下载和等待中的）
  pub async fn pause_all<R: Runtime>(&self, app: &tauri::AppHandle<R>) {
    let pids: Vec<u32> = {
      let mut inner = self.inner.lock().await;
      // 第一步：收集需要暂停的任务 ID，并修改状态
      let mut ids_to_pause: Vec<String> = Vec::new();
      for (id, task) in inner.tasks.iter_mut() {
        if matches!(
          task.status,
          DownloadStatus::Downloading | DownloadStatus::Waiting
        ) {
          task.status = DownloadStatus::Paused;
          ids_to_pause.push(id.clone());
        }
      }
      // 第二步：从 running_pids 中取出对应的 pid
      let mut pids = Vec::new();
      for id in &ids_to_pause {
        if let Some(pid) = inner.running_pids.remove(id) {
          pids.push(pid);
        }
      }
      pids
    };
    for pid in &pids {
      kill_process(*pid);
    }
    self.save_to_disk().await;
    let _ = app.emit("video-download:tasks-changed", ());
  }

  /// 重试所有失败/取消/暂停的任务
  pub async fn retry_all<R: Runtime>(&self, app: &tauri::AppHandle<R>) {
    let has_retry = {
      let mut inner = self.inner.lock().await;
      let mut count = 0;
      for task in inner.tasks.values_mut() {
        if matches!(
          task.status,
          DownloadStatus::Error | DownloadStatus::Cancelled | DownloadStatus::Paused
        ) {
          task.status = DownloadStatus::Waiting;
          task.error_msg = None;
          task.progress = 0.0;
          task.finished_at = None;
          count += 1;
        }
      }
      count > 0
    };
    if has_retry {
      self.save_to_disk().await;
      let _ = app.emit("video-download:tasks-changed", ());
      self.try_start_next(app).await;
    }
  }

  /// 删除所有任务
  pub async fn delete_all<R: Runtime>(&self, app: &tauri::AppHandle<R>) {
    let pids: Vec<u32> = {
      let mut inner = self.inner.lock().await;
      let pids: Vec<u32> = inner.running_pids.drain().map(|(_, pid)| pid).collect();
      inner.tasks.clear();
      pids
    };
    for pid in &pids {
      kill_process(*pid);
    }
    self.save_to_disk().await;
    let _ = app.emit("video-download:tasks-changed", ());
  }

  /// 启动剪贴板监控（200ms 轮询，检测到视频 URL 自动添加任务）
  pub async fn start_clipboard_monitor<R: Runtime>(&self, app: tauri::AppHandle<R>) {
    if self.clipboard_monitor_running.load(Ordering::SeqCst) {
      log::info!("[clipboard_monitor] 已在运行，跳过启动");
      return;
    }

    // 先清理旧的 handle（如果之前没正确清理），用 spawn_blocking 避免阻塞 async runtime
    {
      let old_handle = self.clipboard_monitor_handle.lock().await.take();
      if let Some(old) = old_handle {
        // 旧线程应该在 stop 时已收到停止信号，join 应立即返回
        let _ = tokio::task::spawn_blocking(move || old.join()).await;
      }
    }

    self.clipboard_monitor_running.store(true, Ordering::SeqCst);
    log::info!("[clipboard_monitor] 启动剪贴板监控");

    let running = self.clipboard_monitor_running.clone();
    let app_clone = app.clone();

    let handle = std::thread::spawn(move || {
      let mut last_content: Option<String> = None;

      // 每次循环创建新的 Clipboard 实例，避免 Windows 剪贴板句柄失效
      let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
          log::error!("[clipboard_monitor] 初始化失败: {}", e);
          running.store(false, Ordering::SeqCst);
          return;
        }
      };

      // 初始读取一次作为基准，避免历史内容触发下载
      if let Ok(text) = clipboard.get_text() {
        let len = text.len();
        last_content = Some(text);
        log::info!("[clipboard_monitor] 初始基准已设置，内容长度: {}", len);
      } else {
        log::warn!("[clipboard_monitor] 初始读取剪贴板失败，将跳过第一次变化");
      }

      let mut fail_count = 0u32;

      loop {
        if !running.load(Ordering::SeqCst) {
          break;
        }

        std::thread::sleep(Duration::from_millis(200));

        if !running.load(Ordering::SeqCst) {
          break;
        }

        let text = match clipboard.get_text() {
          Ok(t) => t,
          Err(e) => {
            fail_count += 1;
            if fail_count % 50 == 1 {
              log::warn!(
                "[clipboard_monitor] 读取剪贴板失败({}次): {}",
                fail_count,
                e
              );
            }
            // 每 100 次失败重建 Clipboard 实例
            if fail_count.is_multiple_of(100) {
              log::info!("[clipboard_monitor] 重建 Clipboard 实例");
              clipboard = match arboard::Clipboard::new() {
                Ok(c) => c,
                Err(e) => {
                  log::error!("[clipboard_monitor] 重建失败: {}", e);
                  continue;
                }
              };
            }
            continue;
          }
        };
        fail_count = 0;

        // 内容没变化
        if Some(&text) == last_content.as_ref() {
          continue;
        }

        let prev = last_content.clone();
        last_content = Some(text.clone());

        log::info!(
          "[clipboard_monitor] 剪贴板内容变化: {} -> {}",
          prev.as_deref().map(|s| s.len()).unwrap_or(0),
          text.len()
        );

        if prev.is_none() {
          continue;
        }

        // 提取视频 URL
        let urls = extract_video_urls(&text);
        if urls.is_empty() {
          continue;
        }

        log::info!("[clipboard_monitor] 检测到 {} 个视频 URL", urls.len());

        // 检测到了新 URL，用 tauri 主 runtime 处理
        let running_check = running.clone();
        let app_for_inner = app_clone.clone();
        let urls_clone = urls.clone();

        tauri::async_runtime::spawn(async move {
          if !running_check.load(Ordering::SeqCst) {
            return;
          }

          let dl = get_downloader();
          for url in urls_clone {
            dl.add_task(url, &app_for_inner).await;
          }
        });
      }
      log::info!("[clipboard_monitor] 监控线程退出");
    });

    *self.clipboard_monitor_handle.lock().await = Some(handle);
  }

  /// 停止剪贴板监控（不阻塞 async runtime）
  pub async fn stop_clipboard_monitor(&self) {
    self
      .clipboard_monitor_running
      .store(false, Ordering::SeqCst);
    log::info!("[clipboard_monitor] 已设置停止标志");

    // 不在这里 join 线程，避免阻塞 tokio runtime
    // 旧线程会在下次 start_clipboard_monitor 时被清理
    // 线程会在 200ms 内检测到标志并退出
    log::info!("[clipboard_monitor] 停止信号已发送");
  }

  /// 检查剪贴板监控是否在运行
  pub fn is_clipboard_monitor_running(&self) -> bool {
    self.clipboard_monitor_running.load(Ordering::SeqCst)
  }

  /// 注册运行中的子进程 PID
  async fn register_pid(&self, task_id: &str, pid: u32) {
    let mut inner = self.inner.lock().await;
    inner.running_pids.insert(task_id.to_string(), pid);
  }

  /// 注销子进程 PID
  async fn unregister_pid(&self, task_id: &str) {
    let mut inner = self.inner.lock().await;
    inner.running_pids.remove(task_id);
  }

  /// 设置视频标题
  async fn set_task_title(&self, task_id: &str, title: String) {
    let mut inner = self.inner.lock().await;
    if let Some(task) = inner.tasks.get_mut(task_id) {
      task.title = Some(title);
    }
  }

  /// 设置视频分辨率
  async fn set_task_resolution(&self, task_id: &str, resolution: String) {
    let mut inner = self.inner.lock().await;
    if let Some(task) = inner.tasks.get_mut(task_id) {
      task.resolution = Some(resolution);
    }
  }

  /// 设置任务状态文本（解析阶段反馈）
  async fn set_task_status_text(&self, task_id: &str, text: String) {
    let mut inner = self.inner.lock().await;
    if let Some(task) = inner.tasks.get_mut(task_id) {
      task.status_text = Some(text);
    }
  }

  /// 更新任务进度
  async fn update_task_progress(
    &self,
    task_id: &str,
    progress: f64,
    speed_text: String,
    speed_bytes: u64,
    downloaded: u64,
    total: u64,
  ) {
    let mut inner = self.inner.lock().await;
    if let Some(task) = inner.tasks.get_mut(task_id) {
      task.progress = progress;
      task.speed_text = speed_text.clone();
      task.downloaded = downloaded;
      task.total = total;
      // 记录峰值速度
      if speed_bytes > 0 {
        let current_peak = parse_peak_speed_bytes(&task.peak_speed_text);
        if speed_bytes > current_peak {
          task.peak_speed_text = speed_text;
        }
      }
    }
  }

  /// 标记任务完成
  async fn set_task_completed(&self, task_id: &str) {
    {
      let mut inner = self.inner.lock().await;
      if let Some(task) = inner.tasks.get_mut(task_id) {
        task.status = DownloadStatus::Completed;
        task.progress = 100.0;
        task.finished_at = Some(now_secs());
      }
    }
    self.save_to_disk().await;
  }

  /// 标记任务错误
  async fn set_task_error(&self, task_id: &str, error: String) {
    {
      let mut inner = self.inner.lock().await;
      if let Some(task) = inner.tasks.get_mut(task_id) {
        task.status = DownloadStatus::Error;
        // 截断超长错误（yt-dlp 的 URL 级报错可达数千字符），避免撑爆任务卡片
        let truncated = if error.chars().count() > 300 {
          format!(
            "{}…（完整错误见下载日志）",
            error.chars().take(300).collect::<String>()
          )
        } else {
          error
        };
        task.error_msg = Some(truncated);
        task.finished_at = Some(now_secs());
      }
    }
    self.save_to_disk().await;
  }

  async fn set_task_filename(&self, task_id: &str, filename: String) {
    let mut inner = self.inner.lock().await;
    if let Some(task) = inner.tasks.get_mut(task_id) {
      task.filename = Some(filename);
    }
  }

  /// 设置任务总大小
  async fn set_task_total(&self, task_id: &str, total: u64) {
    let mut inner = self.inner.lock().await;
    if let Some(task) = inner.tasks.get_mut(task_id) {
      task.total = total;
      task.downloaded = total;
    }
  }

  /// 检查并启动下一个等待中的任务
  pub async fn try_start_next<R: Runtime>(&self, app: &tauri::AppHandle<R>) {
    let (yt_dlp, settings, task_id, task_url) = {
      let mut inner = self.inner.lock().await;

      let downloading_count = inner
        .tasks
        .values()
        .filter(|t| t.status == DownloadStatus::Downloading)
        .count();

      let max = inner.settings.max_concurrent;
      if max > 0 && downloading_count >= max as usize {
        return;
      }

      // 只找 waiting 状态的任务，paused 的任务不会自动重启（只能手动继续）
      let waiting = inner
        .tasks
        .values()
        .find(|t| t.status == DownloadStatus::Waiting)
        .map(|t| (t.id.clone(), t.url.clone()));

      let (task_id, task_url) = match waiting {
        Some(t) => t,
        None => return,
      };

      // 标记为 downloading
      if let Some(task) = inner.tasks.get_mut(&task_id) {
        task.status = DownloadStatus::Downloading;
        task.started_at = Some(now_secs());
      }

      // 通知前端状态变了
      let _ = app.emit("video-download:task-started", &task_id);

      (
        inner.yt_dlp_path.clone(),
        inner.settings.clone(),
        task_id,
        task_url,
      )
    };

    let yt_dlp = yt_dlp.unwrap_or_else(|| {
      let dir = tools_dir();
      let local = dir.join(yt_dlp_filename());
      if local.exists() {
        local
      } else {
        PathBuf::from("yt-dlp")
      }
    });
    let app_clone = app.clone();
    let self_arc = get_downloader().clone();
    let task_id_clone = task_id.clone();

    tauri::async_runtime::spawn(async move {
      let is_youtube = task_url.contains("youtube.com") || task_url.contains("youtu.be");
      let max_attempts = if is_youtube { 6u32 } else { 3u32 };
      let mut last_error: Option<String> = None;
      let mut cookie_refreshed = false;

      for attempt in 0..max_attempts {
        let force_cookie = if attempt > 0
          && settings.cookie_from_browser
          && !cookie_refreshed
          && is_youtube
          && last_error.as_deref().map(is_cookie_error).unwrap_or(false)
        {
          cookie_refreshed = true;
          true
        } else {
          false
        };

        if attempt > 0 {
          log::warn!(
            "[video_download] 第 {} 次重试: {} (策略: {})",
            attempt,
            task_url,
            if is_youtube {
              match attempt {
                1 => "web+mweb",
                2 => "web",
                3 => "mweb",
                4 => "tv",
                5 => "web_embedded",
                _ => "默认",
              }
            } else {
              "默认"
            }
          );
        }

        let result = run_single_download(
          &yt_dlp,
          &task_id_clone,
          &task_url,
          &settings,
          &self_arc,
          &app_clone,
          force_cookie,
          attempt,
        )
        .await;

        match result {
          Ok(()) => {
            last_error = None;
            break;
          }
          Err(e) => {
            // 检查是否被暂停了，如果是就跳出循环
            let is_paused = {
              let inner = self_arc.inner.lock().await;
              inner
                .tasks
                .get(&task_id_clone)
                .map(|t| t.status == DownloadStatus::Paused)
                .unwrap_or(false)
            };
            if is_paused {
              last_error = None;
              break;
            }
            last_error = Some(e);

            // 还有重试机会，提示用户
            if attempt + 1 < max_attempts {
              let retry_msg = format!("下载失败，正在第 {} 次重试...", attempt + 1);
              self_arc
                .set_task_status_text(&task_id_clone, retry_msg.clone())
                .await;
              let _ = app_clone.emit(
                "video-download:status",
                &serde_json::json!({
                  "taskId": task_id_clone,
                  "statusText": retry_msg,
                  "status": "downloading"
                }),
              );
            }
          }
        }
      }

      // 注销 PID
      self_arc.unregister_pid(&task_id_clone).await;

      match last_error {
        None => {
          // 成功了（或者暂停了都不算错误）
          let is_paused = {
            let inner = self_arc.inner.lock().await;
            inner
              .tasks
              .get(&task_id_clone)
              .map(|t| t.status == DownloadStatus::Paused)
              .unwrap_or(false)
          };
          if !is_paused {
            self_arc.set_task_completed(&task_id_clone).await;
            // 发送完整任务数据，前端直接更新
            let task_data = {
              let inner = self_arc.inner.lock().await;
              inner.tasks.get(&task_id_clone).cloned()
            };
            if let Some(task) = task_data {
              let _ = app_clone.emit("video-download:task-completed", &task);
            }
          }
        }
        Some(mut e) => {
          // 如果自动刷新 cookie 重试过还是失败，加上提示
          if cookie_refreshed {
            e = format!(
              "{} (已自动刷新 Cookie 重试仍失败，请确认浏览器已登录 YouTube)",
              e
            );
          }
          self_arc.set_task_error(&task_id_clone, e).await;
          let task_data = {
            let inner = self_arc.inner.lock().await;
            inner.tasks.get(&task_id_clone).cloned()
          };
          if let Some(task) = task_data {
            let _ = app_clone.emit("video-download:task-error", &task);
          }
        }
      }

      // 持久化
      self_arc.save_to_disk().await;

      // 通知启动下一个下载任务（通过事件打破递归 async fn 的 Send 推断循环）
      let _ = app_clone.emit("video-download:try-start-next", ());
    });
  }
}

/// 云端解析分享链接（bwbrowser_resolve.php）：解析逻辑集中在服务端，
/// 平台改版时改 PHP 即可，客户端不用发版。登录用户凭据做鉴权。
async fn resolve_share_url_via_cloud(share_url: &str) -> Result<(String, Option<String>), String> {
  use crate::bwbrowser_cloud::BWBROWSER_AUTH;
  use crate::cloud_domain::BWBROWSER_API_URL;

  let Some((username, password)) = BWBROWSER_AUTH.get_credentials() else {
    return Err("未登录 BW Browser 账号，无法使用云端解析".to_string());
  };

  let form = format!(
    "action=resolve&username={}&password={}&url={}",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    crate::bwbrowser_cloud::urlencode(share_url),
  );

  let resolve_url = BWBROWSER_API_URL.replace("bwbrowser_sync.php", "bwbrowser_resolve.php");
  let resp = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(40))
    .build()
    .map_err(|e| e.to_string())?
    .post(&resolve_url)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form)
    .send()
    .await
    .map_err(|e| format!("云端解析请求失败: {e}"))?;

  let status = resp.status();
  let body = resp.text().await.map_err(|e| e.to_string())?;
  let v: serde_json::Value =
    serde_json::from_str(&body).map_err(|_| format!("云端解析返回非 JSON（HTTP {status}）"))?;

  if v["success"].as_bool() != Some(true) {
    let msg = v["message"].as_str().unwrap_or("云端解析失败").to_string();
    return Err(msg);
  }
  let play_url = v["data"]["play_url"]
    .as_str()
    .ok_or("云端解析返回缺少 play_url")?
    .to_string();
  let title = v["data"]["title"].as_str().map(|s| s.to_string());
  log::info!(
    "[video_download] 云端解析成功: source={} url_len={}",
    v["data"]["source"].as_str().unwrap_or("?"),
    play_url.len()
  );
  Ok((play_url, title))
}

/// 运行单个 yt-dlp 下载
/// 解析红果短剧/漫剧分享页，返回 MP4 直链。
/// 已知入口（均重定向到同系分享页，移动端 UA 才返回含 play_url 的数据）：
///   - novelquickapp.com/s/xxx （短剧）
///   - kylin.hainanyuyue.com/s/xxx 等 hainanyuyue.com 域（漫剧，302 → hainanyuyue.com/share/...）
///
/// 数据载体两种格式：
///   - novelquickapp：React 流式块里 `&quot;play_url&quot;:&quot;...&quot;`（HTML 实体转义）
///   - hainanyuyue：`window._ROUTER_DATA = {...}` 里未转义 JSON `"play_url":"...\u002F..."`
///
/// 直链带签名有时效，拿到后立即下载。
/// wco.tv/wcostream 解析脚本（编译期嵌入）：curl_cffi 模拟 Chrome TLS 过
/// Cloudflare，四步链路拿真实 CDN mp4 直链。stdout 单行 JSON。
const WCO_RESOLVER_PY: &str = include_str!("wco_resolver.py");

/// wco 视频解析：本地托管 Python（faster-whisper 同环境）跑解析脚本，
/// 返回 (直链, 标题)。失败返回可读错误。
async fn resolve_wco_url(page_url: &str) -> Result<(String, String), String> {
  use tokio::io::AsyncWriteExt;

  let python = crate::ai_rewrite::local_python_path();
  let mut cmd = tokio::process::Command::new(&python);
  cmd
    .arg("-")
    .arg(page_url)
    .env("PYTHONIOENCODING", "utf-8")
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  let mut child = cmd.spawn().map_err(|e| {
    format!(
      "启动 Python（{python}）失败：{e}；解析 wco 需要 Python 环境（AI 洗稿本地转写已配置过）"
    )
  })?;
  if let Some(mut stdin) = child.stdin.take() {
    let _ = stdin.write_all(WCO_RESOLVER_PY.as_bytes()).await;
    drop(stdin);
  }
  let output = child
    .wait_with_output()
    .await
    .map_err(|e| format!("等待解析进程失败：{e}"))?;

  let stdout = String::from_utf8_lossy(&output.stdout);
  // 取最后一行 JSON（脚本只输出一行，防 stderr 混入）
  let line = stdout
    .lines()
    .rev()
    .find(|l| l.trim_start().starts_with('{'))
    .ok_or_else(|| format!("解析器无有效输出：{}", &stdout[..stdout.len().min(120)]))?;

  let v: serde_json::Value =
    serde_json::from_str(line).map_err(|e| format!("解析器输出异常：{e}"))?;
  if v["ok"].as_bool() != Some(true) {
    return Err(v["error"].as_str().unwrap_or("未知错误").to_string());
  }
  let url = v["url"]
    .as_str()
    .ok_or_else(|| "解析结果缺少 url".to_string())?
    .to_string();
  let title = v["title"].as_str().unwrap_or("wco_video").to_string();
  Ok((url, title))
}

async fn resolve_hongguo_share_url(share_url: &str) -> Result<String, String> {
  let resp = reqwest::Client::builder()
    .user_agent("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36")
    .timeout(std::time::Duration::from_secs(30))
    .build()
    .map_err(|e| e.to_string())?
    .get(share_url)
    .send()
    .await
    .map_err(|e| format!("请求分享页失败: {e}"))?;

  if !resp.status().is_success() {
    return Err(format!("分享页返回 HTTP {}", resp.status()));
  }
  let html = resp.text().await.map_err(|e| e.to_string())?;
  log::info!("[video_download] 红果分享页抓取完成: {} bytes", html.len());

  // 两种转义格式都尝试；取第一个 http 开头的直链
  for m in PLAY_URL_RES.iter().flat_map(|re| re.captures_iter(&html)) {
    let raw = m.get(1).map(|g| g.as_str()).unwrap_or("");
    let url = raw
      .replace("&amp;", "&")
      .replace("\\u002F", "/")
      .replace("\\/", "/");
    if url.starts_with("http") {
      return Ok(url);
    }
  }
  Err("页面中没有找到视频直链（可能分享已过期）".to_string())
}

static PLAY_URL_RES: [once_cell::sync::Lazy<regex_lite::Regex>; 2] = [
  // 格式 1：HTML 实体转义（novelquickapp React 流式块）
  once_cell::sync::Lazy::new(|| {
    regex_lite::Regex::new("&quot;play_url&quot;:&quot;(.*?)&quot;").expect("valid regex")
  }),
  // 格式 2：未转义 JSON，值里用 \u002F 表示斜杠（hainanyuyue _ROUTER_DATA）
  once_cell::sync::Lazy::new(|| {
    regex_lite::Regex::new(r#""play_url"\s*:\s*"([^"]+)""#).expect("valid regex")
  }),
];

async fn run_single_download<R: Runtime>(
  yt_dlp: &PathBuf,
  task_id: &str,
  url: &str,
  settings: &DownloadSettings,
  dl: &VideoDownloader,
  app: &tauri::AppHandle<R>,
  force_refresh_cookie: bool,
  attempt: u32,
) -> Result<(), String> {
  // 最先创建日志文件，确保任何阶段都有迹可循
  let log_dir = tools_dir().join("logs");
  let _ = std::fs::create_dir_all(&log_dir);
  let log_path = log_dir.join(format!("download_{}.log", task_id));
  let start_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
  let _ = std::fs::write(
    &log_path,
    format!("[{}] 任务启动\nURL: {}\n", start_time, url),
  );

  let output_dir = if settings.download_dir.is_empty() {
    default_download_dir()
  } else {
    PathBuf::from(&settings.download_dir)
  };

  // 确保目录存在
  let _ = std::fs::create_dir_all(&output_dir);

  // 阶段 1：正在准备
  dl.set_task_status_text(task_id, "正在准备下载...".to_string())
    .await;
  let _ = app.emit(
    "video-download:status",
    &serde_json::json!({
      "taskId": task_id,
      "statusText": "正在准备下载...",
      "status": "downloading"
    }),
  );

  // 临时信息文件（yt-dlp 把标题/分辨率等信息写入这里，避免和 stdout 的进度输出混在一起）
  let info_file = tools_dir().join(format!("info_{}.txt", task_id));
  let _ = std::fs::remove_file(&info_file); // 清理旧文件

  let mut args: Vec<String> = vec![
    "--no-playlist".to_string(),
    "--geo-bypass".to_string(),
    "--newline".to_string(), // 强制进度输出用换行符，方便逐行读取
    "--concurrent-fragments".to_string(),
    "4".to_string(),
    "--retries".to_string(),
    "10".to_string(),
    "--fragment-retries".to_string(),
    "10".to_string(),
    "--extractor-retries".to_string(),
    "3".to_string(),
    "--file-access-retries".to_string(),
    "3".to_string(),
    "--socket-timeout".to_string(),
    "30".to_string(),
    "--merge-output-format".to_string(),
    "mp4".to_string(),
    "--windows-filenames".to_string(),
    "--format-sort".to_string(),
    "res,fps,hdr:12,vcodec,acodec,br".to_string(),
    "--format-sort-force".to_string(),
    "--add-header".to_string(),
    "User-Agent:Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
    "--add-header".to_string(),
    "Accept-Language:zh-CN,zh;q=0.9,en;q=0.8".to_string(),
    "-P".to_string(),
    output_dir.to_string_lossy().to_string(),
    // 把视频信息写入文件，避免和 stdout 进度输出混在一起
    "--print-to-file".to_string(),
    "before_dl:BW_INFO_TITLE:%(title)s".to_string(),
    info_file.to_string_lossy().to_string(),
    "--print-to-file".to_string(),
    "before_dl:BW_INFO_RESOLUTION:%(resolution)s".to_string(),
    info_file.to_string_lossy().to_string(),
    "--print-to-file".to_string(),
    "before_dl:BW_INFO_UPLOADER:%(uploader|Unknown)s".to_string(),
    info_file.to_string_lossy().to_string(),
    "--print-to-file".to_string(),
    "before_dl:BW_INFO_THUMBNAIL:%(thumbnail)s".to_string(),
    info_file.to_string_lossy().to_string(),
  ];

  // 分享链接（yt-dlp 无提取器的平台）：先经云端解析出直链再下载。
  // 是否支持由云端路由判断（支持红果系/腾讯火龙漫剧等），客户端不再维护
  // 平台域名清单——云端加了新平台，客户端无需发版。本地兜底只覆盖红果系。
  let is_hongguo = url.contains("novelquickapp.com/s/")
    || url.contains("hainanyuyue.com/s/")
    || url.contains("hainanyuyue.com/share/");
  // wco.tv / wcostream 系列：Cloudflare + 自研混淆，yt-dlp 过不了；
  // 走本地 Python 解析器（curl_cffi 模拟 Chrome TLS）拿真实 CDN 直链
  let is_wco = url.contains("wco.tv/")
    || url.contains("wcostream.")
    || url.contains("wcofun.")
    || url.contains("wcostream.org/");
  // 云端候选：红果系 + 已接入云端的其他平台（qq.com 漫剧分享页）
  let is_cloud_candidate = is_hongguo
    || (url.contains("qq.com/") && (url.contains("/kairos/share/") || url.contains("manju")));
  let mut hongguo_fixed_name: Option<String> = None;
  let url: String = if is_wco {
    // wco 系列：本地 Python 解析器（curl_cffi 模拟 Chrome TLS 过 Cloudflare）。
    // 直链有时效且绑定会话，解析后立即下载，不能当作普通 URL 复用。
    dl.set_task_status_text(task_id, "解析 wco 视频源...".to_string())
      .await;
    match resolve_wco_url(url).await {
      Ok((direct, title)) => {
        log::info!("[video_download] wco 解析成功（title={title:?}）");
        dl.set_task_status_text(task_id, "解析成功，开始下载...".to_string())
          .await;
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let base = title
          .trim()
          .chars()
          .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
              '_'
            } else {
              c
            }
          })
          .collect::<String>();
        hongguo_fixed_name = Some(format!("{}_{}.%(ext)s", base, stamp));
        direct
      }
      Err(e) => {
        log::warn!("[video_download] wco 解析失败: {e}");
        return Err(format!("wco 视频解析失败：{e}"));
      }
    }
  } else if is_cloud_candidate {
    dl.set_task_status_text(task_id, "解析分享链接...".to_string())
      .await;
    // 云端优先：解析逻辑在服务端，平台改版不用发客户端；失败回退本地解析
    let resolved = match resolve_share_url_via_cloud(url).await {
      Ok((direct, cloud_title)) => {
        log::info!("[video_download] 云端解析成功（title={cloud_title:?}）");
        Ok((direct, cloud_title))
      }
      Err(cloud_err) => {
        // 本地兜底只有红果系解析；其他平台（如腾讯漫剧）没有本地实现，
        // 直接把云端错误透传，避免产生误导性的 yt-dlp Unsupported URL
        if !is_hongguo {
          return Err(format!("云端解析失败：{cloud_err}"));
        }
        log::warn!("[video_download] 云端解析失败，回退本地解析: {cloud_err}");
        dl.set_task_status_text(task_id, "云端解析不可用，使用本地解析...".to_string())
          .await;
        resolve_hongguo_share_url(url).await.map(|d| (d, None))
      }
    };
    match resolved {
      Ok((direct, cloud_title)) => {
        log::info!(
          "[video_download] 分享链接解析成功: {} -> direct ({} bytes)",
          url,
          direct.len()
        );
        dl.set_task_status_text(task_id, "解析成功，开始下载...".to_string())
          .await;
        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        // 有云端标题用标题做文件名（云端能拿到时更友好），否则退回 URL 主干
        let base = cloud_title
          .filter(|t| !t.trim().is_empty())
          .map(|t| {
            t.trim()
              .chars()
              .map(|c| {
                if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                  '_'
                } else {
                  c
                }
              })
              .collect::<String>()
          })
          .unwrap_or_else(|| {
            std::path::Path::new(url)
              .file_stem()
              .map(|n| n.to_string_lossy().to_string())
              .unwrap_or_else(|| "hongguo".to_string())
              .split(['?', '&'])
              .next()
              .unwrap_or("hongguo")
              .to_string()
          });
        hongguo_fixed_name = Some(format!("{}_{}.%(ext)s", base, stamp));
        direct
      }
      Err(e) => {
        log::warn!("[video_download] 分享链接解析失败: {e}");
        return Err(format!("短剧链接解析失败：{e}"));
      }
    }
  } else {
    url.to_string()
  };
  let url: &str = url.as_str();

  // 输出文件名：红果直链无标题，URL 尾巴含非法字符，必须用固定名
  args.push("-o".to_string());
  args.push(
    hongguo_fixed_name
      .unwrap_or_else(|| "%(uploader|Unknown)s_%(title).80B_%(id)s.%(ext)s".to_string()),
  );

  // 代理
  if let Some(proxy_url) = build_proxy_url(&settings.proxy_id) {
    args.push("--proxy".to_string());
    args.push(proxy_url);
  }

  // 代理信息 + 完整命令 写入日志
  let proxy_info = match build_proxy_url(&settings.proxy_id) {
    Some(url) => format!("代理: {}\n", url),
    None => "代理: 直连（不使用代理）\n".to_string(),
  };
  let log_content = format!(
    "[{}] 开始下载\n{}\n命令: {} {}\n\n",
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
    proxy_info,
    yt_dlp.to_string_lossy(),
    args.join(" ")
  );
  let _ = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)
    .and_then(|mut f| {
      use std::io::Write;
      f.write_all(log_content.as_bytes())
    });

  // 分辨率
  if settings.max_height > 0 {
    args.push("-f".to_string());
    args.push(format!(
      "bv*[height<={}]+ba/b[height<={}]/best[height<={}]",
      settings.max_height, settings.max_height, settings.max_height
    ));
  } else {
    args.push("-f".to_string());
    args.push("bv*+ba/bestvideo+bestaudio/best".to_string());
  }

  // YouTube 专用 header + player_client（根据重试次数换策略）
  let is_youtube = url.contains("youtube.com") || url.contains("youtu.be");
  let is_douyin = url.contains("douyin.com") || url.contains("v.douyin.com");
  if is_youtube {
    args.push("--add-header".to_string());
    args.push("Referer:https://www.youtube.com/".to_string());
    args.push("--add-header".to_string());
    args.push("Origin:https://www.youtube.com".to_string());

    // attempt 0: 默认（不传 player_client，让 yt-dlp 自己选）
    // attempt 1: web + mweb
    // attempt 2: web
    // attempt 3: mweb
    // attempt 4: tv
    // attempt 5: web_embedded
    let player_clients = match attempt {
      1 => "web,mweb",
      2 => "web",
      3 => "mweb",
      4 => "tv",
      5 => "web_embedded",
      _ => "",
    };
    if !player_clients.is_empty() {
      args.push("--extractor-args".to_string());
      args.push(format!("youtube:player_client={}", player_clients));
    }
  }

  // 抖音：重试时降级格式 + 设置 Referer（借鉴爆文库）
  if is_douyin && attempt > 0 {
    // 降级为 best/bv*+ba
    if let Some(f_idx) = args.iter().position(|a| a == "-f") {
      args[f_idx + 1] = "best/bv*+ba".to_string();
    }
    args.push("--add-header".to_string());
    args.push("Referer:https://www.douyin.com/".to_string());
  }

  // 抖音：加载签名插件（官方提取器缺少 x-secsdk-web-signature，固定 403）
  if is_douyin {
    match ensure_douyin_plugin() {
      Ok(dir) => {
        args.push("--plugin-dirs".to_string());
        args.push(dir.to_string_lossy().to_string());
      }
      Err(e) => log::warn!("[video_download] 抖音签名插件准备失败: {}", e),
    }
  }

  // ffmpeg 路径
  let ffmpeg_path = tools_dir().join(ffmpeg_filename());
  if ffmpeg_path.exists() {
    args.push("--ffmpeg-location".to_string());
    args.push(ffmpeg_path.to_string_lossy().to_string());
  }

  // cookie - 从浏览器导出
  let mut cookie_error: Option<String> = None;
  let is_tiktok = url.contains("tiktok.com");
  // TikTok 有专用 API 提取器，不需要 cookie
  // 抖音的访客 cookie 与请求签名都由插件自行注册，跳过浏览器 cookie 导出：
  // 等待 CDP 必然超时（浏览器运行时独占 Cookie 库），而过期的 cookie 文件
  // 会让接口只返回带水印的低清格式。
  if settings.cookie_from_browser && !is_tiktok && !is_douyin {
    dl.set_task_status_text(task_id, "正在提取浏览器 Cookie...".to_string())
      .await;
    let _ = app.emit(
      "video-download:status",
      &serde_json::json!({
        "taskId": task_id,
        "statusText": "正在提取浏览器 Cookie...",
        "status": "downloading"
      }),
    );
    match export_vps_cookies_to_file(force_refresh_cookie, dl, app, Some(url)).await {
      Ok(cookie_file) => {
        // 检查 cookie 文件是否包含对应平台的 cookie
        let cookie_content = std::fs::read_to_string(&cookie_file).unwrap_or_default();
        let lower_url = url.to_lowercase();
        let needs_douyin = lower_url.contains("douyin.com") || lower_url.contains("v.douyin.com");
        let needs_tiktok = lower_url.contains("tiktok.com");
        let needs_bilibili = lower_url.contains("bilibili.com") || lower_url.contains("b23.tv");
        let needs_youtube = lower_url.contains("youtube.com") || lower_url.contains("youtu.be");

        let has_platform_cookie = (needs_douyin
          && cookie_content.to_lowercase().contains("douyin.com"))
          || (needs_tiktok && cookie_content.to_lowercase().contains("tiktok.com"))
          || (needs_bilibili && cookie_content.to_lowercase().contains("bilibili.com"))
          || (needs_youtube
            && (cookie_content.to_lowercase().contains("youtube.com")
              || cookie_content.to_lowercase().contains("google.com")))
          || (!needs_douyin && !needs_tiktok && !needs_bilibili && !needs_youtube);

        if has_platform_cookie {
          log::info!(
            "[video_download] 使用浏览器 cookie: {}",
            cookie_file.display()
          );
          args.push("--cookies".to_string());
          args.push(cookie_file.to_string_lossy().to_string());
          dl.set_task_status_text(task_id, "Cookie 提取完成，正在启动下载...".to_string())
            .await;
          let _ = app.emit(
            "video-download:status",
            &serde_json::json!({
              "taskId": task_id,
              "statusText": "Cookie 提取完成，正在启动下载...",
              "status": "downloading"
            }),
          );
        } else {
          log::info!(
            "[video_download] Cookie 文件无对应平台 cookie，跳过 --cookies，让 yt-dlp 自行获取访客 cookie"
          );
          dl.set_task_status_text(task_id, "无平台 Cookie，yt-dlp 自动获取...".to_string())
            .await;
          let _ = app.emit(
            "video-download:status",
            &serde_json::json!({
              "taskId": task_id,
              "statusText": "无平台 Cookie，yt-dlp 自动获取...",
              "status": "downloading"
            }),
          );
        }
      }
      Err(e) => {
        log::warn!("[video_download] Cookie 导出失败: {}", e);
        cookie_error = Some(e);
        dl.set_task_status_text(task_id, "Cookie 提取失败，继续下载...".to_string())
          .await;
        let _ = app.emit(
          "video-download:status",
          &serde_json::json!({
            "taskId": task_id,
            "statusText": "Cookie 提取失败，继续下载...",
            "status": "downloading"
          }),
        );
      }
    }
  }

  args.push(url.to_string());

  // 记录完整命令到日志（所有参数加完之后）
  let _ = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)
    .and_then(|mut f| {
      use std::io::Write;
      writeln!(
        f,
        "完整命令: {} {}\n",
        yt_dlp.to_string_lossy(),
        args.join(" ")
      )
    });

  if let Some(ce) = &cookie_error {
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&log_path)
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(f, "Cookie 状态: {}", ce)
      });
  }

  log::info!(
    "[video_download] 启动下载: {} (yt-dlp: {})",
    url,
    yt_dlp.display()
  );

  // 诊断：先把路径信息写到日志
  {
    let exists = yt_dlp.exists();
    let args_str = args.join(" ");
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&log_path)
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(f, "\n[诊断] yt-dlp 路径: {}", yt_dlp.display())?;
        writeln!(f, "[诊断] 文件是否存在: {}", exists)?;
        writeln!(f, "[诊断] 完整命令: {} {}", yt_dlp.display(), args_str)?;
        writeln!(f, "[诊断] 准备 spawn 进程...")?;
        Ok(())
      });
  }

  // 阶段 3：正在启动 yt-dlp
  dl.set_task_status_text(task_id, "正在启动下载器...".to_string())
    .await;
  let _ = app.emit(
    "video-download:status",
    &serde_json::json!({
      "taskId": task_id,
      "statusText": "正在启动下载器...",
      "status": "downloading"
    }),
  );

  // 抖音与其他平台统一走 yt-dlp CLI；抖音请求签名由上面的插件补上
  let mut yt_cmd = {
    let mut cmd = std::process::Command::new(yt_dlp);
    cmd
      .args(args)
      .env("PYTHONIOENCODING", "utf-8")
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::piped());
    #[cfg(target_os = "windows")]
    {
      use std::os::windows::process::CommandExt;
      cmd.creation_flags(0x08000000);
    }
    cmd
  };
  // 记录下载开始时刻：兜底挑文件时只认本任务期间新出现/改动的文件，
  // 防止把目录里以前任务的旧视频当成本次下载的结果。
  let download_started = std::time::SystemTime::now() - std::time::Duration::from_secs(5);
  let mut child = match yt_cmd.spawn() {
    Ok(c) => c,
    Err(e) => {
      let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut f| {
          use std::io::Write;
          writeln!(f, "[诊断] spawn 失败: {}", e)?;
          Ok(())
        });
      return Err(format!("启动 yt-dlp 失败: {}", e));
    }
  };

  // 注册 PID，用于暂停/取消时 kill
  let pid = child.id();
  dl.register_pid(task_id, pid).await;

  // 写启动日志
  let _ = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)
    .and_then(|mut f| {
      use std::io::Write;
      writeln!(f, "[诊断] 进程已启动，PID: {}", pid)?;
      writeln!(f, "[诊断] 等待进程完成 (wait_with_output)...")?;
      Ok(())
    });

  // 实时读取输出：两个线程分别读 stdout/stderr，通过 channel 发回
  use std::io::{BufRead, BufReader};

  let child_stdout = child.stdout.take().expect("stdout not piped");
  let child_stderr = child.stderr.take().expect("stderr not piped");

  let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();

  // stdout 读取线程
  let tx_out = line_tx.clone();
  let log_path_out = log_path.clone();
  std::thread::spawn(move || {
    let mut reader = BufReader::new(child_stdout);
    let mut total_bytes: usize = 0;
    let mut line_count: usize = 0;
    let mut buf: Vec<u8> = Vec::new();
    loop {
      buf.clear();
      match reader.read_until(b'\n', &mut buf) {
        Ok(0) => break, // EOF
        Ok(n) => {
          total_bytes += n;
          // 去掉末尾的 \n 和 \r
          while buf.ends_with(b"\n") || buf.ends_with(b"\r") {
            buf.pop();
          }
          let line = String::from_utf8_lossy(&buf).to_string();
          if !line.is_empty() {
            line_count += 1;
            let _ = tx_out.send((line, "stdout".to_string()));
          }
        }
        Err(e) => {
          let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_out)
            .and_then(|mut f| {
              use std::io::Write;
              writeln!(f, "[诊断] stdout 读取错误: {}", e)?;
              Ok(())
            });
          break;
        }
      }
    }
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&log_path_out)
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(
          f,
          "[诊断] stdout 线程结束，共 {} 行，{} 字节",
          line_count, total_bytes
        )?;
        Ok(())
      });
  });

  // stderr 读取线程
  let tx_err = line_tx.clone();
  let log_path_err = log_path.clone();
  std::thread::spawn(move || {
    let mut reader = BufReader::new(child_stderr);
    let mut total_bytes: usize = 0;
    let mut line_count: usize = 0;
    let mut buf: Vec<u8> = Vec::new();
    loop {
      buf.clear();
      match reader.read_until(b'\n', &mut buf) {
        Ok(0) => break, // EOF
        Ok(n) => {
          total_bytes += n;
          while buf.ends_with(b"\n") || buf.ends_with(b"\r") {
            buf.pop();
          }
          let line = String::from_utf8_lossy(&buf).to_string();
          if !line.is_empty() {
            line_count += 1;
            let _ = tx_err.send((line, "stderr".to_string()));
          }
        }
        Err(e) => {
          let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_err)
            .and_then(|mut f| {
              use std::io::Write;
              writeln!(f, "[诊断] stderr 读取错误: {}", e)?;
              Ok(())
            });
          break;
        }
      }
    }
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&log_path_err)
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(
          f,
          "[诊断] stderr 线程结束，共 {} 行，{} 字节",
          line_count, total_bytes
        )?;
        Ok(())
      });
  });

  // 丢掉我们自己的 sender，两个 reader 都结束后 channel 自然关闭
  drop(line_tx);

  let mut stdout_lines: Vec<String> = Vec::new();
  let mut stderr_lines: Vec<String> = Vec::new();
  let mut completed = false;
  let mut error_output = String::new();

  // 异步循环：接收行 + 更新状态
  loop {
    tokio::select! {
      Some((line, source)) = line_rx.recv() => {
        let trimmed = line.trim();
        if trimmed.is_empty() {
          continue;
        }

        // 收集
        if source == "stdout" {
          stdout_lines.push(trimmed.to_string());
        } else {
          stderr_lines.push(trimmed.to_string());
        }

        // ERROR 行
        if trimmed.starts_with("ERROR:") {
          error_output.push_str(trimmed);
          error_output.push('\n');
        }

        // 解析阶段反馈（[youtube]、[info] 等非下载/合并阶段的输出）
        if trimmed.starts_with("[") && !trimmed.starts_with("[download]")
          && !trimmed.starts_with("[Merger]")
          && !trimmed.starts_with("[ExtractAudio]")
          && !trimmed.starts_with("[debug]")
        {
          if let Some(status_text) = extract_status_text(trimmed) {
            let display_text = if attempt > 0 {
              format!("{} (第 {} 次重试)", status_text, attempt)
            } else {
              status_text
            };
            dl.set_task_status_text(task_id, display_text.clone()).await;
            let event = serde_json::json!({
              "taskId": task_id,
              "statusText": display_text,
              "status": "downloading"
            });
            let _ = app.emit("video-download:status", &event);
          }
        }

        // 进度行：[download]  23.9% of   81.01MiB at   11.11MiB/s ETA 00:05
        if trimmed.starts_with("[download]") && trimmed.contains("% of") {
          if let Some((percent, total, speed)) = parse_download_line(trimmed) {
            let downloaded = if total > 0 {
              (total as f64 * percent / 100.0) as u64
            } else {
              0
            };
            let speed_text = format_size_per_sec(speed);
            dl.update_task_progress(task_id, percent, speed_text.clone(), speed, downloaded, total).await;
            // 读取峰值速度
            let peak_text = {
              let inner = dl.inner.lock().await;
              inner.tasks.get(task_id).map(|t| t.peak_speed_text.clone()).unwrap_or_default()
            };
            let event = serde_json::json!({
              "taskId": task_id,
              "progress": percent,
              "speedText": speed_text,
              "peakSpeedText": peak_text,
              "downloaded": downloaded,
              "total": total,
              "status": "downloading"
            });
            let _ = app.emit("video-download:progress", &event);
          }
        }

        // 文件名（Destination）
        if trimmed.contains("[download] Destination:") {
          if let Some(dest) = trimmed.split("Destination:").nth(1) {
            let fname = dest.trim().trim_matches('"').to_string();
            if !fname.is_empty() {
              dl.set_task_filename(task_id, fname.clone()).await;
              // 下载开始了，尝试从 info 文件读取视频标题和分辨率
              read_info_from_file(&info_file, task_id, dl, app).await;
            }
          }
        }

        // 合并（最终文件名）
        if trimmed.starts_with("[Merger]") || trimmed.starts_with("[ExtractAudio]") {
          if let Some(merged) = trimmed
            .split("Merging formats into")
            .nth(1)
            .or_else(|| trimmed.split("Destination:").nth(1))
          {
            let fname = merged.trim().trim_matches('"').to_string();
            if !fname.is_empty() {
              dl.set_task_filename(task_id, fname).await;
            }
          }
        }

        // 已下载过
        // 格式: "[download] 文件名 has already been downloaded"
        if trimmed.contains("has already been downloaded") {
          completed = true;
          // 从行里提取文件名
          if let Some(rest) = trimmed.strip_prefix("[download]") {
            if let Some(end) = rest.find("has already been downloaded") {
              let fname = rest[..end].trim().trim_matches('"').to_string();
              if !fname.is_empty() {
                dl.set_task_filename(task_id, fname).await;
              }
            }
          }
          read_info_from_file(&info_file, task_id, dl, app).await;
        }
      }

      // channel 关闭了（两个 reader 都结束），等待进程退出
      else => {
        break;
      }
    }
  }

  // 等待进程退出并获取状态
  let status = match child.wait() {
    Ok(s) => s,
    Err(e) => {
      log::error!("[video_download] wait 失败: {}", e);
      return Err(format!("等待进程失败: {}", e));
    }
  };

  // 把完整输出写到日志
  let _ = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)
    .and_then(|mut f| {
      use std::io::Write;
      writeln!(f, "\n[诊断] 进程退出，退出码: {:?}", status.code())?;
      writeln!(f, "[诊断] stdout 行数: {}", stdout_lines.len())?;
      writeln!(f, "[诊断] stderr 行数: {}", stderr_lines.len())?;
      if !stdout_lines.is_empty() {
        writeln!(f, "\n--- stdout ---")?;
        for line in &stdout_lines {
          writeln!(f, "{}", line)?;
        }
        writeln!(f, "--- stdout end ---")?;
      }
      if !stderr_lines.is_empty() {
        writeln!(f, "\n--- stderr ---")?;
        for line in &stderr_lines {
          writeln!(f, "{}", line)?;
        }
        writeln!(f, "--- stderr end ---")?;
      }
      Ok(())
    });

  let stdout_output = stdout_lines.join("\n");

  if status.success() || completed {
    log::info!("[video_download] 下载完成: {}", url);

    // 最后再读一次 info 文件，确保拿到标题和分辨率
    read_info_from_file(&info_file, task_id, dl, app).await;
    // 清理临时 info 文件
    let _ = std::fs::remove_file(&info_file);

    // 读取实际文件大小（从 filename 反查）
    let inner_snapshot = dl.inner.lock().await;
    let task_filename = inner_snapshot
      .tasks
      .get(task_id)
      .and_then(|t| t.filename.clone());
    let task_title = inner_snapshot
      .tasks
      .get(task_id)
      .and_then(|t| t.title.clone());
    let task_resolution = inner_snapshot
      .tasks
      .get(task_id)
      .and_then(|t| t.resolution.clone());
    drop(inner_snapshot);

    // 写详细调试日志
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(log_path.clone())
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(f, "\n[调试] 进程退出码: {:?}", status.code())?;
        writeln!(f, "[调试] 输出目录: {}", output_dir.display())?;
        writeln!(f, "[调试] 任务 filename: {:?}", task_filename)?;
        writeln!(f, "[调试] 任务 title: {:?}", task_title)?;
        writeln!(f, "[调试] 任务 resolution: {:?}", task_resolution)?;
        Ok(())
      });

    let actual_file = {
      // 优先用视频 ID 在输出目录中搜索最终文件（最可靠）
      // 支持 YouTube、B 站等能从 URL 提取 ID 的平台
      let video_id = extract_video_id(url);
      if let Some(ref id) = video_id {
        if let Some(found) = find_final_video_file(&output_dir, id) {
          Some(found)
        } else {
          // ID 搜索没找到，fallback 到 filename
          let from_filename = task_filename.as_ref().map(|fname| {
            let p = PathBuf::from(fname);
            if p.is_absolute() && p.exists() {
              p
            } else {
              output_dir.join(fname)
            }
          });
          // filename 也找不到的话，兜底：在本任务开始后新出现的视频文件里找最大的
          if from_filename.as_ref().is_none_or(|p| !p.exists()) {
            find_largest_video_file(&output_dir, download_started)
          } else {
            from_filename
          }
        }
      } else {
        // 无法提取视频 ID，先用 filename 尝试
        let from_filename = task_filename.as_ref().map(|fname| {
          let p = PathBuf::from(fname);
          if p.is_absolute() && p.exists() {
            p
          } else {
            output_dir.join(fname)
          }
        });
        // filename 路径不存在，兜底：在本任务开始后新出现的视频文件里找最大的
        if from_filename.as_ref().is_none_or(|p| !p.exists()) {
          find_largest_video_file(&output_dir, download_started)
        } else {
          from_filename
        }
      }
    };

    if let Some(file_path) = actual_file {
      let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut f| {
          use std::io::Write;
          writeln!(f, "[调试] 计算文件路径: {}", file_path.display())?;
          writeln!(f, "[调试] 文件是否存在: {}", file_path.exists())?;
          Ok(())
        });

      if file_path.exists() {
        if let Ok(meta) = std::fs::metadata(&file_path) {
          dl.set_task_total(task_id, meta.len()).await;
          // 把 filename 统一为绝对路径，方便前端打开
          dl.set_task_filename(task_id, file_path.to_string_lossy().to_string())
            .await;
          let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| {
              use std::io::Write;
              writeln!(f, "[调试] 文件大小: {} bytes", meta.len())?;
              writeln!(f, "[调试] 最终路径: {}", file_path.display())
            });
        }
      } else {
        // 文件不存在，列出目录看看有什么
        let _ = std::fs::OpenOptions::new()
          .create(true)
          .append(true)
          .open(&log_path)
          .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "[调试] !!! 文件不存在，列出目录内容:")?;
            if let Ok(entries) = std::fs::read_dir(&output_dir) {
              for entry in entries.flatten() {
                let _ = writeln!(f, "  - {}", entry.file_name().to_string_lossy());
              }
            }
            Ok(())
          });
      }
    }

    // 标记完成
    if !completed {
      dl.set_task_completed(task_id).await;
      let event = serde_json::json!({
        "taskId": task_id,
        "progress": 100,
        "status": "completed"
      });
      let _ = app.emit("video-download:progress", &event);
    }

    // 发送完整的 task 对象到前端，确保 filename/title 等字段同步
    {
      let inner = dl.inner.lock().await;
      if let Some(task) = inner.tasks.get(task_id) {
        let _ = app.emit("video-download:task-completed", &task);
      }
    }

    // 写完成日志
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&log_path)
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(
          f,
          "\n[{}] 下载完成",
          chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        )
      });

    // 上报视频下载记录到服务器
    let final_title = task_title.clone().unwrap_or_default();
    let final_resolution = task_resolution.clone().unwrap_or_default();
    let final_size = {
      let inner = dl.inner.lock().await;
      inner.tasks.get(task_id).map(|t| t.total).unwrap_or(0)
    };
    log_video_download_to_server(
      url,
      "success",
      &final_resolution,
      final_size,
      &output_dir.to_string_lossy(),
      "",
      &final_title,
    );

    Ok(())
  } else {
    let code = status.code().unwrap_or(-1);
    let mut error_msg = extract_error_message(&error_output);

    // 如果 stdout 和 stderr 都几乎是空的，说明读取可能有问题
    if stdout_output.is_empty() && error_output.trim().is_empty() {
      error_msg = format!("下载失败 (exit {})，未读取到进程输出", code);
    }

    // 如果 cookie 导出失败，把这个信息也加上
    if let Some(ce) = cookie_error {
      error_msg = format!("{} (Cookie 不可用: {})", error_msg, ce);
    }

    log::error!(
      "[video_download] 失败 (exit {}): {} - {}",
      code,
      url,
      error_msg
    );

    // 写失败日志
    let _ = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open(&log_path)
      .and_then(|mut f| {
        use std::io::Write;
        writeln!(
          f,
          "\n[{}] 下载失败 (exit {}): {}",
          chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
          code,
          error_msg
        )
      });

    // 上报视频下载失败记录到服务器
    log_video_download_to_server(
      url,
      "failed",
      "",
      0,
      &output_dir.to_string_lossy(),
      &error_msg,
      "",
    );

    Err(error_msg)
  }
}

/// 上报视频下载记录到服务器（异步，不阻塞下载流程）
fn log_video_download_to_server(
  url: &str,
  result: &str,
  resolution: &str,
  file_size: u64,
  output_dir: &str,
  message: &str,
  title: &str,
) {
  // 异步上报，不阻塞下载
  let url = url.to_string();
  let result = result.to_string();
  let resolution = resolution.to_string();
  let output_dir = output_dir.to_string();
  let message = message.to_string();
  let title = title.to_string();

  tauri::async_runtime::spawn(async move {
    if let Some((username, password)) = crate::bwbrowser_cloud::BWBROWSER_AUTH.get_credentials() {
      let form_data = format!(
        "action=log_video_download&username={}&password={}&url={}&result={}&resolution={}&file_size={}&output_dir={}&message={}&title={}",
        urlencode(&username),
        urlencode(&password),
        urlencode(&url),
        urlencode(&result),
        urlencode(&resolution),
        file_size,
        urlencode(&output_dir),
        urlencode(&message),
        urlencode(&title),
      );

      let client = reqwest::Client::new();
      let _ = client
        .post(crate::cloud_domain::BWBROWSER_API_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_data)
        .send()
        .await;

      log::info!(
        "[video_download] 上报下载记录: url={}, result={}",
        url,
        result
      );
    }
  });
}

/// 从错误输出中提取有用的信息（只保留 ERROR 行或第一行）
fn extract_error_message(output: &str) -> String {
  for line in output.lines() {
    let line = line.trim();
    if line.starts_with("ERROR:") {
      return line.to_string();
    }
  }
  for line in output.lines() {
    let lower = line.to_lowercase();
    if lower.contains("error") || lower.contains("failed") || lower.contains("cannot") {
      return line.trim().to_string();
    }
  }
  // 跳过 WARNING，找真正的错误或最后一行非空行
  for line in output.lines() {
    let trimmed = line.trim();
    if !trimmed.is_empty() && !trimmed.starts_with("WARNING:") {
      return trimmed.to_string();
    }
  }
  "下载失败".to_string()
}

/// 判断是否为 cookie 相关错误
fn is_cookie_error(error_msg: &str) -> bool {
  let lower = error_msg.to_lowercase();
  lower.contains("page needs to be reloaded")
    || lower.contains("cookie")
    || lower.contains("sign in")
    || lower.contains("login required")
    || lower.contains("consent")
    || lower.contains("age verification")
    || lower.contains("unable to extract")
}

/// 音频下载实时回调载荷（供 AI 洗稿展示下载速度/进度/日志）。
#[derive(Debug, Clone, Default)]
pub struct AudioDlProgress {
  /// 0-100；负数表示该行只是日志，没有进度
  pub percent: f64,
  /// 可读速度文本，如 "1.23 MB/s"
  pub speed_text: String,
  pub downloaded: u64,
  pub total: u64,
  /// yt-dlp 原始输出行（便于排查问题）
  pub log: String,
}

/// 复用「视频下载」的 yt-dlp / 代理 / Cookie 逻辑，仅抽取音频。
/// 供 AI 洗稿等模块调用，避免另建一套下载系统。
/// `on_progress` 会随 yt-dlp 输出实时回调（下载速度 / 百分比 / 原始日志行）。
///
/// 策略：先按音频直取（`bestaudio` + `-x`）；若失败则回退为「视频下载」方式，
/// 先下载视频再本地用 ffmpeg 抽取音频，保证拿到可转写的音频文件。
pub async fn download_audio_for_asr<R: Runtime>(
  app: &tauri::AppHandle<R>,
  url: &str,
  workdir: &std::path::Path,
  mut on_progress: impl FnMut(AudioDlProgress) + Send,
) -> Result<PathBuf, String> {
  let url = url.trim();
  if url.is_empty() {
    return Err("视频链接为空".to_string());
  }
  let _ = std::fs::create_dir_all(workdir);

  // yt-dlp 路径：优先使用「视频下载」页已保存的路径，其次 tools_dir 内的本地二进制
  let dl = get_downloader();
  let (saved_path, settings) = {
    let inner = dl.inner.lock().await;
    (inner.yt_dlp_path.clone(), inner.settings.clone())
  };
  let yt_dlp = saved_path.filter(|p| p.exists()).unwrap_or_else(|| {
    let local = tools_dir().join(yt_dlp_filename());
    if local.exists() {
      local
    } else {
      PathBuf::from("yt-dlp")
    }
  });

  // 公共参数（音频直取 / 视频回退两种方式共用）
  let mut common: Vec<String> = vec![
    "--no-playlist".to_string(),
    "--no-warnings".to_string(),
    "--newline".to_string(),
    "--retries".to_string(),
    "10".to_string(),
    "--fragment-retries".to_string(),
    "10".to_string(),
    // TikTok 等平台偶发 403 / 连接重置，让 yt-dlp 自身也多重试页面抓取
    "--extractor-retries".to_string(),
    "10".to_string(),
    "--socket-timeout".to_string(),
    "30".to_string(),
    "--add-header".to_string(),
    "User-Agent:Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
    "--add-header".to_string(),
    "Accept-Language:zh-CN,zh;q=0.9,en;q=0.8".to_string(),
  ];

  // ffmpeg（抽取音频 / 合并音视频必需）
  let ffmpeg_path = tools_dir().join(ffmpeg_filename());
  if ffmpeg_path.exists() {
    common.push("--ffmpeg-location".to_string());
    common.push(ffmpeg_path.to_string_lossy().to_string());
  }

  // 代理：沿用「视频下载」页的代理设置
  let proxy = build_proxy_url(&settings.proxy_id);
  if let Some(ref proxy_url) = proxy {
    common.push("--proxy".to_string());
    common.push(proxy_url.clone());
  }
  log::info!(
    "[ai_rewrite] 下载音频 url={} | yt-dlp={} | proxy={} | cookie_from_browser={}",
    url,
    yt_dlp.display(),
    proxy.as_deref().unwrap_or("直连"),
    settings.cookie_from_browser
  );

  let lower = url.to_lowercase();
  let is_youtube = lower.contains("youtube.com") || lower.contains("youtu.be");
  let is_tiktok = lower.contains("tiktok.com");
  let is_douyin = lower.contains("douyin.com") || lower.contains("v.douyin.com");
  if is_youtube {
    common.push("--add-header".to_string());
    common.push("Referer:https://www.youtube.com/".to_string());
    common.push("--add-header".to_string());
    common.push("Origin:https://www.youtube.com".to_string());
  }
  if is_douyin {
    common.push("--add-header".to_string());
    common.push("Referer:https://www.douyin.com/".to_string());
  }

  // Cookie：沿用「视频下载」的浏览器 Cookie 提取逻辑（抖音/YouTube 等需要）
  if settings.cookie_from_browser && !is_tiktok {
    match export_vps_cookies_to_file(true, dl, app, Some(url)).await {
      Ok(cookie_file) => {
        common.push("--cookies".to_string());
        common.push(cookie_file.to_string_lossy().to_string());
      }
      Err(e) => {
        log::warn!("[ai_rewrite] 浏览器 Cookie 提取失败，继续尝试下载: {}", e);
      }
    }
  }

  // TikTok 等平台会按 TLS 指纹 + JS 挑战做拦截，单次请求常偶发 403 / 连接重置，
  // 因此整体重试多次；每次都是「先直取音频，失败再下载视频后本地抽音频」。
  // 实测单次成功率约 3/5，重试 6 次后失败概率极低。
  let max_attempts: usize = 6;
  let mut use_impersonate = true;
  let mut last_err = String::new();

  for attempt in 1..=max_attempts {
    if attempt > 1 {
      on_progress(AudioDlProgress {
        percent: -1.0,
        log: format!("第 {} 次尝试失败：{}；3 秒后重试", attempt - 1, last_err),
        ..Default::default()
      });
      tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // ---- 方式一：直接下载音频（bestaudio + 抽取 mp3） ----
    let mut audio_args = common.clone();
    if use_impersonate {
      push_impersonate(&mut audio_args);
    }
    audio_args.extend([
      "-f".to_string(),
      "bestaudio/best".to_string(),
      "-x".to_string(),
      "--audio-format".to_string(),
      "mp3".to_string(),
      "-o".to_string(),
      workdir.join("audio.%(ext)s").to_string_lossy().to_string(),
    ]);
    audio_args.push(url.to_string());

    let audio_err = match run_ytdlp(&yt_dlp, &audio_args, &mut on_progress).await {
      Ok(()) => match find_output(workdir, "audio.") {
        Some(found) => return Ok(found),
        None => "下载完成但未找到音频文件".to_string(),
      },
      Err(e) => {
        if e.to_lowercase().contains("impersonate") {
          use_impersonate = false;
        }
        e
      }
    };

    // ---- 方式二（回退）：按「视频下载」方式下载视频，再本地用 ffmpeg 抽音频 ----
    on_progress(AudioDlProgress {
      percent: -1.0,
      log: format!("音频下载失败：{audio_err}；改用视频方式下载后本地抽取音频"),
      ..Default::default()
    });

    let mut video_args = common.clone();
    if use_impersonate {
      push_impersonate(&mut video_args);
    }
    video_args.extend([
      "-f".to_string(),
      "bv*+ba/bestvideo+bestaudio/best".to_string(),
      "--merge-output-format".to_string(),
      "mp4".to_string(),
      "-o".to_string(),
      workdir.join("video.%(ext)s").to_string_lossy().to_string(),
    ]);
    video_args.push(url.to_string());

    match run_ytdlp(&yt_dlp, &video_args, &mut on_progress).await {
      Ok(()) => {
        if let Some(video) = find_output(workdir, "video.") {
          on_progress(AudioDlProgress {
            percent: -1.0,
            log: "视频已下载，正在用 ffmpeg 抽取音频…".to_string(),
            ..Default::default()
          });
          let audio_out = workdir.join("audio.mp3");
          extract_audio_with_ffmpeg(&video, &audio_out).await?;
          let _ = std::fs::remove_file(&video);
          return Ok(audio_out);
        }
        last_err = "视频下载完成但未找到视频文件".to_string();
      }
      Err(e) => {
        if e.to_lowercase().contains("impersonate") {
          use_impersonate = false;
        }
        last_err = format!("视频方式下载也失败：{e}（音频方式失败：{audio_err}）");
      }
    }
  }

  Err(format!("下载失败（已重试 {max_attempts} 次）：{last_err}"))
}

/// 用 yt-dlp 拉取平台原生字幕（TikTok/YouTube 自带的自动 CC），解析为逐段时间轴
/// （[{start,end,text}]，单位秒）。拿不到返回空 Vec（调用方回退语音转写），不报错。
pub async fn fetch_native_subtitles<R: Runtime>(
  app: &tauri::AppHandle<R>,
  url: &str,
  workdir: &std::path::Path,
) -> Vec<serde_json::Value> {
  let url = url.trim();
  if url.is_empty() {
    return Vec::new();
  }
  let _ = std::fs::create_dir_all(workdir);
  let dl = get_downloader();
  let (saved_path, settings) = {
    let inner = dl.inner.lock().await;
    (inner.yt_dlp_path.clone(), inner.settings.clone())
  };
  let yt_dlp = saved_path.filter(|p| p.exists()).unwrap_or_else(|| {
    let local = tools_dir().join(yt_dlp_filename());
    if local.exists() {
      local
    } else {
      PathBuf::from("yt-dlp")
    }
  });

  let mut args: Vec<String> = vec![
    "--skip-download".to_string(),
    "--write-auto-sub".to_string(),
    "--write-sub".to_string(),
    "--sub-langs".to_string(),
    ".*".to_string(),
    "--sub-format".to_string(),
    "vtt/srt/best".to_string(),
    "--no-warnings".to_string(),
    "--no-playlist".to_string(),
    "--socket-timeout".to_string(),
    "20".to_string(),
    "-o".to_string(),
    workdir
      .join("native_subs.%(ext)s")
      .to_string_lossy()
      .to_string(),
  ];
  if let Some(proxy_url) = build_proxy_url(&settings.proxy_id) {
    args.push("--proxy".to_string());
    args.push(proxy_url);
  }
  let lower = url.to_lowercase();
  let is_youtube = lower.contains("youtube.com") || lower.contains("youtu.be");
  if is_youtube {
    args.push("--add-header".to_string());
    args.push("User-Agent:Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string());
    args.push("--add-header".to_string());
    args.push("Referer:https://www.youtube.com/".to_string());
  }
  // Cookie：登录墙/年龄限制/会员视频的字幕也需要登录态，与「视频下载」同一套提取逻辑
  let is_tiktok = lower.contains("tiktok.com");
  if settings.cookie_from_browser && !is_tiktok {
    match export_vps_cookies_to_file(true, dl, app, Some(url)).await {
      Ok(cookie_file) => {
        args.push("--cookies".to_string());
        args.push(cookie_file.to_string_lossy().to_string());
      }
      Err(e) => log::warn!("[ai_rewrite] 字幕拉取的浏览器 Cookie 提取失败，继续尝试: {e}"),
    }
  }
  args.push(url.to_string());

  let mut noop = |_p: AudioDlProgress| {};
  if let Err(e) = run_ytdlp(&yt_dlp, &args, &mut noop).await {
    log::info!("[ai_rewrite] 平台原生字幕拉取失败（回退语音转写）：{e}");
    return Vec::new();
  }

  // 解析命中的字幕文件（取最大的一个，通常就是主语言字幕）
  let mut segs: Vec<serde_json::Value> = Vec::new();
  let mut candidates: Vec<PathBuf> = std::fs::read_dir(workdir)
    .map(|entries| {
      entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
          p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("native_subs.") && (n.ends_with(".vtt") || n.ends_with(".srt")))
            .unwrap_or(false)
        })
        .collect()
    })
    .unwrap_or_default();
  candidates.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
  if let Some(first) = candidates.first().cloned() {
    match std::fs::read_to_string(&first) {
      Ok(text) => {
        segs = parse_subtitle_segments(&text);
        log::info!(
          "[ai_rewrite] 平台原生字幕命中：{}（{} 段）",
          first
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("subtitle"),
          segs.len()
        );
      }
      Err(e) => log::info!("[ai_rewrite] 字幕文件读取失败：{e}"),
    }
  }
  // 清理临时字幕文件
  for p in candidates {
    let _ = std::fs::remove_file(p);
  }
  segs
}

/// 解析 VTT/SRT 文本为逐段时间轴（秒）；去掉内嵌标签，合并自动字幕的滚动重复行。
fn parse_subtitle_segments(text: &str) -> Vec<serde_json::Value> {
  use serde_json::json;
  let to_secs = |h: &str, m: &str, s: &str, ms: &str| -> f64 {
    h.parse::<f64>().unwrap_or(0.0) * 3600.0
      + m.parse::<f64>().unwrap_or(0.0) * 60.0
      + s.parse::<f64>().unwrap_or(0.0)
      + format!("0.{ms}").parse::<f64>().unwrap_or(0.0)
  };
  let Ok(re_time) = regex_lite::Regex::new(
    r"(\d{1,2}):(\d{2}):(\d{2})[.,](\d{1,3})\s*-->\s*(\d{1,2}):(\d{2}):(\d{2})[.,](\d{1,3})",
  ) else {
    return Vec::new();
  };
  let Ok(re_tag) = regex_lite::Regex::new(r"<[^>]*>|\{[^}]*\}") else {
    return Vec::new();
  };

  let mut segs: Vec<(f64, f64, String)> = Vec::new();
  let mut lines = text.lines().peekable();
  while let Some(line) = lines.next() {
    let Some(caps) = re_time.captures(line) else {
      continue;
    };
    let start = to_secs(&caps[1], &caps[2], &caps[3], &caps[4]);
    let end = to_secs(&caps[5], &caps[6], &caps[7], &caps[8]);
    let mut buf = String::new();
    while let Some(next) = lines.peek() {
      let n = next.trim();
      if n.is_empty() || re_time.is_match(n) || n.chars().all(|c| c.is_ascii_digit()) {
        break;
      }
      if !buf.is_empty() {
        buf.push(' ');
      }
      buf.push_str(n);
      lines.next();
    }
    let t = re_tag.replace_all(&buf, "").trim().to_string();
    if t.is_empty() || end <= start {
      continue;
    }
    if let Some((_, last_end, last_t)) = segs.last_mut() {
      if *last_t == t {
        if end > *last_end {
          *last_end = end;
        }
        continue;
      }
    }
    segs.push((start, end, t));
  }
  segs
    .into_iter()
    .map(|(s, e, t)| json!({ "start": s, "end": e, "text": t }))
    .collect()
}

/// 加浏览器指纹模拟参数（TikTok 等按 TLS 指纹拦截，需要伪装成真实浏览器）。
fn push_impersonate(args: &mut Vec<String>) {
  args.push("--impersonate".to_string());
  args.push("chrome".to_string());
}

/// 运行一次 yt-dlp，实时把输出转成进度回调；失败时返回带 stderr 摘要的错误。
async fn run_ytdlp(
  yt_dlp: &std::path::Path,
  args: &[String],
  on_progress: &mut (impl FnMut(AudioDlProgress) + Send),
) -> Result<(), String> {
  use tokio::io::AsyncBufReadExt;

  let mut cmd = tokio::process::Command::new(yt_dlp);
  cmd
    .args(args)
    .env("PYTHONIOENCODING", "utf-8")
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
  #[cfg(target_os = "windows")]
  {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
  }

  let mut child = cmd.spawn().map_err(|e| format!("启动 yt-dlp 失败：{e}"))?;
  log::info!(
    "[ai_rewrite] yt-dlp 启动: url={}",
    args.last().map(|s| s.as_str()).unwrap_or("")
  );
  let stdout = child
    .stdout
    .take()
    .ok_or_else(|| "无法读取 yt-dlp 输出".to_string())?;
  let stderr = child
    .stderr
    .take()
    .ok_or_else(|| "无法读取 yt-dlp 错误输出".to_string())?;

  // stderr 用独立异步任务排空，避免管道写满阻塞子进程
  let err_buf = Arc::new(tokio::sync::Mutex::new(String::new()));
  {
    let err_buf = err_buf.clone();
    tokio::spawn(async move {
      use tokio::io::AsyncBufReadExt;
      let mut lines = tokio::io::BufReader::new(stderr).lines();
      while let Ok(Some(line)) = lines.next_line().await {
        let mut b = err_buf.lock().await;
        if b.len() < 8000 {
          b.push_str(&line);
          b.push('\n');
        }
      }
    });
  }

  // 实时读取 stdout：进度行解析出速度/百分比，其余行作为日志
  let run = async move {
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut last_log = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
      let trimmed = line.trim();
      if trimmed.is_empty() {
        continue;
      }
      // [download]  23.9% of 81.01MiB at 11.11MiB/s ETA 00:05
      if trimmed.starts_with("[download]") && trimmed.contains("% of") {
        if let Some((percent, total, speed)) = parse_download_line(trimmed) {
          let downloaded = if total > 0 {
            (total as f64 * percent / 100.0) as u64
          } else {
            0
          };
          on_progress(AudioDlProgress {
            percent,
            speed_text: format_size_per_sec(speed),
            downloaded,
            total,
            log: trimmed.to_string(),
          });
          last_log.clear();
          continue;
        }
      }
      if trimmed != last_log {
        let brief: String = trimmed.chars().take(300).collect();
        log::info!("[ai_rewrite][yt-dlp] {brief}");
        on_progress(AudioDlProgress {
          percent: -1.0,
          log: brief,
          ..Default::default()
        });
        last_log = trimmed.to_string();
      }
    }
    child
      .wait()
      .await
      .map_err(|e| format!("等待 yt-dlp 结束失败：{e}"))
  };

  let status = match tokio::time::timeout(Duration::from_secs(900), run).await {
    Ok(Ok(s)) => s,
    Ok(Err(e)) => {
      log::warn!("[ai_rewrite] yt-dlp 等待失败: {e}");
      return Err(e);
    }
    Err(_) => {
      log::warn!("[ai_rewrite] yt-dlp 下载超时（900 秒）");
      return Err("下载超时（900 秒）".to_string());
    }
  };

  if !status.success() {
    let err = err_buf.lock().await.clone();
    let brief: String = err.trim().chars().take(400).collect();
    log::warn!(
      "[ai_rewrite] yt-dlp 退出码={:?} 错误={}",
      status.code(),
      brief
    );
    return Err(format!("下载失败：{brief}"));
  }
  Ok(())
}

/// 在 workdir 中查找以 `prefix` 开头的产物文件。
fn find_output(workdir: &std::path::Path, prefix: &str) -> Option<PathBuf> {
  let rd = std::fs::read_dir(workdir).ok()?;
  for entry in rd.flatten() {
    let p = entry.path();
    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
      if name.starts_with(prefix) {
        return Some(p);
      }
    }
  }
  None
}

/// source_url 是否指向本地已存在的视频/音频文件（洗稿跳过下载与云端查重/补拉）。
pub fn is_local_video(source_url: &str) -> bool {
  let p = std::path::Path::new(source_url.trim());
  if !p.is_file() {
    return false;
  }
  matches!(
    p.extension()
      .and_then(|e| e.to_str())
      .map(|e| e.to_lowercase())
      .as_deref(),
    Some(
      "mp4"
        | "mkv"
        | "webm"
        | "mov"
        | "avi"
        | "flv"
        | "ts"
        | "m4v"
        | "wmv"
        | "3gp"
        | "mp3"
        | "m4a"
        | "wav"
    )
  )
}

/// 列出本地文件夹内全部视频/音频文件（洗稿批量入口，递归一层）。
pub fn list_local_videos(dir: &str) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  let Ok(entries) = std::fs::read_dir(dir) else {
    return out;
  };
  for e in entries.flatten() {
    let p = e.path();
    if p.is_file() {
      let name = p.to_string_lossy().to_string();
      if is_local_video(&name) {
        out.push(name);
      }
    }
  }
  out.sort();
  out
}

/// 用 ffmpeg 从视频中抽取 mp3 音频（本地兜底，不依赖 yt-dlp 后处理）。
pub(crate) async fn extract_audio_with_ffmpeg(
  video: &std::path::Path,
  out: &std::path::Path,
) -> Result<(), String> {
  let bundled = tools_dir().join(ffmpeg_filename());
  let ffmpeg = if bundled.exists() {
    bundled
  } else {
    PathBuf::from("ffmpeg")
  };

  let mut cmd = tokio::process::Command::new(&ffmpeg);
  cmd
    .arg("-y")
    .arg("-i")
    .arg(video)
    .arg("-vn")
    .arg("-acodec")
    .arg("libmp3lame")
    .arg("-q:a")
    .arg("4")
    .arg(out)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
  #[cfg(target_os = "windows")]
  {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
  }

  let output = tokio::time::timeout(Duration::from_secs(600), cmd.output())
    .await
    .map_err(|_| "ffmpeg 抽取音频超时（600 秒）".to_string())?
    .map_err(|e| format!("启动 ffmpeg 失败：{e}"))?;

  if !output.status.success() {
    let err = String::from_utf8_lossy(&output.stderr);
    let brief: String = err.trim().chars().take(400).collect();
    return Err(format!("ffmpeg 抽取音频失败：{brief}"));
  }
  if !out.exists() {
    return Err("ffmpeg 未生成音频文件".to_string());
  }
  Ok(())
}

/// 使用 yt-dlp --cookies-from-browser 从指定 Chrome 用户数据目录提取 Cookie
/// 这种方式能拿到所有持久化在 SQLite 里的 cookie，比 CDP 更全面
/// cookie_url 决定 yt-dlp 访问哪个网站提取对应域名的 cookie
async fn extract_cookies_via_ytdlp(
  yt_dlp_path: &std::path::Path,
  browser_arg: &str,
  output_path: &std::path::Path,
  cookie_url: &str,
) -> Result<usize, String> {
  let tmp_path = output_path.with_extension("tmp.txt");
  let _ = std::fs::remove_file(&tmp_path);

  log::info!(
    "[video_download] 使用 yt-dlp 从浏览器提取 Cookie ({}): {}",
    cookie_url,
    browser_arg
  );

  let mut cmd = tokio::process::Command::new(yt_dlp_path);
  cmd
    .arg("--cookies-from-browser")
    .arg(browser_arg)
    .arg("--cookies")
    .arg(&tmp_path)
    .arg("--skip-download")
    .arg("--no-warnings")
    .arg("--no-playlist")
    .arg(cookie_url)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  #[cfg(target_os = "windows")]
  {
    cmd.creation_flags(0x08000000);
  }

  // 本地读库 + 写 cookie 文件，10 秒绰绰有余；系统浏览器逐个回退时
  // 每个最多卡 10 秒，避免网络慢时把总时长拖垮
  let output = tokio::time::timeout(std::time::Duration::from_secs(10), cmd.output())
    .await
    .map_err(|_| "yt-dlp 提取 Cookie 超时（10秒）".to_string())?
    .map_err(|e| format!("yt-dlp 进程启动失败: {}", e))?;

  if !tmp_path.exists() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(format!(
      "yt-dlp 未生成 Cookie 文件: {}",
      stderr.lines().next().unwrap_or("未知错误")
    ));
  }

  // 统计有效 cookie 数量
  let content =
    std::fs::read_to_string(&tmp_path).map_err(|e| format!("读取临时 Cookie 文件失败: {}", e))?;
  let count = content
    .lines()
    .filter(|l| !l.starts_with('#') && l.contains('\t'))
    .count();

  // 移动到最终路径
  std::fs::rename(&tmp_path, output_path).map_err(|e| format!("移动 Cookie 文件失败: {}", e))?;

  log::info!("[video_download] yt-dlp 提取到 {} 条 cookie", count);
  Ok(count)
}

/// 从 VPS 登录 profile 提取 Cookie
/// 优先级：yt-dlp --cookies-from-browser（最全面） > CDP（实时内存） > SQLite 数据库
/// download_url 用于判断访问哪个网站提取对应平台的 cookie
async fn export_vps_cookies_to_file<R: Runtime>(
  force_refresh: bool,
  dl: &VideoDownloader,
  app: &tauri::AppHandle<R>,
  download_url: Option<&str>,
) -> Result<PathBuf, String> {
  // 找到 VPS 登录 profile（即左上角 logo 点击打开的浏览器）
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().map_err(|e| e.to_string())?;
  let vps_profile = profiles
    .into_iter()
    .find(|p| p.name.to_lowercase() == "vps登录")
    .ok_or_else(|| {
      "未找到名为「VPS登录」的浏览器 profile，请先点击左上角 Logo 创建并登录 YouTube".to_string()
    })?;

  let profile_id = vps_profile.id.to_string();
  let profiles_dir = pm.get_profiles_dir();
  let user_data_dir = vps_profile.get_profile_data_path(&profiles_dir);

  // 根据下载 URL 决定访问哪个网站提取对应平台的 cookie
  let cookie_url = match download_url {
    Some(u) => {
      let lower = u.to_lowercase();
      if lower.contains("douyin.com") || lower.contains("v.douyin.com") {
        "https://www.douyin.com/"
      } else if lower.contains("tiktok.com") {
        "https://www.tiktok.com/"
      } else if lower.contains("bilibili.com") || lower.contains("b23.tv") {
        "https://www.bilibili.com/"
      } else {
        "https://www.youtube.com/"
      }
    }
    None => "https://www.youtube.com/",
  };

  // 按平台分文件缓存 cookie，避免 YouTube 的 cookie 覆盖抖音的
  let platform_tag = match download_url {
    Some(u) => {
      let lower = u.to_lowercase();
      if lower.contains("douyin.com") || lower.contains("v.douyin.com") {
        "douyin"
      } else if lower.contains("tiktok.com") {
        "tiktok"
      } else if lower.contains("bilibili.com") || lower.contains("b23.tv") {
        "bilibili"
      } else {
        "youtube"
      }
    }
    None => "youtube",
  };
  let cookie_path = tools_dir().join(format!("cookies_{}.txt", platform_tag));

  // 获取 yt-dlp 路径与系统浏览器回退开关
  let inner = dl.inner.lock().await;
  let yt_dlp_path = inner.yt_dlp_path.clone();
  let system_fallback = inner.settings.cookie_from_system_browser;
  drop(inner);

  // 找到 VPS 登录 profile（即左上角 logo 点击打开的浏览器）
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().map_err(|e| e.to_string())?;
  let vps_profile = match profiles
    .into_iter()
    .find(|p| p.name.to_lowercase() == "vps登录")
  {
    Some(p) => p,
    None => {
      // 没有 VPS 登录 profile：设置允许时回退到系统浏览器
      if system_fallback {
        if let Some(yt_dlp) = yt_dlp_path.as_ref() {
          if yt_dlp.exists() {
            if let Ok(n) = try_system_browser_cookie(yt_dlp, &cookie_path, cookie_url).await {
              dl.set_last_cookie_time(now_secs()).await;
              log::info!("[video_download] Cookie 已刷新（系统浏览器回退，{n} 条）");
              return Ok(cookie_path);
            }
          }
        }
      }
      return Err(
        "未找到名为「VPS登录」的浏览器 profile，请先点击左上角 Logo 创建并登录 YouTube".to_string(),
      );
    }
  };

  // 如果不是强制刷新且文件存在且1小时内，直接复用
  if !force_refresh && cookie_path.exists() {
    if let Ok(metadata) = std::fs::metadata(&cookie_path) {
      if let Ok(modified) = metadata.modified() {
        if let Ok(elapsed) = modified.elapsed() {
          if elapsed.as_secs() < 3600 {
            log::debug!("[video_download] 复用缓存的 cookie 文件 ({})", platform_tag);
            return Ok(cookie_path);
          }
        }
      }
    }
  }

  // 强制刷新时先删除旧文件
  if force_refresh && cookie_path.exists() {
    let _ = std::fs::remove_file(&cookie_path);
  }

  // 抖音需要先导航浏览器到 douyin.com 获取访客 cookie
  let needs_douyin_nav = download_url
    .map(|u| {
      let lower = u.to_lowercase();
      lower.contains("douyin.com") || lower.contains("v.douyin.com")
    })
    .unwrap_or(false);

  // 抖音跳过 yt-dlp 提取（yt-dlp 访问 douyin.com 会 403），
  // 直接走 CDP 导航 + 导出，从浏览器内存获取新鲜 cookie
  if !needs_douyin_nav {
    if let Some(yt_dlp) = yt_dlp_path.as_ref() {
      if yt_dlp.exists() && user_data_dir.exists() {
        let vps_arg = format!("chrome:{}:Default", user_data_dir.to_string_lossy());
        match extract_cookies_via_ytdlp(yt_dlp, &vps_arg, &cookie_path, cookie_url).await {
          Ok(count) if count > 0 => {
            dl.set_last_cookie_time(now_secs()).await;
            log::info!(
              "[video_download] Cookie 已刷新（yt-dlp 提取，{} 条）",
              count
            );
            return Ok(cookie_path);
          }
          Ok(_) => {
            log::warn!("[video_download] yt-dlp 提取到 0 条 cookie，尝试 CDP 方式");
          }
          Err(e) => {
            log::warn!(
              "[video_download] yt-dlp 提取 Cookie 失败: {}，回退到 CDP 方式",
              e
            );
          }
        }
      }
    }
  }

  if needs_douyin_nav {
    log::info!("[video_download] 抖音: 尝试导航浏览器到 douyin.com 获取访客 cookie");
    match crate::cookie_sync::navigate_to_url(&vps_profile, cookie_url).await {
      Ok(_) => {
        log::info!("[video_download] 抖音: 导航成功，等待页面加载");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
      }
      Err(e) => {
        log::warn!("[video_download] 抖音: 导航失败（浏览器可能未运行）: {}", e);
      }
    }
  }

  // 回退：通过 CDP 实时导出（浏览器正在运行时能读到内存中的全部 cookie）
  match crate::cookie_sync::export_cookies_via_cdp(&vps_profile).await {
    Ok(cookie_json) => {
      let cookies: Vec<serde_json::Value> =
        serde_json::from_str(&cookie_json).map_err(|e| format!("解析 cookie 失败: {}", e))?;
      log::info!(
        "[video_download] 通过 CDP 导出到 {} 条 cookie",
        cookies.len()
      );
      let txt = cookies_to_netscape(&cookies);
      std::fs::write(&cookie_path, txt).map_err(|e| format!("写入 cookie 文件失败: {}", e))?;
      dl.set_last_cookie_time(now_secs()).await;
      log::info!("[video_download] Cookie 已刷新（CDP 导出）");
      Ok(cookie_path)
    }
    Err(cdp_err) => {
      // CDP 不可用就直接走后面的回退，不在下载前空等浏览器起来
      log::warn!(
        "[video_download] CDP 导出失败（浏览器可能未运行）: {}，跳过等待",
        cdp_err
      );
      let _ = app.emit("video-download:auto-open-vps", ());

      // 设置允许时先回退系统浏览器，最后 SQLite
      if system_fallback {
        if let Some(yt_dlp) = yt_dlp_path.as_ref() {
          if yt_dlp.exists() {
            match try_system_browser_cookie(yt_dlp, &cookie_path, cookie_url).await {
              Ok(n) => {
                dl.set_last_cookie_time(now_secs()).await;
                log::info!("[video_download] Cookie 已刷新（系统浏览器回退，{n} 条）");
                return Ok(cookie_path);
              }
              Err(e) => {
                log::warn!("[video_download] 系统浏览器回退失败: {}", e);
              }
            }
          }
        }
      }

      // 全部失败，最终 fallback 到 SQLite 数据库
      log::warn!("[video_download] 回退到 SQLite 数据库");
      let result = crate::cookie_manager::CookieManager::read_cookies(&profile_id)
        .map_err(|e| format!("Cookie 导出失败（CDP 不可用，SQLite: {}）", e))?;
      let all_cookies: Vec<crate::cookie_manager::UnifiedCookie> =
        result.domains.into_iter().flat_map(|d| d.cookies).collect();
      log::info!(
        "[video_download] 从 Cookie 数据库读取到 {} 条 cookie",
        all_cookies.len()
      );
      let txt = crate::cookie_manager::CookieManager::format_netscape_cookies(&all_cookies);
      std::fs::write(&cookie_path, txt).map_err(|e| format!("写入 cookie 文件失败: {}", e))?;
      dl.set_last_cookie_time(now_secs()).await;
      log::info!("[video_download] Cookie 已刷新（SQLite 回退）");
      Ok(cookie_path)
    }
  }
}

/// 回退：从系统浏览器（Chrome/Edge/Firefox/Brave/Chromium）提取 Cookie。
/// yt-dlp 直读默认 profile 的 cookie 库，无需浏览器在运行。
/// Windows 下 Chrome 127+ 的 App-Bound 加密可能失败，因此逐个尝试。
async fn try_system_browser_cookie(
  yt_dlp_path: &std::path::Path,
  output_path: &std::path::Path,
  cookie_url: &str,
) -> Result<usize, String> {
  let mut errors: Vec<String> = Vec::new();
  for browser in ["chrome", "edge", "firefox", "brave", "chromium"] {
    if !system_browser_cookie_db_exists(browser) {
      continue;
    }
    match extract_cookies_via_ytdlp(yt_dlp_path, browser, output_path, cookie_url).await {
      Ok(n) if n > 0 => {
        log::info!(
          "[video_download] 从系统浏览器 {} 提取到 {} 条 cookie",
          browser,
          n
        );
        return Ok(n);
      }
      Ok(_) => errors.push(format!("{browser}: 0 条")),
      Err(e) => errors.push(format!("{browser}: {e}")),
    }
  }
  Err(format!(
    "系统浏览器均未提取到 Cookie（{}）",
    if errors.is_empty() {
      "未检测到已安装的系统浏览器".to_string()
    } else {
      errors.join("; ")
    }
  ))
}

/// 判断某个系统浏览器的 cookie 库文件是否存在
fn system_browser_cookie_db_exists(browser: &str) -> bool {
  #[cfg(target_os = "windows")]
  {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    // Chromium 系 cookie 库：新版在 Default/Network/Cookies，旧版（<96）在 Default/Cookies
    let chromium_db = |base: &str| {
      let root = std::path::Path::new(base);
      root.join("Default/Network/Cookies").exists() || root.join("Default/Cookies").exists()
    };
    match browser {
      "chrome" => chromium_db(
        &std::path::Path::new(&local)
          .join("Google/Chrome/User Data")
          .to_string_lossy(),
      ),
      "edge" => chromium_db(
        &std::path::Path::new(&local)
          .join("Microsoft/Edge/User Data")
          .to_string_lossy(),
      ),
      "brave" => chromium_db(
        &std::path::Path::new(&local)
          .join("BraveSoftware/Brave-Browser/User Data")
          .to_string_lossy(),
      ),
      "firefox" => firefox_cookie_db(&roaming).is_some(),
      _ => false,
    }
  }
  #[cfg(target_os = "macos")]
  {
    let home = std::env::var("HOME").unwrap_or_default();
    let app_support = format!("{home}/Library/Application Support");
    match browser {
      "chrome" => std::path::Path::new(&app_support)
        .join("Google/Chrome/Default/Cookies")
        .exists(),
      "edge" => std::path::Path::new(&app_support)
        .join("Microsoft Edge/Default/Cookies")
        .exists(),
      "brave" => std::path::Path::new(&app_support)
        .join("BraveSoftware/Brave-Browser/Default/Cookies")
        .exists(),
      "firefox" => firefox_cookie_db(&format!("{app_support}/Firefox")).is_some(),
      _ => false,
    }
  }
  #[cfg(all(unix, not(target_os = "macos")))]
  {
    let home = std::env::var("HOME").unwrap_or_default();
    let config = format!("{home}/.config");
    match browser {
      "chrome" => std::path::Path::new(&config)
        .join("google-chrome/Default/Cookies")
        .exists(),
      "edge" => std::path::Path::new(&config)
        .join("microsoft-edge/Default/Cookies")
        .exists(),
      "brave" => std::path::Path::new(&config)
        .join("BraveSoftware/Brave-Browser/Default/Cookies")
        .exists(),
      "chromium" => std::path::Path::new(&config)
        .join("chromium/Default/Cookies")
        .exists(),
      "firefox" => firefox_cookie_db(&format!("{home}/.mozilla/firefox")).is_some(),
      _ => false,
    }
  }
}

/// 找 Firefox 的 cookie 库（Profiles/<随机>.default*/cookies.sqlite）
fn firefox_cookie_db(base: &str) -> Option<std::path::PathBuf> {
  let profiles = std::path::Path::new(base).join("Profiles");
  let mut entries: Vec<_> = std::fs::read_dir(&profiles).ok()?.flatten().collect();
  entries.sort_by_key(|e| {
    // modified 优先，其次 default-release 这类名字
    let name = e.file_name().to_string_lossy().to_string();
    let mtime = e
      .metadata()
      .ok()
      .and_then(|m| m.modified().ok())
      .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
      .map(|d| d.as_secs())
      .unwrap_or(0);
    (std::cmp::Reverse(mtime), name)
  });
  entries
    .iter()
    .map(|e| e.path().join("cookies.sqlite"))
    .find(|p| p.exists())
}

/// 把 CDP 导出的 cookie JSON 数组转成 Netscape cookies.txt 格式
fn cookies_to_netscape(cookies: &[serde_json::Value]) -> String {
  let mut txt = String::from("# Netscape HTTP Cookie File\n");
  for c in cookies {
    let domain = c["domain"].as_str().unwrap_or("");
    let flag = if domain.starts_with('.') {
      "TRUE"
    } else {
      "FALSE"
    };
    let path = c["path"].as_str().unwrap_or("/");
    let secure = if c["secure"].as_bool().unwrap_or(false) {
      "TRUE"
    } else {
      "FALSE"
    };
    let expires = c["expires"].as_f64().unwrap_or(0.0) as u64;
    let name = c["name"].as_str().unwrap_or("");
    let value = c["value"].as_str().unwrap_or("");
    txt.push_str(&format!(
      "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
      domain, flag, path, secure, expires, name, value
    ));
  }
  txt
}

fn now_secs() -> i64 {
  use std::time::{SystemTime, UNIX_EPOCH};
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0)
}

/// 从文件名提取标题
fn extract_title_from_filename(filename: &str) -> Option<String> {
  // 格式: "标题 [id].mp4" 或 "标题.mp4"
  let path = std::path::Path::new(filename);
  let stem = path.file_stem()?.to_str()?;

  // 去掉末尾的 [id] 部分
  if let Some(bracket_pos) = stem.rfind(" [") {
    Some(stem[..bracket_pos].trim().to_string())
  } else {
    Some(stem.to_string())
  }
}

/// 终止指定 PID 的进程
fn kill_process(pid: u32) {
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let _ = Command::new("taskkill")
      .args(["/F", "/T", "/PID", &pid.to_string()])
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .creation_flags(0x08000000)
      .status();
  }
  #[cfg(not(windows))]
  {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;
    let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
  }
}

/// 解析 yt-dlp 进度行
/// 格式: "[download]  12.3% of 52.0MiB at  2.1MiB/s ETA 00:00:20"
/// 或自定义模板: "[download]  12.3% of 54525952 at 2202009 ETA 20"
fn parse_progress(line: &str) -> Option<(f64, String, u64, u64)> {
  // 提取百分比：找 "%" 前面的数字
  let pct_end = line.find('%')?;
  let before_pct = &line[..pct_end];
  let pct_start = before_pct.rfind(|c: char| !c.is_ascii_digit() && c != '.' && c != ' ')? + 1;
  let progress_str = before_pct[pct_start..].trim();
  let progress: f64 = progress_str.parse().ok()?;

  // 提取总大小：找 "of " 后面的部分
  let of_pos = line.find(" of ")? + 4;
  let after_of = &line[of_pos..];
  // 找到 " at " 之前的是大小
  let at_pos = after_of.find(" at ")?;
  let size_str = after_of[..at_pos].trim();

  let (total, speed_text) = parse_size_and_speed(size_str, &after_of[at_pos + 4..]);

  let downloaded = if total > 0 && progress > 0.0 {
    (total as f64 * progress / 100.0) as u64
  } else {
    0
  };

  Some((progress, speed_text, downloaded, total))
}

/// 解析大小字符串和速度字符串
fn parse_size_and_speed(size_str: &str, rest: &str) -> (u64, String) {
  // 解析大小
  let total = parse_human_size(size_str);

  // 解析速度：取 "at " 后面到 ETA 之前的部分
  let speed_text = if let Some(eta_pos) = rest.find(" ETA") {
    rest[..eta_pos].trim().to_string()
  } else {
    rest.trim().to_string()
  };

  // 如果速度是纯数字（字节/秒），格式化为可读形式
  let speed_text =
    if speed_text.chars().all(|c| c.is_ascii_digit() || c == '.') && !speed_text.is_empty() {
      let bytes = speed_text.parse::<f64>().unwrap_or(0.0) as u64;
      format_size_per_sec(bytes)
    } else {
      speed_text
    };

  (total, speed_text)
}

fn parse_human_size(s: &str) -> u64 {
  let s = s.trim().trim_start_matches('~');
  if s.is_empty() {
    return 0;
  }

  // 找到数字和单位的分界
  let num_end = s
    .find(|c: char| !c.is_ascii_digit() && c != '.')
    .unwrap_or(s.len());
  let num: f64 = s[..num_end].parse().unwrap_or(0.0);
  let unit = s[num_end..].trim().to_lowercase();

  let multiplier: u64 = match unit.as_str() {
    "b" | "" => 1,
    "kib" | "ki" | "kb" | "k" => 1024,
    "mib" | "mi" | "mb" | "m" => 1024 * 1024,
    "gib" | "gi" | "gb" | "g" => 1024 * 1024 * 1024,
    _ => 1,
  };

  (num * multiplier as f64) as u64
}

/// 解析 yt-dlp 进度行
/// 格式: "[download]  23.9% of   81.01MiB at   11.11MiB/s ETA 00:05"
/// 返回: (百分比, 总字节数, 速度字节/秒)
fn parse_download_line(line: &str) -> Option<(f64, u64, u64)> {
  // 提取百分比
  let percent_str = line.find('%').and_then(|end| {
    let start = line[..end].rfind(|c: char| !c.is_ascii_digit() && c != '.' && c != ' ')? + 1;
    Some(line[start..end].trim())
  })?;
  let percent: f64 = percent_str.parse().ok()?;

  // 提取总大小 ("of XXX" 后面)
  let total = line
    .find("of ")
    .map(|pos| {
      let rest = line[pos + 3..].trim();
      let end = rest.find(" at ").unwrap_or(rest.len());
      parse_human_size(&rest[..end])
    })
    .unwrap_or(0);

  // 提取速度 ("at XXX/s" 部分)
  let speed = line
    .find(" at ")
    .map(|pos| {
      let rest = line[pos + 4..].trim();
      let end = rest.find(" ETA ").unwrap_or(rest.len());
      let speed_str = &rest[..end];
      // 去掉末尾的 "/s"
      let speed_str = speed_str.strip_suffix("/s").unwrap_or(speed_str);
      parse_human_size(speed_str)
    })
    .unwrap_or(0);

  Some((percent, total, speed))
}

/// 从 info 文件读取视频标题、分辨率和缩略图
async fn read_info_from_file<R: Runtime>(
  info_file: &std::path::Path,
  task_id: &str,
  dl: &VideoDownloader,
  app: &tauri::AppHandle<R>,
) {
  if let Ok(content) = std::fs::read_to_string(info_file) {
    for line in content.lines() {
      let line = line.trim();
      if let Some(title) = line.strip_prefix("BW_INFO_TITLE:") {
        let title = title.trim().to_string();
        if !title.is_empty() && title != "NA" {
          dl.set_task_title(task_id, title).await;
        }
      } else if let Some(resolution) = line.strip_prefix("BW_INFO_RESOLUTION:") {
        let resolution = resolution.trim().to_string();
        if !resolution.is_empty() && resolution != "NA" {
          dl.set_task_resolution(task_id, resolution).await;
        }
      } else if let Some(thumbnail) = line.strip_prefix("BW_INFO_THUMBNAIL:") {
        let thumbnail = thumbnail.trim().to_string();
        if !thumbnail.is_empty() && thumbnail != "NA" {
          let mut inner = dl.inner.lock().await;
          if let Some(task) = inner.tasks.get_mut(task_id) {
            task.thumbnail = Some(thumbnail);
          }
          drop(inner);
          let _ = app.emit("video-download:task-updated", task_id);
        }
      }
    }
  }
}

fn format_size_per_sec(bytes: u64) -> String {
  if bytes >= 1024 * 1024 * 1024 {
    format!("{:.2} GB/s", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
  } else if bytes >= 1024 * 1024 {
    format!("{:.2} MB/s", bytes as f64 / (1024.0 * 1024.0))
  } else if bytes >= 1024 {
    format!("{:.2} KB/s", bytes as f64 / 1024.0)
  } else {
    format!("{} B/s", bytes)
  }
}

/// 从峰值速度文本解析回字节数（用于比较）
fn parse_peak_speed_bytes(text: &str) -> u64 {
  if text.is_empty() {
    return 0;
  }
  // 去掉 "/s" 后缀后用 parse_human_size 解析
  let cleaned = text.strip_suffix("/s").unwrap_or(text);
  parse_human_size(cleaned)
}

/// 从 yt-dlp 输出行提取用户可读的状态文本
fn extract_status_text(line: &str) -> Option<String> {
  let line = line.trim();

  // [youtube] xxx: Downloading webpage → 正在获取视频信息
  // [youtube] xxx: Downloading web embedded client config → 正在加载播放器配置
  // [youtube] xxx: Downloading player ... → 正在加载播放器
  // [youtube] xxx: Downloading web embedded player API JSON → 正在解析播放器
  // [youtube] xxx: Downloading tv downgraded player API JSON → 正在解析播放器
  // [youtube] [jsc:deno] Solving JS challenges using deno → 正在通过 JS 验证
  // [info] xxx: Downloading 1 format(s): 399+251 → 已找到视频格式，准备下载
  // [youtube] xxx: Downloading yt initial data response → 正在获取初始数据

  if line.contains("Downloading webpage") {
    return Some("正在获取视频信息...".to_string());
  }
  if line.contains("Downloading player") {
    return Some("正在加载播放器...".to_string());
  }
  if line.contains("Solving JS challenges") {
    return Some("正在通过安全验证...".to_string());
  }
  if line.contains("Downloading") && line.contains("player") {
    return Some("正在解析播放器...".to_string());
  }
  if line.contains("Downloading initial data") || line.contains("initial data response") {
    return Some("正在获取初始数据...".to_string());
  }
  if line.contains("Downloading web embedded client") {
    return Some("正在加载客户端配置...".to_string());
  }
  if line.starts_with("[info]") && line.contains("Downloading") && line.contains("format") {
    return Some("已找到视频格式，准备下载...".to_string());
  }
  if line.contains("Downloading") && line.contains("api json") {
    return Some("正在获取视频数据...".to_string());
  }
  if line.starts_with("[youtube] Extracting URL") {
    return Some("正在解析视频链接...".to_string());
  }

  // 其他 [xxx] 行，去掉前缀，取最后一部分
  if let Some(colon_pos) = line.find(':') {
    let after = &line[colon_pos + 1..];
    let trimmed = after.trim();
    if !trimmed.is_empty() && trimmed.len() < 80 {
      return Some(trimmed.to_string());
    }
  }

  None
}

/// 从文本中提取视频 URL（YouTube、Bilibili、抖音等常见视频平台）
/// 支持从分享文本中提取 URL（如抖音分享：`https://v.douyin.com/xxx/` 复制此链接...）
fn extract_video_urls(text: &str) -> Vec<String> {
  let mut urls = Vec::new();
  let url_re = regex_lite::Regex::new(r#"https?://[^\s'"<>`\]]+"#).unwrap();
  for caps in url_re.captures_iter(text) {
    let url = caps
      .get(0)
      .map(|m| m.as_str().trim_end_matches('/').to_string());
    let url = match url {
      Some(u) if !u.is_empty() => u,
      _ => continue,
    };
    let lower = url.to_lowercase();
    let is_video_url = lower.contains("youtube.com")
      || lower.contains("youtu.be")
      || lower.contains("bilibili.com")
      || lower.contains("b23.tv")
      || lower.contains("vimeo.com")
      || lower.contains("tiktok.com")
      || lower.contains("douyin.com")
      || lower.contains("kuaishou.com")
      || lower.contains("xigua.com")
      || lower.contains("ixigua.com")
      || lower.contains("weibo.com") && lower.contains("/video/")
      || lower.contains("twitter.com") && (lower.contains("/status/") || lower.contains("/video/"))
      || lower.contains("x.com") && (lower.contains("/status/") || lower.contains("/video/"))
      || lower.contains("reddit.com") && lower.contains("/video/")
      || lower.contains("twitch.tv");
    if is_video_url {
      urls.push(url);
    }
  }
  urls
}

fn extract_youtube_id(url: &str) -> Option<String> {
  // 常见格式:
  // - https://www.youtube.com/watch?v=ID
  // - https://youtu.be/ID
  // - https://www.youtube.com/shorts/ID
  // - https://www.youtube.com/embed/ID
  let url_lower = url.to_lowercase();

  // youtu.be/ID
  if url_lower.contains("youtu.be/") {
    let after = url.split_once("youtu.be/")?.1;
    let id = after.split(['?', '&', '/', '#']).next()?;
    if !id.is_empty() {
      return Some(id.to_string());
    }
  }

  // v=ID 参数
  if let Some(after) = url.split_once("v=") {
    let id = after.1.split(['&', '#']).next()?;
    if !id.is_empty() {
      return Some(id.to_string());
    }
  }

  // /shorts/ID, /embed/ID, /v/ID
  for pattern in ["/shorts/", "/embed/", "/v/"] {
    if let Some(pos) = url_lower.find(pattern) {
      let after = &url[pos + pattern.len()..];
      let id = after.split(['?', '&', '/', '#']).next()?;
      if !id.is_empty() {
        return Some(id.to_string());
      }
    }
  }

  None
}

/// 从 B 站 URL 提取 BV 号
fn extract_bilibili_id(url: &str) -> Option<String> {
  // 常见格式:
  // - https://www.bilibili.com/video/BV1Xz421X7YQ
  // - https://b23.tv/BV1Xz421X7YQ
  // - https://www.bilibili.com/video/BV1Xz421X7YQ?p=1
  let url_lower = url.to_lowercase();

  if !url_lower.contains("bilibili.com") && !url_lower.contains("b23.tv") {
    return None;
  }

  // 查找 BV 号（BV + 10 位字母数字）
  let mut chars = url.chars();
  while let Some(c) = chars.next() {
    if c == 'B' || c == 'b' {
      let rest: String = chars.clone().take(11).collect();
      if rest.len() >= 11 {
        let rest_lower = rest.to_lowercase();
        if rest_lower.starts_with('v') {
          let id_part: String = rest.chars().take(11).collect();
          let id = format!("B{}", id_part);
          // 验证：BV 后面跟 10 位字母数字
          let after_bv: Vec<char> = id.chars().skip(2).collect();
          if after_bv.len() == 10 && after_bv.iter().all(|c| c.is_ascii_alphanumeric()) {
            return Some(id);
          }
        }
      }
    }
  }

  None
}

/// 从 URL 提取视频 ID（优先 YouTube，其次 B 站）
fn extract_video_id(url: &str) -> Option<String> {
  extract_youtube_id(url)
    .or_else(|| extract_bilibili_id(url))
    .or_else(|| {
      // TikTok: /video/<纯数字ID>。完成后靠 ID 在输出目录定位实际文件——
      // Windows 下 stdout 管道会丢文件名里的 emoji，只有 ID 是可靠的。
      let idx = url.find("/video/")?;
      let rest = &url[idx + 7..];
      let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
      if id.len() >= 8 {
        Some(id)
      } else {
        None
      }
    })
}

/// 在输出目录中查找包含视频 ID 的最大视频文件
fn find_final_video_file(output_dir: &PathBuf, video_id: &str) -> Option<PathBuf> {
  let mut largest: Option<(PathBuf, u64)> = None;

  if let Ok(entries) = std::fs::read_dir(output_dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      if !path.is_file() {
        continue;
      }
      let name = path.file_name()?.to_string_lossy().to_string();
      // 文件名包含视频 ID，且是视频格式（mp4, mkv, webm, mov, avi）
      let name_lower = name.to_lowercase();
      let is_video = name_lower.ends_with(".mp4")
        || name_lower.ends_with(".mkv")
        || name_lower.ends_with(".webm")
        || name_lower.ends_with(".mov")
        || name_lower.ends_with(".avi");
      if !is_video {
        continue;
      }
      // 跳过临时文件（.part, .ytdl）
      if name_lower.ends_with(".part") || name_lower.ends_with(".ytdl") {
        continue;
      }
      if !name.contains(video_id) {
        continue;
      }
      if let Ok(meta) = std::fs::metadata(&path) {
        let size = meta.len();
        match &largest {
          Some((_, s)) if size > *s => {
            largest = Some((path, size));
          }
          None => {
            largest = Some((path, size));
          }
          _ => {}
        }
      }
    }
  }

  largest.map(|(p, _)| p)
}

/// 在输出目录中查找最大的视频文件（兜底用，当 ID 和 filename 都找不到时）。
/// 只考虑 `min_modified` 之后有改动的文件——下载目录里往往有以前任务的
/// 旧视频，不看时间会把旧文件当成刚下载的结果，点「打开」就打开了别的视频。
fn find_largest_video_file(
  output_dir: &PathBuf,
  min_modified: std::time::SystemTime,
) -> Option<PathBuf> {
  let mut largest: Option<(PathBuf, u64)> = None;

  if let Ok(entries) = std::fs::read_dir(output_dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      if !path.is_file() {
        continue;
      }
      let name = path.file_name()?.to_string_lossy().to_string();
      let name_lower = name.to_lowercase();
      // 视频格式（mp4, mkv, webm, mov, avi, m4v, flv, wmv）
      let is_video = name_lower.ends_with(".mp4")
        || name_lower.ends_with(".mkv")
        || name_lower.ends_with(".webm")
        || name_lower.ends_with(".mov")
        || name_lower.ends_with(".avi")
        || name_lower.ends_with(".m4v")
        || name_lower.ends_with(".flv")
        || name_lower.ends_with(".wmv");
      if !is_video {
        continue;
      }
      // 跳过临时文件（.part, .ytdl, .tmp）
      if name_lower.ends_with(".part")
        || name_lower.ends_with(".ytdl")
        || name_lower.ends_with(".tmp")
      {
        continue;
      }
      if let Ok(meta) = std::fs::metadata(&path) {
        let size = meta.len();
        // 小于 100KB 的忽略（可能是残留的小文件）
        if size < 100 * 1024 {
          continue;
        }
        // 下载开始前就存在的文件属于以前的任务，跳过。
        // 不能用 mtime：yt-dlp 默认把文件 mtime 设成视频的上传时间（--mtime 默认
        // 开启），刚下完的文件 mtime 可能是几年前的；创建时间才是真实下载时刻，
        // yt-dlp 不会改它。created 不可用时退回 mtime。
        let file_time = meta
          .created()
          .or_else(|_| meta.modified())
          .unwrap_or(min_modified);
        if file_time < min_modified {
          continue;
        }
        match &largest {
          Some((_, s)) if size > *s => {
            largest = Some((path, size));
          }
          None => {
            largest = Some((path, size));
          }
          _ => {}
        }
      }
    }
  }

  largest.map(|(p, _)| p)
}

// 全局单例
static DOWNLOADER: std::sync::OnceLock<Arc<VideoDownloader>> = std::sync::OnceLock::new();

pub fn get_downloader() -> &'static Arc<VideoDownloader> {
  DOWNLOADER.get_or_init(|| Arc::new(VideoDownloader::new()))
}

// ========== Tauri Commands ==========

#[tauri::command]
pub async fn video_download_list_tasks() -> Result<Vec<DownloadTask>, String> {
  Ok(get_downloader().list_tasks().await)
}

#[tauri::command]
pub async fn video_download_add<R: Runtime>(
  url: String,
  app: tauri::AppHandle<R>,
) -> Result<String, String> {
  if url.trim().is_empty() {
    return Err("URL 不能为空".to_string());
  }
  let id = get_downloader()
    .add_task(url.trim().to_string(), &app)
    .await;
  Ok(id)
}

#[tauri::command]
pub async fn video_download_add_batch<R: Runtime>(
  urls: Vec<String>,
  app: tauri::AppHandle<R>,
) -> Result<Vec<String>, String> {
  let mut ids = Vec::new();
  for url in urls {
    if !url.trim().is_empty() {
      let id = get_downloader()
        .add_task(url.trim().to_string(), &app)
        .await;
      ids.push(id);
    }
  }
  Ok(ids)
}

#[tauri::command]
pub async fn video_download_cancel<R: Runtime>(
  task_id: String,
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().cancel_task(&task_id, &app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_pause<R: Runtime>(
  task_id: String,
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().pause_task(&task_id, &app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_resume<R: Runtime>(
  task_id: String,
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().retry_task(&task_id, &app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_retry<R: Runtime>(
  task_id: String,
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().retry_task(&task_id, &app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_delete<R: Runtime>(
  task_id: String,
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().delete_task(&task_id, &app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_clear_finished() -> Result<(), String> {
  get_downloader().clear_finished().await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_pause_all<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
  get_downloader().pause_all(&app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_retry_all<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
  get_downloader().retry_all(&app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_start_pending<R: Runtime>(
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().try_start_next(&app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_delete_all<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
  get_downloader().delete_all(&app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_get_settings() -> Result<DownloadSettings, String> {
  Ok(get_downloader().get_settings().await)
}

/// 视频下载用的代理简要信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDownloadProxyItem {
  pub id: String,
  pub name: String,
  pub proxy_type: String,
  pub host: String,
  pub port: u16,
}

#[tauri::command]
pub async fn video_download_list_proxies() -> Result<Vec<VideoDownloadProxyItem>, String> {
  let stored = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
  let list: Vec<VideoDownloadProxyItem> = stored
    .iter()
    .map(|p| VideoDownloadProxyItem {
      id: p.id.clone(),
      name: p.name.clone(),
      proxy_type: p.proxy_settings.proxy_type.clone(),
      host: p.proxy_settings.host.clone(),
      port: p.proxy_settings.port,
    })
    .collect();
  Ok(list)
}

#[tauri::command]
pub async fn video_download_update_settings<R: Runtime>(
  app: tauri::AppHandle<R>,
  settings: DownloadSettings,
) -> Result<(), String> {
  let was_auto_paste = get_downloader().get_settings().await.auto_paste_download;
  let is_auto_paste = settings.auto_paste_download;
  log::info!(
    "[video_download] update_settings: auto_paste {} -> {}, cookie_from_browser {} -> {}",
    was_auto_paste,
    is_auto_paste,
    get_downloader().get_settings().await.cookie_from_browser,
    settings.cookie_from_browser
  );
  get_downloader().update_settings(settings).await;

  // 根据 auto_paste_download 变化自动启停剪贴板监控
  if was_auto_paste != is_auto_paste {
    if is_auto_paste {
      log::info!("[video_download] 启动剪贴板监控");
      get_downloader().start_clipboard_monitor(app).await;
    } else {
      log::info!("[video_download] 停止剪贴板监控");
      get_downloader().stop_clipboard_monitor().await;
    }
  } else {
    log::info!("[video_download] auto_paste 未变化，跳过监控启停");
  }
  Ok(())
}

#[tauri::command]
pub async fn video_download_start_clipboard_monitor<R: Runtime>(
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  get_downloader().start_clipboard_monitor(app).await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_stop_clipboard_monitor() -> Result<(), String> {
  get_downloader().stop_clipboard_monitor().await;
  Ok(())
}

#[tauri::command]
pub async fn video_download_set_yt_dlp_path(path: String) -> Result<(), String> {
  get_downloader().set_yt_dlp_path(PathBuf::from(path)).await;
  Ok(())
}

// ========== 工具管理 (yt-dlp / ffmpeg / ffprobe) ==========

/// 工具状态
#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
  pub yt_dlp: bool,
  pub ffmpeg: bool,
  pub ffprobe: bool,
  pub yt_dlp_path: Option<String>,
  pub ffmpeg_path: Option<String>,
  pub ffprobe_path: Option<String>,
  pub yt_dlp_version: Option<String>,
  pub ffmpeg_version: Option<String>,
  pub ffprobe_version: Option<String>,
}

/// 获取工具目录
pub(crate) fn tools_dir() -> PathBuf {
  let app_dir = crate::app_dirs::data_dir();
  app_dir.join("video-tools")
}

/// 抖音签名插件源码（编译期嵌入，运行时写入工具目录）
const DOUYIN_PLUGIN_SOURCE: &str =
  include_str!("../yt-dlp-plugins/douyin_fix/yt_dlp_plugins/extractor/douyin_fix.py");

/// 把抖音签名插件写入工具目录，返回 `--plugin-dirs` 需要的目录
fn ensure_douyin_plugin() -> Result<PathBuf, String> {
  let plugin_dir = tools_dir().join("yt-dlp-plugins");
  let plugin_file = plugin_dir
    .join("douyin_fix")
    .join("yt_dlp_plugins")
    .join("extractor")
    .join("douyin_fix.py");
  let up_to_date = std::fs::read_to_string(&plugin_file)
    .map(|content| content == DOUYIN_PLUGIN_SOURCE)
    .unwrap_or(false);
  if !up_to_date {
    if let Some(parent) = plugin_file.parent() {
      std::fs::create_dir_all(parent).map_err(|e| format!("创建插件目录失败: {e}"))?;
    }
    std::fs::write(&plugin_file, DOUYIN_PLUGIN_SOURCE).map_err(|e| format!("写入插件失败: {e}"))?;
  }
  Ok(plugin_dir)
}

/// yt-dlp 二进制文件名（按平台）
fn yt_dlp_filename() -> &'static str {
  #[cfg(target_os = "windows")]
  {
    "yt-dlp.exe"
  }
  #[cfg(target_os = "macos")]
  {
    "yt-dlp_macos"
  }
  #[cfg(target_os = "linux")]
  {
    "yt-dlp_linux"
  }
}

/// yt-dlp GitHub 下载 URL（按平台）
fn yt_dlp_download_url() -> &'static str {
  #[cfg(target_os = "windows")]
  {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
  }
  #[cfg(target_os = "macos")]
  {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
  }
  #[cfg(target_os = "linux")]
  {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
  }
}

/// ffmpeg 二进制文件名（按平台）
fn ffmpeg_filename() -> &'static str {
  #[cfg(target_os = "windows")]
  {
    "ffmpeg.exe"
  }
  #[cfg(not(target_os = "windows"))]
  {
    "ffmpeg"
  }
}

/// ffprobe 二进制文件名（按平台）
fn ffprobe_filename() -> &'static str {
  #[cfg(target_os = "windows")]
  {
    "ffprobe.exe"
  }
  #[cfg(not(target_os = "windows"))]
  {
    "ffprobe"
  }
}

/// 轻量检查：yt-dlp 文件是否存在（不启动进程，纯文件判断）
fn yt_dlp_exists() -> bool {
  let dir = tools_dir();
  dir.join(yt_dlp_filename()).exists()
}

/// 轻量检查：三个工具文件是否都存在（不启动进程，纯文件判断，不阻塞）
#[tauri::command]
pub async fn video_download_tools_exist() -> bool {
  let dir = tools_dir();
  dir.join(yt_dlp_filename()).exists()
    && dir.join(ffmpeg_filename()).exists()
    && dir.join(ffprobe_filename()).exists()
}

/// 检查工具是否存在（纯文件判断，不检查系统 PATH）
pub async fn check_tools() -> ToolStatus {
  // 用 spawn_blocking 包装，版本检测的同步进程调用不会阻塞 tokio worker 线程
  // 这样 video_download_add 等命令不会被 check_tools 阻塞排队
  tokio::task::spawn_blocking(|| {
    let dir = tools_dir();
    log::info!(
      "[video_download] check_tools: tools_dir = {}",
      dir.display()
    );

    let yt_dlp_path = dir.join(yt_dlp_filename());
    let ffmpeg_path = dir.join(ffmpeg_filename());
    let ffprobe_path = dir.join(ffprobe_filename());

    let yt_dlp = yt_dlp_path.exists();
    let ffmpeg = ffmpeg_path.exists();
    let ffprobe = ffprobe_path.exists();

    log::info!(
      "[video_download] check_tools: yt-dlp={}({}), ffmpeg={}({}), ffprobe={}({})",
      yt_dlp,
      yt_dlp_path.display(),
      ffmpeg,
      ffmpeg_path.display(),
      ffprobe,
      ffprobe_path.display()
    );

    let yt_dlp_version = if yt_dlp {
      get_tool_version(&yt_dlp_path, "--version")
        .ok()
        .map(|v| v.trim().to_string())
    } else {
      None
    };
    let ffmpeg_version = if ffmpeg {
      get_tool_version(&ffmpeg_path, "-version")
        .ok()
        .and_then(|v| extract_ffmpeg_version(&v))
    } else {
      None
    };
    let ffprobe_version = if ffprobe {
      get_tool_version(&ffprobe_path, "-version")
        .ok()
        .and_then(|v| extract_ffmpeg_version(&v))
    } else {
      None
    };

    if yt_dlp && yt_dlp_version.is_none() {
      log::warn!("[video_download] yt-dlp 文件存在但版本检测失败，仍视为可用");
    }
    if ffmpeg && ffmpeg_version.is_none() {
      log::warn!("[video_download] ffmpeg 文件存在但版本检测失败，仍视为可用");
    }

    ToolStatus {
      yt_dlp,
      ffmpeg,
      ffprobe,
      yt_dlp_path: if yt_dlp {
        Some(yt_dlp_path.to_string_lossy().to_string())
      } else {
        None
      },
      ffmpeg_path: if ffmpeg {
        Some(ffmpeg_path.to_string_lossy().to_string())
      } else {
        None
      },
      ffprobe_path: if ffprobe {
        Some(ffprobe_path.to_string_lossy().to_string())
      } else {
        None
      },
      yt_dlp_version,
      ffmpeg_version,
      ffprobe_version,
    }
  })
  .await
  .unwrap_or(ToolStatus {
    yt_dlp: false,
    ffmpeg: false,
    ffprobe: false,
    yt_dlp_path: None,
    ffmpeg_path: None,
    ffprobe_path: None,
    yt_dlp_version: None,
    ffmpeg_version: None,
    ffprobe_version: None,
  })
}

/// 运行工具获取版本输出（带 5 秒超时，避免卡住下载流程）
fn get_tool_version(path: &std::path::Path, arg: &str) -> Result<String, String> {
  let path = path.to_path_buf();
  let arg = arg.to_string();
  let (tx, rx) = std::sync::mpsc::channel();
  std::thread::spawn(move || {
    let mut cmd = std::process::Command::new(&path);
    cmd.arg(&arg).env("PYTHONIOENCODING", "utf-8");
    #[cfg(target_os = "windows")]
    {
      use std::os::windows::process::CommandExt;
      cmd.creation_flags(0x08000000);
    }
    let result = cmd
      .output()
      .map_err(|e| format!("执行失败: {}", e))
      .and_then(|output| String::from_utf8(output.stdout).map_err(|e| format!("解码失败: {}", e)));
    let _ = tx.send(result);
  });
  match rx.recv_timeout(std::time::Duration::from_secs(5)) {
    Ok(result) => result,
    Err(_) => Err("版本检测超时".to_string()),
  }
}

/// 从 ffmpeg -version 输出中提取版本号（第一行的第二个字段）
/// 例如: "ffmpeg version 6.1.1 Copyright ..." -> "6.1.1"
fn extract_ffmpeg_version(output: &str) -> Option<String> {
  output.lines().next().and_then(|line| {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 3
      && (parts[0].to_ascii_lowercase().contains("ffmpeg")
        || parts[0].to_ascii_lowercase().contains("ffprobe"))
    {
      Some(parts[2].to_string())
    } else {
      None
    }
  })
}

fn which(name: &str) -> Option<PathBuf> {
  let exe_name = if cfg!(windows) {
    format!("{}.exe", name)
  } else {
    name.to_string()
  };

  if let Ok(path) = std::env::var("PATH") {
    for dir in std::env::split_paths(&path) {
      let full = dir.join(&exe_name);
      if full.exists() {
        return Some(full);
      }
    }
  }
  None
}

/// 下载 yt-dlp
pub async fn download_yt_dlp<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
  let dl = get_downloader();
  if !dl.begin_tool_download("yt-dlp").await {
    log::info!("[video_download] yt-dlp 已在下载中，跳过");
    return Ok(());
  }

  let result = download_yt_dlp_inner(&app).await;
  dl.end_tool_download("yt-dlp").await;
  result
}

async fn download_yt_dlp_inner<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
  let dir = tools_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;

  let target = dir.join(yt_dlp_filename());
  let url = yt_dlp_download_url();

  log::info!("[video_download] 下载 yt-dlp...");
  download_file_with_progress(url, &target, app, "yt-dlp").await?;

  // macOS/Linux 上需要加执行权限
  #[cfg(not(target_os = "windows"))]
  {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&target) {
      let mut perms = meta.permissions();
      perms.set_mode(perms.mode() | 0o755);
      let _ = std::fs::set_permissions(&target, perms);
    }
  }

  get_downloader().set_yt_dlp_path(target).await;

  log::info!("[video_download] yt-dlp 下载完成");
  Ok(())
}

/// ffmpeg 下载 URL（按平台）
fn ffmpeg_download_url() -> &'static str {
  #[cfg(target_os = "windows")]
  {
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
  }
  #[cfg(target_os = "macos")]
  {
    "https://evermeet.cx/ffmpeg/getrelease/zip"
  }
  #[cfg(target_os = "linux")]
  {
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"
  }
}

/// 下载 ffmpeg + ffprobe
pub async fn download_ffmpeg<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
  let dl = get_downloader();
  if !dl.begin_tool_download("ffmpeg").await {
    log::info!("[video_download] ffmpeg 已在下载中，跳过");
    return Ok(());
  }

  let result = download_ffmpeg_inner(&app).await;
  dl.end_tool_download("ffmpeg").await;
  result
}

async fn download_ffmpeg_inner<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
  let dir = tools_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;

  let url = ffmpeg_download_url();

  // 按平台选择归档文件名和解压方式
  #[cfg(target_os = "linux")]
  let archive_path = dir.join("ffmpeg.tar.xz");
  #[cfg(not(target_os = "linux"))]
  let archive_path = dir.join("ffmpeg.zip");

  log::info!("[video_download] 下载 ffmpeg...");
  download_file_with_progress(url, &archive_path, app, "ffmpeg").await?;

  // 解压
  log::info!("[video_download] 解压 ffmpeg...");
  let extractor = crate::extraction::Extractor;
  #[cfg(target_os = "linux")]
  {
    extractor
      .extract_tar_xz(&archive_path, &dir, None)
      .await
      .map_err(|e| format!("解压失败: {}", e))?;
  }
  #[cfg(not(target_os = "linux"))]
  {
    extractor
      .extract_zip(&archive_path, &dir, None)
      .await
      .map_err(|e| format!("解压失败: {}", e))?;
  }

  // evermeet.cx 的 ffmpeg zip 只含 ffmpeg 单个二进制，ffprobe 必须单独下载，
  // 否则 macOS 上工具检查永远缺 ffprobe、视频下载不可用。
  #[cfg(target_os = "macos")]
  {
    let ffprobe_archive = dir.join("ffprobe.zip");
    download_file_with_progress(
      "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip",
      &ffprobe_archive,
      app,
      "ffprobe",
    )
    .await?;
    extractor
      .extract_zip(&ffprobe_archive, &dir, None)
      .await
      .map_err(|e| format!("解压 ffprobe 失败: {}", e))?;
    let _ = std::fs::remove_file(&ffprobe_archive);
  }

  // 找到解压出来的 ffmpeg 和 ffprobe
  // 解压后结构可能是:
  //   1. video-tools/ffmpeg (macOS evermeet.cx: 根目录直接是二进制)
  //   2. video-tools/bin/ffmpeg (flatten 后，单层目录被拍平)
  //   3. video-tools/xxx/bin/ffmpeg (没被拍平，保留原始子目录)
  let mut found_ffmpeg = None;
  let mut found_ffprobe = None;

  // 先检查根目录（macOS evermeet.cx 的 zip 解压后直接在根目录）
  let root_ffmpeg = dir.join(ffmpeg_filename());
  let root_ffprobe = dir.join(ffprobe_filename());
  if root_ffmpeg.exists() {
    found_ffmpeg = Some(root_ffmpeg);
  }
  if root_ffprobe.exists() {
    found_ffprobe = Some(root_ffprobe);
  }

  // 再检查直接的 bin 目录（flatten 后的情况）
  if found_ffmpeg.is_none() || found_ffprobe.is_none() {
    let direct_bin = dir.join("bin");
    if direct_bin.exists() {
      let ffmpeg = direct_bin.join(ffmpeg_filename());
      let ffprobe = direct_bin.join(ffprobe_filename());
      if found_ffmpeg.is_none() && ffmpeg.exists() {
        found_ffmpeg = Some(ffmpeg);
      }
      if found_ffprobe.is_none() && ffprobe.exists() {
        found_ffprobe = Some(ffprobe);
      }
    }
  }

  // 如果直接 bin 没找到，再遍历子目录查找（兼容未拍平的情况）
  if found_ffmpeg.is_none() || found_ffprobe.is_none() {
    if let Ok(entries) = std::fs::read_dir(&dir) {
      for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.file_name().and_then(|n| n.to_str()) != Some("bin") {
          let bin_dir = path.join("bin");
          if bin_dir.exists() {
            let ffmpeg = bin_dir.join(ffmpeg_filename());
            let ffprobe = bin_dir.join(ffprobe_filename());
            if found_ffmpeg.is_none() && ffmpeg.exists() {
              found_ffmpeg = Some(ffmpeg);
            }
            if found_ffprobe.is_none() && ffprobe.exists() {
              found_ffprobe = Some(ffprobe);
            }
          }
        }
      }
    }
  }

  // 复制到工具目录根
  if let Some(src) = found_ffmpeg {
    let dst = dir.join(ffmpeg_filename());
    let _ = std::fs::copy(&src, &dst);
    // macOS/Linux 上需要加执行权限
    #[cfg(not(target_os = "windows"))]
    {
      use std::os::unix::fs::PermissionsExt;
      if let Ok(meta) = std::fs::metadata(&dst) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(&dst, perms);
      }
    }
  }
  if let Some(src) = found_ffprobe {
    let dst = dir.join(ffprobe_filename());
    let _ = std::fs::copy(&src, &dst);
    #[cfg(not(target_os = "windows"))]
    {
      use std::os::unix::fs::PermissionsExt;
      if let Ok(meta) = std::fs::metadata(&dst) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(&dst, perms);
      }
    }
    // 单独发 ffprobe 的完成进度，让前端显示独立提示框
    let _ = app.emit(
      "video-download:tool-download-progress",
      serde_json::json!({
        "tool": "ffprobe",
        "progress": 100.0,
        "downloaded": 1,
        "total": 1,
      }),
    );
  }

  // 清理归档
  let _ = std::fs::remove_file(&archive_path);

  log::info!("[video_download] ffmpeg 下载完成");
  Ok(())
}

/// 带进度的文件下载
async fn download_file_with_progress<R: Runtime>(
  url: &str,
  target: &std::path::Path,
  app: &tauri::AppHandle<R>,
  tool_name: &str,
) -> Result<(), String> {
  let client = reqwest::Client::builder()
    .build()
    .map_err(|e| format!("创建客户端失败: {}", e))?;

  let resp = client
    .get(url)
    .send()
    .await
    .map_err(|e| format!("下载失败: {}", e))?;

  let total_size = resp.content_length().unwrap_or(0);
  let mut downloaded: u64 = 0;

  use tokio::io::AsyncWriteExt;
  let mut file = tokio::fs::File::create(target)
    .await
    .map_err(|e| format!("创建文件失败: {}", e))?;

  let mut stream = resp.bytes_stream();
  use futures_util::StreamExt;

  while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(|e| format!("下载错误: {}", e))?;
    file
      .write_all(&chunk)
      .await
      .map_err(|e| format!("写入文件失败: {}", e))?;
    downloaded += chunk.len() as u64;

    // 发送进度事件
    if total_size > 0 {
      let progress = (downloaded as f64 / total_size as f64) * 100.0;
      let _ = app.emit(
        "video-download:tool-download-progress",
        serde_json::json!({
          "tool": tool_name,
          "progress": progress,
          "downloaded": downloaded,
          "total": total_size,
        }),
      );
    }
  }

  file.flush().await.map_err(|e| e.to_string())?;
  Ok(())
}

#[tauri::command]
pub async fn video_download_check_tools() -> Result<ToolStatus, String> {
  Ok(check_tools().await)
}

#[tauri::command]
pub async fn video_download_download_yt_dlp<R: Runtime>(
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  download_yt_dlp(app).await
}

/// 更新或下载 yt-dlp：先尝试 yt-dlp -U 自更新，失败则从 GitHub 下载
#[tauri::command]
pub async fn video_download_update_tool<R: Runtime>(
  app: tauri::AppHandle<R>,
) -> Result<String, String> {
  let dir = tools_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;

  let yt_dlp_path = dir.join(yt_dlp_filename());

  // 如果文件存在，先尝试 yt-dlp -U 自更新
  if yt_dlp_path.exists() {
    log::info!("[video_download] 尝试 yt-dlp -U 自更新...");
    let _ = app.emit(
      "video-download:tool-update-status",
      serde_json::json!({ "status": "updating", "message": "正在自更新 yt-dlp..." }),
    );

    let mut tcmd = tokio::process::Command::new(&yt_dlp_path);
    tcmd
      .arg("-U")
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::piped());
    #[cfg(target_os = "windows")]
    {
      tcmd.creation_flags(0x08000000);
    }
    let output = tcmd.output().await;

    match output {
      Ok(result) => {
        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        let combined = format!("{}\n{}", stdout, stderr);

        if result.status.success() {
          log::info!("[video_download] yt-dlp 自更新成功");
          let _ = app.emit(
            "video-download:tool-update-status",
            serde_json::json!({ "status": "done", "message": "yt-dlp 更新成功" }),
          );
          return Ok("yt-dlp 自更新成功".to_string());
        }

        // 检查是否是网络错误或不可用
        let is_network_error = combined.contains("unable")
          || combined.contains("connection")
          || combined.contains("timeout")
          || combined.contains("proxy")
          || combined.contains("403")
          || combined.contains("ERROR");

        if !is_network_error {
          // 不是网络错误，可能是版本已最新
          log::info!("[video_download] yt-dlp 已是最新版本");
          let _ = app.emit(
            "video-download:tool-update-status",
            serde_json::json!({ "status": "done", "message": "yt-dlp 已是最新版本" }),
          );
          return Ok("yt-dlp 已是最新版本".to_string());
        }

        log::warn!(
          "[video_download] yt-dlp -U 失败，回退到 GitHub 下载: {}",
          combined.trim()
        );
      }
      Err(e) => {
        log::warn!(
          "[video_download] yt-dlp -U 执行失败，回退到 GitHub 下载: {}",
          e
        );
      }
    }
  }

  // 文件不存在或自更新失败，从 GitHub 下载
  let _ = app.emit(
    "video-download:tool-update-status",
    serde_json::json!({ "status": "downloading", "message": "正在从 GitHub 下载 yt-dlp..." }),
  );

  let url = yt_dlp_download_url();
  download_file_with_progress(url, &yt_dlp_path, &app, "yt-dlp").await?;

  // macOS/Linux 上需要加执行权限
  #[cfg(not(target_os = "windows"))]
  {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&yt_dlp_path) {
      let mut perms = meta.permissions();
      perms.set_mode(perms.mode() | 0o755);
      let _ = std::fs::set_permissions(&yt_dlp_path, perms);
    }
  }

  get_downloader().set_yt_dlp_path(yt_dlp_path).await;

  let _ = app.emit(
    "video-download:tool-update-status",
    serde_json::json!({ "status": "done", "message": "yt-dlp 下载完成" }),
  );

  log::info!("[video_download] yt-dlp 下载完成");

  // 检查 ffmpeg 和 ffprobe 是否存在，不存在则下载
  let ffmpeg_path = dir.join(ffmpeg_filename());
  let ffprobe_path = dir.join(ffprobe_filename());

  if !ffmpeg_path.exists() || !ffprobe_path.exists() {
    log::info!("[video_download] 检测到 ffmpeg/ffprobe 缺失，开始下载...");
    let _ = app.emit(
      "video-download:tool-update-status",
      serde_json::json!({ "status": "downloading", "message": "正在下载 ffmpeg/ffprobe..." }),
    );
    download_ffmpeg(app.clone()).await?;
    let _ = app.emit(
      "video-download:tool-update-status",
      serde_json::json!({ "status": "done", "message": "ffmpeg 下载完成" }),
    );
    log::info!("[video_download] ffmpeg 下载完成");
  } else {
    log::info!("[video_download] ffmpeg/ffprobe 已存在，跳过下载");
  }

  Ok("工具更新完成".to_string())
}

#[tauri::command]
pub async fn video_download_download_ffmpeg<R: Runtime>(
  app: tauri::AppHandle<R>,
) -> Result<(), String> {
  download_ffmpeg(app).await
}

#[tauri::command]
pub async fn video_download_is_downloading_tools() -> Result<bool, String> {
  Ok(get_downloader().is_downloading_tools().await)
}

#[tauri::command]
pub async fn video_download_get_last_cookie_time() -> Result<i64, String> {
  Ok(get_downloader().get_last_cookie_time().await)
}

#[tauri::command]
pub async fn video_download_refresh_cookie<R: Runtime>(
  app: tauri::AppHandle<R>,
) -> Result<i64, String> {
  let settings = get_downloader().get_settings().await;
  if !settings.cookie_from_browser {
    return Err("Cookie 功能未启用".to_string());
  }

  let dl = get_downloader().clone();
  let _cookie_path = export_vps_cookies_to_file(true, &dl, &app, None).await?;
  let last_time = dl.get_last_cookie_time().await;
  let _ = app.emit("video-download:cookie-refreshed", &last_time);
  Ok(last_time)
}

/// 打开下载目录（在资源管理器/访达中显示）
#[tauri::command]
pub async fn video_download_open_dir(path: String) -> Result<(), String> {
  // 规范化路径（Windows 下把 / 转成 \）
  let target = if cfg!(windows) {
    PathBuf::from(path.replace('/', "\\"))
  } else {
    PathBuf::from(&path)
  };
  let dir = if target.is_file() {
    target.parent().map(|d| d.to_path_buf()).unwrap_or(target)
  } else {
    target
  };

  log::info!("[video_download] open_dir 请求, 原始路径: {}", path);
  log::info!("[video_download] open_dir 目标目录: {}", dir.display());
  log::info!("[video_download] open_dir 目录是否存在: {}", dir.exists());

  if !dir.exists() {
    log::error!("[video_download] open_dir 失败: 目录不存在");
    return Err("目录不存在".to_string());
  }

  let dir_str = dir.to_string_lossy().to_string();

  #[cfg(target_os = "windows")]
  {
    log::info!("[video_download] open_dir 执行: explorer \"{}\"", dir_str);
    // Windows: 用 explorer 打开目录
    std::process::Command::new("explorer")
      .arg(&dir_str)
      .spawn()
      .map_err(|e| format!("Failed to open dir: {e}"))?;
  }
  #[cfg(target_os = "macos")]
  {
    std::process::Command::new("open")
      .arg(&dir_str)
      .spawn()
      .map_err(|e| format!("Failed to open dir: {e}"))?;
  }
  #[cfg(target_os = "linux")]
  {
    std::process::Command::new("xdg-open")
      .arg(&dir_str)
      .spawn()
      .map_err(|e| format!("Failed to open dir: {e}"))?;
  }

  Ok(())
}

/// 获取日志目录路径
#[tauri::command]
pub async fn video_download_get_log_dir() -> Result<String, String> {
  let dir = tools_dir().join("logs");
  let _ = std::fs::create_dir_all(&dir);
  Ok(dir.to_string_lossy().to_string())
}

/// 获取指定任务的日志文件路径
#[tauri::command]
pub async fn video_download_get_task_log_path(task_id: String) -> Result<String, String> {
  let dir = tools_dir().join("logs");
  let _ = std::fs::create_dir_all(&dir);
  let path = dir.join(format!("download_{}.log", task_id));
  Ok(path.to_string_lossy().to_string())
}

/// 打开失败时的自愈定位：任务记录的文件名可能因 Windows stdout 编码
/// 丢失 emoji 等字符（emoji→空格）。从记录名里提取数字 ID，在原目录里
/// 按名字含该 ID 的视频文件找回真实路径。
fn relocate_broken_file(path: &std::path::Path) -> Option<PathBuf> {
  let name = path.file_name()?.to_string_lossy().to_string();
  let id = name
    .split(|c: char| !c.is_ascii_digit())
    .filter(|s| s.len() >= 8)
    .max_by_key(|s| s.len())?;
  let dir = path.parent()?;
  let mut best: Option<(PathBuf, u64)> = None;
  for entry in std::fs::read_dir(dir).ok()?.flatten() {
    let p = entry.path();
    if !p.is_file() {
      continue;
    }
    let n = p.file_name()?.to_string_lossy().to_string();
    let nl = n.to_lowercase();
    let is_video = [
      ".mp4", ".mkv", ".webm", ".mov", ".avi", ".m4v", ".flv", ".wmv",
    ]
    .iter()
    .any(|e| nl.ends_with(e));
    if !is_video || !n.contains(id) {
      continue;
    }
    if let Ok(meta) = std::fs::metadata(&p) {
      if best.as_ref().is_none_or(|(_, s)| meta.len() > *s) {
        best = Some((p, meta.len()));
      }
    }
  }
  best.map(|(p, _)| p)
}

/// 用默认程序打开视频文件
#[tauri::command]
pub async fn video_download_open_file(path: String) -> Result<(), String> {
  // 规范化路径（Windows 下把 / 转成 \）
  let mut p = if cfg!(windows) {
    PathBuf::from(path.replace('/', "\\"))
  } else {
    PathBuf::from(&path)
  };

  // 记录名打不开时先尝试自愈定位（文件名里 emoji 等字符丢失的场景）
  if !p.exists() {
    if let Some(found) = relocate_broken_file(&p) {
      log::info!("[video_download] open_file 自愈定位: {}", found.display());
      p = found;
    }
  }

  log::info!("[video_download] open_file 请求, 原始路径: {}", path);
  log::info!("[video_download] open_file 规范化后: {}", p.display());
  log::info!("[video_download] open_file 文件是否存在: {}", p.exists());
  log::info!("[video_download] open_file 是否文件: {}", p.is_file());

  if !p.exists() {
    log::error!("[video_download] open_file 失败: 文件不存在");
    return Err("文件不存在".to_string());
  }

  let file_str = p.to_string_lossy().to_string();

  #[cfg(target_os = "windows")]
  {
    // Windows 下用 explorer 直接打开文件（更可靠，支持中文路径）
    log::info!("[video_download] open_file 执行: explorer \"{}\"", file_str);
    match std::process::Command::new("explorer")
      .arg(&file_str)
      .spawn()
    {
      Ok(_) => {
        log::info!("[video_download] open_file explorer 启动成功");
      }
      Err(e) => {
        log::error!("[video_download] open_file explorer 启动失败: {}", e);
        // fallback: 用 cmd /c start
        log::info!("[video_download] open_file fallback 到 cmd /c start");
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", &file_str]);
        #[cfg(target_os = "windows")]
        {
          use std::os::windows::process::CommandExt;
          cmd.creation_flags(0x08000000);
        }
        cmd
          .spawn()
          .map_err(|e| format!("Failed to open file: {e}"))?;
      }
    }
  }
  #[cfg(target_os = "macos")]
  {
    std::process::Command::new("open")
      .arg(&file_str)
      .spawn()
      .map_err(|e| format!("Failed to open file: {e}"))?;
  }
  #[cfg(target_os = "linux")]
  {
    std::process::Command::new("xdg-open")
      .arg(&file_str)
      .spawn()
      .map_err(|e| format!("Failed to open file: {e}"))?;
  }

  Ok(())
}
