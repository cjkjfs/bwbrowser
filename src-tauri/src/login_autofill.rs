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

/// The autofill script with the profile's credentials and the per-platform
/// plans substituted in.
///
/// `serde_json::to_string` on a `&str` produces the quoted, escaped literal the
/// script's `const EMAIL = ...` needs. The plans come from platform_config and
/// let the server push selector fixes without a client release.
fn autofill_script(
  credentials: &LoginCredentials,
  plans: &[crate::bwbrowser_cloud::AutofillPlan],
  totp: &str,
  sms_code: &str,
) -> String {
  include_str!("../assets/login_autofill.js")
    .replace(
      "__EMAIL__",
      &serde_json::to_string(&credentials.email).unwrap_or_else(|_| "\"\"".to_string()),
    )
    .replace(
      "__PASSWORD__",
      &serde_json::to_string(&credentials.password).unwrap_or_else(|_| "\"\"".to_string()),
    )
    // 每次轮询重新生成：脚本里 2FA 字段的值变了才会重输，30 秒窗口刷新
    // 后自动用最新验证码覆盖未提交的旧码
    .replace(
      "__TOTP__",
      &serde_json::to_string(totp).unwrap_or_else(|_| "\"\"".to_string()),
    )
    // 手机号（bind_phone）：phone 步骤填；短信验证码：服务端实时拉取，
    // 没到短信时为空串（脚本自动跳过该步，下一轮再试）
    .replace(
      "__PHONE__",
      &serde_json::to_string(credentials.phone.as_deref().unwrap_or(""))
        .unwrap_or_else(|_| "\"\"".to_string()),
    )
    .replace(
      "__SMSCODE__",
      &serde_json::to_string(sms_code).unwrap_or_else(|_| "\"\"".to_string()),
    )
    .replace(
      "__AUTOFILL_PLANS__",
      &serde_json::to_string(plans).unwrap_or_else(|_| "[]".to_string()),
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
  // 「CDP 端口就绪」只在首次拿到端口时打一条：轮询期间端口不会变，逐轮
  // 打印只会刷屏。
  let mut ready_logged = false;
  // 凭据构成只在首次读到凭据时打一条：密码没保存、TOTP 缺失这类
  // 「从源头就没值可填」的问题，一条日志即可定位。
  let mut cred_logged = false;
  // Fields already filled once on a page (`"{documentURL}|email"` /
  // `"{documentURL}|password"`). A filled field is left alone on every later
  // probe — Google often re-renders the same page and we must not retype over
  // what the user typed by hand, nor append a duplicate.
  let mut filled_once: HashSet<String> = HashSet::new();
  // 2FA 最近一次输入的验证码（DOM 回退路径用）：验证码随 30 秒窗口变化，
  // 变了就重新输入，覆盖还没提交的旧码
  let mut last_2fa_code: Option<String> = None;
  // 「已到登录页但未填充」按 URL 去重：同一结论 30 秒只记一条（轮询 1.5s
  // 一轮，不去重会刷屏），结论变化立即再记，方便对照操作时序。
  let mut verdict_log: std::collections::HashMap<String, (String, std::time::Instant)> =
    std::collections::HashMap::new();
  // 「Runtime.evaluate 受限」每个页面只记一次：回退 DOM 域后每轮行为一致。
  let mut gated_logged: HashSet<String> = HashSet::new();
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
    if !cred_logged {
      cred_logged = true;
      crate::bwbrowser_cloud::log_bwbrowser(
        "autofill",
        &format!(
          "凭据: email={}，密码{}，TOTP密钥{}，手机号{}",
          credentials.email,
          if credentials.password.is_empty() {
            "为空！（账号编辑里没保存密码，无值可填）"
          } else {
            "已设置"
          },
          if credentials
            .totp_secret
            .as_deref()
            .is_some_and(|s| !s.is_empty())
          {
            "已设置"
          } else {
            "未设置"
          },
          if credentials.phone.as_deref().is_some_and(|s| !s.is_empty()) {
            "已设置"
          } else {
            "未设置！（云端账号没填绑定手机号，phone 步无值可填）"
          },
        ),
      );
    }
    if !profile
      .process_id
      .is_some_and(crate::proxy_storage::is_process_running)
    {
      break "browser exited";
    }

    // The launch-time cookie-health check has confirmed login: the account is
    // already signed in, so there is nothing left to autofill — stop instead of
    // polling the login page forever.
    //
    // Valid 必须稳定 20 秒才采信：云端 cookie 恢复流程注入成功后立刻写一个
    // 过渡 Valid（依据是 cookie 评分的 HTTP 探测，页面亲证还没跑），页面亲证
    // 发现仍停在登录页后约 15 秒才改标 Expired。立即采信会在恢复失败时误杀
    // watcher（2026-11-24 perskytanberg 实测：邮箱已填好、停在密码页没人填，
    // 日志 11:51:14 写 Valid → watcher 秒停 → 11:51:29 才改标 Expired）。
    // 真正的登录确认晚停 20 秒没有任何代价。
    if crate::cookie_health::profile_health_map()
      .get(profile_id)
      .is_some_and(|r| {
        r.status == crate::cookie_health::HealthStatus::Valid
          && r.checked_at >= started_at
          && crate::proxy_manager::now_secs() >= r.checked_at.saturating_add(20)
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
    if !ready_logged {
      crate::bwbrowser_cloud::log_bwbrowser(
        "autofill",
        &format!("CDP 端口就绪: {port}，开始探测页面"),
      );
      ready_logged = true;
    }

    // 每个轮询周期解析一次自动填表方案：内部带 TTL 缓存，服务端改选择器最多 5 分钟生效。
    let plans = crate::bwbrowser_cloud::autofill_plans().await;

    let totp = credentials
      .totp_secret
      .as_deref()
      .and_then(crate::totp::generate_totp)
      .unwrap_or_default();
    // 短信验证码实时拉取（逗哥配音等手机号登录平台）：方案里带 sms 步骤
    // 且凭据有手机号才拉，避免无关账号每 1.5s 白打一次服务端
    let mut sms_code = String::new();
    let has_phone = credentials.phone.as_deref().is_some_and(|s| !s.is_empty());
    let needs_sms = has_phone
      && plans
        .iter()
        .any(|p| p.steps.iter().any(|s| s.role == "sms"));
    if needs_sms {
      sms_code = crate::bwbrowser_cloud::fetch_sms_code(credentials.phone.as_deref().unwrap_or(""))
        .await
        .unwrap_or_default();
    }
    match probe_pages(
      port,
      &autofill_script(credentials, &plans, &totp, &sms_code),
      &credentials.email,
      &credentials.password,
      credentials.phone.as_deref().unwrap_or(""),
      &sms_code,
      &profile.name,
      profile_id,
      &mut filled_once,
      &plans,
      &totp,
      &mut last_2fa_code,
      &mut gated_logged,
      &mut verdict_log,
    )
    .await
    {
      ProbeResult::Filled => {
        consecutive_failures = 0;
      }
      ProbeResult::Nothing => consecutive_failures = 0,
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
#[allow(clippy::too_many_arguments)]
async fn probe_pages(
  port: u16,
  script: &str,
  email: &str,
  password: &str,
  phone: &str,
  sms_code: &str,
  profile_name: &str,
  profile_id: &str,
  filled_once: &mut HashSet<String>,
  plans: &[crate::bwbrowser_cloud::AutofillPlan],
  totp: &str,
  last_2fa_code: &mut Option<String>,
  gated_logged: &mut HashSet<String>,
  verdict_log: &mut std::collections::HashMap<String, (String, std::time::Instant)>,
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

    match probe_page(
      ws_url,
      script,
      email,
      password,
      phone,
      sms_code,
      filled_once,
      plans,
      totp,
      last_2fa_code,
      gated_logged,
    )
    .await
    {
      Ok(verdict) if verdict.starts_with("filled:") && !verdict.contains("nothing") => {
        // 同 (url, verdict) 30 秒去重：轮询 1.5s 一轮，反复重填类问题
        // （受控组件弹回）会每轮重复报「已填充」，首条 + 变化才记
        let now = std::time::Instant::now();
        let mut should_log = true;
        if let Some((last_verdict, at)) = verdict_log.get_mut(url) {
          if *last_verdict == verdict && now.duration_since(*at) < Duration::from_secs(30) {
            should_log = false;
          } else {
            *last_verdict = verdict.clone();
            *at = now;
          }
        }
        if should_log {
          verdict_log.insert(url.to_string(), (verdict.clone(), now));
          let url_short = &url[..url.len().min(120)];
          crate::bwbrowser_cloud::log_bwbrowser(
            "autofill",
            &format!(
              "已填充登录框（{}）: {verdict} (url={url_short})",
              profile_name
            ),
          );
        }
        any_filled = true;
      }
      Ok(verdict) if verdict == "skip:not-signin" => {
        log::debug!("Login autofill: not a sign-in page, skipping ({url})");
      }
      Ok(verdict) if verdict.starts_with("filled:") && verdict.contains("nothing") => {
        // 「字段全部已填好」的状态（只剩 already-filled/already-typed）不值得
        // 每轮吵；只有这些 token 才代表真的没填上，值得大声记日志：
        let needs_attention = verdict.contains("no-field")
          || verdict.contains("no-value")
          || verdict.contains("-hidden(")
          || verdict.contains("-bounced")
          || verdict.contains("write-refused")
          || verdict.contains("query-error")
          || verdict.contains("type-guard");
        if needs_attention {
          // 到达登录页但字段缺失/未就绪：这是最需要关注的诊断信号。
          // 同一 (url, verdict) 30 秒只记一条，结论变化立即再记。
          // 顺带打出命中方案的选择器清单——对着页面 HTML 在 platform_config
          // 的平台条目里补/改 autofill 选择器即可（服务端改 5 分钟内生效）。
          let now = std::time::Instant::now();
          let mut should_log = true;
          if let Some((last_verdict, at)) = verdict_log.get_mut(url) {
            if *last_verdict == verdict && now.duration_since(*at) < Duration::from_secs(30) {
              should_log = false;
            } else {
              *last_verdict = verdict.clone();
              *at = now;
            }
          }
          if should_log {
            verdict_log.insert(url.to_string(), (verdict.clone(), now));
            let short: String = verdict.chars().take(200).collect();
            let url_short = &url[..url.len().min(120)];
            let sel_desc = crate::bwbrowser_cloud::autofill_plan_for_url(plans, url)
              .map(|p| {
                p.steps
                  .iter()
                  .map(|s| format!("{}:{}", s.role, s.selectors.join(" | ")))
                  .collect::<Vec<_>>()
                  .join(" ;; ")
              })
              .unwrap_or_else(|| "无命中方案（检查 platform_config 的 matching 规则）".to_string());
            crate::bwbrowser_cloud::log_bwbrowser(
              "autofill",
              &format!("已到登录页但未填充: {short} (url={url_short}) 方案选择器=[{sel_desc}]"),
            );
          }
        }
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
#[allow(clippy::too_many_arguments)]
async fn probe_page(
  ws_url: &str,
  script: &str,
  email: &str,
  password: &str,
  phone: &str,
  sms_code: &str,
  filled_once: &mut HashSet<String>,
  plans: &[crate::bwbrowser_cloud::AutofillPlan],
  totp: &str,
  last_2fa_code: &mut Option<String>,
  gated_logged: &mut HashSet<String>,
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
      // 每个页面只记一次：回退 DOM 域后每轮行为一致，逐轮打印只会刷屏。
      if gated_logged.insert(ws_url.to_string()) {
        crate::bwbrowser_cloud::log_bwbrowser(
          "autofill",
          &format!("Runtime.evaluate 受限（{err_str}），本页改用 DOM 域填充"),
        );
      }
      probe_page_dom(
        &target,
        email,
        password,
        phone,
        sms_code,
        filled_once,
        plans,
        totp,
        last_2fa_code,
      )
      .await
    }
  }
}

/// Fill the credentials through the DOM + Input domains when the gate blocks
/// scripts.
///
/// nodeId is scoped to one connection, so `getDocument`, every `querySelector`
/// and the type-into calls share a single connection. Returns the same
/// "filled:..." / "skip:not-signin" verdicts the scripted probe produces.
#[allow(clippy::too_many_arguments)]
async fn probe_page_dom(
  target: &CdpTarget,
  email: &str,
  password: &str,
  phone: &str,
  sms_code: &str,
  filled_once: &mut HashSet<String>,
  plans: &[crate::bwbrowser_cloud::AutofillPlan],
  totp: &str,
  last_2fa_code: &mut Option<String>,
) -> Result<String, String> {
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
  let Some(plan) = crate::bwbrowser_cloud::autofill_plan_for_url(plans, &url) else {
    conn.close().await;
    return Ok("skip:not-signin".to_string());
  };
  let root_id = root.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
  if root_id == 0 {
    conn.close().await;
    return Ok("filled:nothing:no-root-node".to_string());
  }

  let mut cmd_id = 2u64;
  let mut filled = Vec::new();
  let mut skipped = Vec::new();

  // 按方案步骤顺序逐个尝试。字段可见才输入（隐藏框没有盒模型，自动跳过），
  // 所以：Google 两步流的 identifier 页只会填到邮箱、challenge 页只会填到
  // 密码/2FA；单页同时有邮箱+密码框的平台（如 PayPal）一步全填。
  //
  // A field is filled once per page (keyed by document URL + role): once a
  // fill lands, every later probe on the same page leaves that field well
  // alone so a user typing over it by hand is never disturbed.
  let (host, path) = host_path(&url);
  let is_challenge = path.contains("challenge") || path.contains("pwd");
  // Google 两步流是确定的：identifier 页没有密码/2FA 框、challenge 页没有
  // 邮箱框——这些「本来就不在本页」的 no-field 不算问题，安静跳过；
  // 其它平台的流程未知，no-field 一律按可疑上报。
  let is_google = host.contains("accounts.google.") || host.contains("accounts.youtube.");

  for step in &plan.steps {
    let role = step.role.as_str();
    let value = match role {
      "email" => email,
      "password" => password,
      "2fa" => totp,
      "phone" => phone,
      "sms" => sms_code,
      _ => {
        skipped.push(format!("{role}:unknown-role"));
        continue;
      }
    };
    if step.selectors.is_empty() {
      continue;
    }
    let key = format!("{url}|{role}");
    if value.is_empty() {
      // 没存对应凭据：无值可填，最常见的「不填」根因
      // （配合 watcher 启动时的凭据日志定位）
      // sms 例外：短信还没到是正常态（用户刚点发送），不打 no-value 噪音
      if role != "sms" {
        skipped.push(format!("{role}:no-value"));
      }
      continue;
    }
    // 2FA 换码例外：TOTP 30 秒窗口变了（last_2fa_code != 新码）要重新输入，
    // 不被 filled_once 拦截；其余角色填过一次就永不重碰（用户手改不覆盖）
    let code_changed = role == "2fa" && last_2fa_code.as_deref() != Some(value);
    if filled_once.contains(&key) && !code_changed {
      skipped.push(format!("{role}:already-filled-once"));
      continue;
    }
    if role == "2fa" && last_2fa_code.as_deref() == Some(value) {
      skipped.push("2fa:already-typed".to_string());
      continue;
    }
    let selectors: Vec<&str> = step.selectors.iter().map(String::as_str).collect();
    let (outcome, detail) = find_and_fill(
      &mut conn,
      &mut cmd_id,
      root_id,
      role,
      &selectors,
      value,
      step.split.as_ref().map(|s| s.length),
    )
    .await;
    match outcome {
      FillOutcome::Filled => {
        filled_once.insert(key);
        if role == "2fa" {
          // 只有真的写进去了才记录：identifier 页上 2FA 框不存在时
          // 不能提前记码，否则到了 TOTP 页会被误判为已输入
          *last_2fa_code = Some(value.to_string());
        }
        filled.push(detail);
      }
      FillOutcome::AlreadyFilled => skipped.push(detail),
      FillOutcome::NotFound | FillOutcome::Failed => {
        // Google 两步流是确定的：identifier 页没有密码/2FA 框、challenge 页
        // 没有邮箱框；challenge 里只有 pwd 页有密码框、只有 totp/otp 页有
        // 2FA 框（recaptcha 等中间页两者都没有）——「本就不在本页」安静
        // 跳过；其它平台流程未知，no-field 一律按可疑上报。
        let expected_absent = is_google
          && match role {
            "email" => is_challenge,
            "password" => is_challenge && !path.contains("pwd"),
            "2fa" => is_challenge && !path.contains("totp") && !path.contains("otp"),
            _ => false,
          };
        if expected_absent {
          skipped.push(format!("{role}:not-on-this-step"));
        } else {
          skipped.push(detail);
        }
      }
    }
  }
  conn.close().await;

  Ok(if !filled.is_empty() {
    // 填上的明细之外，把跳过项也带上：一条日志同时看到「填了什么」和
    // 「另一条腿为什么没填」（如 TOTP 未配置）
    if skipped.is_empty() {
      format!("filled:{}", filled.join("+"))
    } else {
      format!("filled:{} |skipped:{}", filled.join("+"), skipped.join("+"))
    }
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

/// Locate the first matching, visible input and type the value through the
/// real input pipeline. Returns the outcome plus a diagnostic token that names
/// the role, the selector that landed (or which selectors were hidden/absent)
/// — it lands in the probe verdict, so one log line explains what happened.
///
/// `split_length`（Some(n)）走分体式：querySelectorAll 找 >= n 个元素的逐格
/// 选择器，第 N 格填验证码第 N 位（PayPal 6 格 OTP）。
#[allow(clippy::too_many_arguments)]
async fn find_and_fill(
  conn: &mut crate::cdp_target::CdpConnection,
  cmd_id: &mut u64,
  root_id: i64,
  role: &str,
  selectors: &[&str],
  value: &str,
  split_length: Option<usize>,
) -> (FillOutcome, String) {
  if let Some(n) = split_length {
    return find_and_fill_split(conn, cmd_id, root_id, role, selectors, value, n).await;
  }
  let mut hidden: Vec<&str> = Vec::new();
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
      Err(e) => {
        *cmd_id += 1;
        return (FillOutcome::Failed, format!("{role}:query-error({e})"));
      }
    };
    *cmd_id += 1;
    if node_id == 0 {
      continue;
    }
    // 可见性校验：Google 登录页藏着别步骤的输入框（如 identifier 页里的
    // 隐藏 password 框），页面切换的瞬间可能先被匹配到——不渲染的框
    // 没有盒模型，跳过它继续找下一个选择器，避免把值打进看不见的框。
    let visible = match conn
      .call(
        *cmd_id,
        "DOM.getBoxModel",
        serde_json::json!({ "nodeId": node_id }),
      )
      .await
    {
      Ok(box_result) => box_result.get("model").is_some(),
      Err(_) => false,
    };
    *cmd_id += 1;
    if !visible {
      hidden.push(*selector);
      continue;
    }
    return match type_into_field(conn, cmd_id, node_id, value).await {
      FillOutcome::Filled => (FillOutcome::Filled, format!("{role}({selector})")),
      _ => (
        FillOutcome::AlreadyFilled,
        format!("{role}:write-refused({selector})"),
      ),
    };
  }
  let detail = if hidden.is_empty() {
    format!("{role}:no-field")
  } else {
    format!("{role}:no-field(hidden:{})", hidden.join(","))
  };
  (FillOutcome::NotFound, detail)
}

/// 分体式验证码框（DOM 回退路径）：与 JS 路径同一选择器语义——selector 需
/// 一条匹配全部格子（如 PayPal 的 `input[name^="otpCode-"]`），querySelectorAll
/// 文档顺序即 Digit 1..N，第 i 格填验证码第 i 位。逐格可见性校验 + 覆盖式输入。
#[allow(clippy::too_many_arguments)]
async fn find_and_fill_split(
  conn: &mut crate::cdp_target::CdpConnection,
  cmd_id: &mut u64,
  root_id: i64,
  role: &str,
  selectors: &[&str],
  value: &str,
  n: usize,
) -> (FillOutcome, String) {
  let mut absent: Vec<&str> = Vec::new();
  for selector in selectors {
    let node_ids: Vec<i64> = match conn
      .call(
        *cmd_id,
        "DOM.querySelectorAll",
        serde_json::json!({ "nodeId": root_id, "selector": selector }),
      )
      .await
    {
      Ok(result) => result
        .get("nodeIds")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default(),
      Err(e) => {
        *cmd_id += 1;
        return (FillOutcome::Failed, format!("{role}:query-error({e})"));
      }
    };
    *cmd_id += 1;
    if node_ids.len() < n {
      absent.push(selector);
      continue;
    }
    if value.chars().count() != n {
      // 验证码位数与格数不符（TOTP 密钥配置异常等），不硬塞
      return (
        FillOutcome::AlreadyFilled,
        format!("{role}:length-mismatch({}!={n})", value.chars().count()),
      );
    }
    let mut states: Vec<String> = Vec::new();
    let mut typed = 0usize;
    for (idx, node_id) in node_ids.iter().take(n).enumerate() {
      let Some(ch) = value.chars().nth(idx) else {
        break;
      };
      // 逐格可见性校验：隐藏格跳过（状态记录，不中断）
      let visible = match conn
        .call(
          *cmd_id,
          "DOM.getBoxModel",
          serde_json::json!({ "nodeId": node_id }),
        )
        .await
      {
        Ok(box_result) => box_result.get("model").is_some(),
        Err(_) => false,
      };
      *cmd_id += 1;
      if !visible {
        states.push(format!("{idx}:hidden"));
        continue;
      }
      match type_into_field(conn, cmd_id, *node_id, &ch.to_string()).await {
        FillOutcome::Filled => {
          typed += 1;
          states.push(format!("{idx}:ok"));
        }
        _ => states.push(format!("{idx}:refused")),
      }
    }
    if typed == n {
      return (FillOutcome::Filled, format!("{role}-split{n}({selector})"));
    }
    if typed > 0 {
      return (
        FillOutcome::AlreadyFilled,
        format!("{role}:split-partial({typed}/{n}:{})", states.join(",")),
      );
    }
    // 一格都没写进去：记录后继续试下一条 selector
    absent.push(selector);
  }
  let detail = if absent.is_empty() {
    format!("{role}:no-field(split{n})")
  } else {
    format!("{role}:no-field(split{n},absent:{})", absent.join(","))
  };
  (FillOutcome::NotFound, detail)
}

/// Focus a field and type into it via `Input.insertText`.
///
/// `DOM.setInputValue` is not exposed by the trimmed browser protocol (it
/// answers `'-32601' wasn't found`), so the write goes through the real input
/// path: focus the node, select-all + delete any stale value (real keystrokes
/// through `Input.dispatchKeyEvent`), then `Input.insertText`. That fires the
/// `beforeinput`/`input` events React listen to, which is exactly what
/// Google's sign-in inputs need.
///
/// The select-all + backspace is what makes refills overwrite instead of
/// append: the TOTP code re-types every 30 s window, and without clearing the
/// field `insertText` would append the new code after the stale one.
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

  // 覆盖式输入：Ctrl+A 全选旧值 → 退格删除 → 插入新值。空字段上这两步
  // 是无害的空操作；key 事件失败不致命（接着 insertText 尽力而为）。
  for (event_type, key, code, vk, modifiers) in [
    ("rawKeyDown", "a", "KeyA", 65u32, 2u32),
    ("keyUp", "a", "KeyA", 65, 2),
    ("rawKeyDown", "Backspace", "Backspace", 8, 0),
    ("keyUp", "Backspace", "Backspace", 8, 0),
  ] {
    let _ = conn
      .call(
        *cmd_id,
        "Input.dispatchKeyEvent",
        serde_json::json!({
          "type": event_type,
          "key": key,
          "code": code,
          "windowsVirtualKeyCode": vk,
          "nativeVirtualKeyCode": vk,
          "modifiers": modifiers,
        }),
      )
      .await;
    *cmd_id += 1;
  }

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
