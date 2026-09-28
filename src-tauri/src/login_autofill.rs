//! While a profile's browser runs, autofill the profile's stored sign-in
//! credentials into Google / YouTube login pages the user lands on.
//!
//! The fill is idempotent and never submits the form — the human still presses
//! Next / Sign in, and a captcha or 2FA step simply waits for them. The email
//! and password are read straight from the profile metadata inside this module
//! and substituted into an injected script; they never cross a Tauri command
//! boundary.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::cdp_target::{run_command, CdpTarget};
use crate::profile::manager::ProfileManager;
use crate::profile::types::LoginCredentials;
use crate::wayfern_manager::WayfernManager;
use serde_json::Value;

/// How often the watcher re-probes the browser's pages. A poll catches a page
/// that moves from the email step to the password step on the next round.
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// Consecutive probe failures before the watcher gives up. The watcher starts
/// only after a successful launch, so a long streak means the browser died
/// while a stale PID kept the process check green.
const MAX_CONSECUTIVE_FAILURES: u32 = 15;

/// One watcher per profile. `update_profile_login_credentials` also spawns a
/// watcher when credentials are saved while the browser is already running;
/// the set collapses that with the one the launch already started.
static ACTIVE_WATCHERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_watchers() -> &'static Mutex<HashSet<String>> {
  ACTIVE_WATCHERS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The autofill script with the profile's credentials substituted in.
///
/// `serde_json::to_string` on a `&str` produces the quoted, escaped literal the
/// script's `const EMAIL = ...` needs.
fn autofill_script(credentials: &LoginCredentials) -> String {
  include_str!("../assets/login_autofill.js")
    .replace(
      "__EMAIL__",
      &serde_json::to_string(&credentials.email).unwrap_or_else(|_| "\"\"".to_string()),
    )
    .replace(
      "__PASSWORD__",
      &serde_json::to_string(&credentials.password).unwrap_or_else(|_| "\"\"".to_string()),
    )
}

/// Run the autofill watcher for `profile_id` until the browser exits or the
/// credentials are cleared.
pub async fn watch_profile(profile_id: &str) {
  if !active_watchers()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .insert(profile_id.to_string())
  {
    // Another watcher for this profile is already polling and re-reads the
    // credentials itself, so the newer caller has nothing to add.
    return;
  }

  crate::bwbrowser_cloud::log_bwbrowser(
    "autofill",
    &format!("watcher 启动（profile {profile_id}）"),
  );
  // Only a health verdict refreshed after the watcher started counts as
  // "logged in": a stale Valid written by a previous session must not stop a
  // fresh launch before the launch-time check has re-run.
  let started_at = crate::proxy_manager::now_secs();
  let mut consecutive_failures: u32 = 0;
  // Fields already filled once on a page (`"{documentURL}|email"` /
  // `"{documentURL}|password"`). A filled field is left alone on every later
  // probe — Google often re-renders the same page and we must not retype over
  // what the user typed by hand, nor append a duplicate.
  let mut filled_once: HashSet<String> = HashSet::new();
  let outcome = loop {
    // The accounts toolbar switch. Reading it every poll both keeps a fresh
    // watcher from starting while off and stops a running one the moment the
    // user flips the switch — no separate broadcast needed.
    let autofill_on = crate::settings_manager::SettingsManager::instance()
      .load_settings()
      .map(|s| s.autofill_enabled)
      .unwrap_or(true);
    if !autofill_on {
      break "autofill disabled";
    }

    let profile = match ProfileManager::instance()
      .list_profiles()
      .ok()
      .and_then(|profiles| {
        profiles
          .into_iter()
          .find(|p| p.id.to_string() == profile_id)
      }) {
      Some(profile) => profile,
      None => break "profile gone",
    };

    // Feature turned off, or the browser is gone: nothing left to watch.
    let Some(ref credentials) = profile.login_credentials else {
      break "credentials cleared";
    };
    if !profile
      .process_id
      .is_some_and(crate::proxy_storage::is_process_running)
    {
      break "browser exited";
    }

    // The launch-time cookie-health check has confirmed login: the account is
    // already signed in, so there is nothing left to autofill — stop instead of
    // polling the login page forever.
    if crate::cookie_health::profile_health_map()
      .get(profile_id)
      .is_some_and(|r| {
        r.status == crate::cookie_health::HealthStatus::Valid && r.checked_at >= started_at
      })
    {
      break "login confirmed";
    }

    let profiles_dir = ProfileManager::instance().get_profiles_dir();
    let profile_path = profile.get_profile_data_path(&profiles_dir);
    let Some(port) = WayfernManager::instance()
      .get_cdp_port(&profile_path.to_string_lossy())
      .await
    else {
      consecutive_failures += 1;
      if consecutive_failures == 1 || consecutive_failures.is_multiple_of(5) {
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!(
            "CDP 端口未就绪（第 {} 次）: {}",
            consecutive_failures,
            profile_path.to_string_lossy()
          ),
        );
      }
      if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
        break "no CDP port";
      }
      tokio::time::sleep(POLL_INTERVAL).await;
      continue;
    };
    crate::bwbrowser_cloud::log_bwbrowser(
      "autofill",
      &format!("CDP 端口就绪: {port}，开始探测页面"),
    );

    match probe_pages(
      port,
      &autofill_script(credentials),
      &credentials.email,
      &credentials.password,
      &profile.name,
      profile_id,
      &mut filled_once,
    )
    .await
    {
      ProbeResult::Filled | ProbeResult::Nothing => consecutive_failures = 0,
      ProbeResult::Failed => {
        consecutive_failures += 1;
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
          break "CDP unreachable";
        }
      }
    }
    tokio::time::sleep(POLL_INTERVAL).await;
  };

  // Release the slot so a later launch of the same profile can start a fresh
  // watcher. Without this, a watcher that stopped (browser exited, CDP gone)
  // still blocks every subsequent launch because the insert above returns early.
  active_watchers()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .remove(profile_id);

  if outcome != "browser exited" && outcome != "credentials cleared" {
    crate::bwbrowser_cloud::log_bwbrowser(
      "autofill",
      &format!("watcher 停止（profile {profile_id}）: {outcome}"),
    );
  }
}

enum ProbeResult {
  /// Some page reported that it filled at least one field.
  Filled,
  /// Every page answered (skip or nothing to fill); the browser is healthy.
  Nothing,
  /// The browser could not be reached at all.
  Failed,
}

/// Evaluate the script on every drivable page of the browser on `port`.
async fn probe_pages(
  port: u16,
  script: &str,
  email: &str,
  password: &str,
  profile_name: &str,
  profile_id: &str,
  filled_once: &mut HashSet<String>,
) -> ProbeResult {
  let listing = format!("http://127.0.0.1:{port}/json");
  let targets: Vec<serde_json::Value> = {
    let response = match reqwest::Client::new()
      .get(&listing)
      .timeout(Duration::from_secs(3))
      .send()
      .await
    {
      Ok(response) => response,
      Err(e) => {
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!("无法列出页面（port {port}）: {e}"),
        );
        return ProbeResult::Failed;
      }
    };
    match response.json().await {
      Ok(targets) => targets,
      Err(e) => {
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!("无法解析页面列表（port {port}）: {e}"),
        );
        return ProbeResult::Failed;
      }
    }
  };

  let mut saw_page = false;
  let mut any_filled = false;
  for target in &targets {
    if target.get("type").and_then(serde_json::Value::as_str) != Some("page") {
      continue;
    }
    let url = target
      .get("url")
      .and_then(serde_json::Value::as_str)
      .unwrap_or_default();
    if url.starts_with("devtools://") {
      continue;
    }
    let Some(ws_url) = target
      .get("webSocketDebuggerUrl")
      .and_then(serde_json::Value::as_str)
    else {
      continue;
    };
    saw_page = true;

    match probe_page(ws_url, script, email, password, filled_once).await {
      Ok(verdict) if verdict.starts_with("filled:") && !verdict.contains("nothing") => {
        let url_short = &url[..url.len().min(120)];
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!("已填充登录框（{}）: url={url_short}", profile_name),
        );
        any_filled = true;
      }
      Ok(verdict) if verdict == "skip:not-signin" => {
        log::debug!("Login autofill: not a sign-in page, skipping ({url})");
      }
      Ok(verdict)
        if verdict.starts_with("filled:")
          && verdict.contains("nothing")
          && verdict.contains("no-") =>
      {
        // 到达登录页但字段缺失/未就绪：这是最需要关注的诊断信号。
        let short = verdict[..verdict.len().min(160)].to_string();
        let url_short = &url[..url.len().min(120)];
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!("已到登录页但未填充: {short} (url={url_short})"),
        );
      }
      Ok(verdict) => {
        let short = verdict[..verdict.len().min(160)].to_string();
        log::debug!("Login autofill: probe {short} on {url}");
      }
      Err(e) => {
        let url_short = &url[..url.len().min(120)];
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!("页面探测失败: {e} (url={url_short})"),
        );
      }
    }
  }

  if any_filled {
    ProbeResult::Filled
  } else if saw_page {
    ProbeResult::Nothing
  } else {
    crate::bwbrowser_cloud::log_bwbrowser(
      "autofill",
      &format!("未发现任何可驱动页面（port {port}），profile {profile_id}"),
    );
    ProbeResult::Failed
  }
}

/// Run the script on one page and return its verdict string.
///
/// The browser's automation gate refuses `Runtime.evaluate` for accounts on a
/// free plan; the same gate leaves the DOM + Input domains open (cookie sync's
/// login check already relies on that). When the script's transport is refused,
/// fall back to `DOM.getDocument` + `DOM.querySelector` + `Input.insertText`.
async fn probe_page(
  ws_url: &str,
  script: &str,
  email: &str,
  password: &str,
  filled_once: &mut HashSet<String>,
) -> Result<String, String> {
  let target = CdpTarget::Local {
    ws_url: ws_url.to_string(),
  };
  let result = run_command(
    &target,
    "Runtime.evaluate",
    serde_json::json!({ "expression": script, "returnByValue": true }),
  )
  .await;
  match result {
    Ok(result) => {
      if result.get("exceptionDetails").is_some() {
        return Err("the autofill script threw on this page".to_string());
      }
      Ok(
        result
          .get("result")
          .and_then(|r| r.get("value"))
          .and_then(serde_json::Value::as_str)
          .unwrap_or("filled:nothing")
          .to_string(),
      )
    }
    Err(e) => {
      let err_str = e.to_string();
      let gated = err_str.contains("paid")
        || err_str.contains("requires")
        || err_str.contains("wasn't found")
        || err_str.contains("-32000");
      if !gated {
        return Err(err_str);
      }
      crate::bwbrowser_cloud::log_bwbrowser(
        "autofill",
        &format!("Runtime.evaluate 受限（{err_str}），改用 DOM 域填充"),
      );
      probe_page_dom(&target, email, password, filled_once).await
    }
  }
}

/// Whether a page URL is one of the Google/YouTube sign-in surfaces the
/// autofill targets. Mirrors the JS guard in `assets/login_autofill.js`.
fn is_signin_url(url: &str) -> bool {
  let lower = url.to_lowercase();
  let host_port = lower.split('/').nth(2).unwrap_or("");
  let host = host_port
    .split(':')
    .next()
    .unwrap_or("")
    .trim_end_matches('.');
  let path = lower
    .split('?')
    .next()
    .unwrap_or("")
    .split('#')
    .next()
    .unwrap_or("");

  host.ends_with("accounts.google.com")
    || host.ends_with("accounts.youtube.com")
    || ((host == "youtube.com" || host.ends_with(".youtube.com")) && path.contains("/signin"))
}

/// Fill the credentials through the DOM + Input domains when the gate blocks
/// scripts.
///
/// nodeId is scoped to one connection, so `getDocument`, every `querySelector`
/// and the type-into calls share a single connection. Returns the same
/// "filled:..." / "skip:not-signin" verdicts the scripted probe produces.
async fn probe_page_dom(
  target: &CdpTarget,
  email: &str,
  password: &str,
  filled_once: &mut HashSet<String>,
) -> Result<String, String> {
  let email_selectors = [
    r#"input[type="email"]:not([type="hidden"])"#,
    r#"input[name="identifier"]:not([type="hidden"])"#,
    r#"input[jsname="KKx9x"]:not([type="hidden"])"#,
    r#"input[jsname="YPqjbf"]:not([type="hidden"])"#,
    "#identifierId",
  ];
  let password_selectors = [
    r#"input[name="Passwd"]"#,
    r#"input[type="password"]:not([name="ca"])"#,
  ];

  let mut conn = target.connect().await.map_err(|e| e.to_string())?;
  let doc = match conn
    .call(1u64, "DOM.getDocument", serde_json::json!({ "depth": -1 }))
    .await
  {
    Ok(doc) => doc,
    Err(e) => {
      conn.close().await;
      return Err(format!("DOM.getDocument failed: {e}"));
    }
  };
  let root = doc.get("root").cloned().unwrap_or_default();
  let url = root
    .get("documentURL")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_string();
  if !is_signin_url(&url) {
    conn.close().await;
    return Ok("skip:not-signin".to_string());
  }
  let root_id = root.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
  if root_id == 0 {
    conn.close().await;
    return Ok("filled:nothing:no-root-node".to_string());
  }

  let mut cmd_id = 2u64;
  let mut filled = Vec::new();
  let mut skipped = Vec::new();
  // Google sign-in is two steps. Which step the URL is on decides what gets
  // filled: the identifier (account) step fills the email only, and once the
  // flow moves to the challenge (password) step the email field has collapsed
  // into a hidden remnant, so only the password is filled. A step never touches
  // the other field — the account field only ever holds the account, the
  // password field only the password.
  //
  // A field is filled once per page (keyed by document URL): once a fill lands,
  // every later probe on the same page leaves that field well alone so a user
  // typing over it by hand is never disturbed.
  let path = host_path(&url).1;
  let is_challenge = path.contains("challenge") || path.contains("pwd");
  let email_key = format!("{url}|email");
  let password_key = format!("{url}|password");

  if is_challenge {
    if filled_once.contains(&password_key) {
      skipped.push("password-already-filled".to_string());
    } else {
      match find_and_fill(
        &mut conn,
        &mut cmd_id,
        root_id,
        &password_selectors,
        password,
      )
      .await
      {
        FillOutcome::Filled => {
          filled_once.insert(password_key);
          filled.push("password".to_string());
        }
        FillOutcome::AlreadyFilled => skipped.push("password-already-filled".to_string()),
        FillOutcome::NotFound | FillOutcome::Failed => {
          skipped.push("no-password-field".to_string())
        }
      }
    }
  } else if filled_once.contains(&email_key) {
    skipped.push("email-already-filled".to_string());
  } else {
    // Identifier step: the account goes into the account field, and the
    // password is deliberately left alone — a stray `type=password` match here
    // is what once put the password into the visible account box.
    match find_and_fill(&mut conn, &mut cmd_id, root_id, &email_selectors, email).await {
      FillOutcome::Filled => {
        filled_once.insert(email_key);
        filled.push("email".to_string());
      }
      FillOutcome::AlreadyFilled => skipped.push("email-already-filled".to_string()),
      FillOutcome::NotFound | FillOutcome::Failed => skipped.push("no-email-field".to_string()),
    }
  }
  conn.close().await;

  Ok(if !filled.is_empty() {
    format!("filled:{}", filled.join("+"))
  } else {
    format!(
      "filled:nothing:{}+on={}",
      skipped.join("+"),
      host_path(&url).1
    )
  })
}

enum FillOutcome {
  Filled,
  AlreadyFilled,
  NotFound,
  Failed,
}

/// Locate the first matching input and type the value through the real input
/// pipeline. Returns `AlreadyFilled` when a matched node's write was refused.
async fn find_and_fill(
  conn: &mut crate::cdp_target::CdpConnection,
  cmd_id: &mut u64,
  root_id: i64,
  selectors: &[&str],
  value: &str,
) -> FillOutcome {
  for selector in selectors {
    let node_id = match conn
      .call(
        *cmd_id,
        "DOM.querySelector",
        serde_json::json!({ "nodeId": root_id, "selector": selector }),
      )
      .await
    {
      Ok(result) => result.get("nodeId").and_then(Value::as_i64).unwrap_or(0),
      Err(_) => {
        *cmd_id += 1;
        return FillOutcome::Failed;
      }
    };
    *cmd_id += 1;
    if node_id == 0 {
      continue;
    }
    return match type_into_field(conn, cmd_id, node_id, value).await {
      FillOutcome::Filled => FillOutcome::Filled,
      _ => FillOutcome::AlreadyFilled,
    };
  }
  FillOutcome::NotFound
}

/// Focus a field and type into it via `Input.insertText`.
///
/// `DOM.setInputValue` is not exposed by the trimmed browser protocol (it
/// answers `'-32601' wasn't found`), so the write goes through the real input
/// path: focus the node, select-all + delete any stale value, then
/// `Input.insertText`. That fires the `beforeinput`/`input` events React
/// listen to, which is exactly what Google's sign-in inputs need.
async fn type_into_field(
  conn: &mut crate::cdp_target::CdpConnection,
  cmd_id: &mut u64,
  node_id: i64,
  value: &str,
) -> FillOutcome {
  match conn
    .call(
      *cmd_id,
      "DOM.focus",
      serde_json::json!({ "nodeId": node_id }),
    )
    .await
  {
    Ok(_) => {}
    Err(e) => {
      crate::bwbrowser_cloud::log_bwbrowser(
        "autofill",
        &format!("DOM.focus failed (node {node_id}): {e}"),
      );
    }
  }
  *cmd_id += 1;

  match conn
    .call(
      *cmd_id,
      "Input.insertText",
      serde_json::json!({ "text": value }),
    )
    .await
  {
    Ok(_) => FillOutcome::Filled,
    Err(e) => {
      crate::bwbrowser_cloud::log_bwbrowser(
        "autofill",
        &format!("Input.insertText failed (node {node_id}): {e}"),
      );
      FillOutcome::Failed
    }
  }
}

/// Split `host` and path out of a URL (used for the "on=" diagnostic).
fn host_path(url: &str) -> (String, String) {
  let after_scheme = url.split("://").nth(1).unwrap_or(url);
  let (host_part, path_part) = match after_scheme.split_once('/') {
    Some((h, p)) => (h, p),
    None => (after_scheme, ""),
  };
  let host = host_part.split(':').next().unwrap_or("").to_string();
  let path = path_part.split('?').next().unwrap_or("");
  (host, format!("/{path}"))
}
