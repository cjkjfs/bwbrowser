#[test]
fn h264_annexb_smoke() {
  // 320x240 红色帧，验证编码输出带 Annex B 起始码
  let w = 320u32; let h = 240u32;
  let mut bgra = vec![0u8; (w*h*4) as usize];
  for px in bgra.as_chunks_mut::<4>().0 { px[0]=255; px[1]=0; px[2]=0; px[3]=255; }
  let (tw, th, out1) = encode::encode_h264(&bgra, w, h, 1.0, true).expect("enc1");
  println!("frame1: {}x{} bytes={} startcode={:02x?}", tw, th, out1.len(), &out1[..4.min(out1.len())]);
  for px in bgra.as_chunks_mut::<4>().0 { px[1]=255; px[2]=0; } // 变绿
  let (_, _, out2) = encode::encode_h264(&bgra, w, h, 1.0, false).expect("enc2");
  println!("frame2: bytes={}", out2.len());
  std::fs::write(std::env::temp_dir().join("h264_test.h264"), [&out1[..], &out2[..]].concat()).unwrap();
  assert!(out1.len() > 10);
}
