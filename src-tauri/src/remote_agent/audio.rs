//! 被控端音频采集：Windows WASAPI loopback（系统混音输出）→ Opus 编码包。
//!
//! 设计：
//! - loopback 采集的是「系统正在播放的混音」，不是麦克风；静音时 WASAPI 直接
//!   标记 SILENT（不送数据指针），补零编码，包序列保持 viewer 播放时钟连续。
//! - 48kHz mono、20ms 帧（960 样本）、Opus 32kbps：带宽约 4KB/s，CPU 可忽略。
//! - 独立 OS 线程跑 COM/采集/编码（tokio 线程池不碰 COM）；tx 被 drop（viewer
//!   断开/关音频）线程自动退出。
//! - 平台：仅 Windows 实现；其它系统返回「不可用」，开关打开也无害。

/// Opus 编码出的单个音频包（20ms）
pub struct AudioPacket {
  /// Opus 负载
  pub data: Vec<u8>,
}

/// 音频采集会话。采集线程持续产出编码包，由 relay 主循环取走发送。
pub struct AudioCapture {
  rx: tokio::sync::mpsc::Receiver<AudioPacket>,
}

impl AudioCapture {
  /// 启动采集+编码线程。失败返回 Err（设备不存在、API 不可用等）。
  pub fn start() -> Result<Self, String> {
    let (tx, rx) = tokio::sync::mpsc::channel::<AudioPacket>(64);
    spawn_capture_thread(tx)?;
    Ok(Self { rx })
  }

  /// 非阻塞取走一个已就绪的编码包（relay 主循环 select 分支调用）。
  /// sender 端已全部关闭（线程退出）时返回 None 且 rx 断开。
  pub fn try_recv(&mut self) -> Option<AudioPacket> {
    self.rx.try_recv().ok()
  }
}

#[cfg(windows)]
fn spawn_capture_thread(tx: tokio::sync::mpsc::Sender<AudioPacket>) -> Result<(), String> {
  std::thread::Builder::new()
    .name("remote-audio".into())
    .spawn(move || {
      if let Err(e) = capture_loop_windows(tx) {
        crate::remote_agent::log_remote_file(&format!("音频采集退出: {e}"));
      }
    })
    .map_err(|e| format!("音频线程启动失败: {e}"))?;
  Ok(())
}

#[cfg(windows)]
fn capture_loop_windows(tx: tokio::sync::mpsc::Sender<AudioPacket>) -> Result<(), String> {
  use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX,
  };
  use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
  };

  // 采集线程自己的 COM 单元
  let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
  if hr.is_err() {
    return Err(format!("COM 初始化失败: {hr}"));
  }

  let result = (|| -> Result<(), String> {
    unsafe {
      let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|e| format!("创建设备枚举器失败: {e}"))?;
      // loopback 取默认渲染设备（eRender）的输出混音
      let device = enumerator
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| format!("取默认输出设备失败: {e}"))?;
      let client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("激活音频客户端失败: {e}"))?;

      // 48kHz mono f32：WASAPI 共享模式原生通常 48k 立体声 f32，
      // Initialize 会接受我们给的格式并在混音引擎内部转换。
      let format = WAVEFORMATEX {
        wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
        nChannels: 1,
        nSamplesPerSec: 48000,
        wBitsPerSample: 32,
        nAvgBytesPerSec: 48000 * 4,
        nBlockAlign: 4,
        cbSize: 0,
      };
      // 200ms 缓冲（REFERENCE_TIME 100ns 单位）
      let buffer_duration: i64 = 2_000_000;
      client
        .Initialize(
          AUDCLNT_SHAREMODE_SHARED,
          AUDCLNT_STREAMFLAGS_LOOPBACK,
          buffer_duration,
          0,
          &format,
          None,
        )
        .map_err(|e| format!("音频客户端初始化失败: {e}"))?;

      let capture: IAudioCaptureClient = client
        .GetService()
        .map_err(|e| format!("取采集服务失败: {e}"))?;
      client.Start().map_err(|e| format!("启动采集失败: {e}"))?;

      let mut encoder = super::audio_codec::OpusEncoder::new()?;

      // 960 样本（20ms@48k）的 f32 帧
      const FRAME_SAMPLES: usize = super::audio_codec::FRAME_SAMPLES;
      let mut frame_buf: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 4);

      loop {
        // sender 被 drop（viewer 断开/关音频）即结束
        if tx.is_closed() {
          break;
        }

        let mut available = capture
          .GetNextPacketSize()
          .map_err(|e| format!("查询包大小失败: {e}"))?;
        while available > 0 {
          let mut data_ptr = std::ptr::null_mut::<u8>();
          let mut frames = 0u32;
          let mut flags = 0u32;
          capture
            .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
            .map_err(|e| format!("GetBuffer 失败: {e}"))?;
          let sample_count = frames as usize; // mono: 1 样本/帧
          let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
          if !silent && !data_ptr.is_null() {
            let slice = std::slice::from_raw_parts(data_ptr as *const f32, sample_count);
            for s in slice {
              frame_buf.push(s.clamp(-1.0, 1.0));
            }
          } else {
            frame_buf.resize(frame_buf.len() + sample_count, 0.0);
          }
          let _ = capture.ReleaseBuffer(frames);
          available = capture.GetNextPacketSize().unwrap_or(0);

          // 攒够一帧就编码发送
          while frame_buf.len() >= FRAME_SAMPLES {
            let frame_data: Vec<f32> = frame_buf.drain(..FRAME_SAMPLES).collect();
            match encoder.encode(&frame_data) {
              Ok(packet) => {
                // channel 满时丢弃（阻塞会拖垮采集实时性）；
                // viewer 按时间播放，丢包表现为瞬时卡顿，可接受
                let _ = tx.try_send(AudioPacket { data: packet });
              }
              Err(e) => {
                crate::remote_agent::log_remote_file(&format!("Opus 编码失败: {e}"));
              }
            }
          }
        }

        // 5ms 轮询：WASAPI 事件模式对 loopback 兼容性差（共享模式 loopback
        // 事件句柄不可靠），轮询对 CPU 影响可忽略（一次系统调用）
        std::thread::sleep(std::time::Duration::from_millis(5));
      }

      let _ = client.Stop();
    }
    Ok(())
  })();

  unsafe { CoUninitialize() };
  result
}

#[cfg(not(windows))]
fn spawn_capture_thread(_tx: tokio::sync::mpsc::Sender<AudioPacket>) -> Result<(), String> {
  Err("当前系统不支持声音传输（仅 Windows）".to_string())
}
