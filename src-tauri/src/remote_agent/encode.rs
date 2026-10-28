//! BGRA → JPEG（image crate，兼容旧协议）/ BGRA → H264（openh264，Annex B）

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::RgbImage;

/// 把 32 位 BGRA 像素缓冲编码为 JPEG（质量 10-100）。
/// scale < 1 时先降采样再编码（网络差时压缩数据量的主手段），
/// 返回 (实际宽, 实际高, JPEG) —— 降采样后尺寸与输入不同。
pub fn bgra_to_jpeg(
  bgra: &[u8],
  w: u32,
  h: u32,
  quality: u8,
  scale: f32,
) -> Result<(u32, u32, Vec<u8>), String> {
  let expect = (w * h * 4) as usize;
  if bgra.len() != expect {
    return Err(format!("像素缓冲长度不符: {} != {}", bgra.len(), expect));
  }
  let (tw, th) = if (0.0..0.999).contains(&scale) {
    (
      ((w as f32 * scale) as u32).max(1),
      ((h as f32 * scale) as u32).max(1),
    )
  } else {
    (w, h)
  };
  let rgb = bgra_to_rgb(bgra);
  let img = RgbImage::from_raw(w, h, rgb).ok_or_else(|| "图像尺寸非法".to_string())?;
  let resized = if tw != w || th != h {
    image::imageops::resize(&img, tw, th, FilterType::Triangle)
  } else {
    img
  };
  let mut out = Vec::with_capacity((tw * th * 3 / 2) as usize);
  let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
  enc.encode_image(&resized).map_err(|e| e.to_string())?;
  Ok((tw, th, out))
}

/// BGRA 字节序 → RGB
fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
  let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
  for px in bgra.as_chunks::<4>().0 {
    rgb.push(px[2]);
    rgb.push(px[1]);
    rgb.push(px[0]);
  }
  rgb
}

// ==================== H264（openh264 软编，多线程） ====================

use std::sync::Mutex;

use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod};
use openh264::formats::YUVBuffer;

/// 全局编码器：同一被控端同一时刻只有一个 viewer 在推流。
/// 尺寸/缩放变化时重建（H264 流尺寸固定）。
static H264_ENC: Mutex<Option<H264State>> = Mutex::new(None);

struct H264State {
  enc: Encoder,
  w: u32,
  h: u32,
}

/// BGRA 直接缩放 + 转 I420（单遍完成，不经过 RGB 中间层）。
/// 返回 (tw, th, yuv)，尺寸对齐偶数（I420 要求）。
fn bgra_to_i420_scaled(bgra: &[u8], w: u32, h: u32, scale: f32) -> (u32, u32, Vec<u8>) {
  let (tw, th) = if (0.0..0.999).contains(&scale) {
    let mut tw = ((w as f32 * scale) as u32).max(1);
    let mut th = ((h as f32 * scale) as u32).max(1);
    tw -= tw % 2;
    th -= th % 2;
    (tw, th)
  } else {
    (w, h)
  };
  let (yw, yh) = (tw as usize, th as usize);
  let (cw, ch) = (yw / 2, yh / 2);
  let mut yuv = vec![0u8; yw * yh + 2 * cw * ch];
  let (y, rest) = yuv.split_at_mut(yw * yh);
  let (u, v) = rest.split_at_mut(cw * ch);
  let sx = (w as f32) / (tw as f32);
  let sy = (h as f32) / (th as f32);
  let wu = w as usize;
  for row in 0..yh {
    let src_row = ((row as f32 * sy) as usize).min(h as usize - 1);
    for col in 0..yw {
      let src_col = ((col as f32 * sx) as usize).min(wu - 1);
      let i = (src_row * wu + src_col) * 4;
      let (b, g, r) = (bgra[i] as i32, bgra[i + 1] as i32, bgra[i + 2] as i32);
      // BT.601 limited range
      y[row * yw + col] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
      if row % 2 == 0 && col % 2 == 0 {
        let ci = (row / 2) * cw + col / 2;
        u[ci] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        v[ci] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
      }
    }
  }
  (tw, th, yuv)
}

/// 编码一帧 H264（Annex B）。scale 变化自动重建编码器；
/// force_key 时下一帧强制 IDR（viewer 新接入/请求关键帧用）。
/// 返回 (tw, th, bitstream)；失败时调用方回退 JPEG。
pub fn encode_h264(
  bgra: &[u8],
  w: u32,
  h: u32,
  scale: f32,
  force_key: bool,
) -> Result<(u32, u32, Vec<u8>), String> {
  let (tw, th, yuv) = bgra_to_i420_scaled(bgra, w, h, scale);
  let mut guard = H264_ENC.lock().map_err(|_| "H264 编码器锁中毒")?;
  let recreate = !matches!(&*guard, Some(st) if st.w == tw && st.h == th);
  if recreate {
    // 码率按分辨率给：约 0.1 bit/px/帧 @30fps，上限 8Mbps
    let kbps = (((tw as u64) * (th as u64) * 30 / 10) / 1000).clamp(800, 8000) as u32;
    let cfg = EncoderConfig::new()
      .bitrate(BitRate::from_bps(kbps * 1000))
      .max_frame_rate(FrameRate::from_hz(30.0))
      .intra_frame_period(IntraFramePeriod::from_num_frames(240))
      .num_threads(4);
    let enc = Encoder::with_api_config(openh264::OpenH264API::from_source(), cfg)
      .map_err(|e| format!("openh264 初始化失败: {e}"))?;
    *guard = Some(H264State { enc, w: tw, h: th });
  }
  let state = guard.as_mut().expect("刚创建");
  if force_key {
    state.enc.force_intra_frame();
  }
  let frame = YUVBuffer::from_vec(yuv, tw as usize, th as usize);
  let bitstream = state
    .enc
    .encode(&frame)
    .map_err(|e| format!("openh264 编码失败: {e}"))?
    .to_vec();
  Ok((tw, th, bitstream))
}

#[cfg(test)]
mod h264_tests {
  use super::*;
  #[test]
  fn h264_annexb_smoke() {
    let w = 320u32;
    let h = 240u32;
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    for px in bgra.as_chunks_mut::<4>().0 {
      px[0] = 255;
      px[1] = 0;
      px[2] = 0;
      px[3] = 255;
    }
    let (tw, th, out1) = encode_h264(&bgra, w, h, 1.0, true).expect("enc1");
    println!(
      "frame1: {}x{} bytes={} head={:02x?}",
      tw,
      th,
      out1.len(),
      &out1[..4]
    );
    for px in bgra.as_chunks_mut::<4>().0 {
      px[1] = 255;
      px[2] = 0;
    }
    let (_, _, out2) = encode_h264(&bgra, w, h, 1.0, false).expect("enc2");
    println!("frame2: bytes={}", out2.len());
    let path = std::env::temp_dir().join("h264_test.h264");
    std::fs::write(&path, [&out1[..], &out2[..]].concat()).unwrap();
    println!("written to {:?}", path);
    assert!(out1.len() > 10);
  }
}
