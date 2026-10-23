// ============================================================
// 任务队列：把「开始匹配」变成可堆积的任务（串行执行，一次一个）
// ============================================================

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::video_matcher::{
  extract_frame_hashes, find_matching_segments, resolve_cache_dir, video_matcher_get_settings,
  MatchSegment, MatcherProgress,
};

/// 匹配任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum MatcherTaskStatus {
  Waiting,
  Running,
  Completed,
  Failed,
  Cancelled,
}

/// 匹配任务
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatcherTask {
  pub id: String,
  pub source_path: String, // 对标视频
  pub target_path: String, // 原视频
  pub status: MatcherTaskStatus,
  pub percent: u32,
  pub message: String,
  pub segment_count: u32,
  pub created_at: i64,
  pub finished_at: Option<i64>,
  /// 完成后的匹配结果（仅 Completed 有值，任务列表页直接展示/导出）
  pub segments: Option<Vec<MatchSegment>>,
}

/// 进程内任务表（不持久化：应用重启后任务列表清空，特征缓存仍在）
static MATCHER_TASKS: once_cell::sync::Lazy<std::sync::Mutex<Vec<MatcherTask>>> =
  once_cell::sync::Lazy::new(|| std::sync::Mutex::new(Vec::new()));

/// 通知任务列表变化
fn emit_tasks_changed<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
  use tauri::Emitter;
  let _ = app.emit("video-matcher-tasks-changed", ());
}

/// Tauri 命令：添加匹配任务（进入队列尾部）
#[tauri::command]
pub fn video_matcher_add_task(
  app: tauri::AppHandle,
  source_path: String,
  target_path: String,
) -> Result<String, String> {
  if source_path.is_empty() || target_path.is_empty() {
    return Err("对标视频和原视频都必须选择".to_string());
  }
  if !Path::new(&source_path).exists() {
    return Err(format!("对标视频不存在: {source_path}"));
  }
  if !Path::new(&target_path).exists() {
    return Err(format!("原视频不存在: {target_path}"));
  }
  let id = uuid::Uuid::new_v4().to_string();
  let task = MatcherTask {
    id: id.clone(),
    source_path,
    target_path,
    status: MatcherTaskStatus::Waiting,
    percent: 0,
    message: "排队中".to_string(),
    segment_count: 0,
    created_at: chrono::Utc::now().timestamp(),
    finished_at: None,
    segments: None,
  };
  MATCHER_TASKS.lock().unwrap().push(task);
  emit_tasks_changed(&app);
  Ok(id)
}

/// Tauri 命令：列出所有任务
#[tauri::command]
pub fn video_matcher_list_tasks() -> Vec<MatcherTask> {
  MATCHER_TASKS.lock().unwrap().clone()
}

/// Tauri 命令：取消任务（等待中的直接标记取消；运行中的由执行循环在阶段间检查退出）
#[tauri::command]
pub fn video_matcher_cancel_task(app: tauri::AppHandle, task_id: String) -> Result<bool, String> {
  let mut tasks = MATCHER_TASKS.lock().unwrap();
  let mut cancelled = false;
  for t in tasks.iter_mut() {
    if t.id == task_id
      && matches!(
        t.status,
        MatcherTaskStatus::Waiting | MatcherTaskStatus::Running
      )
    {
      t.status = MatcherTaskStatus::Cancelled;
      t.message = "已取消".to_string();
      t.finished_at = Some(chrono::Utc::now().timestamp());
      cancelled = true;
    }
  }
  drop(tasks);
  emit_tasks_changed(&app);
  Ok(cancelled)
}

/// Tauri 命令：删除任务（从列表移除）
#[tauri::command]
pub fn video_matcher_delete_task(app: tauri::AppHandle, task_id: String) -> Result<bool, String> {
  let mut tasks = MATCHER_TASKS.lock().unwrap();
  let before = tasks.len();
  tasks.retain(|t| t.id != task_id);
  let removed = tasks.len() != before;
  drop(tasks);
  emit_tasks_changed(&app);
  Ok(removed)
}

/// Tauri 命令：清空已完成/失败/取消的任务
#[tauri::command]
pub fn video_matcher_clear_finished_tasks(app: tauri::AppHandle) -> Result<u32, String> {
  let mut tasks = MATCHER_TASKS.lock().unwrap();
  let before = tasks.len();
  tasks.retain(|t| {
    matches!(
      t.status,
      MatcherTaskStatus::Waiting | MatcherTaskStatus::Running
    )
  });
  let removed = (before - tasks.len()) as u32;
  drop(tasks);
  emit_tasks_changed(&app);
  Ok(removed)
}

/// Tauri 命令：启动队列调度（前端在添加任务后调用；幂等——同时只有一个调度循环在跑）
#[tauri::command]
pub async fn video_matcher_run_queue(app: tauri::AppHandle) -> Result<(), String> {
  {
    let tasks = MATCHER_TASKS.lock().unwrap();
    if tasks.iter().any(|t| t.status == MatcherTaskStatus::Running) {
      return Ok(()); // 调度已在跑
    }
    if !tasks.iter().any(|t| t.status == MatcherTaskStatus::Waiting) {
      return Ok(());
    }
  }

  // 后台调度循环：串行处理 Waiting 任务，直到队列空
  tauri::async_runtime::spawn(async move {
    loop {
      let next_id = {
        let tasks = MATCHER_TASKS.lock().unwrap();
        tasks
          .iter()
          .find(|t| t.status == MatcherTaskStatus::Waiting)
          .map(|t| t.id.clone())
      };
      let Some(task_id) = next_id else { break };

      let (source_path, target_path) = {
        let mut tasks = MATCHER_TASKS.lock().unwrap();
        let Some(t) = tasks.iter_mut().find(|t| t.id == task_id) else {
          break;
        };
        if t.status == MatcherTaskStatus::Cancelled {
          continue;
        }
        t.status = MatcherTaskStatus::Running;
        t.percent = 0;
        t.message = "开始匹配...".to_string();
        (t.source_path.clone(), t.target_path.clone())
      };
      emit_tasks_changed(&app);

      let tid = task_id.clone();
      let is_cancelled = move || -> bool {
        MATCHER_TASKS
          .lock()
          .unwrap()
          .iter()
          .any(|t| t.id == tid && t.status == MatcherTaskStatus::Cancelled)
      };

      let result = run_match_task(&app, &task_id, &source_path, &target_path, is_cancelled).await;

      {
        let mut tasks = MATCHER_TASKS.lock().unwrap();
        if let Some(t) = tasks.iter_mut().find(|t| t.id == task_id) {
          if t.status == MatcherTaskStatus::Cancelled {
            // 用户取消了：保持 Cancelled，丢弃结果
          } else {
            match result {
              Ok(segments) => {
                t.status = MatcherTaskStatus::Completed;
                t.percent = 100;
                t.message = format!("匹配完成，{} 个片段", segments.len());
                t.segment_count = segments.len() as u32;
                t.segments = Some(segments);
              }
              Err(e) => {
                t.status = MatcherTaskStatus::Failed;
                t.message = e;
              }
            }
            t.finished_at = Some(chrono::Utc::now().timestamp());
          }
        }
      }
      emit_tasks_changed(&app);
    }
  });

  Ok(())
}

/// 单任务执行体：复用特征提取与匹配逻辑，进度写回任务表并发事件
async fn run_match_task<R: tauri::Runtime>(
  app: &tauri::AppHandle<R>,
  task_id: &str,
  source_path: &str,
  target_path: &str,
  is_cancelled: impl Fn() -> bool,
) -> Result<Vec<MatchSegment>, String> {
  use tauri::Emitter;

  let settings = video_matcher_get_settings();
  let interval = settings.interval_sec;
  let threshold = settings.threshold.clamp(0.40, 0.75);
  let min_frames = settings.min_frames.clamp(1, 6) as usize;
  let extractor = settings.extractor.clone();
  let cache_dir = resolve_cache_dir(&settings);

  let update = |percent: u32, msg: String| {
    {
      let mut tasks = MATCHER_TASKS.lock().unwrap();
      if let Some(t) = tasks.iter_mut().find(|t| t.id == task_id) {
        if t.status == MatcherTaskStatus::Cancelled {
          return; // 已取消不再更新
        }
        t.percent = percent.min(99);
        t.message = msg.clone();
      }
    }
    let _ = app.emit(
      "video-matcher-task-progress",
      MatcherProgress {
        stage: task_id.to_string(),
        percent,
        message: msg,
      },
    );
  };

  log::info!(
    "[video_matcher] 任务 {task_id} 开始 | 对标: {source_path} | 原片: {target_path} | 模型: {extractor} | 阈值: {threshold} | min_frames: {min_frames} | 间隔: {interval}s"
  );

  update(2, "提取对标视频特征...".to_string());
  let source_hashes = extract_frame_hashes(
    app,
    source_path,
    interval,
    &cache_dir,
    &extractor,
    "对标",
    5,
    40,
  )
  .await?;
  if is_cancelled() {
    return Err("已取消".to_string());
  }

  update(50, "提取原视频特征...".to_string());
  let target_hashes = extract_frame_hashes(
    app,
    target_path,
    interval,
    &cache_dir,
    &extractor,
    "原片",
    50,
    30,
  )
  .await?;
  if is_cancelled() {
    return Err("已取消".to_string());
  }

  if source_hashes.first().map(|f| f.feature.len())
    != target_hashes.first().map(|f| f.feature.len())
  {
    let msg = "特征维度不匹配——缓存可能来自不同模型，请清除缓存后重试".to_string();
    return Err(msg);
  }

  update(85, "正在匹配...".to_string());
  let segments = find_matching_segments(
    &source_hashes,
    &target_hashes,
    threshold,
    min_frames,
    interval,
  );
  log::info!(
    "[video_matcher] 任务 {task_id} 完成: {} 个片段",
    segments.len()
  );
  Ok(segments)
}
