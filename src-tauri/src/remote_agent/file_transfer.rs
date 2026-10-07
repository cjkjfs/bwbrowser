//! 控制端文件传输：本机文件浏览 + 远程文件读写。
//!
//! 本机侧直接复用 `files::*`（同一份目录枚举/新建/重命名/删除逻辑，被控端与本机
//! 一致）；远程侧以 viewer 身份连到被控端，复用被控端的 `file_*` 协议：
//! 局域网 `ws://host:port/ws`，跨网段 `wss://<relay>?role=viewer&target_uuid=<uuid>`。
//! 每次操作开一条短连接（鉴权 → 一条指令 → 收结果 → 关闭），无需在被控端维持会话。

use std::path::PathBuf;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use super::files;

/// 单条消息的等待上限：列目录/小操作很快，避免前端无限转圈
const OP_TIMEOUT: Duration = Duration::from_secs(20);
/// 传输分块（与被控端 `files::CHUNK` 一致）
const CHUNK: usize = 65536;

type Ws =
  tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn err(code: &str, detail: impl std::fmt::Display) -> String {
  json!({ "code": code, "params": { "detail": detail.to_string() } }).to_string()
}

fn parse(text: String) -> Result<Value, String> {
  serde_json::from_str(&text).map_err(|e| err("REMOTE_DESKTOP_INTERNAL", e))
}

fn value_text(v: &Value, key: &str) -> String {
  v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// 远程文件传输目标：由 `control::file_target` 解析一次，前端原样回传复用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTarget {
  /// "lan" | "relay"
  pub method: String,
  pub host: String,
  pub port: u16,
  pub password: String,
  /// 中继连接所需的被控端 uuid（局域网直连时为空）
  pub uuid: String,
  pub relay_url: String,
  pub hostname: String,
}

// ==================== 本机文件（控制端自己的磁盘） ====================

/// 本机文件浏览的起始目录（用户主目录）；取不到时回退「磁盘/根」视图
pub fn local_home() -> String {
  std::env::var("USERPROFILE")
    .or_else(|_| std::env::var("HOME"))
    .unwrap_or_default()
}

pub fn local_list(path: String) -> Result<Value, String> {
  parse(files::list(&path))
}

pub fn local_mkdir(path: String, name: String) -> Result<Value, String> {
  parse(files::make_dir(&path, &name))
}

pub fn local_rename(path: String, new_name: String) -> Result<Value, String> {
  parse(files::rename(&path, &new_name))
}

pub fn local_delete(path: String) -> Result<Value, String> {
  parse(files::remove(&path))
}

// ==================== 远程文件（被控端磁盘） ====================

/// viewer 连接地址：局域网直连被控端 `/ws`，跨网段走中继 `role=viewer`
fn viewer_url(t: &FileTarget) -> String {
  if t.method == "relay" {
    let base = if t.relay_url.is_empty() {
      crate::cloud_domain::RELAY_URL
    } else {
      t.relay_url.as_str()
    };
    format!(
      "{base}?role=viewer&target_uuid={}",
      urlencoding::encode(&t.uuid)
    )
  } else {
    format!("ws://{}:{}/ws", t.host, t.port)
  }
}

/// 建立到被控端的 viewer 连接并完成鉴权
async fn open(t: &FileTarget) -> Result<Ws, String> {
  let url = viewer_url(t);
  let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str())
    .await
    .map_err(|e| err("REMOTE_DESKTOP_UNREACHABLE", format!("连接失败: {e}")))?;
  ws.send(Message::Text(
    json!({ "type": "auth", "password": t.password })
      .to_string()
      .into(),
  ))
  .await
  .map_err(|e| err("REMOTE_DESKTOP_UNREACHABLE", format!("发送鉴权失败: {e}")))?;

  loop {
    let v = read_message(&mut ws).await?;
    match v.get("type").and_then(Value::as_str) {
      Some("auth_ok") => return Ok(ws),
      Some("auth_fail") => {
        return Err(json!({ "code": "REMOTE_DESKTOP_BAD_PASSWORD" }).to_string())
      }
      Some("error") => {
        return Err(err("REMOTE_DESKTOP_UNREACHABLE", value_text(&v, "message")));
      }
      _ => {}
    }
  }
}

/// 读一条文本消息并解析为 JSON；忽略二进制/控制帧
async fn read_message(ws: &mut Ws) -> Result<Value, String> {
  loop {
    let next = tokio::time::timeout(OP_TIMEOUT, ws.next())
      .await
      .map_err(|_| err("REMOTE_DESKTOP_UNREACHABLE", "等待被控端响应超时"))?;
    match next {
      Some(Ok(Message::Text(text))) => {
        return serde_json::from_str(&text)
          .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("响应解析失败: {e}")));
      }
      Some(Ok(_)) => continue,
      Some(Err(e)) => {
        return Err(err("REMOTE_DESKTOP_UNREACHABLE", format!("接收失败: {e}")));
      }
      None => return Err(err("REMOTE_DESKTOP_UNREACHABLE", "被控端已断开连接")),
    }
  }
}

/// 读到 `types` 中任意一种消息为止；收到中继的 `error` 直接失败
async fn read_until(ws: &mut Ws, types: &[&str]) -> Result<Value, String> {
  loop {
    let v = read_message(ws).await?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    if types.contains(&kind) {
      return Ok(v);
    }
    if kind == "error" {
      return Err(err("REMOTE_DESKTOP_UNREACHABLE", value_text(&v, "message")));
    }
  }
}

async fn send(ws: &mut Ws, payload: Value) -> Result<(), String> {
  ws.send(Message::Text(payload.to_string().into()))
    .await
    .map_err(|e| err("REMOTE_DESKTOP_UNREACHABLE", format!("发送失败: {e}")))
}

/// 单指令单回复（列目录/新建/重命名/删除）
async fn request(t: &FileTarget, payload: Value, expect: &str) -> Result<Value, String> {
  let mut ws = open(t).await?;
  send(&mut ws, payload).await?;
  read_until(&mut ws, &[expect]).await
}

pub async fn remote_list(t: FileTarget, path: String) -> Result<Value, String> {
  request(
    &t,
    json!({ "type": "file_list", "path": path }),
    "file_list_result",
  )
  .await
}

pub async fn remote_mkdir(t: FileTarget, path: String, name: String) -> Result<Value, String> {
  request(
    &t,
    json!({ "type": "file_mkdir", "path": path, "name": name }),
    "file_op_result",
  )
  .await
}

pub async fn remote_rename(t: FileTarget, path: String, new_name: String) -> Result<Value, String> {
  request(
    &t,
    json!({ "type": "file_rename", "path": path, "new_name": new_name }),
    "file_op_result",
  )
  .await
}

pub async fn remote_delete(t: FileTarget, path: String) -> Result<Value, String> {
  request(
    &t,
    json!({ "type": "file_delete", "path": path }),
    "file_op_result",
  )
  .await
}

/// 只取末段文件名，避免远端返回的名字带路径分隔符写到目录之外
fn safe_name(name: &str) -> Result<String, String> {
  std::path::Path::new(name)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .filter(|n| !n.is_empty() && n != "." && n != "..")
    .ok_or_else(|| err("REMOTE_DESKTOP_INTERNAL", "文件名非法"))
}

/// 远程 → 本机：把被控端文件下载到本机目录，返回落盘信息
pub async fn remote_download(
  t: FileTarget,
  remote_path: String,
  local_dir: String,
) -> Result<Value, String> {
  let dir = PathBuf::from(&local_dir);
  if !dir.is_dir() {
    return Err(err("REMOTE_DESKTOP_INTERNAL", "本机目标不是目录"));
  }
  let mut ws = open(&t).await?;
  send(
    &mut ws,
    json!({ "type": "file_download", "path": remote_path }),
  )
  .await?;

  let start = read_until(&mut ws, &["file_download_start", "file_download_error"]).await?;
  if start.get("type").and_then(Value::as_str) == Some("file_download_error") {
    return Err(err(
      "REMOTE_DESKTOP_INTERNAL",
      value_text(&start, "message"),
    ));
  }
  let name = safe_name(&value_text(&start, "name"))?;
  let size = start.get("size").and_then(Value::as_u64).unwrap_or(0);
  let target = dir.join(&name);

  use tokio::io::AsyncWriteExt;
  let mut file = tokio::fs::File::create(&target)
    .await
    .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("创建本地文件失败: {e}")))?;

  loop {
    let v = read_until(
      &mut ws,
      &[
        "file_download_chunk",
        "file_download_end",
        "file_download_error",
      ],
    )
    .await?;
    match v.get("type").and_then(Value::as_str) {
      Some("file_download_chunk") => {
        let bytes = STANDARD
          .decode(value_text(&v, "data"))
          .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("分块解码失败: {e}")))?;
        file
          .write_all(&bytes)
          .await
          .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("写入失败: {e}")))?;
      }
      Some("file_download_end") => break,
      _ => {
        let _ = tokio::fs::remove_file(&target).await;
        return Err(err("REMOTE_DESKTOP_INTERNAL", value_text(&v, "message")));
      }
    }
  }
  file
    .flush()
    .await
    .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("写入失败: {e}")))?;
  Ok(json!({
    "name": name,
    "size": size,
    "path": target.to_string_lossy(),
  }))
}

/// 本机 → 远程：把本机文件上传到被控端目录，返回远端落盘信息
pub async fn remote_upload(
  t: FileTarget,
  local_path: String,
  remote_dir: String,
) -> Result<Value, String> {
  let src = PathBuf::from(&local_path);
  let meta = tokio::fs::metadata(&src)
    .await
    .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("读取本机文件失败: {e}")))?;
  if !meta.is_file() {
    return Err(err("REMOTE_DESKTOP_INTERNAL", "只能上传文件"));
  }
  let name = src
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .ok_or_else(|| err("REMOTE_DESKTOP_INTERNAL", "文件名非法"))?;
  let size = meta.len();

  let mut ws = open(&t).await?;
  send(
    &mut ws,
    json!({ "type": "file_upload_start", "path": remote_dir, "name": name, "size": size }),
  )
  .await?;
  let ready = read_until(&mut ws, &["file_upload_ready", "file_upload_result"]).await?;
  if ready.get("success").and_then(Value::as_bool) != Some(true) {
    return Err(err(
      "REMOTE_DESKTOP_INTERNAL",
      value_text(&ready, "message"),
    ));
  }

  use tokio::io::AsyncReadExt;
  let mut file = tokio::fs::File::open(&src)
    .await
    .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("打开本机文件失败: {e}")))?;
  let mut buf = vec![0u8; CHUNK];
  loop {
    let n = file
      .read(&mut buf)
      .await
      .map_err(|e| err("REMOTE_DESKTOP_INTERNAL", format!("读取失败: {e}")))?;
    if n == 0 {
      break;
    }
    send(
      &mut ws,
      json!({ "type": "file_upload_chunk", "data": STANDARD.encode(&buf[..n]) }),
    )
    .await?;
    let progress = read_until(&mut ws, &["file_upload_progress", "file_upload_result"]).await?;
    if progress.get("type").and_then(Value::as_str) == Some("file_upload_result")
      && progress.get("success").and_then(Value::as_bool) != Some(true)
    {
      return Err(err(
        "REMOTE_DESKTOP_INTERNAL",
        value_text(&progress, "message"),
      ));
    }
  }
  send(&mut ws, json!({ "type": "file_upload_end" })).await?;
  let done = read_until(&mut ws, &["file_upload_result"]).await?;
  if done.get("success").and_then(Value::as_bool) != Some(true) {
    return Err(err("REMOTE_DESKTOP_INTERNAL", value_text(&done, "message")));
  }
  Ok(json!({ "name": name, "size": size }))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// 本机文件浏览直接复用 files::*，返回的仍是 viewer 那一套结果结构，
  /// 前端左右两栏可以共用同一份渲染逻辑。
  #[test]
  fn local_list_returns_viewer_shaped_result() {
    let home = local_home();
    let v = local_list(home).unwrap();
    assert_eq!(v["type"], "file_list_result");
    assert_eq!(v["success"], true);
    assert!(v["entries"].is_array());
  }

  /// 远端文件名里的路径分隔符/`..` 必须被剥掉，不能写到目标目录之外。
  #[test]
  fn safe_name_rejects_traversal() {
    assert_eq!(safe_name("a.txt").unwrap(), "a.txt");
    assert_eq!(safe_name("../../etc/passwd").unwrap(), "passwd");
    assert!(safe_name("..").is_err());
    assert!(safe_name("").is_err());
  }

  /// 中继地址按 role=viewer + target_uuid 拼装；局域网走 /ws。
  #[test]
  fn target_urls_are_built_per_method() {
    let relay = FileTarget {
      method: "relay".into(),
      host: String::new(),
      port: 8765,
      password: "p".into(),
      uuid: "abc".into(),
      relay_url: crate::cloud_domain::RELAY_URL.into(),
      hostname: String::new(),
    };
    assert_eq!(
      viewer_url(&relay),
      format!(
        "{}?role=viewer&target_uuid=abc",
        crate::cloud_domain::RELAY_URL
      )
    );

    let lan = FileTarget {
      method: "lan".into(),
      host: "192.168.1.9".into(),
      port: 8765,
      password: "p".into(),
      uuid: String::new(),
      relay_url: String::new(),
      hostname: String::new(),
    };
    assert_eq!(viewer_url(&lan), "ws://192.168.1.9:8765/ws");
  }
}
