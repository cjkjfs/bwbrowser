function base32Decode(secret: string): Uint8Array {
  const cleaned = secret.replace(/[\s=-]/g, "").toUpperCase();
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0;
  let value = 0;
  const output: number[] = [];
  for (const c of cleaned) {
    const idx = alphabet.indexOf(c);
    if (idx === -1) continue;
    value = (value << 5) | idx;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      output.push((value >> bits) & 0xff);
    }
  }
  return new Uint8Array(output);
}

async function generateHmacSHA1(
  key: Uint8Array,
  message: Uint8Array,
): Promise<Uint8Array> {
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    key.buffer as ArrayBuffer,
    { name: "HMAC", hash: "SHA-1" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign(
    "HMAC",
    cryptoKey,
    message.buffer as ArrayBuffer,
  );
  return new Uint8Array(sig);
}

export async function generateTOTP(
  secret: string,
  period = 30,
  digits = 6,
): Promise<string> {
  const key = base32Decode(secret);
  const counter = Math.floor(Date.now() / 1000 / period);
  const counterBytes = new Uint8Array(8);
  let c = counter;
  for (let i = 7; i >= 0; i--) {
    counterBytes[i] = c & 0xff;
    c >>= 8;
  }
  const hmac = await generateHmacSHA1(key, counterBytes);
  const offset = hmac[hmac.length - 1] & 0x0f;
  const binary =
    ((hmac[offset] & 0x7f) << 24) |
    ((hmac[offset + 1] & 0xff) << 16) |
    ((hmac[offset + 2] & 0xff) << 8) |
    (hmac[offset + 3] & 0xff);
  const code = binary % 10 ** digits;
  return code.toString().padStart(digits, "0");
}

export function extractSecretFromUrl(url: string): string | null {
  try {
    const parsed = new URL(url);
    const secret = parsed.searchParams.get("secret");
    if (secret) return secret;
  } catch {
    // not a valid URL
  }
  if (/^[A-Z2-7]+=*$/i.test(url.trim())) {
    return url.trim();
  }
  return null;
}
