//! Cookie sync: CDP-based cookie injection at browser launch.
//!
//! Ports the core logic from simprint's cookie_sync.rs, adapted to use
//! bwbrowser's existing CDP infrastructure (cdp_target) instead of raw
//! WebSocket connections. Cookies fetched from the cloud are sanitized,
//! filtered by platform domain, and injected via CDP `Network.setCookies`
//! with a per-cookie fallback for robustness.

use crate::cdp_target::{self, CdpError};
use crate::profile::BrowserProfile;
use serde_json::Value;

/// Command ids for the CDP injection sequence.
// CMD_CLEAR 已废弃：不清空浏览器 Cookie，避免覆盖本地正常登录态
const CMD_BATCH_SET: u64 = 2;
const CMD_RELOAD: u64 = 3;
const CMD_BASE_PER_COOKIE: u64 = 100;

/// Returns the cookie domains associated with a platform.
/// Cookies not matching these domains are filtered out during injection.
pub fn platform_domains(platform: &str) -> Vec<&'static str> {
  let lower = platform.to_lowercase();
  let p = lower.trim();
  match p {
    "tiktok" | "tk" | "tiktok us" => {
      vec!["tiktok.com", "tiktokcdn.com", "tiktokv.com", "musical.ly"]
    }
    "youtube" | "yt" => vec![
      "youtube.com",
      "google.com",
      "youtu.be",
      "googleusercontent.com",
      "gstatic.com",
    ],
    "douyin" | "抖音" | "dy" | "抖音号" | "douyin_hao" => vec![
      "douyin.com",
      "douyincdn.com",
      "amemv.com",
      "iesdouyin.com",
      "snssdk.com",
      "toutiao.com",
    ],
    "xiaohongshu" | "小红书" | "xhs" | "little_red_book" => {
      vec!["xiaohongshu.com", "xhscdn.com"]
    }
    "kuaishou" | "快手" => vec![
      "kuaishou.com",
      "gifshow.com",
      "kwaicdn.com",
      "yxixy.com",
      "chenzhongtech.com",
    ],
    "weibo" | "微博" | "wb" | "wb_zt" => {
      vec!["weibo.com", "weibo.cn", "sina.com.cn", "sinaimg.cn"]
    }
    "bilibili" | "b站" | "哔哩哔哩" => {
      vec!["bilibili.com", "bilivideo.com", "bilivideo.cn", "hdslb.com"]
    }
    "netease" | "网易" | "网易邮箱" | "mail_163" => vec!["163.com", "126.com", "netease.com"],
    "outlook" => vec![
      "outlook.com",
      "live.com",
      "office.com",
      "microsoft.com",
      "msn.com",
    ],
    "toutiao" | "头条" | "今日头条" => {
      vec!["toutiao.com", "bytedance.com", "byteimg.com", "douyin.com"]
    }
    "baijiahao" | "百度号" | "百家号" | "baidu" | "百度" => {
      vec!["baidu.com", "bdstatic.com", "bdimg.com"]
    }
    "wechat_video" | "微信视频号" => vec!["weixin.qq.com", "wx.qq.com", "tenpay.com"],
    "wechat_mp" | "微信公众号" => vec!["mp.weixin.qq.com", "weixin.qq.com", "wx.qq.com"],
    "iqiyi" | "爱奇艺" => vec!["iqiyi.com", "iqiyipic.com", "qiyi.com"],
    "pdd" | "拼多多" => vec!["pinduoduo.com", "yangkeduo.com", "pddpic.com"],
    "sohu_video" | "搜狐视频" | "sohu" | "搜狐" => {
      vec!["sohu.com", "sohucs.com", "tv.sohu.com"]
    }
    "xigua" | "西瓜视频" => vec!["ixigua.com", "toutiao.com", "bytedance.com", "douyin.com"],
    "jingdong" | "京东" | "jd" => vec!["jd.com", "jdstatic.com", "360buyimg.com", "paipai.com"],
    "taobao" | "淘宝" => vec!["taobao.com", "taobaoimg.com", "tmall.com"],
    "tengxun_video" | "腾讯视频" | "qq" | "qq视频" => {
      vec!["qq.com", "tencent.com", "qpic.cn", "gtimg.com"]
    }
    "facebook" | "fb" => vec!["facebook.com", "fbcdn.net", "fb.me"],
    "instagram" | "ig" => vec!["instagram.com", "cdninstagram.com", "fbcdn.net"],
    "twitter" | "tw" | "x" => vec!["twitter.com", "x.com", "twimg.com", "t.co"],
    "zhihu" | "知乎" | "zhihu_zhuanlan" | "知乎专栏" => vec!["zhihu.com", "zhimg.cn"],
    "youku" | "优酷" => vec!["youku.com", "ykimg.com", "youkutv.com"],
    "meituan" | "美团" => vec!["meituan.com", "meituan.net", "dianping.com", "51ping.com"],
    "eleme" | "饿了么" => vec!["ele.me", "eleme.com"],
    "vipshop" | "唯品会" | "vip" => vec!["vip.com", "vipstatic.com"],
    "suning" | "苏宁" => vec!["suning.com", "suncdn.com"],
    "tieba" | "百度贴吧" => vec!["tieba.baidu.com", "baidu.com", "bdstatic.com"],
    "dian" | "大众点评" | "点评" => vec!["dianping.com", "51ping.com", "meituan.com"],
    "dingtalk" | "钉钉" => vec!["dingtalk.com", "aliyun.com"],
    "csdn" => vec!["csdn.net", "csdn.com"],
    "juejin" | "掘金" => vec!["juejin.cn", "juejin.com"],
    "芒果tv" | "芒果" => vec!["mgtv.com", "hunantv.com"],
    "mogu_v" | "蘑菇街" => vec!["mogujie.com", "mogu.com", "mogucdn.com"],
    "dangdang" | "当当" => vec!["dangdang.com"],
    "kaola" | "考拉" => vec!["kaola.com", "kaolacdn.com"],
    "yhd" | "一号店" => vec!["yhd.com", "yihaodian.com"],
    "flyme" | "魅族" | "meizu" => vec!["flyme.cn", "meizu.com"],
    "oppo" => vec!["oppo.com", "nearme.com.cn", "coloros.com"],
    "vivo" => vec!["vivo.com", "vivvo.com"],
    "huawei" | "华为" => vec!["huawei.com", "vmall.com", "hicloud.com"],
    "xiaomi" | "小米" => vec!["mi.com", "xiaomi.com", "miui.com"],
    "360" => vec!["360.cn", "haosou.com", "qihoo.com", "360.com"],
    "sina" | "新浪" => vec!["sina.com.cn", "sina.cn", "sina.com"],
    "ifeng" | "凤凰网" => vec!["ifeng.com", "ifengimg.com"],
    "gmw" | "光明网" => vec!["gmw.cn", "gmw.com.cn"],
    "people" | "人民网" => vec!["people.com.cn", "people.cn"],
    "china" | "中国网" => vec!["china.com.cn", "china.com"],
    "cctv" | "央视网" => vec!["cctv.com", "cntv.cn", "cctv.cn"],
    "huanqiu" | "环球网" => vec!["huanqiu.com", "huanqiu.net"],
    "eastday" | "东方网" => vec!["eastday.com", "eastday.cn"],
    "redstar" | "红星新闻" => vec!["redstar.cn", "redstar.com"],
    "yicai" | "第一财经" => vec!["yicai.com", "yicai.tv"],
    "cls" | "财联社" => vec!["cls.cn", "cailianpress.com"],
    "wallstreetcn" | "华尔街见闻" => vec!["wallstreetcn.com", "wallstreetcn.cn"],
    "xueqiu" | "雪球" => vec!["xueqiu.com", "xueqiu.net"],
    "eastmoney" | "东方财富" => vec!["eastmoney.com", "eastmoney.cn"],
    "douyin_xingtu" | "巨量星图" => vec![
      "douyin.com",
      "bytedance.com",
      "oceanengine.com",
      "xingtu.cn",
    ],
    _ => vec![],
  }
}

/// Check if a cookie domain matches any of the platform domains.
fn cookie_domain_matches(cookie_domain: &str, platform_domains: &[&str]) -> bool {
  let cd = cookie_domain.to_lowercase();
  let cd = cd.trim_start_matches('.');
  for pd in platform_domains {
    let pd = pd.trim_start_matches('.');
    if cd == pd || cd.ends_with(&format!(".{}", pd)) {
      return true;
    }
  }
  false
}

/// Parse a cookie payload from various formats (string/JSON/array/single object/Netscape text).
fn parse_cookie_payload(payload: &Value) -> Vec<Value> {
  match payload {
    Value::Array(_) => payload.as_array().unwrap().clone(),
    Value::String(s) => {
      let trimmed = s.trim();
      if trimmed.is_empty() {
        return vec![];
      }
      if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return serde_json::from_str::<Value>(trimmed)
          .ok()
          .and_then(|v| {
            if v.is_array() {
              v.as_array().cloned()
            } else if v.is_object() {
              Some(vec![v])
            } else {
              None
            }
          })
          .unwrap_or_default();
      }
      parse_netscape_cookies(trimmed)
    }
    Value::Object(_) => vec![payload.clone()],
    _ => vec![],
  }
}

/// Parse Netscape cookie file format:
/// domain  include_subdomains  path  secure  expires  name  value
/// Lines starting with # are comments (except #HttpOnly_ prefix).
fn parse_netscape_cookies(text: &str) -> Vec<Value> {
  let mut cookies = Vec::new();
  for line in text.lines() {
    let line = line.trim();
    if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
      continue;
    }
    let mut http_only = false;
    let line = if let Some(stripped) = line.strip_prefix("#HttpOnly_") {
      http_only = true;
      stripped
    } else {
      line
    };
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 7 {
      continue;
    }
    let domain = parts[0];
    let secure = parts[3].eq_ignore_ascii_case("TRUE");
    let expires: f64 = parts[4].parse().unwrap_or(0.0);
    let name = parts[5];
    let value = parts[6];
    let path = if parts.len() > 2 && !parts[2].is_empty() {
      parts[2].to_string()
    } else {
      "/".to_string()
    };
    cookies.push(serde_json::json!({
      "name": name,
      "value": value,
      "domain": domain,
      "path": path,
      "secure": secure,
      "httpOnly": http_only,
      "expires": expires,
    }));
  }
  cookies
}

/// Normalize a cookie item from various field name conventions.
/// Accepts: key/cookieName/name, value/cookieValue/val, domain/host/host_key,
/// path, secure, httpOnly/http_only, expires/expiry/expiration/expireTime.
fn normalize_cookie_item(c: &Value) -> Value {
  let mut out = serde_json::Map::new();

  for key in &["name", "key", "cookieName"] {
    if let Some(v) = c.get(key) {
      out.insert("name".to_string(), v.clone());
      break;
    }
  }
  for key in &["value", "cookieValue", "val"] {
    if let Some(v) = c.get(key) {
      out.insert("value".to_string(), v.clone());
      break;
    }
  }
  for key in &["domain", "host", "host_key"] {
    if let Some(v) = c.get(key) {
      out.insert("domain".to_string(), v.clone());
      break;
    }
  }
  for key in &["path"] {
    if let Some(v) = c.get(key) {
      out.insert("path".to_string(), v.clone());
      break;
    }
  }
  for key in &["secure", "is_secure"] {
    if let Some(v) = c.get(key) {
      let b = v.as_bool().unwrap_or_else(|| {
        v.as_str()
          .map(|s| s.eq_ignore_ascii_case("true"))
          .unwrap_or_else(|| v.as_i64().map(|n| n != 0).unwrap_or(false))
      });
      out.insert("secure".to_string(), Value::Bool(b));
      break;
    }
  }
  for key in &["httpOnly", "http_only", "httponly", "is_http_only"] {
    if let Some(v) = c.get(key) {
      let b = v.as_bool().unwrap_or_else(|| {
        v.as_str()
          .map(|s| s.eq_ignore_ascii_case("true"))
          .unwrap_or_else(|| v.as_i64().map(|n| n != 0).unwrap_or(false))
      });
      out.insert("httpOnly".to_string(), Value::Bool(b));
      break;
    }
  }
  for key in &["sameSite", "same_site", "samesite"] {
    if let Some(v) = c.get(key) {
      out.insert("sameSite".to_string(), v.clone());
      break;
    }
  }
  for key in &[
    "expires",
    "expiry",
    "expiration",
    "expireTime",
    "expires_at",
    "expirationDate",
  ] {
    if let Some(v) = c.get(key) {
      let exp = v
        .as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .unwrap_or(0.0);
      let secs = if exp > 1e15 { exp / 1000.0 } else { exp };
      out.insert("expires".to_string(), serde_json::json!(secs));
      break;
    }
  }

  Value::Object(out)
}

/// Sanitize a CDP cookie: fix SameSite=None+Secure, __Host- prefix, expires unit,
/// and add a `url` field so Chromium generates matching domain cookies.
fn sanitize_cdp_cookie(c: Value) -> Option<Value> {
  let c_val = normalize_cookie_item(&c);
  let mut c = c_val;
  let name = c
    .get("name")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_string();
  if name.is_empty() {
    return None;
  }

  let domain = c
    .get("domain")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .trim();
  if domain.is_empty() || domain.chars().any(|ch| ch.is_whitespace() || ch == '/') {
    return None;
  }
  let host = domain.trim_start_matches('.').to_string();

  let raw_secure = c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false);
  let raw_http = c.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false);
  let mut secure = raw_secure;
  c["secure"] = Value::Bool(secure);
  c["httpOnly"] = Value::Bool(raw_http);

  if name.starts_with("__Secure-") || name.starts_with("__Host-") {
    secure = true;
    c["secure"] = Value::Bool(true);
  }
  if name.starts_with("__Host-") {
    if let Some(o) = c.as_object_mut() {
      o.remove("domain");
    }
    c["path"] = Value::String("/".to_string());
  }

  if c
    .get("path")
    .and_then(|v| v.as_str())
    .map(|s| s.trim().is_empty())
    .unwrap_or(true)
  {
    c["path"] = Value::String("/".to_string());
  }

  if let Some(ss) = c.get("sameSite").and_then(|v| v.as_str()) {
    let norm = match ss.to_ascii_lowercase().as_str() {
      "lax" => "Lax",
      "strict" => "Strict",
      "none" => {
        c["secure"] = Value::Bool(true);
        secure = true;
        "None"
      }
      _ => {
        if let Some(o) = c.as_object_mut() {
          o.remove("sameSite");
        }
        "Unspecified"
      }
    };
    c["sameSite"] = Value::String(norm.to_string());
  }

  if let Some(exp) = c.get("expires").and_then(|v| v.as_f64()) {
    let now = chrono::Utc::now().timestamp() as f64;
    if exp <= 0.0 {
      if let Some(o) = c.as_object_mut() {
        o.remove("expires");
      }
    } else if exp > 1e15 {
      let secs = exp / 1000.0;
      if secs > 0.0 {
        c["expires"] = serde_json::json!(secs);
      } else if let Some(o) = c.as_object_mut() {
        o.remove("expires");
      }
    } else if exp <= now + 60.0 {
      if let Some(o) = c.as_object_mut() {
        o.remove("expires");
      }
    }
  }

  if !host.is_empty() {
    let scheme = if secure { "https" } else { "http" };
    c["url"] = Value::String(format!("{}://{}", scheme, host));
  }
  Some(c)
}

/// Filter cookies to only include those matching the platform's domains.
/// If platform is unknown (empty domain list), returns all cookies unchanged.
pub fn filter_cookies_by_platform(cookies: &[Value], platform: &str) -> Vec<Value> {
  let domains = platform_domains(platform);
  if domains.is_empty() {
    return cookies.to_vec();
  }
  cookies
    .iter()
    .filter(|c| {
      let domain = c.get("domain").and_then(|v| v.as_str()).unwrap_or("");
      !domain.is_empty() && cookie_domain_matches(domain, &domains)
    })
    .cloned()
    .collect()
}

/// Detect whether the browser already has login cookies for the platform.
pub fn has_login_cookies(browser_cookies: &[Value], platform: &str) -> bool {
  let platform_lower = platform.to_lowercase();
  let p = platform_lower.trim();
  let key_names: &[&str] = match p {
    "youtube" | "yt" => &[
      // 强登录 cookie：只有真正登录后才会设置
      // HSID/SSID/APISID/SAPISID 未登录也会有（访客 cookie），不能作为登录凭证
      "LOGIN_INFO",
      "SID",
      "__Secure-1PSID",
      "__Secure-3PSID",
      "SIDCC",
    ],
    "tiktok" | "tk" | "tiktok us" => &[
      "sessionid",
      "sessionid_ss",
      "sid_tt",
      "uid_tt",
      "passport_csrf_token",
      "ttwid",
    ],
    "douyin" | "抖音" | "dy" => &[
      "sessionid",
      "sessionid_ss",
      "sid_tt",
      "uid_tt",
      "passport_csrf_token",
      "ttwid",
    ],
    "toutiao" | "头条" | "今日头条" | "tt" => &[
      "sessionid",
      "sessionid_ss",
      "sid_tt",
      "passport_csrf_token",
      "uid_tt_ss",
      "sid_guard",
    ],
    "netease" | "网易" | "网易邮箱" | "mail_163" => &[
      "P_INFO",
      "S_INFO",
      "NTES_SESS",
      "NTES_PASS",
      "MAIL_SINFO",
      "MAIL163_SINFO",
    ],
    "outlook" => &["MSCC", "MSPAuth", "MSAuth1", "RPSSecAuth"],
    "facebook" | "fb" => &["c_user", "xs", "fr", "sb"],
    "instagram" | "ig" => &["sessionid", "ds_user_id", "csrftoken"],
    "bilibili" | "b站" | "哔哩哔哩" => {
      &["SESSDATA", "bili_jct", "DedeUserID", "DedeUserID__ckMd5"]
    }
    "weibo" | "微博" | "wb" => &["SUB", "SUBP", "SSOLockpin"],
    "xiaohongshu" | "小红书" | "xhs" => &["web_session", "xsecappid"],
    "kuaishou" | "快手" => &["userId", "passToken", "kuaishou.server.web_st"],
    _ => &[],
  };
  if key_names.is_empty() {
    return false;
  }
  let mut found = 0;
  for cookie in browser_cookies {
    let name = cookie.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if key_names.contains(&name) {
      if let Some(exp) = cookie.get("expires").and_then(|v| v.as_f64()) {
        if exp > 0.0 && exp < chrono::Utc::now().timestamp() as f64 {
          continue;
        }
      }
      found += 1;
      if found >= 2 {
        return true;
      }
    }
  }
  found >= 2
}

/// Whether a Google auth cookie should be skipped (session-level, server-rotated).
/// Injecting stale Google auth cookies causes accounts.google.com/CookieMismatch.
fn is_google_auth_cookie(c: &Value) -> bool {
  let domain = c
    .get("domain")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_lowercase();
  let domain = domain.trim_start_matches('.');
  let is_google = domain == "google.com"
    || domain == "accounts.google.com"
    || domain == "myaccount.google.com"
    || domain == "mail.google.com"
    || domain == "ssl.google.com"
    || domain.ends_with(".google.com")
    || domain == "googleusercontent.com"
    || domain.ends_with(".googleusercontent.com")
    || domain == "gstatic.com"
    || domain.ends_with(".gstatic.com");
  if !is_google {
    return false;
  }
  let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
  matches!(
    name,
    "SID"
      | "HSID"
      | "SSID"
      | "APISID"
      | "SAPISID"
      | "SIDCC"
      | "__Secure-1PSID"
      | "__Secure-3PSID"
      | "__Secure-1PAPISID"
      | "__Secure-3PAPISID"
      | "__Secure-1PSIDTS"
      | "__Secure-3PSIDTS"
      | "__Secure-1PSIDCC"
      | "__Secure-3PSIDCC"
      | "__Secure-ENID"
      | "__Secure-AAID"
      | "__Secure-3_AAID"
      | "LSID"
      | "GAPS"
      | "GAUTH"
      | "LSOLH"
      | "ACCOUNT_CHOOSER"
      | "GALX"
      | "GEXP"
      | "LSOSID"
      | "OTZ"
      | "OEXK"
      | "__Secure-1PSIDTS_BE"
      | "__Secure-3PSIDTS_BE"
      | "SMSV"
      | "KP_UID"
      | "KP_UIDZ"
  )
}

/// Merge two cookie sets, primary wins on key conflict (name|domain|path).
pub fn merge_cookies(primary: &[Value], secondary: &[Value]) -> Vec<Value> {
  use std::collections::HashMap;
  let mut map: HashMap<String, Value> = HashMap::new();
  for c in secondary {
    if let Some(key) = cookie_key(c) {
      map.insert(key, c.clone());
    }
  }
  for c in primary {
    if let Some(key) = cookie_key(c) {
      map.insert(key, c.clone());
    }
  }
  map.into_values().collect()
}

fn cookie_key(c: &Value) -> Option<String> {
  let name = c.get("name").and_then(|v| v.as_str())?;
  let domain = c.get("domain").and_then(|v| v.as_str()).unwrap_or("");
  let path = c.get("path").and_then(|v| v.as_str()).unwrap_or("/");
  let clean_domain = domain.trim_start_matches('.');
  Some(format!("{}|{}|{}", name, clean_domain, path))
}

/// Prepare raw cookie data (string or JSON) for CDP injection:
/// parse → normalize → sanitize → filter by platform → drop Google auth cookies.
pub fn prepare_cookies_for_injection(raw: &Value, platform: &str) -> Vec<Value> {
  let parsed = parse_cookie_payload(raw);
  // 先按平台过滤（需要 domain 信息，必须在 sanitize 之前）
  // 因为 __Host- 前缀的 cookie 在 sanitize 时会被移除 domain 字段
  let platform_filtered = filter_cookies_by_platform(&parsed, platform);
  // 再 sanitize（会移除 __Host- cookie 的 domain 等操作）
  platform_filtered
    .into_iter()
    .filter_map(sanitize_cdp_cookie)
    .filter(|c| !is_google_auth_cookie(c))
    .collect()
}

/// Cookie 质量评分结果（对齐爆文库 evaluateCookieSet 逻辑）
#[derive(Debug, Clone, Default)]
pub struct CookieScore {
  /// 总分 = validCount*2 + domainMatched*3 + keyCookies*10 + longExpiry*2 + sessionCookies - expired*8
  pub total: i32,
  /// 有效 Cookie 数量（非空 name/value 的）
  pub valid_count: usize,
  /// 过期 Cookie 数量
  pub expired: usize,
  /// 平台域名匹配数量
  pub domain_matched: usize,
  /// 关键会话 Cookie 数量
  pub key_cookies: usize,
  /// 长期有效 Cookie 数量（> 7 天）
  pub long_expiry: usize,
  /// 会话 Cookie 数量（无过期时间）
  pub session_cookies: usize,
  /// 是否可用：validCount>=2 && domainMatched>=1 && (keyCookies>=1 || validCount>=6)
  pub usable: bool,
  /// 是否确定已登录：expired==0 && domainMatched>=2 && keyCookies>=2 && (validCount>=6 || score>=45)
  pub definite_logged_in: bool,
}

/// 关键 Cookie 名称模式（对齐爆文库）
fn is_key_cookie_name(name: &str) -> bool {
  let lower = name.to_lowercase();
  let patterns = [
    "sid",
    "session",
    "token",
    "auth",
    "login",
    "uid",
    "user",
    "passport",
    "csrf",
    "xsrf",
    "ssid",
    "hsid",
    "apisid",
    "sapisid",
    "login_info",
    "sid_guard",
    "uid_tt",
    "n_mh",
    "s_v_web_id",
  ];
  patterns.iter().any(|p| lower.contains(p))
}

/// 计算 Cookie 质量评分（完全对齐爆文库 evaluateCookieSet 逻辑）
/// 评分公式：validCount*2 + domainMatched*3 + keyCookies*10 + longExpiry*2 + sessionCookies - expired*8
pub fn score_cookies(cookies: &[Value], platform: &str) -> CookieScore {
  let platform_cookies = filter_cookies_by_platform(cookies, platform);
  let domains = platform_domains(platform);

  let mut score = CookieScore::default();

  if platform_cookies.is_empty() {
    return score;
  }

  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0);

  for cookie in &platform_cookies {
    let name = cookie.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let value = cookie.get("value").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() || value.is_empty() {
      continue;
    }

    score.valid_count += 1;

    // 检查是否过期（60 秒缓冲）
    let mut has_expiry = false;
    if let Some(expires) = cookie.get("expires").and_then(|v| v.as_f64()) {
      has_expiry = true;
      if expires > 0.0 && (expires as i64) <= now + 60 {
        score.expired += 1;
      }
    }
    if let Some(max_age) = cookie.get("maxAge").and_then(|v| v.as_i64()) {
      has_expiry = true;
      if max_age <= 60 {
        score.expired += 1;
      }
    }

    // 域名匹配（只要 domain 包含任一平台域名就算匹配）
    let cookie_domain = cookie
      .get("domain")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_lowercase();
    if !cookie_domain.is_empty() {
      for d in &domains {
        let d_lower = d.to_lowercase();
        if cookie_domain == d_lower
          || cookie_domain.ends_with(&format!(".{}", d_lower))
          || d_lower.ends_with(&cookie_domain)
        {
          score.domain_matched += 1;
          break;
        }
      }
    }

    // 关键 Cookie
    if is_key_cookie_name(name) {
      score.key_cookies += 1;
    }

    // 会话 Cookie（无过期时间）
    if !has_expiry {
      score.session_cookies += 1;
    }

    // 长期有效（> 7 天）
    if let Some(expires) = cookie.get("expires").and_then(|v| v.as_f64()) {
      if expires > 0.0 && (expires as i64) > now + 86400 * 7 {
        score.long_expiry += 1;
      }
    }
    if let Some(max_age) = cookie.get("maxAge").and_then(|v| v.as_i64()) {
      if max_age > 86400 * 7 {
        score.long_expiry += 1;
      }
    }
  }

  // 评分公式：validCount*2 + domainMatched*3 + keyCookies*10 + longExpiry*2 + sessionCookies - expired*8
  score.total = (score.valid_count as i32) * 2
    + (score.domain_matched as i32) * 3
    + (score.key_cookies as i32) * 10
    + (score.long_expiry as i32) * 2
    + (score.session_cookies as i32)
    - (score.expired as i32) * 8;

  // 可用判断：validCount >= 2 && domainMatched >= 1 && (keyCookies >= 1 || validCount >= 6)
  score.usable = score.valid_count >= 2
    && score.domain_matched >= 1
    && (score.key_cookies >= 1 || score.valid_count >= 6);

  // 确定登录：必须找到至少 2 个平台专属的登录 cookie（has_login_cookies）
  // 不能用通用 key_cookies 数量判断 — 未登录时网站也会设置很多含 "sid"/"uid" 的访客 cookie，
  // 会导致误判为 "确定登录"，从而跳过云端 cookie 注入。
  // 同时要求过期数为 0，保证登录 cookie 都是有效的。
  score.definite_logged_in = score.expired == 0 && has_login_cookies(&platform_cookies, platform);

  score
}

/// Cookie 质量比较的最小分差阈值
/// 只有当服务器分数超过本地至少 8 分时，才用服务器覆盖本地
/// 避免服务器旧 Cookie 覆盖本地正常登录态
/// （原阈值 15 分过于保守，经常导致明明云端更好却不注入）
pub const MIN_SCORE_DIFF: i32 = 8;

/// 选择更优的 Cookie 集合（对齐爆文库 selectBestCookieSet 逻辑）
/// 返回 (是否用本地, 本地评分, 云端评分)
pub fn select_best_cookie_set(
  local_score: &CookieScore,
  cloud_score: &CookieScore,
) -> CookieSelection {
  // 1. 两边都"确定登录" → 比较分数，云端更高则注入（本地可能过期失效）
  if local_score.definite_logged_in && cloud_score.definite_logged_in {
    if cloud_score.total > local_score.total {
      return CookieSelection::CloudBetter;
    }
    return CookieSelection::LocalDefinite;
  }

  // 2. 本地"确定登录"但云端不是 → 用本地
  if local_score.definite_logged_in {
    return CookieSelection::LocalDefinite;
  }

  // 3. 本地可用但云端不可用 → 用本地
  if local_score.usable && !cloud_score.usable {
    return CookieSelection::LocalBetter;
  }

  // 4. 云端可用但本地不可用 → 用云端
  if !local_score.usable && cloud_score.usable {
    return CookieSelection::CloudBetter;
  }

  // 5. 两边都可用：云端必须高 MIN_SCORE_DIFF 分以上才覆盖，否则优先本地
  if local_score.usable && cloud_score.usable {
    if cloud_score.total > local_score.total + MIN_SCORE_DIFF {
      return CookieSelection::CloudBetter;
    }
    return CookieSelection::LocalBetter;
  }

  // 6. 本地有有效 Cookie 但云端没有 → 用本地
  if local_score.valid_count > 0 && cloud_score.valid_count == 0 {
    return CookieSelection::LocalBetter;
  }

  // 7. 云端有有效 Cookie 但本地没有 → 用云端
  if cloud_score.valid_count > 0 && local_score.valid_count == 0 {
    return CookieSelection::CloudBetter;
  }

  // 8. 都没有
  CookieSelection::None
}

/// Cookie 选择结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookieSelection {
  /// 本地确定登录，直接用本地并回传
  LocalDefinite,
  /// 本地更好，用本地
  LocalBetter,
  /// 云端更好，用云端
  CloudBetter,
  /// 都没有
  None,
}

/// 通过 CDP `Network.getAllCookies` 从运行中的浏览器导出所有 Cookie
/// 返回 JSON 字符串（与 CookieManager::export_cookies "json" 格式兼容）
pub async fn export_cookies_via_cdp(profile: &BrowserProfile) -> Result<String, CdpError> {
  let target = cdp_target::resolve(profile)
    .await
    .map_err(|e| CdpError::Unreachable(e.to_string()))?;
  let mut conn = target.connect().await?;

  let result = conn
    .call(100u64, "Network.getAllCookies", serde_json::json!({}))
    .await?;

  let cookies = result
    .get("cookies")
    .and_then(|v| v.as_array())
    .ok_or_else(|| CdpError::Protocol("getAllCookies response missing cookies array".into()))?;

  // 转换为与 SQLite 导出兼容的格式（UnifiedCookie JSON 数组）
  let unified: Vec<serde_json::Value> = cookies
    .iter()
    .map(|c| {
      let mut map = serde_json::Map::new();
      map.insert(
        "name".into(),
        c.get("name").cloned().unwrap_or(serde_json::Value::Null),
      );
      map.insert(
        "value".into(),
        c.get("value").cloned().unwrap_or(serde_json::Value::Null),
      );
      map.insert(
        "domain".into(),
        c.get("domain").cloned().unwrap_or(serde_json::Value::Null),
      );
      map.insert(
        "path".into(),
        c.get("path").cloned().unwrap_or(serde_json::Value::Null),
      );
      map.insert(
        "secure".into(),
        c.get("secure")
          .cloned()
          .unwrap_or(serde_json::Value::Bool(false)),
      );
      map.insert(
        "httpOnly".into(),
        c.get("httpOnly")
          .cloned()
          .unwrap_or(serde_json::Value::Bool(false)),
      );
      if let Some(expires) = c.get("expires") {
        map.insert("expires".into(), expires.clone());
      }
      if let Some(same_site) = c.get("sameSite") {
        map.insert("sameSite".into(), same_site.clone());
      }
      serde_json::Value::Object(map)
    })
    .collect();

  conn.close().await;

  serde_json::to_string(&unified)
    .map_err(|e| CdpError::Protocol(format!("JSON serialize failed: {}", e)))
}

/// Inject cookies into a running browser via CDP.
///
/// Uses bwbrowser's CdpTarget infrastructure. Steps:
/// 1. Enable Network domain
/// 2. Batch-set all cookies (`Network.setCookies`) — merge mode, no clear
/// 3. On batch failure, fall back to per-cookie `Network.setCookie` with
///    4-level param simplification
/// 4. Verify injection via `Network.getAllCookies`
/// 5. Navigate to the target URL so cookies take effect
///
/// Returns the number of cookies verified present after injection.
pub async fn inject_cookies_via_cdp(
  profile: &BrowserProfile,
  cdp_cookies: &[Value],
  nav_url: Option<&str>,
) -> Result<usize, CdpError> {
  if cdp_cookies.is_empty() {
    return Ok(0);
  }

  let target = cdp_target::resolve(profile)
    .await
    .map_err(|e| CdpError::Unreachable(e.to_string()))?;
  let mut conn = target.connect().await?;

  // Step 0: Enable Network domain (required by some CDP implementations)
  let _ = conn
    .call(1u64, "Network.enable", serde_json::json!({}))
    .await;

  // Step 1: Batch set all cookies（仅添加/更新同名 Cookie，保留其他 Cookie）
  let batch_result = conn
    .call(
      CMD_BATCH_SET,
      "Network.setCookies",
      serde_json::json!({ "cookies": cdp_cookies }),
    )
    .await;

  let attempted = match batch_result {
    Ok(_) => {
      log::info!(
        "cookie_sync: batch set {} cookies via CDP (merge mode, no clear)",
        cdp_cookies.len()
      );
      cdp_cookies.len()
    }
    Err(batch_err) => {
      log::warn!("cookie_sync: batch set failed ({batch_err}), falling back to per-cookie");
      per_cookie_fallback(&mut conn, cdp_cookies).await
    }
  };

  // Step 2: Verify injection — read back cookies and count how many of our
  // injected cookies are actually present in the browser.
  let verified = verify_cookies_via_cdp(&mut conn, cdp_cookies).await;
  log::info!(
    "cookie_sync: injection verification — attempted={attempted}, verified={verified} (out of {})",
    cdp_cookies.len()
  );

  // Step 3: Navigate to the platform URL so cookies take effect
  let _ = conn
    .call(CMD_RELOAD - 1, "Page.enable", serde_json::json!({}))
    .await;
  let nav_target = nav_url.unwrap_or("about:blank");
  let nav_result = conn
    .call(
      CMD_RELOAD,
      "Page.navigate",
      serde_json::json!({ "url": nav_target }),
    )
    .await;
  match &nav_result {
    Ok(_) => crate::bwbrowser_cloud::log_bwbrowser(
      "cookie_sync",
      &format!("✓ 导航到 {} 使 Cookie 生效", nav_target),
    ),
    Err(e) => {
      crate::bwbrowser_cloud::log_bwbrowser_error("cookie_sync", &format!("✗ 导航失败: {}", e))
    }
  }

  conn.close().await;
  Ok(verified)
}

/// 仅导航到指定 URL（不注入 Cookie），用于跳过注入时让页面跳转到平台页面
pub async fn navigate_to_url(profile: &BrowserProfile, url: &str) -> Result<(), CdpError> {
  let target = cdp_target::resolve(profile)
    .await
    .map_err(|e| CdpError::Unreachable(e.to_string()))?;
  let mut conn = target.connect().await?;

  let _ = conn
    .call(CMD_RELOAD - 1, "Page.enable", serde_json::json!({}))
    .await;
  let nav_result = conn
    .call(
      CMD_RELOAD,
      "Page.navigate",
      serde_json::json!({ "url": url }),
    )
    .await;
  match &nav_result {
    Ok(_) => crate::bwbrowser_cloud::log_bwbrowser(
      "cookie_sync",
      &format!("✓ 导航到 {} 使 Cookie 生效", url),
    ),
    Err(e) => {
      crate::bwbrowser_cloud::log_bwbrowser_error("cookie_sync", &format!("✗ 导航失败: {}", e))
    }
  }

  conn.close().await;
  Ok(())
}

/// Verify that injected cookies are actually present in the browser.
/// Returns the count of injected cookies that were found after read-back.
async fn verify_cookies_via_cdp(conn: &mut cdp_target::CdpConnection, expected: &[Value]) -> usize {
  let result = match conn
    .call(50u64, "Network.getAllCookies", serde_json::json!({}))
    .await
  {
    Ok(r) => r,
    Err(e) => {
      log::warn!("cookie_sync: verify failed — getAllCookies error: {e}");
      return 0;
    }
  };

  let browser_cookies = match result.get("cookies").and_then(|v| v.as_array()) {
    Some(arr) => arr,
    None => {
      log::warn!("cookie_sync: verify failed — no cookies array in response");
      return 0;
    }
  };

  // Build a lookup set of (name, domain) from browser cookies
  use std::collections::HashSet;
  let mut present: HashSet<(String, String)> = HashSet::new();
  for c in browser_cookies {
    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let domain = c.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    if !name.is_empty() {
      present.insert((
        name.to_lowercase(),
        domain.trim_start_matches('.').to_lowercase(),
      ));
    }
  }

  let mut verified = 0;
  for c in expected {
    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let domain = c.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
      continue;
    }
    let key = (
      name.to_lowercase(),
      domain.trim_start_matches('.').to_lowercase(),
    );
    if present.contains(&key) {
      verified += 1;
    }
  }

  if verified < expected.len() {
    let missing = expected.len() - verified;
    log::warn!("cookie_sync: {missing} injected cookies not found in browser after setCookies");
  }

  verified
}

/// Per-cookie fallback: try each cookie with 4 levels of param simplification.
async fn per_cookie_fallback(conn: &mut cdp_target::CdpConnection, cookies: &[Value]) -> usize {
  let mut next_id = CMD_BASE_PER_COOKIE;
  let mut ok_count = 0usize;

  for cookie in cookies {
    let cookie_name = cookie.get("name").and_then(|v| v.as_str()).unwrap_or("?");
    let domain = cookie.get("domain").and_then(|v| v.as_str()).unwrap_or("");
    let clean_domain = domain.trim_start_matches('.');
    let is_secure = cookie
      .get("secure")
      .and_then(|v| v.as_bool())
      .unwrap_or(false);
    let scheme = if is_secure { "https://" } else { "http://" };
    let cookie_url = cookie
      .get("url")
      .and_then(|v| v.as_str())
      .map(String::from)
      .unwrap_or_else(|| format!("{}{}", scheme, clean_domain));

    let level1 = cookie.clone();
    let mut level2 = cookie.clone();
    if let Some(o) = level2.as_object_mut() {
      o.remove("httpOnly");
    }
    let mut level3 = cookie.clone();
    level3["domain"] = Value::String(clean_domain.to_string());
    level3["url"] = Value::String(cookie_url.clone());
    if let Some(o) = level3.as_object_mut() {
      o.remove("httpOnly");
    }
    let mut level4 = serde_json::json!({
      "url": cookie_url,
      "name": cookie.get("name").cloned().unwrap_or_default(),
      "value": cookie.get("value").cloned().unwrap_or_default(),
    });
    if let Some(p) = cookie.get("path") {
      level4["path"] = p.clone();
    }

    let levels = [level1, level2, level3, level4];
    let mut cookie_ok = false;

    for (i, params) in levels.iter().enumerate() {
      let cmd_id = next_id;
      next_id += 1;
      if conn
        .send_command(cmd_id, "Network.setCookie", params.clone())
        .await
        .is_err()
      {
        continue;
      }
      match conn
        .await_reply(cmd_id, std::time::Duration::from_secs(3))
        .await
      {
        Ok(_) => {
          cookie_ok = true;
          if i > 0 {
            log::info!(
              "cookie_sync: {} succeeded at fallback level {}",
              cookie_name,
              i + 1
            );
          }
          break;
        }
        Err(_) => continue,
      }
    }

    if cookie_ok {
      ok_count += 1;
    } else {
      log::warn!(
        "cookie_sync: {} failed after 4 fallback levels",
        cookie_name
      );
    }
  }

  log::info!(
    "cookie_sync: per-cookie fallback injected {}/{} cookies",
    ok_count,
    cookies.len()
  );
  ok_count
}

// ========== 时区注入（Page.addScriptToEvaluateOnNewDocument） ==========

/// 通过 CDP 注入指纹伪造（时区、语言、地理定位）
/// 使用 Chromium 原生 CDP 方法，比 JS 注入更可靠
pub async fn inject_timezone_via_cdp(
  profile: &crate::profile::BrowserProfile,
  timezone: &str,
) -> Result<(), String> {
  let mut last_error: Option<String> = None;
  for attempt in 0..3 {
    if attempt > 0 {
      tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    if let Some(pid) = profile.process_id {
      use sysinfo::{ProcessRefreshKind, RefreshKind, System};
      let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
      );
      if system.process(sysinfo::Pid::from_u32(pid)).is_none() {
        log::error!(
          "cookie_sync: browser process {} is dead, cannot inject timezone",
          pid
        );
        return Err(format!("浏览器进程 {} 已退出，无法注入时区", pid));
      }
    }

    log::info!(
      "cookie_sync: CDP timezone injection attempt {} (profile={}, tz={})",
      attempt + 1,
      profile.name,
      timezone
    );

    let target = match crate::cdp_target::resolve(profile).await {
      Ok(t) => t,
      Err(e) => {
        last_error = Some(format!("CDP 连接失败: {}", e));
        log::warn!(
          "cookie_sync: resolve failed on attempt {}: {}",
          attempt + 1,
          e
        );
        continue;
      }
    };

    let mut conn = match target.connect().await {
      Ok(c) => c,
      Err(e) => {
        last_error = Some(format!("CDP 连接失败: {}", e));
        log::warn!(
          "cookie_sync: connect failed on attempt {}: {}",
          attempt + 1,
          e
        );
        continue;
      }
    };

    // 1. 启用 Page domain
    let _ = conn
      .call(3000u64, "Page.enable", serde_json::json!({}))
      .await;

    // 2. 构建指纹伪造脚本
    let default_lang = derive_language_from_timezone(timezone);
    let script = fingerprint_spoof_script(timezone, &default_lang);

    // 3. 多层时区注入策略
    //    a) Emulation.setTimezoneOverride — V8 引擎层面，最可靠
    //    b) Page.addScriptToEvaluateOnNewDocument — 注册到新导航，JS 层面补充
    //    c) Runtime.evaluate — 对当前页面立即执行
    //    d) Page.reload — 刷新让 a+b 在页面脚本前生效

    // 3a. Emulation.setTimezoneOverride — V8 引擎级别
    //     browserscan 检测的是 IP 时区 vs 浏览器时区是否一致，
    //     不是检测注入方法。只要时区一致就不会报警。
    let tz_result = conn
      .call(
        3001u64,
        "Emulation.setTimezoneOverride",
        serde_json::json!({
          "timezoneId": timezone,
        }),
      )
      .await;

    match &tz_result {
      Ok(_) => {
        log::info!(
          "cookie_sync: Emulation.setTimezoneOverride set to {}",
          timezone
        );
      }
      Err(e) => {
        log::warn!("cookie_sync: Emulation.setTimezoneOverride failed: {}", e);
      }
    }

    // 3b. 注册 JS 脚本到新导航 — 补充 Emulation 不覆盖的 API
    //     (Date.prototype.getTimezoneOffset, Intl.DateTimeFormat 等)
    //     不传 worldName = 在主世界(main world)运行，影响页面脚本
    //     传 worldName 会创建隔离世界，覆盖不影响页面
    let script_result = conn
      .call(
        3003u64,
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({
          "source": script,
          "runImmediately": true,
        }),
      )
      .await;

    match script_result {
      Ok(resp) => {
        let identifier = resp["identifier"].as_str().unwrap_or("?");
        log::info!(
          "cookie_sync: JS script registered for new documents (identifier={})",
          identifier
        );
      }
      Err(e) => {
        log::warn!(
          "cookie_sync: Page.addScriptToEvaluateOnNewDocument failed: {}",
          e
        );
      }
    }

    // 3c. 对当前页面立即执行 JS 覆盖
    let eval_result = conn
      .call(
        3004u64,
        "Runtime.evaluate",
        serde_json::json!({
          "expression": script,
          "returnByValue": false,
          "userGesture": true,
        }),
      )
      .await;

    match eval_result {
      Ok(_) => {
        log::info!(
          "cookie_sync: JS timezone overrides evaluated on page (timezone={})",
          timezone
        );
      }
      Err(e) => {
        log::warn!("cookie_sync: Runtime.evaluate failed: {}", e);
      }
    }

    // 3d. 不刷新页面！
    //     launch_wayfern 里已经用 Wayfern.setFingerprint 设好了时区（内核级），
    //     Page.reload 会导致 Wayfern 重新生成随机指纹，覆盖掉时区设置。
    //     Emulation.setTimezoneOverride 对当前 tab 已经生效，
    //     addScriptToEvaluateOnNewDocument 对新导航生效，
    //     不需要 reload。

    let _ = conn.close().await;
    return Ok(());
  }

  Err(last_error.unwrap_or_else(|| "未知错误".to_string()))
}

/// 根据时区推导对应语言
fn derive_language_from_timezone(timezone: &str) -> String {
  if timezone.contains("America") {
    "en-US".to_string()
  } else if timezone.contains("London") || timezone.contains("Europe/London") {
    "en-GB".to_string()
  } else if timezone.contains("Tokyo") || timezone.contains("Japan") {
    "ja-JP".to_string()
  } else if timezone.contains("Seoul") || timezone.contains("Korea") {
    "ko-KR".to_string()
  } else if timezone.contains("Shanghai")
    || timezone.contains("Asia/Shanghai")
    || timezone.contains("China")
  {
    "zh-CN".to_string()
  } else if timezone.contains("Europe") {
    "en-GB".to_string()
  } else {
    "en-US".to_string()
  }
}

/// 生成完整的指纹伪造 JS 脚本（作为 CDP 原生方法的兜底）
/// 包含：时区、语言、地理定位
fn fingerprint_spoof_script(timezone: &str, language: &str) -> String {
  format!(
    r#"(function() {{
      'use strict';
      var targetTimezone = "{}";
      var targetLang = "{}";
      var targetLanguages = ["{}"];

      // ========== 1. 时区伪造 ==========
      
      // 用 Intl.DateTimeFormat 计算目标时区的偏移（分钟）
      function getTzOffset(date, tz) {{
        try {{
          var dtf = new Intl.DateTimeFormat('en-US', {{
            timeZone: tz,
            hour12: false,
            year: 'numeric',
            month: '2-digit',
            day: '2-digit',
            hour: '2-digit',
            minute: '2-digit',
            second: '2-digit'
          }});
          var parts = dtf.formatToParts(date);
          var v = {{}};
          parts.forEach(function(p) {{ v[p.type] = p.value; }});
          var asUTC = Date.UTC(
            +v.year, +v.month - 1, +v.day,
            +v.hour, +v.minute, +v.second
          );
          return Math.round((date.getTime() - asUTC) / 60000);
        }} catch (e) {{
          return 0;
        }}
      }}

      // 重写 Date.prototype
      var origGetTimezoneOffset = Date.prototype.getTimezoneOffset;
      var origToString = Date.prototype.toString;
      var origToDateString = Date.prototype.toDateString;
      var origToTimeString = Date.prototype.toTimeString;
      var origToLocaleString = Date.prototype.toLocaleString;
      var origToLocaleDateString = Date.prototype.toLocaleDateString;
      var origToLocaleTimeString = Date.prototype.toLocaleTimeString;

      Date.prototype.getTimezoneOffset = function() {{
        return getTzOffset(this, targetTimezone);
      }};

      // 重写本地时间 getter
      ['FullYear', 'Month', 'Date', 'Day', 'Hours', 'Minutes', 'Seconds', 'Milliseconds'].forEach(function(part) {{
        var method = 'get' + part;
        var orig = Date.prototype[method];
        Date.prototype[method] = function() {{
          var offset = getTzOffset(this, targetTimezone);
          var shifted = new Date(this.getTime() - offset * 60000);
          if (part === 'Day') return shifted.getUTCDay();
          if (part === 'FullYear') return shifted.getUTCFullYear();
          if (part === 'Month') return shifted.getUTCMonth();
          if (part === 'Date') return shifted.getUTCDate();
          if (part === 'Hours') return shifted.getUTCHours();
          if (part === 'Minutes') return shifted.getUTCMinutes();
          if (part === 'Seconds') return shifted.getUTCSeconds();
          if (part === 'Milliseconds') return shifted.getUTCMilliseconds();
          return orig.call(this);
        }};
      }});

      // 重写本地时间 setter
      ['FullYear', 'Month', 'Date', 'Hours', 'Minutes', 'Seconds', 'Milliseconds'].forEach(function(part) {{
        var method = 'set' + part;
        var utcMethod = 'setUTC' + part;
        Date.prototype[method] = function(value) {{
          var offset = getTzOffset(this, targetTimezone);
          var shifted = new Date(this.getTime() - offset * 60000);
          shifted[utcMethod](value);
          this.setTime(shifted.getTime() + offset * 60000);
          return this.getTime();
        }};
      }});

      // 重写 toString 系列
      Date.prototype.toString = function() {{
        var offset = getTzOffset(this, targetTimezone);
        var shifted = new Date(this.getTime() - offset * 60000);
        var utcStr = shifted.toUTCString().replace(' GMT', '');
        var sign = offset <= 0 ? '+' : '-';
        var abs = Math.abs(offset);
        var hh = String(Math.floor(abs / 60)).padStart(2, '0');
        var mm = String(abs % 60).padStart(2, '0');
        return utcStr + ' GMT' + sign + hh + mm + ' (' + targetTimezone + ')';
      }};

      Date.prototype.toDateString = function() {{
        return this.toString().split(' ').slice(0, 4).join(' ');
      }};

      Date.prototype.toTimeString = function() {{
        return this.toString().split(' ').slice(4).join(' ');
      }};

      // 重写 toLocaleString 系列，强制用目标时区
      Date.prototype.toLocaleString = function(locales, options) {{
        var opts = options || {{}};
        opts.timeZone = targetTimezone;
        return origToLocaleString.call(this, locales, opts);
      }};

      Date.prototype.toLocaleDateString = function(locales, options) {{
        var opts = options || {{}};
        opts.timeZone = targetTimezone;
        return origToLocaleDateString.call(this, locales, opts);
      }};

      Date.prototype.toLocaleTimeString = function(locales, options) {{
        var opts = options || {{}};
        opts.timeZone = targetTimezone;
        return origToLocaleTimeString.call(this, locales, opts);
      }};

      // 重写 Intl.DateTimeFormat 的 resolvedOptions
      var origDTF = Intl.DateTimeFormat;
      var origResolved = origDTF.prototype.resolvedOptions;
      origDTF.prototype.resolvedOptions = function() {{
        var result = origResolved.call(this);
        try {{
          Object.defineProperty(result, 'timeZone', {{
            value: targetTimezone,
            writable: true,
            enumerable: true,
            configurable: true
          }});
        }} catch(e) {{}}
        return result;
      }};

      // ========== 2. 语言伪造 ==========

      // 重写 navigator.language / navigator.languages
      try {{
        Object.defineProperty(navigator, 'language', {{
          get: function() {{ return targetLang; }},
          configurable: true
        }});
        Object.defineProperty(navigator, 'languages', {{
          get: function() {{ return targetLanguages; }},
          configurable: true
        }});
      }} catch(e) {{}}

      // 重写 Accept-Language 请求头（通过 XMLHttpRequest 和 fetch 拦截）
      var origOpen = XMLHttpRequest.prototype.open;
      XMLHttpRequest.prototype.open = function(method, url) {{
        var result = origOpen.apply(this, arguments);
        try {{
          this.setRequestHeader('Accept-Language', targetLanguages.join(','));
        }} catch(e) {{}}
        return result;
      }};

      var origFetch = window.fetch;
      window.fetch = function(input, init) {{
        init = init || {{}};
        if (!init.headers) {{
          init.headers = {{}};
        }}
        if (init.headers instanceof Headers) {{
          if (!init.headers.has('Accept-Language')) {{
            init.headers.set('Accept-Language', targetLanguages.join(','));
          }}
        }} else if (typeof init.headers === 'object') {{
          if (!init.headers['Accept-Language']) {{
            init.headers['Accept-Language'] = targetLanguages.join(',');
          }}
        }}
        return origFetch.call(this, input, init);
      }};

      // ========== 3. 地理定位伪造（按需）==========
      // 注意：地理定位需要用户授权，这里只在已有授权时返回伪造坐标
      // 更完善的方案需要通过 Emulation.setGeolocationOverride CDP 方法


    }})();"#,
    timezone, language, language
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn platform_domains_known() {
    assert!(!platform_domains("tiktok").is_empty());
    assert!(!platform_domains("youtube").is_empty());
    assert!(!platform_domains("douyin").is_empty());
    assert!(platform_domains("unknown_platform").is_empty());
  }

  #[test]
  fn domain_match_strips_leading_dot() {
    assert!(cookie_domain_matches(".tiktok.com", &["tiktok.com"]));
    assert!(cookie_domain_matches("sub.tiktok.com", &["tiktok.com"]));
    assert!(!cookie_domain_matches("example.com", &["tiktok.com"]));
  }

  #[test]
  fn sanitize_adds_url_for_domain_cookie() {
    let raw = serde_json::json!({
      "name": "test",
      "value": "val",
      "domain": ".example.com",
      "path": "/",
      "secure": true,
    });
    let sanitized = sanitize_cdp_cookie(raw).unwrap();
    assert_eq!(sanitized["url"], "https://example.com");
    assert_eq!(sanitized["secure"], true);
  }

  #[test]
  fn sanitize_host_prefix_removes_domain() {
    let raw = serde_json::json!({
      "name": "__Host-csrf",
      "value": "val",
      "domain": ".example.com",
      "path": "/",
      "secure": false,
    });
    let sanitized = sanitize_cdp_cookie(raw).unwrap();
    assert!(sanitized.get("domain").is_none());
    assert_eq!(sanitized["secure"], true);
    assert_eq!(sanitized["path"], "/");
  }

  #[test]
  fn sanitize_samesite_none_forces_secure() {
    let raw = serde_json::json!({
      "name": "test",
      "value": "val",
      "domain": ".example.com",
      "sameSite": "none",
      "secure": false,
    });
    let sanitized = sanitize_cdp_cookie(raw).unwrap();
    assert_eq!(sanitized["secure"], true);
    assert_eq!(sanitized["sameSite"], "None");
  }

  #[test]
  fn sanitize_rejects_empty_name() {
    let raw = serde_json::json!({"name": "", "value": "val", "domain": ".example.com"});
    assert!(sanitize_cdp_cookie(raw).is_none());
  }

  #[test]
  fn prepare_cookies_filters_by_platform() {
    let raw = serde_json::json!([
      {"name": "sid", "value": "1", "domain": ".tiktok.com"},
      {"name": "other", "value": "2", "domain": ".example.com"},
    ]);
    let prepared = prepare_cookies_for_injection(&raw, "tiktok");
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0]["name"], "sid");
  }

  #[test]
  fn prepare_cookies_passes_all_when_unknown_platform() {
    let raw = serde_json::json!([
      {"name": "sid", "value": "1", "domain": ".tiktok.com"},
      {"name": "other", "value": "2", "domain": ".example.com"},
    ]);
    let prepared = prepare_cookies_for_injection(&raw, "unknown");
    assert_eq!(prepared.len(), 2);
  }

  #[test]
  fn google_auth_cookies_are_dropped() {
    let raw = serde_json::json!([
      {"name": "SAPISID", "value": "x", "domain": ".google.com"},
      {"name": "SID", "value": "y", "domain": ".google.com"},
    ]);
    let prepared = prepare_cookies_for_injection(&raw, "youtube");
    assert_eq!(prepared.len(), 0);
  }

  #[test]
  fn merge_cookies_dedupes_by_name_domain_path() {
    let primary =
      vec![serde_json::json!({"name": "a", "value": "new", "domain": ".x.com", "path": "/"})];
    let secondary =
      vec![serde_json::json!({"name": "a", "value": "old", "domain": ".x.com", "path": "/"})];
    let merged = merge_cookies(&primary, &secondary);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["value"], "new");
  }

  #[test]
  fn has_login_cookies_detects_tiktok() {
    let cookies = vec![
      serde_json::json!({"name": "sessionid", "value": "x", "domain": ".tiktok.com", "expires": 9999999999.0}),
      serde_json::json!({"name": "ttwid", "value": "y", "domain": ".tiktok.com", "expires": 9999999999.0}),
    ];
    assert!(has_login_cookies(&cookies, "tiktok"));
    assert!(!has_login_cookies(&cookies, "youtube"));
  }

  #[test]
  fn parse_cookie_payload_handles_json_string() {
    let raw = serde_json::json!("[{\"name\":\"a\",\"value\":\"1\"}]");
    let parsed = parse_cookie_payload(&raw);
    assert_eq!(parsed.len(), 1);
  }

  #[test]
  fn parse_cookie_payload_handles_single_object() {
    let raw = serde_json::json!({"name":"a","value":"1"});
    let parsed = parse_cookie_payload(&raw);
    assert_eq!(parsed.len(), 1);
  }

  #[test]
  fn normalize_accepts_alternate_field_names() {
    let raw = serde_json::json!({
      "key": "test",
      "cookieValue": "val",
      "host": ".example.com",
      "is_secure": "true",
      "http_only": 1,
    });
    let normalized = normalize_cookie_item(&raw);
    assert_eq!(normalized["name"], "test");
    assert_eq!(normalized["value"], "val");
    assert_eq!(normalized["domain"], ".example.com");
    assert_eq!(normalized["secure"], true);
    assert_eq!(normalized["httpOnly"], true);
  }

  #[test]
  fn parse_netscape_format() {
    let raw = ".tiktok.com\tTRUE\t/\tTRUE\t9999999999\tsessionid\tabc123\n.tiktok.com\tTRUE\t/\tFALSE\t0\tttwid\tdef456";
    let parsed = parse_cookie_payload(&Value::String(raw.to_string()));
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0]["name"], "sessionid");
    assert_eq!(parsed[0]["domain"], ".tiktok.com");
    assert_eq!(parsed[0]["secure"], true);
    assert_eq!(parsed[1]["name"], "ttwid");
    assert_eq!(parsed[1]["secure"], false);
  }

  #[test]
  fn parse_netscape_with_httponly_prefix() {
    let raw = "#HttpOnly_.google.com\tTRUE\t/\tTRUE\t9999999999\tSID\tvalue123";
    let parsed = parse_cookie_payload(&Value::String(raw.to_string()));
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["name"], "SID");
    assert_eq!(parsed[0]["httpOnly"], true);
    assert_eq!(parsed[0]["domain"], ".google.com");
  }

  #[test]
  fn parse_netscape_skips_comments_and_empty() {
    let raw = "# Netscape HTTP Cookie File\n\n.tiktok.com\tTRUE\t/\tTRUE\t9999999999\tsid\tval";
    let parsed = parse_cookie_payload(&Value::String(raw.to_string()));
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["name"], "sid");
  }

  #[test]
  fn prepare_cookies_from_netscape_format() {
    let raw = ".tiktok.com\tTRUE\t/\tTRUE\t9999999999\tsessionid\tabc123\n.example.com\tTRUE\t/\tFALSE\t0\tother\tval";
    let prepared = prepare_cookies_for_injection(&Value::String(raw.to_string()), "tiktok");
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0]["name"], "sessionid");
  }

  #[test]
  fn score_cookies_empty_returns_zero() {
    let cookies = vec![];
    let score = score_cookies(&cookies, "tiktok");
    assert_eq!(score.total, 0);
    assert_eq!(score.valid_count, 0);
    assert!(!score.usable);
    assert!(!score.definite_logged_in);
  }

  #[test]
  fn score_cookies_session_cookie_detects_login() {
    let cookies = vec![
      serde_json::json!({"name": "sessionid", "value": "x", "domain": ".tiktok.com", "path": "/", "secure": true, "httpOnly": true, "expires": 9999999999.0}),
      serde_json::json!({"name": "ttwid", "value": "y", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let score = score_cookies(&cookies, "tiktok");
    assert!(score.key_cookies >= 1);
    assert!(score.total > 0);
  }

  #[test]
  fn score_cookies_filters_by_platform() {
    let cookies = vec![
      serde_json::json!({"name": "sessionid", "value": "x", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "sid", "value": "y", "domain": ".youtube.com", "path": "/", "expires": 9999999999.0}),
    ];
    let tiktok_score = score_cookies(&cookies, "tiktok");
    let youtube_score = score_cookies(&cookies, "youtube");
    assert_eq!(tiktok_score.valid_count, 1);
    assert_eq!(youtube_score.valid_count, 1);
    assert!(tiktok_score.key_cookies >= 1);
    assert!(youtube_score.key_cookies >= 1);
  }

  #[test]
  fn select_best_picks_local_definite() {
    let local = vec![
      serde_json::json!({"name": "sessionid", "value": "a", "domain": ".tiktok.com", "path": "/", "secure": true, "httpOnly": true, "expires": 9999999999.0}),
      serde_json::json!({"name": "uid_tt", "value": "b", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "ttwid", "value": "c", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "s_v_web_id", "value": "d", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "n_mh", "value": "e", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "sid_guard", "value": "f", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let server = vec![
      serde_json::json!({"name": "ttwid", "value": "x", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let local_score = score_cookies(&local, "tiktok");
    let server_score = score_cookies(&server, "tiktok");
    let result = select_best_cookie_set(&local_score, &server_score);
    // 本地有多个关键 Cookie 且无过期，应该是 definite_logged_in
    assert!(local_score.definite_logged_in);
    assert_eq!(result, CookieSelection::LocalDefinite);
  }

  #[test]
  fn select_best_prefers_local_when_close() {
    // 两边都可用，但分差小于 15 → 优先本地
    let local = vec![
      serde_json::json!({"name": "sessionid", "value": "a", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "ttwid", "value": "b", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "uid_tt", "value": "c", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let server = vec![
      serde_json::json!({"name": "sessionid", "value": "x", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "ttwid", "value": "y", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let local_score = score_cookies(&local, "tiktok");
    let server_score = score_cookies(&server, "tiktok");
    let result = select_best_cookie_set(&local_score, &server_score);
    // 两边都可用，分差不大 → LocalBetter
    assert_eq!(result, CookieSelection::LocalBetter);
  }

  #[test]
  fn select_best_server_much_better() {
    // 服务器明显更好（高 15 分以上）→ 选服务器
    let local = vec![
      serde_json::json!({"name": "ttwid", "value": "a", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let server = vec![
      serde_json::json!({"name": "sessionid", "value": "x", "domain": ".tiktok.com", "path": "/", "secure": true, "httpOnly": true, "expires": 9999999999.0}),
      serde_json::json!({"name": "uid_tt", "value": "y", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "ttwid", "value": "z", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "s_v_web_id", "value": "w", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "sid_tt", "value": "v", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
      serde_json::json!({"name": "n_mh", "value": "u", "domain": ".tiktok.com", "path": "/", "expires": 9999999999.0}),
    ];
    let local_score = score_cookies(&local, "tiktok");
    let server_score = score_cookies(&server, "tiktok");
    let result = select_best_cookie_set(&local_score, &server_score);
    assert_eq!(result, CookieSelection::CloudBetter);
  }
}
