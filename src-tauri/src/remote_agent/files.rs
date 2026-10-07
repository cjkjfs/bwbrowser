//! 远程文件管理（viewer 文件传输协议，与 remote_app.php 兼容）
//!
//! viewer 指令与期望回复：
//! - `file_list`          → `file_list_result` { success, entries[{name,is_dir,size}], path, parent, is_drives }
//! - `file_download`      → `file_download_start` → N×`file_download_chunk` → `file_download_end`
//! - 下载任一步失败改为 `file_download_error` { message }
//! - `file_upload_start`  → `file_upload_ready`   { success }
//! - `file_upload_chunk`  → `file_upload_progress`{ received, total }
//! - `file_upload_end`    → `file_upload_result`  { success, message? }

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::json;

/// 下载分块大小（与 viewer 上传分块一致）
const CHUNK: usize = 65536;

/// 上传会话：跨 `file_upload_*` 多条指令保持打开的文件句柄
pub struct Upload {
  file: std::fs::File,
  received: u64,
  total: u64,
}

/// 空路径代表「磁盘/根」视图：Windows 枚举盘符，类 Unix 只给一个 `/` 入口
pub fn list(path: &str) -> String {
  if path.is_empty() {
    return roots_list();
  }
  match read_dir(path) {
    Ok((entries, parent)) => json!({
      "type": "file_list_result",
      "success": true,
      "entries": entries,
      "path": path,
      "parent": parent,
      "is_drives": false,
    })
    .to_string(),
    Err(e) => json!({
      "type": "file_list_result",
      "success": false,
      "message": e,
      "path": path,
    })
    .to_string(),
  }
}

#[cfg(windows)]
fn roots_list() -> String {
  let mut entries = Vec::new();
  for c in b'A'..=b'Z' {
    let root = format!("{}:\\", c as char);
    if Path::new(&root).exists() {
      entries.push(json!({ "name": root, "is_dir": true, "size": 0 }));
    }
  }
  json!({
    "type": "file_list_result",
    "success": true,
    "entries": entries,
    "path": "",
    "parent": "",
    "is_drives": true,
  })
  .to_string()
}

#[cfg(not(windows))]
fn roots_list() -> String {
  json!({
    "type": "file_list_result",
    "success": true,
    "entries": [{ "name": "/", "is_dir": true, "size": 0 }],
    "path": "",
    "parent": "",
    "is_drives": true,
  })
  .to_string()
}

fn read_dir(path: &str) -> Result<(Vec<serde_json::Value>, String), String> {
  let dir = std::fs::read_dir(path).map_err(|e| format!("无法打开目录: {e}"))?;
  let mut entries = Vec::new();
  for item in dir.flatten() {
    // 必须跟随符号链接：macOS 的 /etc、/tmp、/var 都是指向 /private 的符号链接，
    // DirEntry::metadata() 不跟随会把它们判成文件，导致页面上无法进入这些目录。
    // 断链时回退到链接自身的信息，保证条目仍可见而不是被静默丢弃。
    let path = item.path();
    let Ok(meta) = std::fs::metadata(&path).or_else(|_| item.metadata()) else {
      continue;
    };
    let is_dir = meta.is_dir();
    entries.push(json!({
      "name": item.file_name().to_string_lossy(),
      "is_dir": is_dir,
      "size": if is_dir { 0 } else { meta.len() },
    }));
  }
  // 文件夹在前，其余按名称不区分大小写排序
  entries.sort_by(|a, b| {
    let ad = a["is_dir"].as_bool().unwrap_or(false);
    let bd = b["is_dir"].as_bool().unwrap_or(false);
    bd.cmp(&ad).then_with(|| {
      a["name"]
        .as_str()
        .unwrap_or("")
        .to_lowercase()
        .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    })
  });
  Ok((entries, parent_of(path)))
}

/// 上级路径；已到根（盘符/`/`）时返回空串，交给前端回到磁盘列表
fn parent_of(path: &str) -> String {
  match Path::new(path).parent() {
    Some(p) => {
      let s = p.to_string_lossy().to_string();
      if s == path {
        String::new()
      } else {
        s
      }
    }
    None => String::new(),
  }
}

async fn push(tx: &tokio::sync::mpsc::Sender<String>, v: serde_json::Value) -> Result<(), String> {
  tx.send(v.to_string())
    .await
    .map_err(|_| "接收端已关闭".to_string())
}

/// 生产下载消息：start → 分块 → end，经 channel 交给主循环发送。
///
/// 不直接写 socket：大文件会长时间占住主循环、读不到服务端心跳，20 秒就被
/// relay 判超时断开。走 channel 后主循环照常读 socket / 推流 / 保活。
pub async fn produce_download(
  path: &str,
  tx: tokio::sync::mpsc::Sender<String>,
) -> Result<(), String> {
  let result = download_body(path, &tx).await;
  if let Err(e) = &result {
    let _ = push(&tx, json!({ "type": "file_download_error", "message": e })).await;
  }
  result
}

async fn download_body(path: &str, tx: &tokio::sync::mpsc::Sender<String>) -> Result<(), String> {
  let p = PathBuf::from(path);
  let meta = tokio::fs::metadata(&p)
    .await
    .map_err(|e| format!("无法读取文件: {e}"))?;
  if meta.is_dir() {
    return Err("不能下载文件夹".to_string());
  }
  let name = p
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| path.to_string());
  push(
    tx,
    json!({ "type": "file_download_start", "name": name, "size": meta.len() }),
  )
  .await?;

  let mut file = tokio::fs::File::open(&p)
    .await
    .map_err(|e| format!("打开文件失败: {e}"))?;
  use tokio::io::AsyncReadExt;
  let mut buf = vec![0u8; CHUNK];
  loop {
    let n = file
      .read(&mut buf)
      .await
      .map_err(|e| format!("读取失败: {e}"))?;
    if n == 0 {
      break;
    }
    push(
      tx,
      json!({ "type": "file_download_chunk", "data": STANDARD.encode(&buf[..n]) }),
    )
    .await?;
  }
  push(tx, json!({ "type": "file_download_end" })).await
}

/// 开始上传：创建（覆盖）目标文件。文件名只取末段，避免路径穿越。
pub fn begin_upload(dir: &str, name: &str, size: u64) -> Result<Upload, String> {
  if dir.is_empty() {
    return Err("未指定目标目录".to_string());
  }
  let safe = Path::new(name)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .filter(|n| !n.is_empty())
    .ok_or_else(|| "文件名非法".to_string())?;
  let file =
    std::fs::File::create(Path::new(dir).join(&safe)).map_err(|e| format!("创建文件失败: {e}"))?;
  Ok(Upload {
    file,
    received: 0,
    total: size,
  })
}

/// 写入一个 base64 分块，返回进度回复
pub fn write_chunk(upload: &mut Upload, data_b64: &str) -> Result<String, String> {
  let bytes = STANDARD
    .decode(data_b64)
    .map_err(|e| format!("分块解码失败: {e}"))?;
  upload
    .file
    .write_all(&bytes)
    .map_err(|e| format!("写入失败: {e}"))?;
  upload.received += bytes.len() as u64;
  Ok(
    json!({
      "type": "file_upload_progress",
      "received": upload.received,
      "total": upload.total,
    })
    .to_string(),
  )
}

/// 取单段文件名，拒绝空串与路径分隔符，避免 `..`/子路径穿越到目录之外
fn safe_name(name: &str) -> Result<String, String> {
  Path::new(name)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .filter(|n| !n.is_empty() && n != "." && n != "..")
    .ok_or_else(|| "名称非法".to_string())
}

fn op_result(result: Result<(), String>) -> String {
  match result {
    Ok(()) => json!({ "type": "file_op_result", "success": true }).to_string(),
    Err(e) => json!({ "type": "file_op_result", "success": false, "message": e }).to_string(),
  }
}

/// 在 parent 下新建目录
pub fn make_dir(parent: &str, name: &str) -> String {
  op_result((|| {
    if parent.is_empty() {
      return Err("未指定目录".to_string());
    }
    let safe = safe_name(name)?;
    std::fs::create_dir(Path::new(parent).join(&safe)).map_err(|e| format!("新建文件夹失败: {e}"))
  })())
}

/// 同目录内重命名（new_name 只取末段）
pub fn rename(path: &str, new_name: &str) -> String {
  op_result((|| {
    let src = Path::new(path);
    if path.is_empty() {
      return Err("未指定路径".to_string());
    }
    let safe = safe_name(new_name)?;
    let parent = src.parent().ok_or_else(|| "无法确定上级目录".to_string())?;
    std::fs::rename(src, parent.join(&safe)).map_err(|e| format!("重命名失败: {e}"))
  })())
}

/// 删除文件或目录（目录递归）
pub fn remove(path: &str) -> String {
  op_result((|| {
    if path.is_empty() {
      return Err("未指定路径".to_string());
    }
    let p = Path::new(path);
    let meta = std::fs::symlink_metadata(p).map_err(|e| format!("无法访问: {e}"))?;
    if meta.is_dir() {
      std::fs::remove_dir_all(p).map_err(|e| format!("删除目录失败: {e}"))
    } else {
      std::fs::remove_file(p).map_err(|e| format!("删除失败: {e}"))
    }
  })())
}

/// 收尾：flush 并回复最终结果
pub fn finish_upload(mut upload: Upload) -> String {
  match upload.file.flush() {
    Ok(()) => json!({ "type": "file_upload_result", "success": true }).to_string(),
    Err(e) => {
      json!({ "type": "file_upload_result", "success": false, "message": format!("写入失败: {e}") })
        .to_string()
    }
  }
}
