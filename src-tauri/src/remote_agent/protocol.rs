//! 被控端协议：viewer → 被控端的 JSON 指令 + 视频帧打包（与 server.py 兼容）

use serde::Deserialize;

/// viewer 下发的指令（JSON，type 字段为 snake_case，与 server.py 一致）
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
  Auth {
    password: String,
  },
  StartStream,
  StopStream,
  SetQuality {
    quality: u8,
  },
  SetFps {
    fps: u8,
  },
  SetScale {
    scale: f32,
  },
  SetMode {
    mode: String,
  },
  SetMonitor {
    #[serde(rename = "monitor")]
    _monitor: i32,
  },
  MouseMoveRelative {
    dx: i32,
    dy: i32,
  },
  MouseMove {
    x: i32,
    y: i32,
  },
  MouseDown {
    button: Option<String>,
  },
  MouseUp {
    button: Option<String>,
  },
  MouseClick {
    button: Option<String>,
  },
  MouseScroll {
    dy: i32,
  },
  KeyDown {
    key: String,
  },
  KeyUp {
    key: String,
  },
  TypeText {
    text: String,
  },
  Heartbeat,
  GetCursorPos,
  // 文件传输（remote_app.php 的文件管理面板）
  FileList {
    path: String,
  },
  FileDownload {
    path: String,
  },
  FileUploadStart {
    path: String,
    name: String,
    size: u64,
  },
  FileUploadChunk {
    data: String,
  },
  FileUploadEnd,
  // 文件管理面板的目录/文件操作
  FileMkdir {
    path: String,
    name: String,
  },
  FileRename {
    path: String,
    new_name: String,
  },
  FileDelete {
    path: String,
  },
}

/// 大端 >III 帧头（宽、高、JPEG 长度），与 server.py `struct.pack(">III", ...)` 一致
pub fn frame_header(w: u32, h: u32, len: u32) -> [u8; 12] {
  let mut b = [0u8; 12];
  b[0..4].copy_from_slice(&w.to_be_bytes());
  b[4..8].copy_from_slice(&h.to_be_bytes());
  b[8..12].copy_from_slice(&len.to_be_bytes());
  b
}

#[cfg(test)]
mod tests {
  use super::ClientCommand;

  /// remote_app.php 会下发的每一种指令都必须能反序列化：未知变体会让整条消息
  /// 解析失败被静默丢弃（file_list 曾因此超时），故用测试锁住协议覆盖。
  #[test]
  fn every_viewer_command_deserializes() {
    let samples = [
      r#"{"type":"auth","password":"x"}"#,
      r#"{"type":"start_stream"}"#,
      r#"{"type":"stop_stream"}"#,
      r#"{"type":"set_quality","quality":65}"#,
      r#"{"type":"set_fps","fps":30}"#,
      r#"{"type":"set_scale","scale":1.0}"#,
      r#"{"type":"set_mode","mode":"full"}"#,
      r#"{"type":"set_monitor","monitor":1}"#,
      r#"{"type":"mouse_move","x":1,"y":2}"#,
      r#"{"type":"mouse_move_relative","dx":1,"dy":2}"#,
      r#"{"type":"mouse_down","button":"left"}"#,
      r#"{"type":"mouse_up","button":"left"}"#,
      r#"{"type":"mouse_click","button":"left"}"#,
      r#"{"type":"mouse_scroll","dx":0,"dy":-1}"#,
      r#"{"type":"key_down","key":"a"}"#,
      r#"{"type":"key_up","key":"a"}"#,
      r#"{"type":"type_text","text":"hi"}"#,
      r#"{"type":"heartbeat"}"#,
      r#"{"type":"get_cursor_pos"}"#,
      r#"{"type":"file_list","path":""}"#,
      r#"{"type":"file_download","path":"/tmp/a"}"#,
      r#"{"type":"file_upload_start","path":"/tmp","name":"a","size":1}"#,
      r#"{"type":"file_upload_chunk","data":"AAAA"}"#,
      r#"{"type":"file_upload_end"}"#,
      r#"{"type":"file_mkdir","path":"/tmp","name":"a"}"#,
      r#"{"type":"file_rename","path":"/tmp/a","new_name":"b"}"#,
      r#"{"type":"file_delete","path":"/tmp/a"}"#,
    ];
    for s in samples {
      assert!(
        serde_json::from_str::<ClientCommand>(s).is_ok(),
        "viewer 指令无法反序列化: {s}"
      );
    }
  }
}
