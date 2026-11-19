//! password_sync 的往返测试：临时目录里手工搭一个最小 profile
//! （os_crypt_key + Default/Login Data），导出 → 注入 → 再导出，验证明文一致。

use super::*;
use rusqlite::Connection;
use std::path::PathBuf;

/// 造一个可用 32 字节 Windows 便携密钥的临时 profile。
/// 返回 root 路径和 TempDir（后者必须存活到测试结束，否则目录被清理）。
fn make_profile_dir() -> (PathBuf, tempfile::TempDir) {
  let dir = tempfile::tempdir().expect("tempdir");
  let root = dir.path().to_path_buf();
  std::fs::create_dir_all(root.join("Default")).unwrap();
  // Windows 需要 32 原始字节；非 Windows 平台是 base64(16 bytes)。
  #[cfg(target_os = "windows")]
  let key_bytes: Vec<u8> = (0u8..32).collect();
  #[cfg(not(target_os = "windows"))]
  let key_bytes = {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
      .encode([7u8; 16])
      .into_bytes()
  };
  std::fs::write(
    root.join(crate::profile_import::os_crypt::KEY_FILE_NAME),
    &key_bytes,
  )
  .unwrap();

  // 最小 logins 表：覆盖真实 schema 中 NOT NULL 无默认值的列即可。
  let conn = Connection::open(root.join("Default").join("Login Data")).unwrap();
  conn
    .execute_batch(
      "CREATE TABLE logins (
        origin_url VARCHAR NOT NULL,
        action_url VARCHAR,
        username_element VARCHAR,
        username_value VARCHAR,
        password_element VARCHAR,
        password_value BLOB,
        submit_element VARCHAR,
        signon_realm VARCHAR NOT NULL,
        date_created INTEGER NOT NULL,
        blacklisted_by_user INTEGER NOT NULL,
        scheme INTEGER NOT NULL,
        password_type INTEGER,
        times_used INTEGER,
        date_last_used INTEGER NOT NULL DEFAULT 0,
        date_password_modified INTEGER NOT NULL DEFAULT 0,
        display_name VARCHAR,
        icon_url VARCHAR,
        federation_url VARCHAR,
        skip_zero_click INTEGER
      );",
    )
    .unwrap();
  (root, dir)
}

#[test]
fn export_seals_and_import_roundtrips() {
  let (root, _dir) = make_profile_dir();

  // 本地插入一条（用真实封法手动写：tag + 加密）。
  let (key, tag) = encrypt_key(&root).expect("portable key should exist");
  let sealed = seal(&key, tag, b"hunter2").expect("seal");
  let conn = Connection::open(login_data_path(&root)).unwrap();
  conn
    .execute(
      "INSERT INTO logins (origin_url, signon_realm, date_created, blacklisted_by_user, scheme, \
       username_value, password_value) VALUES ('https://a.com', 'https://a.com', 1, 0, 0, 'u@a.com', ?1)",
      rusqlite::params![sealed],
    )
    .unwrap();
  drop(conn);

  // 导出应还原明文。
  let json = export_passwords(&root)
    .expect("export ok")
    .expect("non-empty");
  let entries: Vec<PasswordEntry> = serde_json::from_str(&json).unwrap();
  assert_eq!(entries.len(), 1);
  assert_eq!(entries[0].password, "hunter2");
  assert_eq!(entries[0].username_value, "u@a.com");

  // 注入到另一个空 profile（明文 JSON → 重新封密）。
  let (dest, _dest_dir) = make_profile_dir();
  let (imported, skipped) = import_passwords(&dest, &json).unwrap();
  assert_eq!((imported, skipped), (1, 0));

  // 再从目的 profile 导出，明文应一致（不同 os_crypt_key 也能还原）。
  let json2 = export_passwords(&dest).unwrap().unwrap();
  let entries2: Vec<PasswordEntry> = serde_json::from_str(&json2).unwrap();
  assert_eq!(entries2.len(), 1);
  assert_eq!(entries2[0].password, "hunter2");

  // 重复注入同一份数据应全部去重。
  let (imported, skipped) = import_passwords(&dest, &json).unwrap();
  assert_eq!((imported, skipped), (0, 1));
}

#[test]
fn empty_or_missing_login_data_exports_none() {
  // 没有 Login Data。
  let (root, _dir) = make_profile_dir();
  std::fs::remove_file(login_data_path(&root)).unwrap();
  assert!(export_passwords(&root).unwrap().is_none());
  assert_eq!(count_local_logins(&root), 0);
  // 注入到不存在的 Login Data：跳过不报错。
  assert_eq!(import_passwords(&root, "[]").unwrap(), (0, 0));
}
