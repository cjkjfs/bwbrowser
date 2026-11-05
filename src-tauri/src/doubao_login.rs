//! Doubao login: QR-scan login in the bound BW environment's browser window
//! and silent login-state verification over CDP (port of DoubaoManager
//! `src/doupool/login/`). The session itself lives in that environment's
//! browser data dir, so there is nothing to export — verification just probes
//! the cookies the doubao tab carries.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tauri::Emitter;

use crate::wayfern_cdp::WayfernSession;

/// DoubaoManager 用 300 秒扫码超时。
const LOGIN_TIMEOUT_SECS: u64 = 300;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const LOCALSTORAGE_TOKENS_JS: &str = r#"(() => {
  try {
    const t = JSON.parse(localStorage.getItem('__tea_cache_tokens_497858') || '{}');
    return JSON.stringify({ id: t.user_unique_id || '', name: t.user_name || '' });
  } catch { return '{}'; }
})()"#;

const LOGIN_EVENT: &str = "doubao-login:progress";

#[derive(Debug, Clone, PartialEq)]
pub struct DoubaoIdentity {
  pub user_id: String,
  pub nickname: Option<String>,
}

/// In-flight logins keyed by account id, each with its own cancel flag —
/// several accounts can log in at the same time (one browser window each),
/// mirroring the cloud accounts page where every launch is independent.
static LOGIN_SESSIONS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
  LazyLock::new(|| Mutex::new(HashMap::new()));

fn login_sessions() -> std::sync::MutexGuard<'static, HashMap<String, Arc<AtomicBool>>> {
  LOGIN_SESSIONS
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Login progress states pushed to the frontend as `doubao-login:progress`:
/// launching -> waiting_for_scan -> verifying -> succeeded, with
/// failed / cancelled / timed_out as the exits.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DoubaoLoginProgress {
  pub state: String,
  pub message: String,
  pub account_id: String,
}

fn emit_progress(app: &tauri::AppHandle, account_id: &str, state: &str, message: &str) {
  let _ = app.emit(
    LOGIN_EVENT,
    DoubaoLoginProgress {
      state: state.to_string(),
      message: message.to_string(),
      account_id: account_id.to_string(),
    },
  );
}

/// Cookies (including httpOnly) the doubao tab currently carries.
async fn read_cookies(session: &mut WayfernSession) -> Result<Vec<Value>, String> {
  let reply = session
    .call("Network.getCookies", json!({}))
    .await
    .map_err(|e| format!("读取豆包 cookies 失败: {e}"))?;
  Ok(
    reply
      .get("cookies")
      .and_then(Value::as_array)
      .cloned()
      .unwrap_or_default(),
  )
}

fn cookie_value<'a>(cookies: &'a [Value], name: &str) -> Option<&'a str> {
  cookies
    .iter()
    .find(|c| c.get("name").and_then(Value::as_str) == Some(name))
    .and_then(|c| c.get("value"))
    .and_then(Value::as_str)
}

/// The detector.verify() port: sessionid + passport_auth_status must both be
/// present, user_id comes from multi_sids or the localStorage token cache.
/// `storage_json` is the evaluated LOCALSTORAGE_TOKENS_JS result.
fn identity_from_state(cookies: &[Value], storage_json: &str) -> Option<DoubaoIdentity> {
  let sessionid =
    cookie_value(cookies, "sessionid").or_else(|| cookie_value(cookies, "sessionid_ss"));
  let auth_status = cookie_value(cookies, "passport_auth_status")
    .or_else(|| cookie_value(cookies, "passport_auth_status_ss"));
  if sessionid.is_none() || auth_status.is_none() {
    return None;
  }
  let mut user_id = cookie_value(cookies, "multi_sids")
    .and_then(|v| v.split("%3A").next())
    .filter(|s| !s.is_empty())
    .map(str::to_string);
  let storage: Value = serde_json::from_str(storage_json).unwrap_or_else(|_| json!({}));
  if user_id.is_none() {
    user_id = storage
      .get("id")
      .and_then(Value::as_str)
      .filter(|s| !s.is_empty())
      .map(str::to_string);
  }
  let nickname = storage
    .get("name")
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(str::to_string);
  let user_id = user_id?;
  Some(DoubaoIdentity { user_id, nickname })
}

/// Probe the doubao tab for a live login. `Ok(None)` = not logged in yet.
pub async fn verify_login_state(
  session: &mut WayfernSession,
) -> Result<Option<DoubaoIdentity>, String> {
  let cookies = read_cookies(session).await?;
  let storage = crate::doubao_executor::evaluate(session, LOCALSTORAGE_TOKENS_JS, false)
    .await
    .unwrap_or_else(|_| json!("{}"))
    .as_str()
    .unwrap_or("{}")
    .to_string();
  let Some(identity) = identity_from_state(&cookies, &storage) else {
    return Ok(None);
  };
  // 登出后的 cookie 罐子仍留着 sessionid，光看 cookies 会把死号判成活。页面 SSR
  // 里带着服务端的结论，不一致时重载页面再确认一次（云端注入后页面不会自己跳）。
  if crate::doubao_executor::ssr_session_expired(session).await {
    let _ = session
      .call(
        "Page.navigate",
        json!({ "url": crate::doubao_executor::DOUBAO_CHAT_URL }),
      )
      .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    if crate::doubao_executor::ssr_session_expired(session).await {
      crate::doubao_executor::debug_log(&format!(
        "账号 {} 的 cookie 仍在但豆包服务端判定会话过期，按未登录处理",
        identity.user_id
      ));
      return Ok(None);
    }
  }
  Ok(Some(identity))
}

fn find_account(account_id: &str) -> Result<crate::doubao_manager::DoubaoAccount, String> {
  crate::doubao_manager::list_accounts()
    .into_iter()
    .find(|a| a.id == account_id)
    .ok_or_else(|| "账号不存在".to_string())
}

fn resolve_profile(
  account: &crate::doubao_manager::DoubaoAccount,
) -> Result<crate::profile::types::BrowserProfile, String> {
  let profile_id = account
    .profile_id
    .as_deref()
    .filter(|id| !id.is_empty())
    .ok_or_else(|| "该账号未绑定 BW 环境，请先在编辑里绑定".to_string())?;
  crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("读取浏览器配置列表失败：{e}"))?
    .into_iter()
    .find(|p| p.id.to_string() == profile_id)
    .ok_or_else(|| "绑定的 BW 环境已被删除，请重新绑定".to_string())
}

/// Bind the account to its derived-name environment if one already exists
/// locally (no launch). Used by non-launch paths like the health probe.
pub(crate) fn bind_existing_profile(
  account: &crate::doubao_manager::DoubaoAccount,
) -> Option<crate::profile::types::BrowserProfile> {
  if let Ok(profile) = resolve_profile(account) {
    return Some(profile);
  }
  let server_id = account.server_id?;
  let derived = crate::bwbrowser_cloud::account_profile_name(&account.display_name, server_id);
  let profile = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .ok()?
    .into_iter()
    .find(|p| p.name.to_lowercase() == derived.to_lowercase())?;
  let mut next = account.clone();
  next.profile_id = Some(profile.id.to_string());
  next.updated_at = crate::proxy_manager::now_secs();
  if let Err(e) = crate::doubao_manager::persist_account(&next) {
    log::warn!("保存豆包账号环境绑定失败: {e}");
  }
  Some(profile)
}

/// True when the profile's browser process is still alive. A launch reuse is
/// cheap; the point is to notice a window the user closed.
pub(crate) fn browser_running(profile: &crate::profile::types::BrowserProfile) -> bool {
  profile
    .process_id
    .is_some_and(crate::proxy_storage::is_process_running)
}

/// Resolve the account's BW environment and guarantee the browser is up:
/// reuse a running window, relaunch a closed one (cloud accounts through the
/// cloud-account launch flow so proxy/fingerprint/timezone all apply), and
/// create the environment through the same flow when the account has none.
pub(crate) async fn resolve_or_create_profile(
  app: &tauri::AppHandle,
  account: &crate::doubao_manager::DoubaoAccount,
) -> Result<crate::profile::types::BrowserProfile, String> {
  if let Some(profile) = bind_existing_profile(account) {
    if browser_running(&profile) {
      return Ok(profile);
    }
    return if account.server_id.is_some() {
      launch_like_cloud_account(app, account).await
    } else {
      crate::ai_agent_browser::ensure_running(&profile).await
    };
  }
  launch_like_cloud_account(app, account).await
}

/// Launch the account's environment exactly like the cloud accounts page does:
/// the full server-driven flow (find-or-create environment, proxy processing,
/// fingerprint, cloud-cookie pull/inject and login-wait upload-back). Different
/// accounts launch concurrently — each gets its own browser window — and
/// relaunching a running environment just reuses it.
pub(crate) async fn launch_like_cloud_account(
  app: &tauri::AppHandle,
  account: &crate::doubao_manager::DoubaoAccount,
) -> Result<crate::profile::types::BrowserProfile, String> {
  let server_id = account
    .server_id
    .ok_or_else(|| "该账号不是云端账号，无法启动浏览器".to_string())?;
  crate::bwbrowser_cloud::bwbrowser_launch_account(
    app.clone(),
    server_id,
    account.display_name.clone(),
    None,
    account.proxy_node.clone(),
    Some("doubao".to_string()),
  )
  .await?;
  bind_existing_profile(account).ok_or_else(|| "浏览器已启动但未找到对应环境".to_string())
}

/// Pull this account's cookies from the cookie-sync server and inject them
/// into the environment's running browser (the same get_account_cookies
/// and CDP injection the cloud accounts page uses), then check whether the
/// doubao session came back. `Ok(None)` means nothing to restore.
pub(crate) async fn restore_login_from_cloud(
  account: &crate::doubao_manager::DoubaoAccount,
  profile: &crate::profile::types::BrowserProfile,
  cancel: &AtomicBool,
) -> Result<Option<DoubaoIdentity>, String> {
  let Some(server_id) = account.server_id else {
    return Ok(None);
  };
  let Some((username, password)) = crate::bwbrowser_cloud::BWBROWSER_AUTH.get_credentials() else {
    return Err("未登录云端账号，无法拉取 Cookie".to_string());
  };
  let form_data = format!(
    "action=get_account_cookies&username={}&password={}&account_id={}&platform=doubao",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    server_id,
  );
  let resp = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .http()
    .post(crate::cloud_domain::BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("拉取云端 Cookie 失败: {e}"))?;
  let body = resp
    .text()
    .await
    .map_err(|e| format!("读取云端 Cookie 响应失败: {e}"))?;
  let result: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
  let cookie_text = if result["success"].as_bool().unwrap_or(false) {
    result["cookie"].as_str().unwrap_or("")
  } else {
    ""
  };
  let cloud_val: Value = serde_json::from_str(cookie_text).unwrap_or_else(|_| json!([]));
  let cdp_cookies = crate::cookie_sync::prepare_cookies_for_injection(&cloud_val, "doubao");
  if cdp_cookies.is_empty() {
    log::info!(
      "[doubao_login] 账号 {} 云端无可用 Cookie，跳过恢复",
      server_id
    );
    return Ok(None);
  }
  let count = crate::cookie_sync::inject_cookies_via_cdp(profile, &cdp_cookies, None)
    .await
    .map_err(|e| format!("注入云端 Cookie 失败: {e}"))?;
  log::info!(
    "[doubao_login] 账号 {} 云端 Cookie 已注入 {} 个，验证登录态",
    server_id,
    count
  );
  let mut session = crate::doubao_executor::open_chat_session(profile, cancel)
    .await
    .map_err(|e| e.to_string())?;
  verify_login_state(&mut session).await
}

/// True when the profile's browser was launched but its process is gone —
/// the user closed the window mid-login, which should end quietly.
fn browser_closed(profile: &crate::profile::types::BrowserProfile) -> bool {
  profile.process_id.is_some() && !browser_running(profile)
}

fn persist_identity(
  account: &crate::doubao_manager::DoubaoAccount,
  identity: Option<&DoubaoIdentity>,
  error_message: Option<&str>,
) {
  let mut next = account.clone();
  next.updated_at = crate::proxy_manager::now_secs();
  if let Some(identity) = identity {
    next.doubao_user_id = Some(identity.user_id.clone());
    next.doubao_nickname = identity.nickname.clone();
    next.status = "active".to_string();
    next.last_verified_at = Some(next.updated_at);
    next.last_error = None;
  }
  if let Some(message) = error_message {
    next.last_error = Some(message.to_string());
  }
  if let Err(e) = crate::doubao_manager::persist_account(&next) {
    log::warn!("保存豆包账号登录状态失败: {e}");
  }
}

/// Start a QR-scan login for one pool account: bring up the bound
/// environment's window on doubao.com and watch for the login to land.
/// Returns immediately; progress streams as `doubao-login:progress` events.
#[tauri::command]
pub async fn doubao_start_login(app: tauri::AppHandle, account_id: String) -> Result<(), String> {
  let cancel = {
    let mut sessions = login_sessions();
    if sessions.contains_key(&account_id) {
      return Err("该账号已有登录流程进行中，请先取消或等待完成".to_string());
    }
    let flag = Arc::new(AtomicBool::new(false));
    sessions.insert(account_id.clone(), flag.clone());
    flag
  };
  tauri::async_runtime::spawn(async move {
    let result = run_login_flow(&app, &account_id, &cancel).await;
    login_sessions().remove(&account_id);
    if let Err(e) = result {
      // 启动期的失败（云端账号不可用/浏览器起不来）发生在第一次
      // emit_progress 之前，必须在这里补发 failed 事件，否则前端毫无反应。
      log::warn!("[doubao_login] 登录流程异常结束: {e}");
      emit_progress(&app, &account_id, "failed", &e);
    }
  });
  Ok(())
}

async fn run_login_flow(
  app: &tauri::AppHandle,
  account_id: &str,
  cancel: &AtomicBool,
) -> Result<(), String> {
  let account = find_account(account_id)?;

  emit_progress(
    app,
    account_id,
    "launching",
    "正在启动浏览器窗口（首次将自动创建环境）",
  );
  // 云端账号走与云端账号页完全相同的启动流程（含云端 Cookie 注入与登录回传），
  // 多账号可各自独立同时启动；浏览器被关掉的会在这里自动重新拉起。
  let profile = if account.server_id.is_some() {
    launch_like_cloud_account(app, &account).await?
  } else {
    resolve_or_create_profile(app, &account).await?
  };

  // 云端有 Cookie 就先恢复，恢复成功则无需扫码；失败/无 Cookie 再走扫码。
  emit_progress(
    app,
    account_id,
    "verifying",
    "正在尝试从云端 Cookie 恢复登录",
  );
  match restore_login_from_cloud(&account, &profile, cancel).await {
    Ok(Some(identity)) => {
      persist_identity(&account, Some(&identity), None);
      emit_progress(
        app,
        account_id,
        "succeeded",
        &format!(
          "登录成功（云端 Cookie 恢复）：{}",
          identity.nickname.as_deref().unwrap_or(&identity.user_id)
        ),
      );
      return Ok(());
    }
    Ok(None) => {
      log::info!("[doubao_login] 账号 {account_id} 云端无可恢复的 Cookie，转扫码登录");
    }
    Err(e) => {
      log::warn!("[doubao_login] 账号 {account_id} 云端 Cookie 恢复失败: {e}，转扫码登录");
    }
  }

  let login = async {
    let mut session = crate::doubao_executor::open_chat_session(&profile, cancel)
      .await
      .map_err(|e| e.to_string())?;
    emit_progress(
      app,
      account_id,
      "waiting_for_scan",
      "请在浏览器窗口中扫码登录豆包",
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(LOGIN_TIMEOUT_SECS);
    loop {
      if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".to_string());
      }
      if tokio::time::Instant::now() >= deadline {
        return Err("timed_out".to_string());
      }
      if browser_closed(&profile) {
        return Err("browser_closed".to_string());
      }
      match verify_login_state(&mut session).await {
        Ok(Some(identity)) => {
          emit_progress(app, account_id, "verifying", "已检测到登录，正在确认账号");
          persist_identity(&account, Some(&identity), None);
          tokio::time::sleep(Duration::from_secs(1)).await;
          return Ok(identity);
        }
        Ok(None) => {}
        Err(e) => {
          // 浏览器窗口被手动关闭：安静结束，不当作登录失败
          if browser_closed(&profile) {
            return Err("browser_closed".to_string());
          }
          return Err(format!("failed:{e}"));
        }
      }
      tokio::time::sleep(POLL_INTERVAL).await;
    }
  };

  match login.await {
    Ok(identity) => {
      emit_progress(
        app,
        account_id,
        "succeeded",
        &format!(
          "登录成功：{}",
          identity.nickname.as_deref().unwrap_or(&identity.user_id)
        ),
      );
      Ok(())
    }
    Err(reason) => {
      let (state, message) = match reason.as_str() {
        "cancelled" => ("cancelled", "登录已取消"),
        "browser_closed" => ("cancelled", "浏览器已关闭，登录未完成"),
        "timed_out" => ("timed_out", "扫码登录已超时，请重试"),
        other => ("failed", other.strip_prefix("failed:").unwrap_or(other)),
      };
      if state != "cancelled" {
        persist_identity(&account, None, Some(message));
      }
      emit_progress(app, account_id, state, message);
      if state == "failed" {
        Err(message.to_string())
      } else {
        Ok(())
      }
    }
  }
}

#[tauri::command]
pub fn doubao_cancel_login(account_id: String) -> bool {
  let sessions = login_sessions();
  match sessions.get(&account_id) {
    Some(flag) => {
      flag.store(true, Ordering::SeqCst);
      true
    }
    None => false,
  }
}

/// Open the bound environment's browser window and bring the doubao page up,
/// for inspecting or operating the account by hand. An account without an
/// environment gets one created via the cloud-account launch flow first.
#[tauri::command]
pub async fn doubao_open_browser(app: tauri::AppHandle, account_id: String) -> Result<(), String> {
  let account = find_account(&account_id)?;
  // 与扫码登录同一启动路径：云端账号走云端账号页的启动流程，可多开；
  // 环境在但浏览器被关的，同样自动重新拉起。
  let running = if account.server_id.is_some() {
    launch_like_cloud_account(&app, &account).await?
  } else {
    resolve_or_create_profile(&app, &account).await?
  };
  let cancel = AtomicBool::new(false);
  match crate::doubao_executor::open_chat_session(&running, &cancel).await {
    Ok(_) => Ok(()),
    Err(_) => crate::cdp_target::open_local_page(&running, "https://www.doubao.com/chats")
      .await
      .map(|_| ()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cookie(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value })
  }

  #[test]
  fn test_identity_requires_session_and_auth_cookies() {
    let cookies = vec![cookie("sessionid", "abc"), cookie("multi_sids", "111%3Ax")];
    assert!(identity_from_state(&cookies, "{}").is_none());
    let cookies = vec![
      cookie("sessionid", "abc"),
      cookie("passport_auth_status", "y"),
      cookie("multi_sids", "111%3Ax"),
    ];
    let identity = identity_from_state(&cookies, "{}").unwrap();
    assert_eq!(identity.user_id, "111");
    assert_eq!(identity.nickname, None);
  }

  #[test]
  fn test_identity_prefers_ss_variants() {
    let cookies = vec![
      cookie("sessionid_ss", "abc"),
      cookie("passport_auth_status_ss", "y"),
      cookie("multi_sids", "222%3A"),
    ];
    let identity = identity_from_state(&cookies, "{}").unwrap();
    assert_eq!(identity.user_id, "222");
  }

  #[test]
  fn test_identity_falls_back_to_localstorage() {
    let cookies = vec![
      cookie("sessionid", "abc"),
      cookie("passport_auth_status", "y"),
    ];
    let identity = identity_from_state(&cookies, r#"{"id":"333","name":"豆包号"}"#).unwrap();
    assert_eq!(identity.user_id, "333");
    assert_eq!(identity.nickname.as_deref(), Some("豆包号"));
  }

  #[test]
  fn test_identity_none_without_user_id() {
    let cookies = vec![
      cookie("sessionid", "abc"),
      cookie("passport_auth_status", "y"),
    ];
    assert!(identity_from_state(&cookies, "{}").is_none());
  }
}
