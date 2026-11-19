//! 密码管理器云端同步（账号级，语义同书签同步）。
//!
//! Chromium 原生密码管理器的内容存在 `<user-data-dir>/Default/Login Data`
//! SQLite 的 `logins` 表里，`password_value` 用 os_crypt 密钥封成
//! `v10/v11 || nonce || ciphertext`。密钥来源按内核分两种：
//!
//! 1. Wayfern 内核：便携密钥文件 `<user-data-dir>/os_crypt_key`
//!    （Windows 32 原始字节 = AES-256-GCM key；mac/Linux 是 PBKDF2 密码）。
//!    文件本身就是可移植的，解密/加密都不依赖系统钥匙环。
//! 2. 标准 Chromium 内核：`Local State` 的 `os_crypt.encrypted_key`
//!    （DPAPI 包裹）。只在 Windows 上做这一路兜底。
//!
//! 上行（浏览器退出后导出明文 JSON 上传云端）挂在
//! `watch_account_browser_close`；下行（启动前本地无保存密码时注入）挂在
//! `launch_account`。云端按 (user_id, account_id) 分账号存储。
//!
//! 复用 profile_import 的 os_crypt 原语，不重复实现加密。

use crate::profile_import::os_crypt::{CryptoKey, Decrypted, SourceKeyring};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// 一条保存密码（明文传输形态，仅存在于内存与云端 JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordEntry {
  #[serde(default)]
  pub origin_url: String,
  #[serde(default)]
  pub action_url: String,
  #[serde(default)]
  pub username_element: String,
  #[serde(default)]
  pub username_value: String,
  #[serde(default)]
  pub password_element: String,
  #[serde(default)]
  pub password: String,
  #[serde(default)]
  pub submit_element: String,
  #[serde(default)]
  pub signon_realm: String,
  #[serde(default)]
  pub scheme: i64,
  #[serde(default)]
  pub password_type: i64,
  #[serde(default)]
  pub times_used: i64,
  #[serde(default)]
  pub date_created: i64,
  #[serde(default)]
  pub date_last_used: i64,
  #[serde(default)]
  pub date_password_modified: i64,
  #[serde(default)]
  pub display_name: String,
  #[serde(default)]
  pub icon_url: String,
  #[serde(default)]
  pub federation_url: String,
  #[serde(default)]
  pub skip_zero_click: i64,
}

// ---------- 密钥 ----------

/// 便携密钥（Wayfern 内核）：读 `os_crypt_key` 文件，按平台组 CryptoKey。
fn portable_key(user_data_dir: &Path) -> Option<CryptoKey> {
  let raw =
    std::fs::read(user_data_dir.join(crate::profile_import::os_crypt::KEY_FILE_NAME)).ok()?;
  if raw.is_empty() {
    return None;
  }
  #[cfg(target_os = "windows")]
  {
    let bytes: [u8; 32] = raw.try_into().ok()?;
    Some(CryptoKey::Aes256Gcm(bytes))
  }
  #[cfg(target_os = "macos")]
  {
    Some(CryptoKey::Aes128Cbc(
      crate::profile_import::os_crypt::derive_key(
        &raw,
        crate::profile_import::os_crypt::MAC_ITERATIONS,
      ),
    ))
  }
  #[cfg(target_os = "linux")]
  {
    Some(CryptoKey::Aes128Cbc(
      crate::profile_import::os_crypt::derive_key(
        &raw,
        crate::profile_import::os_crypt::POSIX_ITERATIONS,
      ),
    ))
  }
}

/// 标准 Chromium 内核兜底（仅 Windows）：Local State 的 DPAPI 包裹密钥。
#[cfg(target_os = "windows")]
fn local_state_key(user_data_dir: &Path) -> Option<CryptoKey> {
  crate::profile_import::keyring::windows_local_state_key(user_data_dir)
    .ok()
    .flatten()
    .map(CryptoKey::Aes256Gcm)
}

/// 解密用的钥匙环：便携密钥优先，否则 Local State。
fn decrypt_keyring(user_data_dir: &Path) -> SourceKeyring {
  let mut ring = SourceKeyring::default();
  if let Some(key) = portable_key(user_data_dir) {
    ring.v10 = Some(key.clone());
    ring.v11 = Some(key);
  } else {
    #[cfg(target_os = "windows")]
    if let Some(key) = local_state_key(user_data_dir) {
      ring.v10 = Some(key.clone());
      ring.v11 = Some(key);
    }
  }
  ring
}

/// 加密用的密钥 + 版本标签：本地写回时用与本地内核一致的封法。
fn encrypt_key(user_data_dir: &Path) -> Option<(CryptoKey, &'static [u8; 3])> {
  if let Some(key) = portable_key(user_data_dir) {
    return Some((key, crate::profile_import::os_crypt::TargetKey::host_tag()));
  }
  #[cfg(target_os = "windows")]
  if let Some(key) = local_state_key(user_data_dir) {
    return Some((key, b"v10"));
  }
  None
}

/// 按 Chromium 的方式封一个值：`tag || nonce || ciphertext`。
fn seal(key: &CryptoKey, tag: &[u8; 3], plaintext: &[u8]) -> Option<Vec<u8>> {
  let body = key.encrypt(plaintext)?;
  let mut out = Vec::with_capacity(3 + body.len());
  out.extend_from_slice(tag);
  out.extend_from_slice(&body);
  Some(out)
}

// ---------- SQLite 访问 ----------

fn login_data_path(user_data_dir: &Path) -> std::path::PathBuf {
  user_data_dir.join("Default").join("Login Data")
}

fn open_login_data(user_data_dir: &Path) -> Option<Connection> {
  let path = login_data_path(user_data_dir);
  if !path.is_file() {
    return None;
  }
  // 读写打开：WAL/-journal 里未合并的数据要在 open 时恢复，否则导出少数据。
  Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).ok()
}

fn table_exists(conn: &Connection, table: &str) -> bool {
  conn
    .query_row(
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
      [table],
      |_| Ok(()),
    )
    .is_ok()
}

/// 密码值可能是 BLOB 也可能是 TEXT（`BindString` 写入的 TEXT 会一直保持
/// TEXT 存储类，BLOB 列无亲和性），两种都要能读。
fn column_bytes(row: &rusqlite::Row<'_>, index: usize) -> Vec<u8> {
  row
    .get_ref(index)
    .ok()
    .and_then(|value| value.as_bytes().ok())
    .map(<[u8]>::to_vec)
    .unwrap_or_default()
}

fn opt_string(row: &rusqlite::Row<'_>, index: usize) -> String {
  row
    .get::<_, Option<String>>(index)
    .ok()
    .flatten()
    .unwrap_or_default()
}

fn opt_int(row: &rusqlite::Row<'_>, index: usize) -> i64 {
  row.get::<_, Option<i64>>(index).ok().flatten().unwrap_or(0)
}

// ---------- 导出（上行） ----------

/// logins 一行的原始内容（密码值还是密文）。
struct RawRow {
  origin_url: String,
  action_url: String,
  username_element: String,
  username_value: String,
  password_element: String,
  stored: Vec<u8>,
  submit_element: String,
  signon_realm: String,
  scheme: i64,
  password_type: i64,
  times_used: i64,
  date_created: i64,
  date_last_used: i64,
  date_password_modified: i64,
  display_name: String,
  icon_url: String,
  federation_url: String,
  skip_zero_click: i64,
}

/// 导出本地密码管理器内容为云端 JSON。
/// 返回 `None` 表示本地没有可上传的内容（Login Data 不存在 / 表不存在 / 0 条）。
/// 解密失败的行跳过并计数，不阻塞其余行。
pub fn export_passwords(user_data_dir: &Path) -> Result<Option<String>, String> {
  let Some(conn) = open_login_data(user_data_dir) else {
    log::info!("password_sync: Login Data 不存在，导出跳过");
    return Ok(None);
  };
  if !table_exists(&conn, "logins") {
    return Ok(None);
  }

  let mut stmt = conn
    .prepare(
      "SELECT origin_url, action_url, username_element, username_value, password_element, \
       password_value, submit_element, signon_realm, scheme, password_type, times_used, \
       date_created, date_last_used, date_password_modified, display_name, icon_url, \
       federation_url, skip_zero_click \
       FROM logins WHERE blacklisted_by_user = 0",
    )
    .map_err(|e| format!("打开 logins 表失败: {e}"))?;

  let rows: Vec<RawRow> = {
    let mapped = stmt
      .query_map([], |row| {
        Ok(RawRow {
          origin_url: opt_string(row, 0),
          action_url: opt_string(row, 1),
          username_element: opt_string(row, 2),
          username_value: opt_string(row, 3),
          password_element: opt_string(row, 4),
          stored: column_bytes(row, 5),
          submit_element: opt_string(row, 6),
          signon_realm: opt_string(row, 7),
          scheme: opt_int(row, 8),
          password_type: opt_int(row, 9),
          times_used: opt_int(row, 10),
          date_created: opt_int(row, 11),
          date_last_used: opt_int(row, 12),
          date_password_modified: opt_int(row, 13),
          display_name: opt_string(row, 14),
          icon_url: opt_string(row, 15),
          federation_url: opt_string(row, 16),
          skip_zero_click: opt_int(row, 17),
        })
      })
      .map_err(|e| format!("读取 logins 行失败: {e}"))?;
    mapped.flatten().collect()
  };
  drop(stmt);

  // 解密在闭包外做：闭包里的错误只能走 rusqlite::Error，不适合承载
  // 「解密失败跳过该行」这类业务语义。
  let keyring = decrypt_keyring(user_data_dir);
  let mut entries: Vec<PasswordEntry> = Vec::new();
  let mut skipped = 0usize;
  for row in rows {
    let password = if row.stored.is_empty() {
      String::new()
    } else {
      match keyring.decrypt(&row.stored) {
        Decrypted::Value(v) => String::from_utf8_lossy(&v).into_owned(),
        Decrypted::NotEncrypted => String::from_utf8_lossy(&row.stored).into_owned(),
        Decrypted::Unrecoverable => {
          skipped += 1;
          continue;
        }
      }
    };
    entries.push(PasswordEntry {
      origin_url: row.origin_url,
      action_url: row.action_url,
      username_element: row.username_element,
      username_value: row.username_value,
      password_element: row.password_element,
      password,
      submit_element: row.submit_element,
      signon_realm: row.signon_realm,
      scheme: row.scheme,
      password_type: row.password_type,
      times_used: row.times_used,
      date_created: row.date_created,
      date_last_used: row.date_last_used,
      date_password_modified: row.date_password_modified,
      display_name: row.display_name,
      icon_url: row.icon_url,
      federation_url: row.federation_url,
      skip_zero_click: row.skip_zero_click,
    });
  }

  if skipped > 0 {
    log::warn!("password_sync: {skipped} 条保存密码解密失败，已跳过（不会上传）");
  }
  if entries.is_empty() {
    return Ok(None);
  }
  let json = serde_json::to_string(&entries).map_err(|e| format!("序列化密码数据失败: {e}"))?;
  Ok(Some(json))
}

// ---------- 计数 / 导入（下行） ----------

/// 本地保存密码条数（不含「永不保存」黑名单行）。Login Data 不存在算 0。
pub fn count_local_logins(user_data_dir: &Path) -> usize {
  let Some(conn) = open_login_data(user_data_dir) else {
    return 0;
  };
  if !table_exists(&conn, "logins") {
    return 0;
  }
  conn
    .query_row(
      "SELECT count(*) FROM logins WHERE blacklisted_by_user = 0",
      [],
      |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
    .max(0) as usize
}

/// 把云端 JSON 注入本地 Login Data。
/// 以 (origin_url, username_value) 为去重键，已存在的行跳过；
/// 返回 (注入条数, 本地已存在跳过条数)。
/// Login Data 尚未生成（浏览器还没跑过第一次）时返回 Ok((0, 0))，
/// 下一次启动会再注入。
pub fn import_passwords(user_data_dir: &Path, json: &str) -> Result<(usize, usize), String> {
  let Some(conn) = open_login_data(user_data_dir) else {
    log::info!("password_sync: Login Data 尚未生成（首次启动前），本次注入跳过");
    return Ok((0, 0));
  };
  if !table_exists(&conn, "logins") {
    return Ok((0, 0));
  }

  let entries: Vec<PasswordEntry> =
    serde_json::from_str(json).map_err(|e| format!("解析云端密码数据失败: {e}"))?;
  if entries.is_empty() {
    return Ok((0, 0));
  }

  let Some((key, tag)) = encrypt_key(user_data_dir) else {
    return Err(
      "无法确定 os_crypt 密钥（本地内核尚未生成密钥），本次注入跳过，下次启动重试".to_string(),
    );
  };

  let mut existing: HashSet<(String, String)> = HashSet::new();
  {
    let mut stmt = conn
      .prepare("SELECT origin_url, username_value FROM logins")
      .map_err(|e| format!("读取本地 logins 失败: {e}"))?;
    let mapped = stmt
      .query_map([], |row| {
        Ok((
          row.get::<_, Option<String>>(0)?.unwrap_or_default(),
          row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
      })
      .map_err(|e| format!("读取本地 logins 失败: {e}"))?;
    for pair in mapped {
      let pair = pair.map_err(|e| format!("读取本地 logins 行失败: {e}"))?;
      existing.insert(pair);
    }
  }

  let mut imported = 0usize;
  let mut skipped = 0usize;
  for entry in entries {
    let dedup = (entry.origin_url.clone(), entry.username_value.clone());
    if existing.contains(&dedup) {
      skipped += 1;
      continue;
    }
    let sealed =
      seal(&key, tag, entry.password.as_bytes()).ok_or_else(|| "密码加密失败".to_string())?;
    let result = conn.execute(
      "INSERT INTO logins \
       (origin_url, action_url, username_element, username_value, password_element, \
        password_value, submit_element, signon_realm, date_created, blacklisted_by_user, \
        scheme, password_type, times_used, date_last_used, date_password_modified, \
        display_name, icon_url, federation_url, skip_zero_click) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
      rusqlite::params![
        entry.origin_url,
        entry.action_url,
        entry.username_element,
        entry.username_value,
        entry.password_element,
        sealed,
        entry.submit_element,
        entry.signon_realm,
        entry.date_created,
        entry.scheme,
        entry.password_type,
        entry.times_used,
        entry.date_last_used,
        entry.date_password_modified,
        entry.display_name,
        entry.icon_url,
        entry.federation_url,
        entry.skip_zero_click,
      ],
    );
    if result.is_ok() {
      existing.insert(dedup);
      imported += 1;
    } else {
      skipped += 1;
    }
  }

  Ok((imported, skipped))
}

#[cfg(test)]
#[path = "password_sync_tests.rs"]
mod tests;
