//! 共享 Cookie 池（2026-11-27）：公司隔离 × 平台隔离，视频下载降级链第三层。
//!
//! 链路：本机 Cookie 源 → 失败（bot 风控）→ 本公司该平台共享池（服务器轮询发放）
//!       → 再失败才报错。池子按 company_id + platform 分组，互不混用。
//!
//! 来源：用户在「云端账号 → 编辑账号」里点「提取Cookie到服务器」，客户端读取
//! 该账号本机 profile 的 Cookie 库（复用 CookieManager 解密管线），**只过滤目标
//! 平台的域**（硬红线：绝不带出其他网站登录态），转 Netscape 格式上传。
//!
//! 轮询：服务端按「最久未用优先」发放；同一来源账号重复提取 = 覆盖 + 重新激活。
//! 自愈：下载失败上报后服务端 fail_count+1，连续 2 次自动禁用（跳过+标红），
//!       成功上报清零；谁重新提取同账号即恢复回池。
//!
//! 服务端：bwbrowser_sync.php 的 upload_shared_cookie / get_shared_cookie /
//! report_shared_cookie / list_shared_cookies / delete_shared_cookie（表
//! browser_shared_cookies，UNIQUE company_id+platform+source_account_id）。

use serde::{Deserialize, Serialize};

/// 支持的平台（与服务端白名单一致）
pub const SUPPORTED_PLATFORMS: &[&str] = &["youtube", "bilibili", "douyin", "tiktok"];

/// Cookie 提取链返回的「个人来源为空，直接跳共享池」错误前缀标记。
/// 外层重试循环看到它就**不再裸跑 yt-dlp**（裸跑必然 bot 报错、白耗 6 次重试），
/// 直接进入共享池领取逻辑。截断提示时去掉标记只留人话。
pub const SHARED_POOL_FALLBACK_MARKER: &str = "\u{1F504}SHARED_FALLBACK\u{1F504}";

/// 各平台允许出现的 Cookie 域（硬红线：提取时只保留这些域，杜绝把
/// profile 里其他网站的登录态（邮箱/银行/后台）上传到服务器）。
fn platform_domains(platform: &str) -> &'static [&'static str] {
  match platform {
    "youtube" => &["youtube.com", "google.com", "googlevideo.com"],
    "bilibili" => &["bilibili.com", "b23.tv"],
    "douyin" => &["douyin.com"],
    "tiktok" => &["tiktok.com"],
    _ => &[],
  }
}

/// domain 是否命中平台白名单（cookie domain 形如 ".youtube.com" / "www.douyin.com"）
fn domain_matches(domain: &str, platform: &str) -> bool {
  let d = domain.to_lowercase();
  platform_domains(platform)
    .iter()
    .any(|pd| d == *pd || d.ends_with(&format!(".{pd}")) || d.starts_with(*pd))
}

/// 由下载 URL 猜平台（与 video_downloader 的 platform_tag 判定同规则）
pub fn platform_from_url(url: &str) -> &'static str {
  let lower = url.to_lowercase();
  if lower.contains("douyin.com") {
    "douyin"
  } else if lower.contains("tiktok.com") {
    "tiktok"
  } else if lower.contains("bilibili.com") || lower.contains("b23.tv") {
    "bilibili"
  } else {
    "youtube"
  }
}

/// 从 CookieManager 的 JSON 导出里滤出目标平台的 cookie，转 Netscape 文本。
/// 返回 (netscape 文本, 有效行数)。域白名单外的一律丢弃（不告警不计数）。
pub fn filter_and_to_netscape(cookie_json: &str, platform: &str) -> Result<(String, usize), String> {
  if !SUPPORTED_PLATFORMS.contains(&platform) {
    return Err(format!("不支持的平台: {platform}"));
  }
  let arr: Vec<serde_json::Value> = serde_json::from_str(cookie_json.trim())
    .map_err(|e| format!("Cookie JSON 解析失败: {e}"))?;
  let mut lines = vec!["# Netscape HTTP Cookie File".to_string()];
  for c in &arr {
    let domain = c.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if domain.is_empty() || name.is_empty() || !domain_matches(domain, platform) {
      continue;
    }
    let value = c.get("value").and_then(|v| v.as_str()).unwrap_or("");
    let path = c.get("path").and_then(|v| v.as_str()).unwrap_or("/");
    let secure = c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false);
    let http_only = c.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut expires = c
      .get("expirationDate")
      .and_then(|v| v.as_i64())
      .unwrap_or(-1);
    // 会话 Cookie 规范值是 0；-1 会被 yt-dlp 当 invalid expires 整条跳过
    if expires < 0 {
      expires = 0;
    }
    let flag = if domain.starts_with('.') { "TRUE" } else { "FALSE" };
    let host = if http_only {
      format!("#HttpOnly_{domain}")
    } else {
      domain.to_string()
    };
    lines.push(format!(
      "{host}\t{flag}\t{path}\t{}\t{expires}\t{name}\t{value}",
      if secure { "TRUE" } else { "FALSE" }
    ));
  }
  let valid = lines.len() - 1; // 去掉头注释
  if valid == 0 {
    return Err(format!(
      "该账号没有 {platform} 平台的 Cookie（请先用此浏览器环境登录 {platform} 再提取）"
    ));
  }
  Ok((lines.join("\n") + "\n", valid))
}

/// 按账号本机 profile 提取指定平台 Cookie 并上传服务器（编辑账号弹窗入口）。
/// 流程：account_profile_name 约定找 profile → CookieManager::export_cookies(json)
/// → 平台域过滤转 Netscape → POST upload_shared_cookie。
/// 返回 (有效行数, 来源 profile 名)。
#[tauri::command]
pub async fn shared_cookie_upload_from_account(
  account_id: i64,
  account_name: String,
  platform: String,
) -> Result<serde_json::Value, String> {
  let platform = platform.to_lowercase();
  if !SUPPORTED_PLATFORMS.contains(&platform.as_str()) {
    return Err(format!("不支持的平台: {platform}"));
  }
  let (username, password) = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "shared_cookie_upload",
    &format!("开始提取: 账号 {account_id}({account_name}) 平台 {platform}"),
  );

  // 按命名约定定位本机 profile（账号首次启动后才会存在）
  let profile_name = crate::bwbrowser_cloud::account_profile_name(&account_name, account_id);
  crate::bwbrowser_cloud::log_bwbrowser(
    "shared_cookie_upload",
    &format!("查找本机 profile: {profile_name}"),
  );
  let pm = crate::profile::manager::ProfileManager::instance();
  let profile = pm
    .list_profiles()
    .map_err(|e| e.to_string())?
    .into_iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase())
    .ok_or_else(|| {
      format!(
        "本机没有该账号的浏览器环境（{profile_name}），请先启动一次该账号再提取"
      )
    })?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "shared_cookie_upload",
    &format!(
      "profile 已找到（id={}），开始读取 Cookie 库（同步读，走 spawn_blocking 不卡 UI）",
      profile.id
    ),
  );

  // CookieManager::export_cookies 是同步 SQLite 读（大库/浏览器运行中可慢达秒~分钟级），
  // 必须放 spawn_blocking，否则会饿死 tauri 异步运行时线程、UI 看起来像卡死
  //（按钮一直「提取中…」）。与 lib.rs read_cookies 的包装惯例一致。
  let profile_id_str = profile.id.to_string();
  let all_json = tokio::task::spawn_blocking(move || {
    crate::cookie_manager::CookieManager::export_cookies(&profile_id_str, "json")
  })
  .await
  .map_err(|e| format!("Cookie 读取任务失败: {e}"))??;
  let (netscape, lines) = filter_and_to_netscape(&all_json, &platform)?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "shared_cookie_upload",
    &format!("Cookie 库读取完成: 平台 {platform} 有效 {lines} 条，开始上传服务器"),
  );

  let form_data = format!(
    "action=upload_shared_cookie&username={}&password={}&platform={}&account_id={}&cookie={}",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    crate::bwbrowser_cloud::urlencode(&platform),
    account_id,
    crate::bwbrowser_cloud::urlencode(&netscape),
  );
  let resp = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .http()
    .post(crate::cloud_domain::BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("上传失败: {e}"))?;
  let body = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
  let result: serde_json::Value =
    serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
  if !result["success"].as_bool().unwrap_or(false) {
    return Err(
      result["message"]
        .as_str()
        .unwrap_or("上传共享 Cookie 失败")
        .to_string(),
    );
  }
  crate::bwbrowser_cloud::log_bwbrowser(
    "upload_shared_cookie",
    &format!(
      "账号 {account_id}({account_name}) 平台 {platform} 上传共享 Cookie {lines} 条（profile「{}」）",
      profile.name
    ),
  );
  Ok(serde_json::json!({
    "lines": lines,
    "profile": profile.name,
    "platform": platform,
    "message": result["message"].as_str().unwrap_or("上传成功"),
  }))
}

/// 登记/移除共享 Cookie（编辑账号弹窗「共享Cookie」开关）。
/// 登记式：服务端只记「公司+平台+来源账号」，Cookie 发放时实时读该账号的
/// 云端快照（浏览器退出自动回传），不传任何 Cookie 内容。
/// 服务端会校验快照存在且含目标平台域，不满足时报错。
pub(crate) async fn register_shared_cookie(
  account_id: i64,
  platform: &str,
  share: bool,
) -> Result<String, String> {
  let (username, password) = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;
  let form_data = format!(
    "action=register_shared_cookie&username={}&password={}&platform={}&account_id={}&share={}",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    crate::bwbrowser_cloud::urlencode(platform),
    account_id,
    if share { "1" } else { "0" },
  );
  let resp = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .http()
    .post(crate::cloud_domain::BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("网络请求失败: {e}"))?;
  let body = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
  let result: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
  let msg = result["message"].as_str().unwrap_or("").to_string();
  if !result["success"].as_bool().unwrap_or(false) {
    return Err(if msg.is_empty() { "登记失败".to_string() } else { msg });
  }
  Ok(if msg.is_empty() { "操作成功".to_string() } else { msg })
}

/// 登记/移除共享 Cookie（编辑账号弹窗「共享Cookie」开关，前端保存时调用）。
#[tauri::command]
pub async fn shared_cookie_register(
  account_id: i64,
  platform: String,
  share: bool,
) -> Result<String, String> {
  let platform = platform.to_lowercase();
  if !SUPPORTED_PLATFORMS.contains(&platform.as_str()) {
    return Err(format!("不支持的平台: {platform}"));
  }
  register_shared_cookie(account_id, &platform, share).await
}

/// 查询（并可选消费）账号的「Cookie 强制回传」标记。
#[tauri::command]
pub async fn shared_cookie_get_force_sync_flag(
  account_id: i64,
  pick: bool,
) -> Result<bool, String> {
  get_force_sync_flag(account_id, pick).await
}

/// 查询（并可选消费）账号的「Cookie 强制回传」标记。
/// 共享 Cookie 下载失败时服务端会置 1；账号下次打开浏览器时客户端读取，
/// pick=1 顺手清掉，并在回传时跳过防覆盖闸门强制覆盖云端快照。
pub(crate) async fn get_force_sync_flag(account_id: i64, pick: bool) -> Result<bool, String> {
  let (username, password) = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;
  let form_data = format!(
    "action=get_force_sync_flag&username={}&password={}&account_id={}&pick={}",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    account_id,
    if pick { "1" } else { "0" },
  );
  let resp = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .http()
    .post(crate::cloud_domain::BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("网络请求失败: {e}"))?;
  let body = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
  let result: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
  if !result["success"].as_bool().unwrap_or(false) {
    return Err(result["message"].as_str().unwrap_or("查询失败").to_string());
  }
  Ok(result["force_sync"].as_bool().unwrap_or(false))
}

/// 从本公司共享池领取一份指定平台的 Cookie（下载降级时调用）。
/// 服务端按最久未用优先轮询发放。
/// 返回 LeaseOutcome::Empty = 池子空（携带服务器原文，便于任务日志排查）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedCookieLease {
  pub cookie_id: i64,
  pub cookie: String,
  /// 来源账号的数据库 id：共享 Cookie 重跑时用它解析代理（IP 与 Cookie 会话同源，
  /// 否则账号 A 的 Cookie 从中国 IP 直连 YouTube 会被判定异常 → 作废 + bot 风控）
  pub source_account_id: i64,
  pub source_account_name: String,
  pub pool_size: i64,
}

/// 领取结果：Lease = 拿到一份可用 Cookie；Empty = 池子空（msg 为服务器返回原文）。
pub(crate) enum LeaseOutcome {
  Lease(SharedCookieLease),
  Empty(String),
}

pub(crate) async fn lease_shared_cookie(platform: &str) -> Result<LeaseOutcome, String> {
  let (username, password) = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;
  log::info!("[shared_cookie] 领取请求发送（platform={platform}）");
  let form_data = format!(
    "action=get_shared_cookie&username={}&password={}&platform={}",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    crate::bwbrowser_cloud::urlencode(platform),
  );
  let resp = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .http()
    .post(crate::cloud_domain::BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await
    .map_err(|e| format!("网络请求失败: {e}"))?;
  let body = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
  let result: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
  if !result["success"].as_bool().unwrap_or(false) {
    let msg = result["message"].as_str().unwrap_or("").to_string();
    // 池子空是正常业务态；nginx 把 4xx 顶成 HTML 时 message 也会缺——把原文记下来便于定位
    if msg.contains("没有可用的") || msg.contains("请联系管理员") {
      log::info!("[shared_cookie] 池子空（platform={platform}）：{msg}");
      return Ok(LeaseOutcome::Empty(msg));
    }
    let snippet: String = body.chars().take(300).collect();
    log::warn!("[shared_cookie] 领取被拒（platform={platform}）message={msg:?} 响应原文={snippet}");
    if msg.is_empty() {
      return Err(format!("服务器响应异常（无 message 字段）: {snippet}"));
    }
    return Err(msg);
  }
  let lease = SharedCookieLease {
    cookie_id: result["cookie_id"].as_i64().unwrap_or(0),
    cookie: result["cookie"].as_str().unwrap_or("").to_string(),
    source_account_id: result["source_account_id"].as_i64().unwrap_or(0),
    source_account_name: result["source_account_name"].as_str().unwrap_or("").to_string(),
    pool_size: result["pool_size"].as_i64().unwrap_or(0),
  };
  let lines = lease
    .cookie
    .lines()
    .filter(|l| !l.trim_start().starts_with('#'))
    .count();
  log::info!(
    "[shared_cookie] 领取成功（platform={platform} id={} 来源={} 池内{}份 Cookie {}行）",
    lease.cookie_id,
    lease.source_account_name,
    lease.pool_size,
    lines
  );
  Ok(LeaseOutcome::Lease(lease))
}

/// 上报一次共享 Cookie 的使用结果（成功清零 / 失败累计，连续 2 次服务端自动禁用）。
/// 异步尽力而为，网络失败不影响下载主流程；返回服务器处理结果文本（供任务日志）。
pub(crate) async fn report_shared_cookie_result(cookie_id: i64, ok: bool, reason: &str) -> String {
  if cookie_id <= 0 {
    return "跳过上报（cookie_id 无效）".to_string();
  }
  let Some((username, password)) = crate::bwbrowser_cloud::BWBROWSER_AUTH.get_credentials() else {
    return "跳过上报（未登录）".to_string();
  };
  let form_data = format!(
    "action=report_shared_cookie&username={}&password={}&cookie_id={}&ok={}&reason={}",
    crate::bwbrowser_cloud::urlencode(&username),
    crate::bwbrowser_cloud::urlencode(&password),
    cookie_id,
    if ok { "1" } else { "0" },
    crate::bwbrowser_cloud::urlencode(reason),
  );
  let verdict = if ok { "成功✓" } else { "失败✗" };
  let client = reqwest::Client::new();
  let resp = client
    .post(crate::cloud_domain::BWBROWSER_API_URL)
    .header("Content-Type", "application/x-www-form-urlencoded")
    .body(form_data)
    .send()
    .await;
  let resp = match resp {
    Ok(r) => r,
    Err(e) => {
      let msg = format!("上报{verdict}请求失败: {e}");
      log::warn!("[shared_cookie] {msg}");
      return msg;
    }
  };
  let body = match resp.text().await {
    Ok(b) => b,
    Err(e) => {
      let msg = format!("上报{verdict}读响应失败: {e}");
      log::warn!("[shared_cookie] {msg}");
      return msg;
    }
  };
  let result: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::json!({}));
  if result["success"].as_bool().unwrap_or(false) {
    let msg = format!(
      "上报{verdict}完成: {}（{}）",
      result["message"].as_str().unwrap_or("ok"),
      // 服务端会返回禁用/复活/清零等处置说明，一并进日志
      if ok { "计数清零" } else { "累计失败计数" }
    );
    log::info!("[shared_cookie] {msg}");
    msg
  } else {
    let msg = format!(
      "上报{verdict}未受理: {}",
      result["message"].as_str().unwrap_or("无 message"),
    );
    let snippet: String = body.chars().take(300).collect();
    log::warn!("[shared_cookie] {msg} 响应原文={snippet}");
    msg
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn 平台域白名单过滤_只留目标平台() {
    let json = serde_json::json!([
      {"domain": ".youtube.com", "name": "SID", "value": "v1", "path": "/", "secure": true, "httpOnly": true, "expirationDate": 1900000000},
      {"domain": ".bilibili.com", "name": "SESSDATA", "value": "v2", "path": "/", "secure": true, "httpOnly": false, "expirationDate": 1900000000},
      {"domain": ".bank.example", "name": "session", "value": "secret", "path": "/", "secure": true, "httpOnly": true, "expirationDate": 1900000000}
    ])
    .to_string();
    let (txt, n) = filter_and_to_netscape(&json, "youtube").unwrap();
    assert_eq!(n, 1);
    assert!(txt.contains("SID"));
    assert!(!txt.contains("SESSDATA"), "B站 cookie 不得混入 YouTube 池");
    assert!(!txt.contains("secret"), "其他网站登录态绝不外传");
  }

  #[test]
  fn 无平台cookie时报错不导空壳() {
    let json = serde_json::json!([
      {"domain": ".example.com", "name": "a", "value": "b", "path": "/", "secure": false, "httpOnly": false, "expirationDate": 0}
    ])
    .to_string();
    assert!(filter_and_to_netscape(&json, "douyin").is_err());
  }

  #[test]
  fn 平台判定与下载器同规则() {
    assert_eq!(platform_from_url("https://www.youtube.com/watch?v=x"), "youtube");
    assert_eq!(platform_from_url("https://youtu.be/x"), "youtube");
    assert_eq!(platform_from_url("https://www.douyin.com/video/1"), "douyin");
    assert_eq!(platform_from_url("https://www.tiktok.com/@a/video/1"), "tiktok");
    assert_eq!(platform_from_url("https://www.bilibili.com/video/BV1"), "bilibili");
    assert_eq!(platform_from_url("https://b23.tv/abc"), "bilibili");
  }

  #[test]
  fn 会话cookie过期时间规范为0() {
    let json = serde_json::json!([
      {"domain": ".youtube.com", "name": "SID", "value": "v", "path": "/", "secure": false, "httpOnly": false}
    ])
    .to_string();
    let (txt, _) = filter_and_to_netscape(&json, "youtube").unwrap();
    // expirationDate 缺失 → 0，而不是 -1
    assert!(txt.contains("\t0\tSID\t"));
  }
}
