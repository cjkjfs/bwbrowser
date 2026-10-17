//! BGRA → JPEG 编码（image crate，与 server.py Pillow JPEG 兼容）

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
