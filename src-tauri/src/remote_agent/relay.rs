//! 被控端主循环：VPS 注册 → relay 连接 → 指令分发 → 推流；另含本地 WS 服务（局域网直连）

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use super::capture;
use super::encode;
use super::input;
use super::protocol::{frame_header, ClientCommand};
use super::vps;
use super::{hostname, AgentStatus, RemoteAgentConfig};

pub const RELAY_URL: &str = "wss://yacm.xin/relay";

/// 统一的结构化错误（前端 translateBackendError 解析）
fn structured(code: &str, detail: impl Into<String>) -> String {
  json!({ "code": code, "params": { "detail": detail.into() } }).to_string()
}

fn set_status(status: &Arc<Mutex<AgentStatus>>, f: impl FnOnce(&mut AgentStatus)) {
  if let Ok(mut st) = status.lock() {
    f(&mut st);
  }
}

/// 单连接会话状态（指令处理 + 推流参数）
struct SessionCtx {
  password: String,
  authed: bool,
  control_mode: String,
  quality: u8,
  fps: u8,
  scale: f32,
  streaming: bool,
}

impl SessionCtx {
  fn new(cfg: &RemoteAgentConfig) -> Self {
    Self {
      password: cfg.password.clone(),
      authed: false,
      control_mode: "full".to_string(),
      quality: cfg.quality,
      fps: cfg.fps,
      scale: cfg.scale,
      streaming: false,
    }
  }

  /// 处理一条指令，返回要回复的 JSON 文本列表
  fn handle(&mut self, cmd: &ClientCommand) -> Vec<String> {
    let mut replies = Vec::new();
    match cmd {
      ClientCommand::Auth { password } => {
        if *password == self.password {
          self.authed = true;
          replies.push(
            json!({
              "type": "auth_ok",
              "screen_width": 0,
              "screen_height": 0,
              "fps": self.fps,
              "quality": self.quality,
              "scale": self.scale,
              "hostname": hostname(),
            })
            .to_string(),
          );
        } else {
          replies.push(json!({"type": "auth_fail", "message": "Wrong password"}).to_string());
        }
      }
      ClientCommand::Heartbeat => {
        replies.push(json!({"type": "heartbeat_ack"}).to_string());
      }
      ClientCommand::StartStream => {
        if !self.authed {
          replies.push(json!({"type": "error", "message": "Auth required"}).to_string());
        } else {
          self.streaming = true;
        }
      }
      ClientCommand::StopStream => {
        self.streaming = false;
      }
      ClientCommand::SetQuality { quality } => {
        self.quality = (*quality).clamp(10, 100);
      }
      ClientCommand::SetFps { fps } => {
        self.fps = (*fps).clamp(5, 60);
      }
      ClientCommand::SetScale { scale } => {
        self.scale = (*scale).clamp(0.25, 1.0);
      }
      ClientCommand::SetMode { mode } => {
        self.control_mode = mode.clone();
        replies.push(json!({"type": "mode_changed", "mode": mode}).to_string());
      }
      ClientCommand::MouseMoveRelative { dx, dy } => {
        if self.control_mode != "view" {
          input::mouse_move_relative(*dx, *dy);
        }
      }
      ClientCommand::MouseMove { x, y } => {
        if self.control_mode != "view" {
          input::mouse_move(*x, *y);
        }
      }
      ClientCommand::MouseDown { button } => {
        if self.control_mode != "view" {
          input::mouse_button(button.as_deref().unwrap_or("left"), true);
        }
      }
      ClientCommand::MouseUp { button } => {
        if self.control_mode != "view" {
          input::mouse_button(button.as_deref().unwrap_or("left"), false);
        }
      }
      ClientCommand::MouseClick { button } => {
        if self.control_mode != "view" {
          let b = button.as_deref().unwrap_or("left");
          input::mouse_button(b, true);
          input::mouse_button(b, false);
        }
      }
      ClientCommand::MouseScroll { dy, .. } => {
        if self.control_mode != "view" {
          input::mouse_scroll(*dy);
        }
      }
      ClientCommand::KeyDown { key } => {
        if self.control_mode != "view" {
          input::key_event(key, true);
        }
      }
      ClientCommand::KeyUp { key } => {
        if self.control_mode != "view" {
          input::key_event(key, false);
        }
      }
      ClientCommand::TypeText { text } => {
        if self.control_mode != "view" {
          input::type_text(text);
        }
      }
      ClientCommand::GetCursorPos => {
        let (x, y) = input::cursor_pos();
        replies.push(json!({"type": "cursor_pos", "x": x, "y": y}).to_string());
      }
    }
    replies
  }
}

/// 阻塞线程里截屏 + 编码一帧
fn capture_jpeg_blocking(quality: u8) -> Result<(u32, u32, Vec<u8>), String> {
  let frame = capture::capture_screen()?;
  let jpeg = encode::bgra_to_jpeg(&frame.bgra, frame.w, frame.h, quality)?;
  Ok((frame.w, frame.h, jpeg))
}

/// 通用帧推送：截屏 → JPEG → >III 头 + 二进制（sink 以引用传入，兼容 tungstenite/axum 两种 Message）
async fn push_frame<S, M>(sink: &mut S, ctx: &mut SessionCtx) -> Result<(), String>
where
  S: futures_util::Sink<M> + Unpin,
  M: From<Vec<u8>>,
  S::Error: std::fmt::Display,
{
  let q = ctx.quality;
  let (w, h, jpeg) = match tokio::task::spawn_blocking(move || capture_jpeg_blocking(q)).await {
    Ok(Ok(frame)) => frame,
    Ok(Err(e)) => return Err(structured("REMOTE_DESKTOP_INTERNAL", e)),
    Err(e) => {
      return Err(structured(
        "REMOTE_DESKTOP_INTERNAL",
        format!("截屏任务异常: {e}"),
      ))
    }
  };
  let mut payload = Vec::with_capacity(12 + jpeg.len());
  payload.extend_from_slice(&frame_header(w, h, jpeg.len() as u32));
  payload.extend_from_slice(&jpeg);
  sink
    .send(payload.into())
    .await
    .map_err(|e| structured("REMOTE_DESKTOP_UNREACHABLE", format!("帧发送失败: {e}")))
}

/// ==================== relay 主循环（重试直到 stop） ====================
pub async fn run_agent_loop(
  cfg: RemoteAgentConfig,
  creds: (String, String),
  stop: Arc<AtomicBool>,
  status: Arc<Mutex<AgentStatus>>,
) {
  // 后台心跳：保持云端在线状态，直到 stop（server.py heartbeat 线程等价物）
  let hb = {
    let stop = stop.clone();
    let uuid = cfg.uuid.clone();
    tokio::spawn(async move {
      while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let _ = vps::heartbeat(&uuid).await;
      }
    })
  };

  let mut permission_denied = false;
  while !stop.load(Ordering::SeqCst) && !permission_denied {
    let outcome = agent_cycle(&cfg, &creds, &stop, &status).await;
    if let Err(e) = outcome {
      set_status(&status, |s| {
        s.last_error = Some(e.clone());
        s.vps_registered = false;
        s.relay_connected = false;
      });
      // 云端权限被撤：不要再重试刷服务端，直接整体停止，
      // 由权限守护任务在权限恢复时重新拉起
      if crate::remote_agent::error_code(&e).as_deref() == Some("NO_REMOTE_PERMISSION") {
        crate::remote_agent::log_remote(&format!(
          "用户 {} 远程桌面被拒绝（users.php 权限组未开通），被控端下线",
          crate::remote_agent::current_user()
        ));
        permission_denied = true;
        continue;
      }
    }
    for _ in 0..5 {
      if stop.load(Ordering::SeqCst) {
        break;
      }
      tokio::time::sleep(Duration::from_secs(2)).await;
    }
  }
  hb.abort();
  if permission_denied {
    let _ = crate::remote_agent::stop_agent().await;
  }
}

async fn agent_cycle(
  cfg: &RemoteAgentConfig,
  creds: &(String, String),
  stop: &Arc<AtomicBool>,
  status: &Arc<Mutex<AgentStatus>>,
) -> Result<(), String> {
  // 1. client_login（云端校验 allow_remote_desktop 权限）
  let identity = vps::client_login()
    .await
    .inspect_err(|e| set_status(status, |s| s.last_error = Some(e.clone())))?;
  let _ = creds;
  set_status(status, |s| s.vps_registered = false);

  // 2. register
  let os_info = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
  vps::register(
    &cfg.uuid,
    &hostname(),
    &os_info,
    &vps::local_ips_json(),
    "",
    cfg.port,
    &cfg.password,
    &identity,
  )
  .await?;
  set_status(status, |s| s.vps_registered = true);

  // 3. relay 连接
  let mut url = url::Url::parse(RELAY_URL).map_err(|e| e.to_string())?;
  url
    .query_pairs_mut()
    .append_pair("role", "client")
    .append_pair("uuid", &cfg.uuid)
    .append_pair("hostname", &hostname());

  let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
    .await
    .map_err(|e| structured("REMOTE_DESKTOP_UNREACHABLE", format!("relay 连接失败: {e}")))?;
  crate::remote_agent::log_remote(&format!(
    "用户 {} relay 连接成功 uuid={}（等待 viewer）",
    creds.0, cfg.uuid
  ));
  set_status(status, |s| {
    s.relay_connected = true;
    s.last_error = None;
  });

  let (mut writer, mut reader) = ws.split();
  let mut ctx = SessionCtx::new(cfg);
  let mut ticker = tokio::time::interval(Duration::from_millis(1000 / 30));
  // viewer 断开后本端不发任何数据，nginx/中间设备会因空闲超时断开，
  // 且断连信号经常丢失（TCP 半开），reader 永远等不到报错，形成"在线但连不上"的僵尸连接。
  // 定期 Ping 保活，Pong 超时即判定连接已死并重连。
  let mut keepalive = tokio::time::interval(Duration::from_secs(10));
  let mut last_pong = std::time::Instant::now();

  loop {
    tokio::select! {
      msg = reader.next() => {
        match msg {
          Some(Ok(Message::Text(text))) => {
            if let Ok(cmd) = serde_json::from_str::<ClientCommand>(&text) {
              for reply in ctx.handle(&cmd) {
                if writer.send(Message::Text(reply.into())).await.is_err() {
                  break;
                }
              }
            } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
              // 未知类型消息（如 viewer_connected 通知），仅记录
              if v.get("type").and_then(|t| t.as_str()) == Some("viewer_connected") {
                set_status(status, |s| s.viewers = s.viewers.saturating_add(1));
                let n = status.lock().unwrap().viewers;
                crate::remote_agent::log_remote(&format!(
                  "用户 {} viewer 接入（当前 {} 个）",
                  creds.0, n
                ));
              } else if v.get("type").and_then(|t| t.as_str()) == Some("viewer_disconnected") {
                // viewer 断开后停止推流，避免继续 30FPS 截屏+编码空烧 CPU；
                // 下一个 viewer 接入时会重新发送 StartStream 恢复推流
                ctx.streaming = false;
                set_status(status, |s| s.viewers = s.viewers.saturating_sub(1));
                let n = status.lock().unwrap().viewers;
                crate::remote_agent::log_remote(&format!(
                  "用户 {} viewer 断开（当前 {} 个）",
                  creds.0, n
                ));
              }
            }
          }
          Some(Ok(Message::Binary(_))) => {}
          Some(Ok(Message::Pong(_))) => {
            last_pong = std::time::Instant::now();
          }
          Some(Ok(_)) => {}
          Some(Err(e)) => {
            return Err(structured("REMOTE_DESKTOP_UNREACHABLE", format!("relay 接收错误: {e}")));
          }
          None => {
            return Err(structured("REMOTE_DESKTOP_UNREACHABLE", "relay 连接已关闭"));
          }
        }
      }
      _ = ticker.tick() => {
        if ctx.streaming {
          if let Err(e) = push_frame(&mut writer, &mut ctx).await {
            // 发送失败说明 viewer 端已不可达：停流避免空烧 CPU，等重连后 viewer 重新 StartStream
            ctx.streaming = false;
            set_status(status, |s| s.last_error = Some(e));
            tokio::time::sleep(Duration::from_millis(500)).await;
          }
        }
      }
      _ = keepalive.tick() => {
        if last_pong.elapsed() > Duration::from_secs(40) {
          return Err(structured(
            "REMOTE_DESKTOP_UNREACHABLE",
            "relay 保活超时（40 秒无 Pong），判定连接已死",
          ));
        }
        if writer.send(Message::Ping(Default::default())).await.is_err() {
          return Err(structured(
            "REMOTE_DESKTOP_UNREACHABLE",
            "relay 保活发送失败，连接已断开",
          ));
        }
      }
    }
    if stop.load(Ordering::SeqCst) {
      break;
    }
  }

  set_status(status, |s| {
    s.relay_connected = false;
    s.viewers = 0;
  });
  Ok(())
}

/// ==================== 本地 WS 服务（局域网直连，axum） ====================
pub async fn run_local_server(
  cfg: RemoteAgentConfig,
  stop: Arc<AtomicBool>,
  status: Arc<Mutex<AgentStatus>>,
) {
  let app = axum::Router::new().route(
    "/ws",
    axum::routing::get({
      let cfg = cfg.clone();
      let stop = stop.clone();
      let status = status.clone();
      move |ws: axum::extract::ws::WebSocketUpgrade| {
        let cfg = cfg.clone();
        let stop = stop.clone();
        let status = status.clone();
        async move { ws.on_upgrade(move |socket| local_session(socket, cfg, stop, status)) }
      }
    }),
  );

  let listener = match tokio::net::TcpListener::bind(("127.0.0.1", cfg.port)).await {
    Ok(l) => l,
    Err(e) => {
      set_status(&status, |s| {
        s.last_error = Some(structured(
          "REMOTE_DESKTOP_UNREACHABLE",
          format!("本地服务启动失败: {e}"),
        ))
      });
      return;
    }
  };

  // 阻塞直到 stop（graceful shutdown 收尾）
  let _ = axum::serve(listener, app)
    .with_graceful_shutdown(async move {
      while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(300)).await;
      }
    })
    .await;
}

async fn local_session(
  mut ws: axum::extract::ws::WebSocket,
  cfg: RemoteAgentConfig,
  stop: Arc<AtomicBool>,
  status: Arc<Mutex<AgentStatus>>,
) {
  use axum::extract::ws::Message as WsMsg;

  let mut ctx = SessionCtx::new(&cfg);
  let mut ticker = tokio::time::interval(Duration::from_millis(1000 / 30));
  set_status(&status, |s| s.viewers = s.viewers.saturating_add(1));

  loop {
    tokio::select! {
      msg = ws.recv() => {
        match msg {
          Some(Ok(WsMsg::Text(text))) => {
            if let Ok(cmd) = serde_json::from_str::<ClientCommand>(&text) {
              for reply in ctx.handle(&cmd) {
                if ws.send(WsMsg::Text(reply.into())).await.is_err() {
                  break;
                }
              }
            }
          }
          Some(Ok(WsMsg::Close(_))) | None => break,
          Some(Err(_)) => break,
          _ => {}
        }
      }
      _ = ticker.tick() => {
        if ctx.streaming {
          if let Err(e) = push_frame(&mut ws, &mut ctx).await {
            set_status(&status, |s| s.last_error = Some(e));
            tokio::time::sleep(Duration::from_millis(500)).await;
          }
        }
      }
    }
    if stop.load(Ordering::SeqCst) {
      break;
    }
  }
  set_status(&status, |s| s.viewers = s.viewers.saturating_sub(1));
}
