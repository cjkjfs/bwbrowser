//! 被控端协议：viewer → 被控端的 JSON 指令 + 视频帧打包（与 server.py 兼容）

use serde::Deserialize;

/// viewer 下发的指令（JSON，type 字段为 snake_case，与 server.py 一致）
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
  Auth { password: String },
  StartStream,
  StopStream,
  SetQuality { quality: u8 },
  SetFps { fps: u8 },
  SetScale { scale: f32 },
  SetMode { mode: String },
  MouseMoveRelative { dx: i32, dy: i32 },
  MouseMove { x: i32, y: i32 },
  MouseDown { button: Option<String> },
  MouseUp { button: Option<String> },
  MouseClick { button: Option<String> },
  MouseScroll { dy: i32 },
  KeyDown { key: String },
  KeyUp { key: String },
  TypeText { text: String },
  Heartbeat,
  GetCursorPos,
}

/// 大端 >III 帧头（宽、高、JPEG 长度），与 server.py `struct.pack(">III", ...)` 一致
pub fn frame_header(w: u32, h: u32, len: u32) -> [u8; 12] {
  let mut b = [0u8; 12];
  b[0..4].copy_from_slice(&w.to_be_bytes());
  b[4..8].copy_from_slice(&h.to_be_bytes());
  b[8..12].copy_from_slice(&len.to_be_bytes());
  b
}
