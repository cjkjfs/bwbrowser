//! 远程管理控制端：列出公司内电脑并做「局域网优先」连接。
//!
//! 思路与 `remote_duli/controller.py` 一致：控制端先探测被控端的 `/discover`
//! （免密，只回主机名/系统/分辨率/端口），局域网可达就直接用被控端内置 viewer
//! `http://host:port/`；不可达才回退中继控制页。
//!
//! 机器列表来自 `remote_api.php` 的 `clients`（与 `remote_app.php` 同源），
//! 超管可传 company_id 切换公司；非超管只能看到本公司。

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::file_transfer::FileTarget;
use super::vps::{form_body, http_client};
use crate::bwbrowser_cloud::BWBROWSER_AUTH;
use crate::cloud_domain::{RELAY_CONTROL_URL, REMOTE_API_URL};

/// remote_api token 进程内缓存（登录后 30 天有效，失效时按提示清缓存重登）
static TOKEN_CACHE: Mutex<Option<String>> = Mutex::new(None);

fn err(code: &str, detail: impl std::fmt::Display) -> String {
  json!({"code": code, "params": {"detail": detail.to_string()}}).to_string()
}

/// PHP 端 PDO 返回的数字可能是 int 也可能是 string，统一取 i64
fn as_i64(v: Option<&Value>) -> i64 {
  match v {
    Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
    Some(Value::String(s)) => s.parse().unwrap_or(0),
    _ => 0,
  }
}

fn as_string(v: Option<&Value>) -> String {
  match v {
    Some(Value::String(s)) => s.clone(),
    Some(Value::Null) | None => String::new(),
    Some(other) => other.to_string(),
  }
}

/// remote_api.php 请求方式。`login`/`clients` 走 POST；`connect_info` 在服务端只从
/// query string 读 `client_id`（remote_app.php 也是用 GET 调的），必须走 GET，否则
/// 服务端拿到 `client_id=0` 直接返回“参数错误”。
#[derive(Clone, Copy)]
enum ApiMethod {
  Post,
  Get,
}

/// remote_api.php 调用；token 为空表示登录接口（login 本身不需要 token）
async fn api_call(
  client: &reqwest::Client,
  method: ApiMethod,
  action: &str,
  params: &[(&str, &str)],
  token: Option<&str>,
) -> Result<Value, String> {
  let mut pairs: Vec<(&str, &str)> = vec![("action", action)];
  pairs.extend_from_slice(params);
  let mut req = match method {
    ApiMethod::Post => client
      .post(REMOTE_API_URL)
      .header("content-type", "application/x-www-form-urlencoded")
      .body(form_body(&pairs)),
    // reqwest 未启用 query() 特性，直接复用已有的 form_body 拼 query string
    ApiMethod::Get => client.get(format!("{REMOTE_API_URL}?{}", form_body(&pairs))),
  };
  if let Some(t) = token {
    req = req.header("X-Auth-Token", t);
  }
  let resp = req
    .send()
    .await
    .map_err(|e| err("REMOTE_MANAGEMENT_UNREACHABLE", format!("网络错误: {e}")))?;
  resp.json::<Value>().await.map_err(|e| {
    err(
      "REMOTE_MANAGEMENT_UNREACHABLE",
      format!("响应解析失败: {e}"),
    )
  })
}

/// 用爆文库登录态换 remote_api token（进程内缓存）
async fn token(client: &reqwest::Client) -> Result<String, String> {
  if let Some(t) = TOKEN_CACHE.lock().unwrap().clone() {
    return Ok(t);
  }
  let (username, password) = BWBROWSER_AUTH
    .get_credentials()
    .ok_or_else(|| json!({"code": "CLOUD_NOT_SIGNED_IN"}).to_string())?;
  let body = api_call(
    client,
    ApiMethod::Post,
    "login",
    &[("username", &username), ("password", &password)],
    None,
  )
  .await?;
  if body
    .get("multiple")
    .and_then(Value::as_bool)
    .unwrap_or(false)
  {
    return Err(json!({"code": "REMOTE_MANAGEMENT_MULTIPLE_ACCOUNTS"}).to_string());
  }
  if !body
    .get("success")
    .and_then(Value::as_bool)
    .unwrap_or(false)
  {
    return Err(err(
      "REMOTE_MANAGEMENT_LOGIN_FAILED",
      as_string(body.get("message")),
    ));
  }
  let t = as_string(body.get("token"));
  if t.is_empty() {
    return Err(err("REMOTE_MANAGEMENT_LOGIN_FAILED", "服务端未返回 token"));
  }
  *TOKEN_CACHE.lock().unwrap() = Some(t.clone());
  Ok(t)
}

/// 带鉴权的调用；token 失效（服务端提示 Token 无效）时清缓存重登一次
async fn authed_call(
  client: &reqwest::Client,
  method: ApiMethod,
  action: &str,
  params: &[(&str, &str)],
) -> Result<Value, String> {
  let t = token(client).await?;
  let body = api_call(client, method, action, params, Some(&t)).await?;
  if body
    .get("success")
    .and_then(Value::as_bool)
    .unwrap_or(false)
  {
    return Ok(body);
  }
  let msg = as_string(body.get("message"));
  if msg.contains("Token") {
    *TOKEN_CACHE.lock().unwrap() = None;
    let t2 = token(client).await?;
    let retry = api_call(client, method, action, params, Some(&t2)).await?;
    if retry
      .get("success")
      .and_then(Value::as_bool)
      .unwrap_or(false)
    {
      return Ok(retry);
    }
    return Err(err(
      "REMOTE_MANAGEMENT_SERVER_REJECTED",
      as_string(retry.get("message")),
    ));
  }
  Err(err("REMOTE_MANAGEMENT_SERVER_REJECTED", msg))
}

/// 公司内的一台被控端
#[derive(Debug, Clone, Serialize)]
pub struct RemoteMachine {
  pub id: i64,
  pub client_uuid: String,
  pub hostname: String,
  pub os_info: String,
  pub note: String,
  pub local_ips: Vec<String>,
  pub public_ip: String,
  pub connect_host: String,
  pub port: u16,
  pub status: String,
  /// 该被控端登录的爆文库账号与姓名（`clients` 里 JOIN users 得来，未登录时为空）
  pub user_username: String,
  pub user_real_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteMachinesResult {
  pub machines: Vec<RemoteMachine>,
  pub relay_url: String,
}

fn parse_machine(v: &Value) -> RemoteMachine {
  RemoteMachine {
    id: as_i64(v.get("id")),
    client_uuid: as_string(v.get("client_uuid")),
    hostname: as_string(v.get("hostname")),
    os_info: as_string(v.get("os_info")),
    note: as_string(v.get("note")),
    local_ips: v
      .get("local_ips")
      .and_then(Value::as_array)
      .map(|a| a.iter().map(|x| as_string(Some(x))).collect())
      .unwrap_or_default(),
    public_ip: as_string(v.get("public_ip")),
    connect_host: as_string(v.get("connect_host")),
    port: as_i64(v.get("port")).clamp(0, 65535) as u16,
    status: as_string(v.get("status")),
    user_username: as_string(v.get("user_username")),
    user_real_name: as_string(v.get("user_real_name")),
  }
}

/// 公司内电脑列表（超管传 company_id 切换公司）
pub async fn list_machines(company_id: Option<i64>) -> Result<RemoteMachinesResult, String> {
  let client = http_client()?;
  let cid = company_id.map(|c| c.to_string());
  let params: Vec<(&str, &str)> = match cid.as_deref() {
    Some(c) => vec![("company_id", c)],
    None => vec![],
  };
  let body = authed_call(&client, ApiMethod::Post, "clients", &params).await?;
  let machines = body
    .get("clients")
    .and_then(Value::as_array)
    .map(|a| a.iter().map(parse_machine).collect())
    .unwrap_or_default();
  Ok(RemoteMachinesResult {
    machines,
    relay_url: as_string(body.get("relay_url")),
  })
}

/// 删除公司内的被控端机器（无用/长期不在线的清理）。服务端按
/// 远程管理权限校验：超管删任意，普通远程管理员限本公司。
pub async fn delete_machine(client_id: i64) -> Result<u64, String> {
  let client = http_client()?;
  let cid = client_id.to_string();
  let body = authed_call(
    &client,
    ApiMethod::Post,
    "delete_client",
    &[("client_id", cid.as_str())],
  )
  .await?;
  Ok(body.get("deleted").and_then(Value::as_u64).unwrap_or(0))
}

/// 连接目标：局域网优先，失败回退中继
#[derive(Debug, Clone, Serialize)]
pub struct RemoteTarget {
  /// "lan" | "relay"
  pub method: String,
  /// 控制端直接打开的地址（局域网=被控端内置 viewer，中继=控制页）
  pub url: String,
  pub host: String,
  pub port: u16,
  pub hostname: String,
}

/// 172.16-31.x.x 是 VPS 隧道地址，不是局域网；其余私网段都算局域网候选
fn is_lan_candidate(ip: &str) -> bool {
  if ip.is_empty() || ip.starts_with("172.") {
    return false;
  }
  ip == "127.0.0.1"
    || ip == "localhost"
    || ip.starts_with("192.168.")
    || ip.starts_with("10.")
    || ip.starts_with("169.254.")
}

/// 并发探测各候选地址的 /discover，返回第一个可达的 host
async fn probe_lan(hosts: &[String], port: u16) -> Option<String> {
  if hosts.is_empty() {
    return None;
  }
  let probe = reqwest::Client::builder()
    .timeout(Duration::from_millis(1200))
    .connect_timeout(Duration::from_millis(800))
    .build()
    .ok()?;

  let mut tasks = Vec::new();
  for host in hosts {
    let probe = probe.clone();
    let url = format!("http://{host}:{port}/discover");
    let host = host.clone();
    tasks.push(tokio::spawn(async move {
      let ok = match probe.get(&url).send().await {
        Ok(resp) => {
          resp.status().is_success()
            && resp
              .json::<Value>()
              .await
              .map(|v| v.get("online").and_then(Value::as_bool).unwrap_or(false))
              .unwrap_or(false)
        }
        Err(_) => false,
      };
      (host, ok)
    }));
  }

  let mut reachable = None;
  for task in tasks {
    if let Ok((host, true)) = task.await {
      reachable = Some(host);
      break;
    }
  }
  reachable
}

/// 被控端 viewer 的控制模式（控制端操作列三个按钮）：
/// `full` 完全控制、`view` 仅查看、`files` 仅文件传输。未知值一律回落到 `full`。
pub const MODE_FULL: &str = "full";
pub const MODE_VIEW: &str = "view";
pub const MODE_FILES: &str = "files";

fn normalize_mode(mode: Option<String>) -> &'static str {
  match mode.as_deref() {
    Some(MODE_VIEW) => MODE_VIEW,
    Some(MODE_FILES) => MODE_FILES,
    _ => MODE_FULL,
  }
}

/// 局域网 viewer 地址：`mode` 决定控制方式，`pwd`（被控端连接密码）让 viewer 自动
/// 鉴权、免手输；密码为空时不带该参数，viewer 会正常弹登录框。
fn lan_viewer_url(host: &str, port: u16, mode: &str, password: &str) -> String {
  let mut query: Vec<(&str, &str)> = vec![("mode", mode)];
  if !password.is_empty() {
    query.push(("pwd", password));
  }
  format!("http://{host}:{port}/?{}", form_body(&query))
}

/// 中继控制页（remote_app.php）走主系统会话鉴权，App 打开的裸地址没有会话会停在
/// login.php。URL 里带上两样东西：
/// - `remote_token`：App 已持有的 remote_api token，页面用它免登入建会话
///   （服务端 `$_GET['remote_token']`）；
/// - `target_uuid` + `mode`：目标设备与模式，页面加载完直接进入该设备的会话，
///   而不是停在设备列表让用户再点一次。
///
/// 拿不到 token（未登录）时仍带上目标设备，页面登录后也能直达。
fn relay_url_with_token(token: Option<&str>, uuid: &str, mode: &str) -> String {
  let mut query: Vec<(&str, &str)> = Vec::new();
  if let Some(t) = token.filter(|t| !t.is_empty()) {
    query.push(("remote_token", t));
  }
  if !uuid.is_empty() {
    query.push(("target_uuid", uuid));
    query.push(("mode", relay_mode(mode)));
  }
  if query.is_empty() {
    return RELAY_CONTROL_URL.to_string();
  }
  format!("{RELAY_CONTROL_URL}?{}", form_body(&query))
}

/// remote_app.php 的模式取值是单数 `file`，与客户端操作列的 `files` 不同
fn relay_mode(mode: &str) -> &'static str {
  match mode {
    MODE_VIEW => MODE_VIEW,
    MODE_FILES => "file",
    _ => MODE_FULL,
  }
}

async fn relay_control_url(uuid: &str, mode: &str) -> String {
  let Ok(client) = http_client() else {
    return relay_url_with_token(None, uuid, mode);
  };
  let t = token(&client).await.ok();
  relay_url_with_token(t.as_deref(), uuid, mode)
}

/// 从云端解析出的被控端信息（连接目标与文件传输共用）
struct ResolvedClient {
  hostname: String,
  port: u16,
  /// 局域网探测命中的地址；不可达时为 None
  lan_host: Option<String>,
  access_password: String,
  uuid: String,
  relay_url: String,
}

/// 拉取被控端连接信息并做一次局域网探测（remote_api.php 的 connect_info）。
/// `connect_info` 只读 query string 的 client_id（remote_app.php 亦用 GET），
/// 用 POST 会被判成参数错误。
async fn resolve_client(client_id: i64) -> Result<ResolvedClient, String> {
  let client = http_client()?;
  let id = client_id.to_string();
  let body = authed_call(
    &client,
    ApiMethod::Get,
    "connect_info",
    &[("client_id", &id)],
  )
  .await?;
  let c = body.get("client").cloned().unwrap_or(Value::Null);
  let port = as_i64(c.get("port")).clamp(1, 65535) as u16;

  let mut lan: Vec<String> = Vec::new();
  if let Some(arr) = c.get("local_ips").and_then(Value::as_array) {
    for ip in arr {
      let ip = as_string(Some(ip));
      if is_lan_candidate(&ip) && !lan.contains(&ip) {
        lan.push(ip);
      }
    }
  }
  let connect_host = as_string(c.get("connect_host"));
  if is_lan_candidate(&connect_host) && !lan.contains(&connect_host) {
    lan.push(connect_host);
  }

  Ok(ResolvedClient {
    hostname: as_string(c.get("hostname")),
    port,
    lan_host: probe_lan(&lan, port).await,
    // 被控端的连接密码；局域网直连时随 URL 带给 viewer 自动鉴权，省去手动输入
    access_password: as_string(c.get("access_password")),
    uuid: as_string(c.get("client_uuid")),
    relay_url: as_string(body.get("relay_url")),
  })
}

/// 解析连接目标：先局域网探测，失败回退中继控制页
pub async fn connect_target(client_id: i64, mode: Option<String>) -> Result<RemoteTarget, String> {
  let mode = normalize_mode(mode);
  let r = resolve_client(client_id).await?;

  if let Some(host) = r.lan_host {
    crate::remote_agent::log_remote(&format!(
      "远程管理：{} 局域网直连 {host}:{}（模式 {mode}）",
      r.hostname, r.port
    ));
    return Ok(RemoteTarget {
      method: "lan".to_string(),
      url: lan_viewer_url(&host, r.port, mode, &r.access_password),
      host,
      port: r.port,
      hostname: r.hostname,
    });
  }

  crate::remote_agent::log_remote(&format!("远程管理：{} 局域网不可达，回退中继", r.hostname));
  Ok(RemoteTarget {
    method: "relay".to_string(),
    url: relay_control_url(&r.uuid, mode).await,
    host: String::new(),
    port: r.port,
    hostname: r.hostname,
  })
}

/// 文件传输目标：云端机器由后端解析（局域网优先/中继 + 连接密码 + uuid）。
pub async fn file_target(client_id: i64, password: Option<String>) -> Result<FileTarget, String> {
  let r = resolve_client(client_id).await?;
  let (method, host, uuid) = match r.lan_host {
    Some(h) => ("lan".to_string(), h, String::new()),
    None => ("relay".to_string(), String::new(), r.uuid),
  };
  Ok(FileTarget {
    method,
    host,
    port: r.port,
    password: password.unwrap_or(r.access_password),
    uuid,
    relay_url: r.relay_url,
    hostname: r.hostname,
  })
}

/// 登出/切换账号时清掉 token 缓存，避免用上一个账号的身份拉列表
pub fn clear_token() {
  *TOKEN_CACHE.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn lan_candidates_exclude_the_vps_tunnel_range() {
    // 被控端通过 WireGuard 隧道上报的 172.16-31.x.x 是本机对 VPS 的地址，
    // 不是局域网地址。当成候选去探测会白等一次超时，还会把中继回退拖慢。
    assert!(!is_lan_candidate("172.16.0.2"));
    assert!(!is_lan_candidate("172.31.255.254"));
    assert!(!is_lan_candidate(""));
    assert!(!is_lan_candidate("203.0.113.7"));

    assert!(is_lan_candidate("192.168.1.20"));
    assert!(is_lan_candidate("10.0.0.5"));
    assert!(is_lan_candidate("169.254.10.1"));
    assert!(is_lan_candidate("127.0.0.1"));
    assert!(is_lan_candidate("localhost"));
  }

  #[test]
  fn machines_parse_string_numbers_and_missing_fields() {
    // PHP 的 PDO 会把 id/port 当字符串返回，缺字段时也要给出可展示的默认值
    // 而不是让整个列表解析失败。
    let m = parse_machine(&json!({
      "id": "7",
      "client_uuid": "abc",
      "hostname": "pc-01",
      "local_ips": ["192.168.1.20", null, "10.0.0.5"],
      "port": "8765",
      "user_username": "zhangsan",
      "user_real_name": "张三",
    }));
    assert_eq!(m.id, 7);
    assert_eq!(m.port, 8765);
    assert_eq!(m.hostname, "pc-01");
    assert_eq!(m.local_ips, vec!["192.168.1.20", "", "10.0.0.5"]);
    assert_eq!(m.os_info, "");
    assert_eq!(m.status, "");
    assert_eq!(m.user_username, "zhangsan");
    assert_eq!(m.user_real_name, "张三");
    // 被控端未登录时 JOIN 出来是 NULL，要落成空串而不是 "null"
    let anonymous = parse_machine(&json!({ "id": 8, "user_username": null }));
    assert_eq!(anonymous.user_username, "");
    assert_eq!(anonymous.user_real_name, "");
  }

  /// 操作列的三个按钮只认 full/view/files，其余（含前端漏传）一律当完全控制，
  /// 避免 URL 里出现任意字符串被 viewer 误判。
  #[test]
  fn connect_mode_normalizes_to_known_values() {
    assert_eq!(normalize_mode(None), MODE_FULL);
    assert_eq!(normalize_mode(Some("full".into())), MODE_FULL);
    assert_eq!(normalize_mode(Some("view".into())), MODE_VIEW);
    assert_eq!(normalize_mode(Some("files".into())), MODE_FILES);
    assert_eq!(normalize_mode(Some("VIEW".into())), MODE_FULL);
    assert_eq!(normalize_mode(Some("evil".into())), MODE_FULL);
  }

  /// 局域网 viewer 地址必须带上 mode；有连接密码时带 pwd（自动鉴权），无密码时不带。
  #[test]
  fn lan_viewer_url_carries_mode_and_password() {
    assert_eq!(
      lan_viewer_url("192.168.1.9", 8765, "full", "remote123"),
      "http://192.168.1.9:8765/?mode=full&pwd=remote123"
    );
    assert_eq!(
      lan_viewer_url("10.0.0.2", 8765, "files", ""),
      "http://10.0.0.2:8765/?mode=files"
    );
    // 密码里的特殊字符要编码，不能破坏 query
    assert_eq!(
      lan_viewer_url("10.0.0.2", 8765, "view", "a b&c"),
      "http://10.0.0.2:8765/?mode=view&pwd=a+b%26c"
    );
  }

  /// 中继控制页 URL：带 token 免登入，并带目标设备（uuid + 模式）直达会话；
  /// `files` 要映射成 remote_app.php 的单数 `file`。
  #[test]
  fn relay_url_carries_token_and_target() {
    assert_eq!(
      relay_url_with_token(Some("abc123"), "uuid-1", MODE_VIEW),
      format!("{RELAY_CONTROL_URL}?remote_token=abc123&target_uuid=uuid-1&mode=view")
    );
    // 未登录仍带目标设备，登录后也能直达
    assert_eq!(
      relay_url_with_token(None, "uuid-1", MODE_FILES),
      format!("{RELAY_CONTROL_URL}?target_uuid=uuid-1&mode=file")
    );
    // 既无 token 也无 uuid 才退回裸地址
    assert_eq!(relay_url_with_token(None, "", MODE_FULL), RELAY_CONTROL_URL);
  }

  #[test]
  fn relay_mode_maps_files_to_singular_file() {
    assert_eq!(relay_mode(MODE_FULL), "full");
    assert_eq!(relay_mode(MODE_VIEW), "view");
    assert_eq!(relay_mode(MODE_FILES), "file");
  }
}
