//! Doubao video task scheduler (port of DoubaoManager `video/service.py`):
//! picks up queued tasks, assigns a pool account (least_used / round_robin),
//! runs same-account jobs serially while cross-account jobs run within
//! `max_concurrency`, enforces the daily-quota business-day window, and
//! requeues on rate limits / session expiry / captcha waits instead of
//! failing the task.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::Emitter;

use crate::doubao_executor::DoubaoProgress;
use crate::doubao_manager::DoubaoVideoTask;

const TASK_EVENT: &str = "doubao-task:progress";
const WAIT_ACCOUNT_POLL: Duration = Duration::from_secs(5);

struct SchedulerState {
  active: tokio::sync::Mutex<HashSet<String>>,
  resume_ids: tokio::sync::Mutex<HashSet<String>>,
  cancellations: tokio::sync::Mutex<HashMap<String, Arc<AtomicBool>>>,
  account_locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
  semaphore: Mutex<Arc<tokio::sync::Semaphore>>,
  semaphore_limit: AtomicU32,
}

static STATE: OnceLock<SchedulerState> = OnceLock::new();

fn state() -> &'static SchedulerState {
  STATE.get_or_init(|| SchedulerState {
    active: tokio::sync::Mutex::new(HashSet::new()),
    resume_ids: tokio::sync::Mutex::new(HashSet::new()),
    cancellations: tokio::sync::Mutex::new(HashMap::new()),
    account_locks: tokio::sync::Mutex::new(HashMap::new()),
    semaphore: Mutex::new(Arc::new(tokio::sync::Semaphore::new(1))),
    semaphore_limit: AtomicU32::new(1),
  })
}

/// Queue one task into the scheduler. Safe to call repeatedly: a task that is
/// already running is left alone. `resume` marks a generating task whose
/// conversation should be re-polled instead of resubmitted.
pub fn schedule(task_id: &str, resume: bool) {
  let task_id = task_id.to_string();
  tauri::async_runtime::spawn(async move {
    let state = state();
    if state.active.lock().await.contains(&task_id) {
      return;
    }
    if resume {
      state.resume_ids.lock().await.insert(task_id.clone());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    state
      .cancellations
      .lock()
      .await
      .insert(task_id.clone(), cancel.clone());
    state.active.lock().await.insert(task_id.clone());
    tauri::async_runtime::spawn(run_flow(task_id, cancel));
  });
}

pub async fn cancel_scheduled(task_id: &str) -> bool {
  match state().cancellations.lock().await.get(task_id) {
    Some(cancel) => {
      cancel.store(true, Ordering::SeqCst);
      true
    }
    None => false,
  }
}

/// Resume everything that survived an app restart: queued tasks resubmit,
/// generating tasks with a conversation re-poll instead (no double quota).
pub async fn resume_queued() -> usize {
  let mut scheduled = 0;
  for task in crate::doubao_manager::list_tasks() {
    match task.status.as_str() {
      "queued" => {
        schedule(&task.id, false);
        scheduled += 1;
      }
      "generating" | "starting" | "verifying" => {
        if task.conversation_id.is_some() {
          schedule(&task.id, true);
        } else {
          crate::doubao_manager::update_task(&task.id, |t| {
            t.status = "queued".to_string();
            t.error_message = Some("应用已停止，重新排队".to_string());
          });
          schedule(&task.id, false);
        }
        scheduled += 1;
      }
      _ => {}
    }
  }
  scheduled
}

fn emit_task_event(task_id: &str, status: &str, error_message: Option<&str>) {
  if let Some(app) = app_handle() {
    let _ = app.emit(
      TASK_EVENT,
      serde_json::json!({
        "task_id": task_id,
        "status": status,
        "error_message": error_message,
      }),
    );
  }
}

fn app_handle() -> Option<tauri::AppHandle> {
  crate::ai_agent::APP_HANDLE.get().cloned()
}

fn update_task_status(task_id: &str, apply: impl FnOnce(&mut DoubaoVideoTask)) {
  let before =
    crate::doubao_manager::find_task(task_id).map(|t| (t.status.clone(), t.error_message.clone()));
  if let Some(task) = crate::doubao_manager::update_task(task_id, apply) {
    if before.as_ref().map(|(s, _)| s.as_str()) != Some(task.status.as_str()) {
      crate::doubao_executor::debug_log(&format!(
        "任务 {} 状态 {} -> {}{}",
        task_id,
        before.as_ref().map(|(s, _)| s.as_str()).unwrap_or("(新建)"),
        task.status,
        match task.error_message.as_deref() {
          Some(message) => format!("｜{message}"),
          None => String::new(),
        }
      ));
      emit_task_event(task_id, &task.status, task.error_message.as_deref());
    }
  }
}

fn apply_progress(task_id: &str, progress: &DoubaoProgress) {
  update_task_status(task_id, |task| {
    if let Some(status) = &progress.status {
      task.status = status.clone();
    }
    task.error_message = progress.error_message.clone();
    if let Some(conversation_id) = &progress.conversation_id {
      if !conversation_id.is_empty() {
        task.conversation_id = Some(conversation_id.clone());
      }
    }
    if let Some(section_id) = &progress.section_id {
      if !section_id.is_empty() {
        task.section_id = Some(section_id.clone());
      }
    }
    if let Some(question_id) = &progress.question_id {
      if !question_id.is_empty() {
        task.question_id = Some(question_id.clone());
      }
    }
  });
}

async fn ensure_semaphore(limit: u32) -> Arc<tokio::sync::Semaphore> {
  let state = state();
  if state.semaphore_limit.load(Ordering::SeqCst) != limit {
    let mut semaphore = state.semaphore.lock().unwrap();
    if state.semaphore_limit.load(Ordering::SeqCst) != limit {
      *semaphore = Arc::new(tokio::sync::Semaphore::new(limit.max(1) as usize));
      state.semaphore_limit.store(limit.max(1), Ordering::SeqCst);
    }
  }
  state.semaphore.lock().unwrap().clone()
}

async fn account_lock(account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
  let mut locks = state().account_locks.lock().await;
  locks
    .entry(account_id.to_string())
    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
    .clone()
}

/// 取消落终态：解除账号绑定并标记 cancelled，前端才能收起「取消」按钮给出「重试」。
/// 已在服务端排队的任务（有 conversation_id）豆包那边仍会生成完，可以在账号池
/// 「拉取视频」把成片收回来，所以提示里带上这条退路。
fn mark_cancelled(task_id: &str) {
  let still_generating =
    crate::doubao_manager::find_task(task_id).is_some_and(|task| task.conversation_id.is_some());
  crate::doubao_manager::assign_task_account(task_id, None);
  update_task_status(task_id, |task| {
    task.status = "cancelled".to_string();
    task.error_message = Some(if still_generating {
      "已取消（豆包可能仍在生成，可用「拉取视频」回收）".to_string()
    } else {
      "已取消".to_string()
    });
  });
}

async fn run_flow(task_id: String, cancel: Arc<AtomicBool>) {
  let state = state();
  let mut resume_polling = state.resume_ids.lock().await.remove(&task_id);
  run_loop(&task_id, &cancel, &mut resume_polling).await;
  state.active.lock().await.remove(&task_id);
  state.cancellations.lock().await.remove(&task_id);
}

async fn run_loop(task_id: &str, cancel: &AtomicBool, resume_polling: &mut bool) {
  loop {
    if cancel.load(Ordering::SeqCst) {
      mark_cancelled(task_id);
      return;
    }
    let settings = crate::doubao_manager::get_settings();
    let semaphore = ensure_semaphore(settings.max_concurrency).await;
    let now_local = chrono::Utc::now().with_timezone(&chrono_tz::Asia::Shanghai);
    let (business_date, next_reset) =
      crate::doubao_manager::quota_window(now_local, &settings.quota_reset_time);
    crate::doubao_manager::reset_daily_quotas(&business_date);
    let Some(task) = crate::doubao_manager::find_task(task_id) else {
      return;
    };
    let account = match task.account_id.as_deref() {
      Some(account_id) => crate::doubao_manager::list_accounts()
        .into_iter()
        .find(|a| a.id == account_id)
        .filter(|a| a.enabled && a.status == "active"),
      None => crate::doubao_manager::choose_available_account(
        settings.daily_quota,
        &settings.scheduler_strategy,
        settings.quota_enabled,
      ),
    };
    let Some(account) = account else {
      update_task_status(task_id, |task| {
        task.status = "queued".to_string();
        task.error_message = Some("等待可用账号".to_string());
      });
      tokio::time::sleep(WAIT_ACCOUNT_POLL).await;
      continue;
    };
    crate::doubao_manager::assign_task_account(task_id, Some(account.id.clone()));
    let Some(app) = app_handle() else {
      update_task_status(task_id, |task| {
        task.status = "failed".to_string();
        task.error_message = Some("应用句柄未初始化，无法准备浏览器环境".to_string());
      });
      return;
    };
    let profile = match crate::doubao_login::resolve_or_create_profile(&app, &account).await {
      Ok(profile) => profile,
      Err(e) => {
        update_task_status(task_id, |task| {
          task.status = "failed".to_string();
          task.error_message = Some(e);
        });
        return;
      }
    };

    let Ok(permit) = semaphore.acquire_owned().await else {
      return;
    };
    let lock = account_lock(&account.id).await;
    let _guard = lock.lock().await;
    if cancel.load(Ordering::SeqCst) {
      mark_cancelled(task_id);
      return;
    }
    update_task_status(task_id, |task| {
      task.status = "starting".to_string();
      task.error_message = None;
    });

    let quota_recorded = Arc::new(AtomicBool::new(*resume_polling));
    let quota_enabled = settings.quota_enabled;
    let quota_account_id = account.id.clone();
    let progress_task_id = task_id.to_string();
    let update = move |progress: DoubaoProgress| {
      if progress.status.as_deref() == Some("generating")
        && !quota_recorded.swap(true, Ordering::SeqCst)
        && quota_enabled
      {
        crate::doubao_manager::increment_account_quota(&quota_account_id);
      }
      apply_progress(&progress_task_id, &progress);
    };

    let params = crate::doubao_executor::DoubaoRunParams {
      prompt: task.prompt.clone(),
      model: task.model.clone(),
      ratio: task.ratio.clone(),
      duration: task.duration,
      mode: task.mode.clone(),
      image_paths: task.image_paths.iter().map(PathBuf::from).collect(),
      timeout_secs: u64::from(settings.video_timeout),
      poll_interval_secs: 5,
    };

    let run = if *resume_polling && task.conversation_id.is_some() {
      crate::doubao_executor::resume_poll(
        &profile,
        task.conversation_id.as_deref().unwrap_or(""),
        task.section_id.as_deref().unwrap_or(""),
        task.question_id.as_deref().unwrap_or(""),
        &params,
        cancel,
        &update,
      )
      .await
    } else {
      crate::doubao_executor::run_task(&profile, &params, cancel, &update).await
    };
    *resume_polling = false;
    drop(_guard);
    drop(permit);

    match run {
      Ok(result) => {
        let completed_at = crate::proxy_manager::now_secs();
        update_task_status(task_id, |task| {
          task.status = "completed".to_string();
          task.error_message = None;
          let non_empty = |s: String| (!s.is_empty()).then_some(s);
          task.conversation_id = non_empty(result.conversation_id);
          task.section_id = non_empty(result.section_id);
          task.question_id = non_empty(result.question_id);
          task.remote_task_id = non_empty(result.remote_task_id);
          task.vid = non_empty(result.vid);
          task.result_url = non_empty(result.result_url);
          task.backup_result_url = non_empty(result.backup_result_url);
          task.fallback_result_url = non_empty(result.fallback_result_url);
          task.cover_url = non_empty(result.cover_url);
          task.completed_at = Some(completed_at);
        });
        let task_id = task_id.to_string();
        tauri::async_runtime::spawn(async move {
          let Some(task) = crate::doubao_manager::find_task(&task_id) else {
            return;
          };
          match crate::doubao_download::download_task_video(&task).await {
            Ok(downloaded) => crate::doubao_executor::debug_log(&format!(
              "任务 {task_id} 视频已自动下载: {}",
              downloaded.local_file.as_deref().unwrap_or("")
            )),
            Err(e) => {
              // 生成已成功，失败只发生在取回本地这一步：状态保持 completed，
              // 把原因挂到行上，用户能在「生成结果」里点「下载」重试。
              crate::doubao_executor::debug_log(&format!("任务 {task_id} 自动下载失败: {e}"));
              crate::doubao_manager::update_task(&task_id, |task| {
                if task.local_file.is_none() {
                  task.error_message =
                    Some(format!("自动下载失败：{e}，可在「生成结果」点「下载」重试"));
                }
              });
            }
          }
        });
        return;
      }
      Err(crate::doubao_executor::DoubaoExecutorError::RateLimited(message)) => {
        // 豆包页面自己写了什么时候恢复就按那个时间锁（免费池是按周算的），
        // 没写就锁到本日额度重置点；两种都要换号，别在同一账号上原地重试。
        let until =
          crate::doubao_manager::parse_recovery_deadline(&message, now_local).unwrap_or(next_reset);
        crate::doubao_manager::mark_account_limited(
          &account.id,
          until,
          settings.daily_quota,
          &message,
        );
        crate::doubao_manager::assign_task_account(task_id, None);
        update_task_status(task_id, |task| {
          task.status = "queued".to_string();
          task.error_message = Some(format!(
            "{message}，已标记账号「{}」，正在切换账号",
            account.display_name
          ));
        });
        continue;
      }
      Err(crate::doubao_executor::DoubaoExecutorError::TooManyRequests(message)) => {
        crate::doubao_manager::assign_task_account(task_id, None);
        update_task_status(task_id, |task| {
          task.status = "queued".to_string();
          task.error_message = Some(format!("豆包访问频繁，稍后重试: {message}"));
        });
        tokio::time::sleep(Duration::from_secs(30)).await;
        continue;
      }
      Err(crate::doubao_executor::DoubaoExecutorError::SessionExpired) => {
        // 会话在服务端过期：先尝试用云端 Cookie 恢复登录，恢复成功原账号重试；
        // 恢复不了才标记失效并换号（用户重新扫码后 verify 会把账号转回 active）。
        let restored =
          crate::doubao_login::restore_login_from_cloud(&account, &profile, cancel).await;
        match restored {
          Ok(Some(_)) => {
            crate::doubao_manager::mark_account_active(&account.id);
            update_task_status(task_id, |task| {
              task.status = "queued".to_string();
              task.error_message = Some("登录态已失效，已从云端 Cookie 恢复，正在重试".to_string());
            });
            continue;
          }
          Err(e) => {
            log::warn!(
              "[doubao_scheduler] 账号 {} 云端 Cookie 恢复失败: {e}",
              account.id
            );
          }
          Ok(None) => {}
        }
        crate::doubao_manager::assign_task_account(task_id, None);
        crate::doubao_manager::mark_account_inactive(&account.id, "登录态已失效，请重新扫码登录");
        update_task_status(task_id, |task| {
          task.status = "queued".to_string();
          task.error_message = Some("账号登录态失效已切换，正在重试其他账号".to_string());
        });
        continue;
      }
      Err(crate::doubao_executor::DoubaoExecutorError::VerifyFailed) => {
        // 风控验证只能人手做：停在 verifying 把「去哪个窗口做什么」写清楚，
        // 账号绑定保持不变（用户处理完就在原账号重试，不用重新抢号）。
        update_task_status(task_id, |task| {
          task.status = "verifying".to_string();
          task.error_message = Some(format!(
            "等待人工处理：请在账号「{}」的浏览器窗口完成验证，任务稍后自动重试",
            account.display_name
          ));
        });
        tokio::time::sleep(Duration::from_secs(60)).await;
        continue;
      }
      Err(crate::doubao_executor::DoubaoExecutorError::Cancelled) => {
        mark_cancelled(task_id);
        return;
      }
      Err(e) => {
        update_task_status(task_id, |task| {
          task.status = "failed".to_string();
          task.error_message = Some(e.to_string());
        });
        return;
      }
    }
  }
}

// ============================================================
// Tauri commands
// ============================================================

#[tauri::command]
pub async fn doubao_retry_task(task_id: String) -> Result<DoubaoVideoTask, String> {
  let Some(task) = crate::doubao_manager::find_task(&task_id) else {
    return Err("任务不存在".to_string());
  };
  if matches!(
    task.status.as_str(),
    "queued" | "starting" | "generating" | "verifying"
  ) {
    return Err("任务正在执行中，无需重试".to_string());
  }
  // 重新排队时解除原账号绑定，由调度器重新选择可用账号
  crate::doubao_manager::assign_task_account(&task_id, None);
  let task = crate::doubao_manager::update_task(&task_id, |task| {
    task.status = "queued".to_string();
    task.error_message = None;
    task.completed_at = None;
  })
  .ok_or_else(|| "任务已被删除".to_string())?;
  schedule(&task_id, false);
  Ok(task)
}

#[tauri::command]
pub async fn doubao_cancel_task(task_id: String) -> bool {
  if cancel_scheduled(&task_id).await {
    return true;
  }
  // 没有驱动器在跑（排队未被接管、或上一轮已退出）：直接落终态，否则任务永远挂着
  let cancellable = crate::doubao_manager::find_task(&task_id).is_some_and(|task| {
    matches!(
      task.status.as_str(),
      "queued" | "starting" | "generating" | "verifying"
    )
  });
  if cancellable {
    mark_cancelled(&task_id);
  }
  cancellable
}

#[tauri::command]
pub async fn doubao_resume_tasks() -> usize {
  resume_queued().await
}

#[tauri::command]
pub async fn doubao_pull_account_videos(account_id: String) -> Result<serde_json::Value, String> {
  let account = crate::doubao_manager::list_accounts()
    .into_iter()
    .find(|a| a.id == account_id)
    .ok_or_else(|| "账号不存在".to_string())?;
  if account.status != "active" {
    return Err("该账号未登录，无法拉取视频".to_string());
  }
  let Some(app) = app_handle() else {
    return Err("应用句柄未初始化".to_string());
  };
  let profile = crate::doubao_login::resolve_or_create_profile(&app, &account).await?;
  let cancel = AtomicBool::new(false);
  let videos = crate::doubao_executor::pull_videos(&profile, 50, &cancel)
    .await
    .map_err(|e| e.to_string())?;
  let existing: HashSet<String> = {
    let vids: HashSet<String> = videos.iter().map(|v| v.vid.clone()).collect();
    crate::doubao_manager::list_tasks()
      .into_iter()
      .filter_map(|t| t.vid)
      .filter(|vid| vids.contains(vid))
      .collect()
  };
  let mut created = 0;
  for video in &videos {
    if existing.contains(&video.vid) {
      continue;
    }
    let now = crate::proxy_manager::now_secs();
    let task = DoubaoVideoTask {
      id: uuid::Uuid::new_v4().to_string(),
      account_id: Some(account.id.clone()),
      prompt: if video.name.is_empty() {
        "拉取视频".to_string()
      } else {
        video.name.clone()
      },
      mode: "t2v".to_string(),
      status: "completed".to_string(),
      vid: Some(video.vid.clone()),
      remote_task_id: Some(video.node_id.clone()),
      result_url: Some(video.result_url.clone()),
      backup_result_url: (!video.backup_result_url.is_empty())
        .then(|| video.backup_result_url.clone()),
      completed_at: Some(now),
      created_at: now,
      updated_at: now,
      ..Default::default()
    };
    crate::doubao_manager::persist_task(&task)
      .map_err(|e| crate::backend_error_with_detail("INTERNAL_ERROR", e.to_string()))?;
    created += 1;
  }
  Ok(serde_json::json!({ "total": videos.len(), "new": created }))
}

#[tauri::command]
pub async fn doubao_download_task_video(task_id: String) -> Result<DoubaoVideoTask, String> {
  let task = crate::doubao_manager::find_task(&task_id).ok_or_else(|| "任务不存在".to_string())?;
  crate::doubao_download::download_task_video(&task).await
}
