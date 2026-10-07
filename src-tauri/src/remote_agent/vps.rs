//! VPS 注册/心跳（remote_api.php，与 server.py VPSClient 兼容）

use serde_json::{json, Value};

use crate::bwbrowser_cloud::BWBROWSER_AUTH;
use crate::cloud_domain::REMOTE_API_URL;

/// client_login 成功后的身份信息
#[derive(Debug, Clone, Default)]
pub struct VpsIdentity {
  pub user_id: i64,
  pub company_id: i64,
}

pub(crate) fn http_client() -> Result<reqwest::Client, String> {
  reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(10))
    .build()
    .map_err(|e| e.to_string())
}

/// reqwest 0.13 没有 .form()，手写 x-www-form-urlencoded（与 bwbrowser_cloud.rs 同风格）
pub(crate) fn form_body(pairs: &[(&str, &str)]) -> String {
  pairs
    .iter()
    .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
    .collect::<Vec<_>>()
    .join("&")
}

fn urlencode(s: &str) -> String {
  s.bytes()
    .map(|b| match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
        (b as char).to_string()
      }
      b' ' => "+".to_string(),
      _ => format!("%{:02X}", b),
    })
    .collect()
}

fn form_post(client: &reqwest::Client, pairs: &[(&str, &str)]) -> reqwest::RequestBuilder {
  client
    .post(REMOTE_API_URL)
    .header("content-type", "application/x-www-form-urlencoded")
    .body(form_body(pairs))
}

/// 用爆文库登录态调 client_login，云端校验 allow_remote_desktop 权限
pub async fn client_login() -> Result<VpsIdentity, String> {
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| json!({"code": "CLOUD_NOT_SIGNED_IN"}).to_string())?;
  let client = http_client()?;
  let resp = form_post(
    &client,
    &[
      ("action", "client_login"),
      ("username", &username),
      ("password", &password),
    ],
  )
  .send()
  .await
  .map_err(|e| {
    json!({"code": "REMOTE_DESKTOP_UNREACHABLE", "params": {"detail": format!("VPS 登录网络错误: {e}")}})
      .to_string()
  })?;
  let body: Value = resp.json().await.map_err(|e| {
    json!({"code": "REMOTE_DESKTOP_UNREACHABLE", "params": {"detail": format!("VPS 响应解析失败: {e}")}})
      .to_string()
  })?;
  if body
    .get("success")
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
  {
    Ok(VpsIdentity {
      user_id: body.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0),
      company_id: body.get("company_id").and_then(|v| v.as_i64()).unwrap_or(0),
    })
  } else {
    let msg = body
      .get("message")
      .and_then(|v| v.as_str())
      .unwrap_or("未知错误")
      .to_string();
    let code = body
      .get("code")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string();
    if code == "NO_REMOTE_PERMISSION" {
      return Err(json!({"code": "NO_REMOTE_PERMISSION"}).to_string());
    }
    Err(json!({"code": "REMOTE_DESKTOP_SERVER_REJECTED", "params": {"detail": msg}}).to_string())
  }
}

#[allow(clippy::too_many_arguments)]
pub async fn register(
  uuid: &str,
  hostname: &str,
  os_info: &str,
  local_ips: &str,
  connect_host: &str,
  port: u16,
  password: &str,
  identity: &VpsIdentity,
) -> Result<i64, String> {
  let client = http_client()?;
  let resp = form_post(
    &client,
    &[
      ("action", "register"),
      ("client_uuid", uuid),
      ("hostname", hostname),
      ("os_info", os_info),
      ("local_ips", local_ips),
      ("public_ip", ""),
      ("connect_host", connect_host),
      ("port", &port.to_string()),
      ("access_password", password),
      ("user_id", &identity.user_id.to_string()),
      ("company_id", &identity.company_id.to_string()),
    ],
  )
  .send()
  .await
  .map_err(|e| {
    json!({"code": "REMOTE_DESKTOP_UNREACHABLE", "params": {"detail": format!("注册网络错误: {e}")}})
      .to_string()
  })?;
  let body: Value = resp.json().await.map_err(|e| {
    json!({"code": "REMOTE_DESKTOP_UNREACHABLE", "params": {"detail": format!("注册响应解析失败: {e}")}})
      .to_string()
  })?;
  if body
    .get("success")
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
  {
    let client_id = body.get("client_id").and_then(|v| v.as_i64()).unwrap_or(0);
    crate::remote_agent::log_remote(&format!(
      "用户 {} VPS 注册成功 uuid={} hostname={} client_id={}",
      crate::remote_agent::current_user(),
      uuid,
      hostname,
      client_id
    ));
    Ok(client_id)
  } else {
    let detail = body
      .get("message")
      .and_then(|v| v.as_str())
      .unwrap_or("注册失败")
      .to_string();
    crate::remote_agent::log_remote(&format!(
      "用户 {} VPS 注册失败 uuid={} hostname={}: {}",
      crate::remote_agent::current_user(),
      uuid,
      hostname,
      detail
    ));
    let code = body
      .get("code")
      .and_then(|v| v.as_str())
      .unwrap_or("")
      .to_string();
    if code == "NO_REMOTE_PERMISSION" {
      return Err(json!({"code": "NO_REMOTE_PERMISSION"}).to_string());
    }
    Err(
      json!({"code": "REMOTE_DESKTOP_SERVER_REJECTED", "params": {
        "detail": detail
      }})
      .to_string(),
    )
  }
}

pub async fn heartbeat(uuid: &str) -> Result<(), String> {
  let client = http_client()?;
  let resp = form_post(&client, &[("action", "heartbeat"), ("client_uuid", uuid)])
    .send()
    .await
    .map_err(|e| {
      json!({"code": "REMOTE_DESKTOP_UNREACHABLE", "params": {"detail": format!("心跳网络错误: {e}")}})
        .to_string()
    })?;
  let body: Value = resp.json().await.map_err(|e| {
    json!({"code": "REMOTE_DESKTOP_UNREACHABLE", "params": {"detail": format!("心跳响应解析失败: {e}")}})
      .to_string()
  })?;
  if body
    .get("success")
    .and_then(|v| v.as_bool())
    .unwrap_or(false)
  {
    Ok(())
  } else {
    Err(
      json!({"code": "REMOTE_DESKTOP_SERVER_REJECTED", "params": {"detail": "心跳失败"}})
        .to_string(),
    )
  }
}

pub async fn unregister(uuid: &str) -> Result<(), String> {
  crate::remote_agent::log_remote(&format!(
    "用户 {} VPS 注销 uuid={}",
    crate::remote_agent::current_user(),
    uuid
  ));
  let client = http_client()?;
  let _ = form_post(&client, &[("action", "unregister"), ("client_uuid", uuid)])
    .send()
    .await;
  Ok(())
}

/// 本地局域网直连地址列表（供 connect_info 返回）
pub fn local_ips_json() -> String {
  let mut ips: Vec<String> = Vec::new();
  if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
    if sock.connect("8.8.8.8:80").is_ok() {
      if let Ok(local) = sock.local_addr() {
        ips.push(local.ip().to_string());
      }
    }
  }
  serde_json::to_string(&json!(ips)).unwrap_or_else(|_| "[]".to_string())
}
