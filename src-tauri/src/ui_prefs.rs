//! UI 偏好本地存取（列显隐、文字大小等纯前端设置）。
//!
//! 存本地 JSON 文件而不是 webview localStorage：指纹浏览器场景下 webview
//! 数据可能被缓存清理/重装/换 profile 清掉（用户反馈"列设置重新登录就丢"），
//! settings_dir 下的文件不受影响，真正的"保存到本地"。
//!
//! key 仅允许字母数字下划线（防止路径穿越）；value 为任意 JSON 字符串原样落盘。

use std::path::PathBuf;

fn pref_path(key: &str) -> Result<PathBuf, String> {
  if key.is_empty()
    || !key
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
  {
    return Err(format!("非法的偏好 key: {key}"));
  }
  Ok(crate::app_dirs::settings_dir().join(format!("ui_pref_{key}.json")))
}

/// 保存一个 UI 偏好（value 为任意 JSON 字符串，原样落盘）。
#[tauri::command]
pub fn ui_pref_save(key: String, value: String) -> Result<(), String> {
  let path = pref_path(&key)?;
  let dir = crate::app_dirs::settings_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
  std::fs::write(&path, value).map_err(|e| format!("写入偏好失败: {e}"))
}

/// 读取一个 UI 偏好；文件不存在或不可读时返回 None（调用方用默认值）。
#[tauri::command]
pub fn ui_pref_load(key: String) -> Result<Option<String>, String> {
  let path = pref_path(&key)?;
  match std::fs::read_to_string(&path) {
    Ok(s) => Ok(Some(s)),
    Err(_) => Ok(None),
  }
}
