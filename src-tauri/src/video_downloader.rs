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
use std::process::Stdio;
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
  pub proxy_id: Option<String>, // None = 直连
}

impl Default for DownloadSettings {
  fn default() -> Self {
    Self {
      download_dir: String::new(),
      max_concurrent: 10,
      max_height: 0,
      auto_paste_download: false,
      cookie_from_browser: true,
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
        inner.settings = saved;
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
        task.error_msg = Some(error);
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
      let local = dir.join("yt-dlp.exe");
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

/// 运行单个 yt-dlp 下载
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
    dirs::desktop_dir()
      .unwrap_or_else(|| dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")))
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
    "-o".to_string(),
    "%(uploader|Unknown)s_%(title).80B_%(id)s.%(ext)s".to_string(),
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

  // ffmpeg 路径
  let ffmpeg_path = tools_dir().join("ffmpeg.exe");
  if ffmpeg_path.exists() {
    args.push("--ffmpeg-location".to_string());
    args.push(ffmpeg_path.to_string_lossy().to_string());
  }

  // cookie - 从浏览器导出
  let mut cookie_error: Option<String> = None;
  let is_tiktok = url.contains("tiktok.com");
  let mut douyin_cookie_file: Option<PathBuf> = None;
  // TikTok 有专用 API 提取器，不需要 cookie
  // 抖音需要从浏览器导出 cookie，否则 yt-dlp 会报 "Fresh cookies needed" 错误
  if settings.cookie_from_browser && !is_tiktok {
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
    match export_vps_cookies_to_file(force_refresh_cookie || is_douyin, dl, app, Some(url)).await {
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
          if is_douyin {
            douyin_cookie_file = Some(cookie_file.clone());
          }
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

  // 抖音用 Python API 桥接脚本（CLI 模式无法获取访客 cookie）
  let use_douyin_bridge = is_douyin;

  let mut yt_cmd = if use_douyin_bridge {
    // 找到 douyin_download.py 脚本路径（与 src-tauri 同目录，编译后在 exe 同目录）
    let script_path = {
      let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
      // dev 模式：在 src-tauri 目录
      let dev_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("douyin_download.py");
      let release_path = exe_dir.join("douyin_download.py");
      if dev_path.exists() {
        dev_path
      } else if release_path.exists() {
        release_path
      } else {
        dev_path
      }
    };

    log::info!(
      "[video_download] 抖音 Python 桥接: script={}, yt-dlp={}",
      script_path.display(),
      yt_dlp.display()
    );

    let output_dir = {
      let inner = dl.inner.lock().await;
      if inner.settings.download_dir.is_empty() {
        dirs::download_dir().unwrap_or_else(|| PathBuf::from("."))
      } else {
        PathBuf::from(&inner.settings.download_dir)
      }
    };

    let mut cmd = std::process::Command::new("python");
    cmd
      .arg(script_path)
      .arg(url)
      .arg(&output_dir)
      .arg(yt_dlp)
      .env("PYTHONIOENCODING", "utf-8")
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::piped());

    if let Some(cf) = &douyin_cookie_file {
      cmd.arg(cf.to_string_lossy().to_string());
      log::info!("[video_download] 抖音桥接脚本传入 cookie: {}", cf.display());
    } else {
      cmd.arg("");
    }

    let max_h = {
      let inner = dl.inner.lock().await;
      inner.settings.max_height
    };
    cmd.arg(max_h.to_string());

    // 传入 VPS 浏览器 profile 路径，Python 脚本可用 cookies_from_browser 读取
    let pm = crate::profile::manager::ProfileManager::instance();
    let profiles = pm.list_profiles().unwrap_or_default();
    let vps_profile_for_path = profiles
      .into_iter()
      .find(|p| p.name.to_lowercase() == "vps登录");
    if let Some(vps_p) = &vps_profile_for_path {
      let profiles_dir = pm.get_profiles_dir();
      let user_data_dir = vps_p.get_profile_data_path(&profiles_dir);
      cmd.arg(user_data_dir.to_string_lossy().to_string());
    } else {
      cmd.arg("");
    }

    #[cfg(target_os = "windows")]
    {
      use std::os::windows::process::CommandExt;
      cmd.creation_flags(0x08000000);
    }
    cmd
  } else {
    // 其他平台用 yt-dlp CLI
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

        // 抖音 Python 桥接：解析 JSON 输出
        if use_douyin_bridge && trimmed.starts_with("{") {
          if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match msg_type {
              "progress" => {
                let percent = json.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let speed = json.get("speed").and_then(|v| v.as_u64()).unwrap_or(0);
                let downloaded = json.get("downloaded").and_then(|v| v.as_u64()).unwrap_or(0);
                let total = json.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
                let speed_text = format_size_per_sec(speed);
                dl.update_task_progress(task_id, percent, speed_text.clone(), speed, downloaded, total).await;
                let peak_text = {
                  let inner = dl.inner.lock().await;
                  inner.tasks.get(task_id).map(|t| t.peak_speed_text.clone()).unwrap_or_default()
                };
                let _ = app.emit("video-download:progress", &serde_json::json!({
                  "taskId": task_id,
                  "progress": percent,
                  "speedText": speed_text,
                  "peakSpeedText": peak_text,
                  "downloaded": downloaded,
                  "total": total,
                  "status": "downloading"
                }));
              }
              "finished" => {
                if let Some(fname) = json.get("filename").and_then(|v| v.as_str()) {
                  if !fname.is_empty() {
                    dl.set_task_filename(task_id, fname.to_string()).await;
                  }
                }
              }
              "retry" => {
                let attempt = json.get("attempt").and_then(|v| v.as_i64()).unwrap_or(0);
                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("重试中");
                dl.set_task_status_text(task_id, format!("{} (第 {} 次)", message, attempt)).await;
                let _ = app.emit("video-download:status", &serde_json::json!({
                  "taskId": task_id,
                  "statusText": format!("{} (第 {} 次)", message, attempt),
                  "status": "downloading"
                }));
              }
              "done" => {
                completed = true;
                if let Some(title) = json.get("title").and_then(|v| v.as_str()) {
                  if !title.is_empty() {
                    dl.set_task_title(task_id, title.to_string()).await;
                  }
                }
                if let Some(res) = json.get("resolution").and_then(|v| v.as_str()) {
                  if !res.is_empty() {
                    dl.set_task_resolution(task_id, res.to_string()).await;
                  }
                }
              }
              "error" => {
                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("未知错误");
                error_output.push_str(message);
                error_output.push('\n');
              }
              _ => {}
            }
            continue;
          }
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
      let video_id = extract_youtube_id(url);
      if let Some(ref id) = video_id {
        if let Some(found) = find_final_video_file(&output_dir, id) {
          Some(found)
        } else {
          // 没找到的话，fallback 到 filename
          task_filename.as_ref().map(|fname| {
            let p = PathBuf::from(fname);
            if p.is_absolute() && p.exists() {
              p
            } else {
              output_dir.join(fname)
            }
          })
        }
      } else {
        // 非 YouTube，用 filename
        task_filename.as_ref().map(|fname| {
          let p = PathBuf::from(fname);
          if p.is_absolute() && p.exists() {
            p
          } else {
            output_dir.join(fname)
          }
        })
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
        .post(crate::bwbrowser_cloud::BWBROWSER_API_URL)
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

/// 使用 yt-dlp --cookies-from-browser 从指定 Chrome 用户数据目录提取 Cookie
/// 这种方式能拿到所有持久化在 SQLite 里的 cookie，比 CDP 更全面
/// cookie_url 决定 yt-dlp 访问哪个网站提取对应域名的 cookie
async fn extract_cookies_via_ytdlp(
  yt_dlp_path: &std::path::Path,
  user_data_dir: &std::path::Path,
  output_path: &std::path::Path,
  cookie_url: &str,
) -> Result<usize, String> {
  let tmp_path = output_path.with_extension("tmp.txt");
  let _ = std::fs::remove_file(&tmp_path);

  let browser_arg = format!("chrome:{}:Default", user_data_dir.to_string_lossy());

  log::info!(
    "[video_download] 使用 yt-dlp 从浏览器数据目录提取 Cookie ({}): {}",
    cookie_url,
    user_data_dir.display()
  );

  let mut cmd = tokio::process::Command::new(yt_dlp_path);
  cmd
    .arg("--cookies-from-browser")
    .arg(&browser_arg)
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

  let output = tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output())
    .await
    .map_err(|_| "yt-dlp 提取 Cookie 超时（20秒）".to_string())?
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

  // 获取 yt-dlp 路径
  let inner = dl.inner.lock().await;
  let yt_dlp_path = inner.yt_dlp_path.clone();
  drop(inner);

  // 抖音跳过 yt-dlp 提取（yt-dlp 访问 douyin.com 会 403），
  // 直接走 CDP 导航 + 导出，从浏览器内存获取新鲜 cookie
  if !needs_douyin_nav {
    if let Some(yt_dlp) = yt_dlp_path {
      if yt_dlp.exists() && user_data_dir.exists() {
        match extract_cookies_via_ytdlp(&yt_dlp, &user_data_dir, &cookie_path, cookie_url).await {
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
      log::warn!(
        "[video_download] CDP 导出失败（浏览器可能未运行）: {}，自动启动 VPS 浏览器后重试",
        cdp_err
      );

      // 自动启动 VPS 浏览器
      log::info!("[video_download] 正在启动 VPS 浏览器以提取 Cookie...");
      let _ = app.emit("video-download:auto-open-vps", ());

      // 轮询等待 CDP 可用（最多 30 秒）
      for i in 1..=15 {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        log::info!("[video_download] 等待 VPS 浏览器 CDP 可用... (第 {} 次)", i);
        // 抖音: 先导航到 douyin.com 再提取 cookie
        if needs_douyin_nav {
          let _ = crate::cookie_sync::navigate_to_url(&vps_profile, cookie_url).await;
          tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
        match crate::cookie_sync::export_cookies_via_cdp(&vps_profile).await {
          Ok(json) => {
            let cookies: Vec<serde_json::Value> =
              serde_json::from_str(&json).map_err(|e| format!("解析 cookie 失败: {}", e))?;
            log::info!(
              "[video_download] 启动后通过 CDP 导出到 {} 条 cookie",
              cookies.len()
            );
            let txt = cookies_to_netscape(&cookies);
            std::fs::write(&cookie_path, txt)
              .map_err(|e| format!("写入 cookie 文件失败: {}", e))?;
            dl.set_last_cookie_time(now_secs()).await;
            log::info!("[video_download] Cookie 已刷新（CDP 启动后导出）");
            return Ok(cookie_path);
          }
          Err(_) => continue,
        }
      }

      // CDP 30 秒内仍不可用，最终 fallback 到 SQLite 数据库
      log::warn!("[video_download] CDP 30 秒内不可用，回退到 SQLite 数据库");
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
    use std::process::Command;
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
fn tools_dir() -> PathBuf {
  let app_dir = crate::app_dirs::data_dir();
  app_dir.join("video-tools")
}

/// 轻量检查：yt-dlp 文件是否存在（不启动进程，纯文件判断）
fn yt_dlp_exists() -> bool {
  let dir = tools_dir();
  dir.join("yt-dlp.exe").exists()
}

/// 轻量检查：三个工具文件是否都存在（不启动进程，纯文件判断，不阻塞）
#[tauri::command]
pub async fn video_download_tools_exist() -> bool {
  let dir = tools_dir();
  dir.join("yt-dlp.exe").exists()
    && dir.join("ffmpeg.exe").exists()
    && dir.join("ffprobe.exe").exists()
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

    let yt_dlp_path = dir.join("yt-dlp.exe");
    let ffmpeg_path = dir.join("ffmpeg.exe");
    let ffprobe_path = dir.join("ffprobe.exe");

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

  let target = dir.join("yt-dlp.exe");
  let url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

  log::info!("[video_download] 下载 yt-dlp...");
  download_file_with_progress(url, &target, app, "yt-dlp").await?;

  get_downloader().set_yt_dlp_path(target).await;

  log::info!("[video_download] yt-dlp 下载完成");
  Ok(())
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

  let zip_path = dir.join("ffmpeg.zip");
  let url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

  log::info!("[video_download] 下载 ffmpeg...");
  download_file_with_progress(url, &zip_path, app, "ffmpeg").await?;

  // 解压
  log::info!("[video_download] 解压 ffmpeg...");
  let extractor = crate::extraction::Extractor;
  extractor
    .extract_zip(&zip_path, &dir, None)
    .await
    .map_err(|e| format!("解压失败: {}", e))?;

  // 找到解压出来的 ffmpeg.exe 和 ffprobe.exe
  // 解压后结构可能是:
  //   1. video-tools/bin/ffmpeg.exe     (flatten 后，单层目录被拍平)
  //   2. video-tools/xxx/bin/ffmpeg.exe (没被拍平，保留原始子目录)
  let mut found_ffmpeg = None;
  let mut found_ffprobe = None;

  // 先检查直接的 bin 目录（flatten 后的情况）
  let direct_bin = dir.join("bin");
  if direct_bin.exists() {
    let ffmpeg = direct_bin.join("ffmpeg.exe");
    let ffprobe = direct_bin.join("ffprobe.exe");
    if ffmpeg.exists() {
      found_ffmpeg = Some(ffmpeg);
    }
    if ffprobe.exists() {
      found_ffprobe = Some(ffprobe);
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
            let ffmpeg = bin_dir.join("ffmpeg.exe");
            let ffprobe = bin_dir.join("ffprobe.exe");
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
    let dst = dir.join("ffmpeg.exe");
    let _ = std::fs::copy(&src, &dst);
  }
  if let Some(src) = found_ffprobe {
    let dst = dir.join("ffprobe.exe");
    let _ = std::fs::copy(&src, &dst);
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

  // 清理 zip
  let _ = std::fs::remove_file(&zip_path);

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

  let yt_dlp_path = dir.join("yt-dlp.exe");

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

  let url = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
  download_file_with_progress(url, &yt_dlp_path, &app, "yt-dlp").await?;

  get_downloader().set_yt_dlp_path(yt_dlp_path).await;

  let _ = app.emit(
    "video-download:tool-update-status",
    serde_json::json!({ "status": "done", "message": "yt-dlp 下载完成" }),
  );

  log::info!("[video_download] yt-dlp 下载完成");

  // 检查 ffmpeg 和 ffprobe 是否存在，不存在则下载
  let ffmpeg_path = dir.join("ffmpeg.exe");
  let ffprobe_path = dir.join("ffprobe.exe");

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

/// 用默认程序打开视频文件
#[tauri::command]
pub async fn video_download_open_file(path: String) -> Result<(), String> {
  // 规范化路径（Windows 下把 / 转成 \）
  let p = if cfg!(windows) {
    PathBuf::from(path.replace('/', "\\"))
  } else {
    PathBuf::from(&path)
  };

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
