//! 被控端主循环：VPS 注册 → relay 连接 → 指令分发 → 推流；另含本地 WS 服务（局域网直连）

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use super::capture;
use super::encode;
use super::files;
use super::input;
use super::protocol::{frame_header, ClientCommand};
use super::vps;
use super::{hostname, os_name, AgentStatus, RemoteAgentConfig};
use crate::cloud_domain::RELAY_URL;

/// 统一的结构化错误（前端 translateBackendError 解析）
fn structured(code: &str, detail: impl Into<String>) -> String {
  json!({ "code": code, "params": { "detail": detail.into() } }).to_string()
}

/// 把结构化错误渲染成一行可读日志（code + detail），避免日志里出现转义 JSON
fn describe(err: &str) -> String {
  match serde_json::from_str::<serde_json::Value>(err) {
    Ok(v) => {
      let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("UNKNOWN");
      match v
        .get("params")
        .and_then(|p| p.get("detail"))
        .and_then(|d| d.as_str())
      {
        Some(detail) => format!("{code}: {detail}"),
        None => code.to_string(),
      }
    }
    Err(_) => err.to_string(),
  }
}

fn set_status(status: &Arc<Mutex<AgentStatus>>, f: impl FnOnce(&mut AgentStatus)) {
  if let Ok(mut st) = status.lock() {
    f(&mut st);
  }
}

/// select 分支用：无进行中的下载时永久挂起，不占用分支
async fn recv_download(rx: &mut Option<tokio::sync::mpsc::Receiver<String>>) -> Option<String> {
  match rx.as_mut() {
    Some(rx) => rx.recv().await,
    None => std::future::pending().await,
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
  upload: Option<files::Upload>,
  upload_error: Option<String>,
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
      upload: None,
      upload_error: None,
    }
  }

  /// 处理一条指令，返回要回复的 JSON 文本列表
  fn handle(&mut self, cmd: &ClientCommand) -> Vec<String> {
    let mut replies = Vec::new();
    match cmd {
      ClientCommand::Auth { password } => {
        if *password == self.password {
          self.authed = true;
          let (sw, sh) = capture::screen_size();
          replies.push(
            json!({
              "type": "auth_ok",
              "screen_width": sw,
              "screen_height": sh,
              "fps": self.fps,
              "quality": self.quality,
              "scale": self.scale,
              "hostname": hostname(),
              "os": os_name(),
            })
            .to_string(),
          );
        } else {
          // viewer 密码不符会立刻收到 auth_fail 并断开，表现为「接入后 1~2 秒秒断」；
          // 此前无任何日志，无法判断 viewer 到底连上了没有。
          crate::remote_agent::log_remote(&format!(
            "用户 {} viewer 密码校验失败，拒绝连接",
            crate::remote_agent::current_user()
          ));
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
      // 多显示器选择：当前捕获整块虚拟屏幕（Windows 含全部显示器）/主屏（macOS），
      // 不单独切换。接受该指令只为避免未知变体让整条消息解析失败被静默丢弃。
      ClientCommand::SetMonitor { .. } => {}
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
      ClientCommand::FileList { path } => {
        if self.authed {
          replies.push(files::list(path));
        }
      }
      ClientCommand::FileDownload { .. } => {
        // 下载需要流式发送多条消息，由上层异步处理（见 relay/local 循环），此处不处理
      }
      ClientCommand::FileUploadStart { path, name, size } => {
        if self.authed {
          match files::begin_upload(path, name, *size) {
            Ok(u) => {
              self.upload = Some(u);
              self.upload_error = None;
              replies.push(json!({"type": "file_upload_ready", "success": true}).to_string());
            }
            Err(e) => {
              self.upload = None;
              self.upload_error = Some(e.clone());
              replies.push(
                json!({"type": "file_upload_ready", "success": false, "message": e}).to_string(),
              );
            }
          }
        }
      }
      ClientCommand::FileUploadChunk { data } => {
        // 起始失败后 upload 为空，后续分块直接忽略，等 file_upload_end 统一报错
        if let Some(u) = self.upload.as_mut() {
          match files::write_chunk(u, data) {
            Ok(progress) => replies.push(progress),
            Err(e) => {
              self.upload = None;
              self.upload_error = Some(e.clone());
              replies.push(
                json!({"type": "file_upload_result", "success": false, "message": e}).to_string(),
              );
            }
          }
        }
      }
      ClientCommand::FileUploadEnd => match self.upload.take() {
        Some(u) => replies.push(files::finish_upload(u)),
        None => {
          let msg = self
            .upload_error
            .take()
            .unwrap_or_else(|| "没有进行中的上传".to_string());
          replies.push(
            json!({"type": "file_upload_result", "success": false, "message": msg}).to_string(),
          );
        }
      },
      ClientCommand::FileMkdir { path, name } => {
        if self.authed {
          replies.push(files::make_dir(path, name));
        }
      }
      ClientCommand::FileRename { path, new_name } => {
        if self.authed {
          replies.push(files::rename(path, new_name));
        }
      }
      ClientCommand::FileDelete { path } => {
        if self.authed {
          replies.push(files::remove(path));
        }
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
  // 失败此前被完全吞掉：心跳一断，云端列表就把这台机器标成离线，
  // 而本地毫无痕迹。只在状态翻转时记一行，避免每 30 秒刷屏。
  let hb = {
    let stop = stop.clone();
    let uuid = cfg.uuid.clone();
    tokio::spawn(async move {
      let mut healthy = true;
      while !stop.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(30)).await;
        match vps::heartbeat(&uuid).await {
          Ok(()) => {
            if !healthy {
              crate::remote_agent::log_remote(&format!(
                "用户 {} VPS 心跳恢复",
                crate::remote_agent::current_user()
              ));
              healthy = true;
            }
          }
          Err(e) => {
            if healthy {
              crate::remote_agent::log_remote(&format!(
                "用户 {} VPS 心跳失败: {}",
                crate::remote_agent::current_user(),
                describe(&e)
              ));
              healthy = false;
            }
          }
        }
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
      // 其余失败（服务端关闭连接、保活超时、网络中断）此前完全静默，
      // 掉线无从排查；记一条带原因的日志，和上面的重连配对即可还原时间线。
      crate::remote_agent::log_remote(&format!(
        "用户 {} relay 断开（{}），10 秒后重连",
        creds.0,
        describe(&e)
      ));
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
  // 定期发心跳保活，超过 40 秒收不到任何回应即判定连接已死并重连。
  let mut keepalive = tokio::time::interval(Duration::from_secs(10));
  // 保活判定用墙上时钟而非单调钟：单调钟在系统休眠期间不前进（macOS/Linux 尤甚），
  // 只靠它会漏掉「休眠期间 TCP 半开、唤醒后连接已死」的僵尸连接 —— 表现为被控端
  // 一直显示在线但 viewer 永远连不上，且不自愈。墙上时钟包含休眠时长，唤醒即判定重连。
  let mut last_alive = std::time::SystemTime::now();
  // 文件下载走 channel 异步生产，主循环在下面的分支里转发。若直接 await 整个文件，
  // 主循环会被钉住读不到服务端 Ping（Python websockets 默认 20s ping_timeout），
  // 大文件传到一半连接就被服务端判死。viewer 逐文件串行下载，故同时只保留一个会话。
  let mut download_rx: Option<tokio::sync::mpsc::Receiver<String>> = None;

  loop {
    tokio::select! {
      msg = reader.next() => {
        // 任何一条收到的消息（服务端 Ping、heartbeat_ack、viewer 指令）都证明连接是活的，
        // 统一在此刷新墙上时钟，供保活分支判断是否已成僵尸连接。
        if matches!(msg, Some(Ok(_))) {
          last_alive = std::time::SystemTime::now();
        }
        match msg {
          Some(Ok(Message::Text(text))) => {
            if let Ok(cmd) = serde_json::from_str::<ClientCommand>(&text) {
              match &cmd {
                // 下载是「start + N 个分块 + end」的流式发送，交给独立任务生产、主循环转发，
                // 避免在 handle 里同步读整个文件而卡住推流/保活
                ClientCommand::FileDownload { path } if ctx.authed => {
                  if download_rx.is_none() {
                    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
                    let path = path.clone();
                    tokio::spawn(async move {
                      let _ = files::produce_download(&path, tx).await;
                    });
                    download_rx = Some(rx);
                  }
                }
                _ => {
                  for reply in ctx.handle(&cmd) {
                    if writer.send(Message::Text(reply.into())).await.is_err() {
                      break;
                    }
                  }
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
          Some(Ok(Message::Pong(_))) => {}
          Some(Ok(Message::Close(frame))) => {
            // 服务端主动关闭（空闲回收、重启、踢下线）：此前落进下面的通配分支被静默忽略，
            // 要等下一次 reader 轮询才发现，这里直接判定为断线并交给外层重连。
            return Err(structured(
              "REMOTE_DESKTOP_UNREACHABLE",
              format!("relay 服务端关闭连接: {frame:?}"),
            ));
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
      chunk = recv_download(&mut download_rx) => {
        match chunk {
          Some(text) => {
            if writer.send(Message::Text(text.into())).await.is_err() {
              // 连接已不可用：丢弃接收端，生产任务随 tx 发送失败自然结束
              download_rx = None;
            }
          }
          None => download_rx = None,
        }
      }
      _ = ticker.tick() => {
        if ctx.streaming {
          if let Err(e) = push_frame(&mut writer, &mut ctx).await {
            // 发送失败说明 viewer 端已不可达：停流避免空烧 CPU，等重连后 viewer 重新 StartStream。
            // 此前静默处理，推流失败时日志里毫无痕迹，viewer 秒断无从排查。
            ctx.streaming = false;
            crate::remote_agent::log_remote(&format!(
              "用户 {} 推流失败: {}",
              creds.0,
              describe(&e)
            ));
            set_status(status, |s| s.last_error = Some(e));
            tokio::time::sleep(Duration::from_millis(500)).await;
          }
        }
      }
      _ = keepalive.tick() => {
        // 墙上时钟倒退（NTP 校时）时 elapsed() 报 Err，视为未超时，避免误判掉线
        let stalled = last_alive
          .elapsed()
          .map(|d| d > Duration::from_secs(40))
          .unwrap_or(false);
        if stalled {
          return Err(structured(
            "REMOTE_DESKTOP_UNREACHABLE",
            "relay 保活超时（40 秒无任何回应），判定连接已死",
          ));
        }
        // relay_server.py 的在线判定只认应用层 {"type":"heartbeat"} 文本（它据此刷新 last_seen），
        // WebSocket Ping 只在协议层被自动 Pong、不刷新 last_seen：只发 Ping 会在
        // HEARTBEAT_TIMEOUT(120s) 后被服务端从 clients 表里摘掉 —— 本端仍显示已连接，
        // 但 viewer 一律收到 "Target offline"。两者都发，既保 TCP 又刷新服务端在线状态。
        if writer
          .send(Message::Text(json!({"type": "heartbeat"}).to_string().into()))
          .await
          .is_err()
        {
          return Err(structured(
            "REMOTE_DESKTOP_UNREACHABLE",
            "relay 心跳发送失败，连接已断开",
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
  let (port, fps, quality, scale) = (cfg.port, cfg.fps, cfg.quality, cfg.scale);
  let app = axum::Router::new()
    .route(
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
    )
    // 局域网发现：控制端扫网段时打这个端点，免密，只暴露主机名/系统/分辨率/端口
    .route(
      "/discover",
      axum::routing::get(move || async move {
        let (w, h) = capture::screen_size();
        axum::Json(json!({
          "online": true,
          "name": hostname(),
          "os": os_name(),
          "width": w,
          "height": h,
          "port": port,
        }))
      }),
    )
    .route(
      "/info",
      axum::routing::get(move || async move {
        let (w, h) = capture::screen_size();
        axum::Json(json!({
          "width": w,
          "height": h,
          "fps": fps,
          "quality": quality,
          "scale": scale,
          "name": hostname(),
          "os": os_name(),
        }))
      }),
    )
    // 被控端自带 viewer：局域网内浏览器打开 http://<ip>:<port>/ 就能直接控制
    .route("/", axum::routing::get(handle_viewer_index))
    .route("/client.js", axum::routing::get(handle_viewer_js));

  // 监听所有网卡，局域网内的 viewer 才能直连（优先局域网）。只绑回环会让局域网
  // 直连必然失败、全部回落到公网中继，内网文件传输也要绕公网。接入仍需 auth 密码。
  let listener = match tokio::net::TcpListener::bind(("0.0.0.0", cfg.port)).await {
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
  let mut download_rx: Option<tokio::sync::mpsc::Receiver<String>> = None;
  set_status(&status, |s| s.viewers = s.viewers.saturating_add(1));

  loop {
    tokio::select! {
      msg = ws.recv() => {
        match msg {
          Some(Ok(WsMsg::Text(text))) => {
            if let Ok(cmd) = serde_json::from_str::<ClientCommand>(&text) {
              match &cmd {
                ClientCommand::FileDownload { path } if ctx.authed => {
                  if download_rx.is_none() {
                    let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
                    let path = path.clone();
                    tokio::spawn(async move {
                      let _ = files::produce_download(&path, tx).await;
                    });
                    download_rx = Some(rx);
                  }
                }
                _ => {
                  for reply in ctx.handle(&cmd) {
                    if ws.send(WsMsg::Text(reply.into())).await.is_err() {
                      break;
                    }
                  }
                }
              }
            }
          }
          Some(Ok(WsMsg::Close(_))) | None => break,
          Some(Err(_)) => break,
          _ => {}
        }
      }
      chunk = recv_download(&mut download_rx) => {
        match chunk {
          Some(text) => {
            if ws.send(WsMsg::Text(text.into())).await.is_err() {
              download_rx = None;
            }
          }
          None => download_rx = None,
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

/// viewer 资源随二进制一起更新，禁止浏览器缓存：旧 client.js 被缓存后，
/// 新模式（如「文件传输」）不生效，仍显示旧版界面的「仅查看」。
const VIEWER_NO_STORE: &str = "no-store, no-cache, must-revalidate";

/// 被控端内置 viewer 首页：局域网内浏览器直接打开 `http://<ip>:<port>/` 即可控制。
/// 资源用 include_str! 编进二进制，不依赖安装目录里的静态文件。
async fn handle_viewer_index() -> impl axum::response::IntoResponse {
  (
    [(axum::http::header::CACHE_CONTROL, VIEWER_NO_STORE)],
    axum::response::Html(include_str!("web/index.html")),
  )
}

/// viewer 前端脚本
async fn handle_viewer_js() -> impl axum::response::IntoResponse {
  (
    [
      (
        axum::http::header::CONTENT_TYPE,
        "application/javascript; charset=utf-8",
      ),
      (axum::http::header::CACHE_CONTROL, VIEWER_NO_STORE),
    ],
    include_str!("web/client.js"),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  fn ctx(authed: bool) -> SessionCtx {
    let mut c = SessionCtx::new(&RemoteAgentConfig::default());
    c.authed = authed;
    c
  }

  /// file_list 必须回复 file_list_result —— viewer 5 秒收不到就显示
  /// 「服务器未响应文件列表请求」，这正是此前的故障现象。
  #[test]
  fn file_list_replies_with_result() {
    let cmd: ClientCommand = serde_json::from_str(r#"{"type":"file_list","path":""}"#).unwrap();
    let replies = ctx(true).handle(&cmd);
    assert_eq!(replies.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&replies[0]).unwrap();
    assert_eq!(v["type"], "file_list_result");
    assert_eq!(v["success"], true);
    assert!(v["entries"].is_array());
  }

  #[test]
  fn file_list_silent_before_auth() {
    let cmd: ClientCommand = serde_json::from_str(r#"{"type":"file_list","path":""}"#).unwrap();
    assert!(ctx(false).handle(&cmd).is_empty());
  }
}
