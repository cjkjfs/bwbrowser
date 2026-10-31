//! 本地 TOTP（RFC 6238，SHA-1 / 30 秒窗口，6 位）生成。
//! 密钥来自云端账号 safe_link 里的 Google Authenticator base32 串，
//! 供登录自动填表在 2FA 步骤输入当前验证码。

use hmac::{Hmac, Mac};
type HmacSha1 = Hmac<sha1::Sha1>;

/// RFC 4648 base32 解码（忽略空格与填充 =）。非字母表字符返回 None。
fn base32_decode(input: &str) -> Option<Vec<u8>> {
  const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let mut acc = 0u32;
  let mut bits = 0u32;
  let mut out = Vec::new();
  for ch in input.to_uppercase().chars() {
    if ch == '=' || ch == ' ' {
      continue;
    }
    let v = ALPHABET.iter().position(|&c| c as char == ch)? as u32;
    acc = (acc << 5) | v;
    bits += 5;
    if bits >= 8 {
      bits -= 8;
      out.push((acc >> bits) as u8);
    }
  }
  Some(out)
}

/// 由 base32 密钥生成当前 30 秒窗口的 6 位验证码。密钥非法返回 None。
pub fn generate_totp(secret: &str) -> Option<String> {
  let key = base32_decode(secret)?;
  if key.is_empty() {
    return None;
  }
  let window = crate::proxy_manager::now_secs() / 30;
  let mut mac = HmacSha1::new_from_slice(&key).ok()?;
  mac.update(&window.to_be_bytes());
  let digest = mac.finalize().into_bytes();
  let offset = (digest[19] & 0x0f) as usize;
  let binary = ((digest[offset] as u32 & 0x7f) << 24)
    | ((digest[offset + 1] as u32) << 16)
    | ((digest[offset + 2] as u32) << 8)
    | (digest[offset + 3] as u32);
  Some(format!("{:06}", binary % 1_000_000))
}
