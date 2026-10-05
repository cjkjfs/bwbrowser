//! BGRA → JPEG 编码（image crate，与 server.py Pillow JPEG 兼容）

use image::codecs::jpeg::JpegEncoder;
use image::RgbImage;

/// 把 32 位 BGRA 像素缓冲编码为 JPEG（质量 10-100）
pub fn bgra_to_jpeg(bgra: &[u8], w: u32, h: u32, quality: u8) -> Result<Vec<u8>, String> {
  let expect = (w * h * 4) as usize;
  if bgra.len() != expect {
    return Err(format!("像素缓冲长度不符: {} != {}", bgra.len(), expect));
  }
  let mut rgb = Vec::with_capacity((w * h * 3) as usize);
  for px in bgra.as_chunks::<4>().0 {
    rgb.push(px[2]);
    rgb.push(px[1]);
    rgb.push(px[0]);
  }
  let img = RgbImage::from_raw(w, h, rgb).ok_or_else(|| "图像尺寸非法".to_string())?;
  let mut out = Vec::with_capacity(expect / 2);
  let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
  enc.encode_image(&img).map_err(|e| e.to_string())?;
  Ok(out)
}
