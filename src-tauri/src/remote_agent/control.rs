//! 远程管理控制端：列出公司内电脑并做「局域网优先」连接。
//!
//! 思路与 `remote_duli/controller.py` 一致：控制端先探测被控端的 `/discover`
//! （免密，只回主机名/系统/分辨率/端口），局域网可达就直接用被控端内置 viewer
//! `http://host:port/`；不可达才回退中继控制页。
//!
//! 机器列表来自 `remote_api.php` 的 `clients`（与 `remote_app.php` 同源），
//! 超管可传 company_id 切换公司；非超管只能看到本公司。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
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
    url: RELAY_CONTROL_URL.to_string(),
    host: String::new(),
    port: r.port,
    hostname: r.hostname,
  })
}

/// 文件传输目标：云端机器由后端解析（局域网优先/中继 + 连接密码 + uuid）；
/// 纯局域网发现的机器不在云端列表里，只有地址，密码由用户在前端输入。
pub async fn file_target(
  client_id: Option<i64>,
  host: Option<String>,
  port: Option<u16>,
  password: Option<String>,
) -> Result<FileTarget, String> {
  if let Some(id) = client_id {
    let r = resolve_client(id).await?;
    let (method, host, uuid) = match r.lan_host {
      Some(h) => ("lan".to_string(), h, String::new()),
      None => ("relay".to_string(), String::new(), r.uuid),
    };
    return Ok(FileTarget {
      method,
      host,
      port: r.port,
      password: r.access_password,
      uuid,
      relay_url: r.relay_url,
      hostname: r.hostname,
    });
  }

  let host = host.unwrap_or_default();
  if host.is_empty() {
    return Err(err("REMOTE_MANAGEMENT_UNREACHABLE", "缺少目标地址"));
  }
  Ok(FileTarget {
    method: "lan".to_string(),
    host,
    port: port.unwrap_or(LAN_SCAN_DEFAULT_PORT),
    password: password.unwrap_or_default(),
    uuid: String::new(),
    relay_url: String::new(),
    hostname: String::new(),
  })
}

// ==================== 局域网网段扫描（remote_duli 控制端） ====================

/// 单次扫描的主机上限，防止 CIDR 写太小（如 /0）时打出几十万并发探测
const LAN_SCAN_MAX_HOSTS: usize = 1024;
/// 并发探测数，与 remote_duli 的 50 线程同量级
const LAN_SCAN_CONCURRENCY: usize = 64;
/// 被控端默认端口（与 `RemoteAgentConfig::default().port` 一致）
pub const LAN_SCAN_DEFAULT_PORT: u16 = 8765;

/// 局域网里发现的一台被控端（`/discover` 返回）
#[derive(Debug, Clone, Serialize)]
pub struct LanMachine {
  pub host: String,
  pub name: String,
  pub os: String,
  pub width: u32,
  pub height: u32,
  pub port: u16,
}

/// 取一个私网 IPv4 的 /24 前缀（`192.168.1`）；非私网、回环、链路本地一律返回 None。
/// 172.16-31 是本项目对 VPS 的 WireGuard 隧道段，扫它只会白等一轮超时。
fn lan_prefix_of(v4: Ipv4Addr) -> Option<String> {
  if v4.is_loopback() || v4.is_link_local() || !v4.is_private() {
    return None;
  }
  let o = v4.octets();
  if o[0] == 172 && (16..=31).contains(&o[1]) {
    return None;
  }
  Some(format!("{}.{}.{}", o[0], o[1], o[2]))
}

/// 本机各网卡的局域网 /24 前缀，供控制端主动扫描。跳过隧道/虚拟网卡。
fn local_lan_prefixes() -> Vec<String> {
  let mut prefixes: Vec<String> = Vec::new();
  let networks = sysinfo::Networks::new_with_refreshed_list();
  for (name, data) in networks.list() {
    let lower = name.to_ascii_lowercase();
    if ["wg", "tun", "tap", "utun", "docker", "veth", "bridge"]
      .iter()
      .any(|p| lower.starts_with(p))
    {
      continue;
    }
    for ipn in data.ip_networks() {
      if let IpAddr::V4(v4) = ipn.addr {
        if let Some(prefix) = lan_prefix_of(v4) {
          if !prefixes.contains(&prefix) {
            prefixes.push(prefix);
          }
        }
      }
    }
  }
  prefixes
}

/// 没有可用私网网卡时的兜底：用默认出口的本地地址推 /24
fn primary_lan_prefix() -> Option<String> {
  let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
  sock.connect("8.8.8.8:80").ok()?;
  match sock.local_addr().ok()?.ip() {
    IpAddr::V4(v4) => lan_prefix_of(v4),
    IpAddr::V6(_) => None,
  }
}

/// 解析扫描范围，与 remote_duli 控制端的 IP 段格式一致：
/// CIDR（`192.168.1.0/24`）、末段范围（`192.168.1.1-254`）、逗号分隔 IP 或单个 IP。
fn parse_scan_targets(text: &str) -> Vec<String> {
  let mut hosts: Vec<String> = Vec::new();
  for part in text.split(',') {
    let part = part.trim();
    if part.is_empty() {
      continue;
    }
    if let Some((addr, bits)) = part.split_once('/') {
      if let (Ok(base), Ok(bits)) = (addr.trim().parse::<Ipv4Addr>(), bits.trim().parse::<u32>()) {
        if bits <= 32 {
          let mask = if bits == 0 {
            0
          } else {
            u32::MAX << (32 - bits)
          };
          let start = (u32::from(base) & mask) as u64;
          let count = 1u64 << (32 - bits);
          // /31、/32 没有网络号/广播地址的概念，整体当主机
          let (first, last) = if bits < 31 {
            (start + 1, start + count - 2)
          } else {
            (start, start + count - 1)
          };
          for value in first..=last {
            hosts.push(Ipv4Addr::from(value as u32).to_string());
            if hosts.len() >= LAN_SCAN_MAX_HOSTS {
              return hosts;
            }
          }
        }
      }
      continue;
    }
    if let Some((head, tail)) = part.rsplit_once('.') {
      if let Some((start, end)) = tail.split_once('-') {
        if let (Ok(start), Ok(end)) = (start.trim().parse::<u16>(), end.trim().parse::<u16>()) {
          if start <= end && end <= 255 {
            for i in start..=end {
              hosts.push(format!("{head}.{i}"));
              if hosts.len() >= LAN_SCAN_MAX_HOSTS {
                return hosts;
              }
            }
          }
          continue;
        }
      }
    }
    if part.parse::<Ipv4Addr>().is_ok() {
      hosts.push(part.to_string());
      if hosts.len() >= LAN_SCAN_MAX_HOSTS {
        return hosts;
      }
    }
  }
  hosts
}

fn ip_sort_key(host: &str) -> u32 {
  host.parse::<Ipv4Addr>().map(u32::from).unwrap_or(u32::MAX)
}

async fn probe_discover(probe: &reqwest::Client, host: &str, port: u16) -> Option<LanMachine> {
  let url = format!("http://{host}:{port}/discover");
  let resp = probe.get(&url).send().await.ok()?;
  if !resp.status().is_success() {
    return None;
  }
  let body: Value = resp.json().await.ok()?;
  if !body.get("online").and_then(Value::as_bool).unwrap_or(false) {
    return None;
  }
  Some(LanMachine {
    host: host.to_string(),
    name: as_string(body.get("name")),
    os: as_string(body.get("os")),
    width: as_i64(body.get("width")).clamp(0, 100_000) as u32,
    height: as_i64(body.get("height")).clamp(0, 100_000) as u32,
    port: as_i64(body.get("port")).clamp(1, 65535) as u16,
  })
}

/// 控制端扫描局域网发现被控端（移植 remote_duli 控制端的 IP 段扫描）。
/// `ranges` 为空时自动取本机各私网网段的 /24。
pub async fn scan_lan(
  ranges: Option<String>,
  port: Option<u16>,
) -> Result<Vec<LanMachine>, String> {
  if BWBROWSER_AUTH.get_credentials().is_none() {
    return Err(json!({"code": "CLOUD_NOT_SIGNED_IN"}).to_string());
  }
  let port = port.unwrap_or(LAN_SCAN_DEFAULT_PORT);
  let hosts = match ranges.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
    Some(text) => parse_scan_targets(text),
    None => {
      let mut prefixes = local_lan_prefixes();
      if prefixes.is_empty() {
        if let Some(prefix) = primary_lan_prefix() {
          prefixes.push(prefix);
        }
      }
      prefixes
        .iter()
        .flat_map(|prefix| (1..=254u16).map(move |i| format!("{prefix}.{i}")))
        .collect()
    }
  };
  if hosts.is_empty() {
    return Err(err(
      "REMOTE_MANAGEMENT_NO_LAN_RANGE",
      "未能确定可扫描的局域网网段",
    ));
  }

  let probe = reqwest::Client::builder()
    .timeout(Duration::from_millis(900))
    .connect_timeout(Duration::from_millis(600))
    .build()
    .map_err(|e| err("REMOTE_MANAGEMENT_UNREACHABLE", e))?;
  let semaphore = Arc::new(tokio::sync::Semaphore::new(LAN_SCAN_CONCURRENCY));
  let mut tasks = tokio::task::JoinSet::new();
  for host in hosts {
    let probe = probe.clone();
    let semaphore = semaphore.clone();
    tasks.spawn(async move {
      let _permit = semaphore.acquire().await.ok()?;
      probe_discover(&probe, &host, port).await
    });
  }

  let mut found: Vec<LanMachine> = Vec::new();
  while let Some(result) = tasks.join_next().await {
    if let Ok(Some(machine)) = result {
      found.push(machine);
    }
  }
  found.sort_by_key(|m| ip_sort_key(&m.host));
  crate::remote_agent::log_remote(&format!("远程管理：局域网扫描发现 {} 台设备", found.len()));
  Ok(found)
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

  #[test]
  fn scan_ranges_parse_cidr_range_and_list() {
    assert_eq!(parse_scan_targets("192.168.1.5"), vec!["192.168.1.5"]);
    assert_eq!(
      parse_scan_targets("192.168.1.1-3"),
      vec!["192.168.1.1", "192.168.1.2", "192.168.1.3"]
    );
    // /30 只有两个可用主机，网络号与广播地址要排除
    assert_eq!(
      parse_scan_targets("10.0.0.0/30"),
      vec!["10.0.0.1", "10.0.0.2"]
    );
    assert_eq!(
      parse_scan_targets("192.168.1.1, 10.0.0.1"),
      vec!["192.168.1.1", "10.0.0.1"]
    );
    assert!(parse_scan_targets("not-an-ip").is_empty());
    assert!(parse_scan_targets("").is_empty());
    // 超大网段要被上限截断，不能真的铺开上亿个地址
    assert_eq!(parse_scan_targets("10.0.0.0/8").len(), LAN_SCAN_MAX_HOSTS);
  }

  #[test]
  fn lan_prefix_skips_tunnel_loopback_and_non_private() {
    assert_eq!(
      lan_prefix_of(Ipv4Addr::new(192, 168, 1, 20)).as_deref(),
      Some("192.168.1")
    );
    assert_eq!(
      lan_prefix_of(Ipv4Addr::new(10, 4, 5, 6)).as_deref(),
      Some("10.4.5")
    );
    // 172.16-31 是 VPS 隧道段；回环/链路本地/公网都不是局域网候选
    assert_eq!(lan_prefix_of(Ipv4Addr::new(172, 16, 0, 2)), None);
    assert_eq!(lan_prefix_of(Ipv4Addr::new(172, 20, 0, 2)), None);
    assert_eq!(lan_prefix_of(Ipv4Addr::new(127, 0, 0, 1)), None);
    assert_eq!(lan_prefix_of(Ipv4Addr::new(169, 254, 10, 1)), None);
    assert_eq!(lan_prefix_of(Ipv4Addr::new(203, 0, 113, 7)), None);
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
}
