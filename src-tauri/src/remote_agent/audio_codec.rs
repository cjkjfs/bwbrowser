//! Opus 编码器封装（纯 Rust 实现 opus1 crate，RFC 6716）。
//!
//! 独立成 audio_codec.rs：audio.rs 是平台采集层，编码器替换/升级只动这里。
//! 48kHz mono、20ms 帧（960 样本）、32kbps：远程桌面声音够用且带宽最省。

use opus1::{
  Application as Opus1App, Bitrate as Opus1Bitrate, Channels as Opus1Channels,
  OpusEncoder as LibOpusEncoder, SampleRate as Opus1SampleRate,
};

/// 帧大小：960 样本 = 20ms @ 48kHz
pub const FRAME_SAMPLES: usize = 960;

pub struct OpusEncoder {
  enc: LibOpusEncoder,
}

impl OpusEncoder {
  pub fn new() -> Result<Self, String> {
    // 无 heap feature 时编码器约 150KB 内联，new 走栈：放到采集线程的独立
    // 栈帧里构建（线程默认栈远大于 256KB），不在主线程做。
    let mut enc = LibOpusEncoder::new(
      Opus1SampleRate::Hz48000,
      Opus1Channels::Mono,
      Opus1App::Voip,
    )
    .map_err(|e| format!("Opus 编码器初始化失败: {e}"))?;
    // 32kbps：远程声音（人声/系统提示音）清晰度足够，带宽 4KB/s
    enc.set_bitrate(Opus1Bitrate::BitsPerSecond(
      std::num::NonZeroU32::new(32000).unwrap(),
    ));
    // 复杂度低档：CPU 占用最小（桌面声音场景音质差异可忽略）
    enc
      .set_complexity(3)
      .map_err(|e| format!("设置复杂度失败: {e}"))?;
    Ok(Self { enc })
  }

  /// 编码一帧 960 个 f32 样本（-1.0..1.0），返回 Opus 负载。
  pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>, String> {
    if samples.len() != FRAME_SAMPLES {
      return Err(format!("帧长不符: {} != {FRAME_SAMPLES}", samples.len()));
    }
    let mut out = vec![0u8; 1276];
    let n = self
      .enc
      .encode(samples, FRAME_SAMPLES, &mut out)
      .map_err(|e| format!("编码失败: {e}"))?;
    out.truncate(n);
    Ok(out)
  }
}

/// viewer 端解码参数（写进音频帧头供校验）：
/// 采样率 48000 / 单声道 / 每帧 960 样本（20ms）/ Opus 负载。
pub const HEADER_SAMPLE_RATE: u32 = 48000;
pub const HEADER_CHANNELS: u16 = 1;

#[cfg(test)]
mod tests {
  use super::*;

  /// 编码往返：960 样本帧必须产出非空 Opus 包
  #[test]
  fn encodes_one_frame() {
    let mut enc = OpusEncoder::new().unwrap();
    let frame = vec![0.0f32; FRAME_SAMPLES];
    let packet = enc.encode(&frame).unwrap();
    assert!(!packet.is_empty());
    assert!(packet.len() <= 1276);
  }
}
