//! Video download + thumbnail (port of DoubaoManager `video/downloader.py`).
//! The video and its cover are both plain HTTP fetches: the app runs on the
//! free kernel where page script execution is refused, so anything that used to
//! render a frame inside the browser has to come from the network instead.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DOWNLOAD_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// `<sanitized prompt>_<vid|task-id prefix>.mp4`, matching the original naming.
pub fn task_filename(prompt: &str, vid: Option<&str>, task_id: &str) -> String {
  let stem: String = (if prompt.trim().is_empty() {
    "video".to_string()
  } else {
    prompt.trim().to_string()
  })
  .chars()
  .map(|c| {
    if matches!(
      c,
      '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\r' | '\n'
    ) {
      '_'
    } else {
      c
    }
  })
  .take(40)
  .collect();
  let stem = if stem.is_empty() {
    "video".to_string()
  } else {
    stem
  };
  let tail: String = vid
    .filter(|v| !v.is_empty())
    .unwrap_or(task_id)
    .chars()
    .take(12)
    .collect();
  format!("{stem}_{tail}.mp4")
}

/// 刚出片时豆包 CDN 可能还没把对象发布完（403/404），隔十几秒重试就能拿到，
/// 所以自动下载不能一次失败就放弃。
const DOWNLOAD_ATTEMPTS: u32 = 3;

/// reqwest 的 `timeout` 是整个请求（含收流）的总时限，不是空闲时限：
/// 一条几百 KB/s 的链路下 2 分钟连 3MB 都收不完，会在中途把请求掐死并留下半截文件。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn download_video(url: &str, target: &Path) -> Result<(), String> {
  let mut last_error = String::from("下载未执行");
  for attempt in 1..=DOWNLOAD_ATTEMPTS {
    if attempt > 1 {
      tokio::time::sleep(Duration::from_secs(if attempt == 2 { 15 } else { 45 })).await;
    }
    match download_once(url, target).await {
      Ok(()) => return Ok(()),
      Err(e) => {
        // 半截文件留在目录里会被当成成片打开；重试也直接从干净状态开始
        let _ = std::fs::remove_file(target);
        crate::doubao_executor::debug_log(&format!(
          "下载视频第 {attempt}/{DOWNLOAD_ATTEMPTS} 次失败: {e}"
        ));
        last_error = e;
      }
    }
  }
  Err(last_error)
}

async fn download_once(url: &str, target: &Path) -> Result<(), String> {
  let client = reqwest::Client::builder()
    .user_agent(DOWNLOAD_UA)
    .timeout(DOWNLOAD_TIMEOUT)
    .build()
    .map_err(|e| format!("创建下载客户端失败: {e}"))?;
  let mut response = client
    .get(url)
    .send()
    .await
    .map_err(|e| format!("下载视频失败: {e}"))?;
  // 不带入整条签名链接：它是几百字符的 URL，会淹没提示，且本身是敏感凭证
  let status = response.status();
  if !status.is_success() {
    return Err(format!("豆包返回 {status}，链接可能已失效或尚未就绪"));
  }
  let expected = response.content_length();
  let mut file =
    std::fs::File::create(target).map_err(|e| format!("创建文件失败 {}: {e}", target.display()))?;
  let mut written = 0u64;
  while let Some(chunk) = response
    .chunk()
    .await
    .map_err(|e| format!("下载视频流中断: {e}"))?
  {
    written += chunk.len() as u64;
    file
      .write_all(&chunk)
      .map_err(|e| format!("写入视频文件失败: {e}"))?;
  }
  file.flush().map_err(|e| format!("写入视频文件失败: {e}"))?;
  if written == 0 {
    return Err("下载内容为空".to_string());
  }
  if let Some(expected) = expected.filter(|n| *n > 0) {
    if written != expected {
      return Err(format!("下载不完整: {written}/{expected} 字节"));
    }
  }
  Ok(())
}

/// Grab the cover Doubao publishes for the finished video. Pure HTTP: the
/// previous "render a frame in a browser tab" approach needs `Runtime.evaluate`,
/// which the free kernel refuses, and it left the tab open behind it.
async fn fetch_cover(url: &str, dir: &Path, stem: &str) -> Option<PathBuf> {
  let client = reqwest::Client::builder()
    .user_agent(DOWNLOAD_UA)
    .timeout(Duration::from_secs(60))
    .build()
    .ok()?;
  let response = client.get(url).send().await.ok()?;
  let content_type = response
    .headers()
    .get(reqwest::header::CONTENT_TYPE)
    .and_then(|v| v.to_str().ok())
    .unwrap_or_default()
    .to_ascii_lowercase();
  let bytes = response.bytes().await.ok()?;
  if bytes.is_empty() {
    return None;
  }
  let ext = if content_type.contains("jpeg") {
    "jpg"
  } else if content_type.contains("webp") {
    "webp"
  } else if content_type.contains("gif") {
    "gif"
  } else {
    "png"
  };
  let target = dir.join(format!("{stem}.{ext}"));
  std::fs::write(&target, &bytes).ok()?;
  Some(target)
}

fn candidate_download_urls(task: &crate::doubao_manager::DoubaoVideoTask) -> Vec<&str> {
  let mut urls = Vec::new();
  for url in [
    task.result_url.as_deref(),
    task.backup_result_url.as_deref(),
    task.fallback_result_url.as_deref(),
  ]
  .into_iter()
  .flatten()
  .filter(|url| !url.trim().is_empty())
  {
    if !urls.contains(&url) {
      urls.push(url);
    }
  }
  urls
}

/// Download one task's video into `<download_dir>/video`, plus its cover as the
/// card thumbnail. Shared by the auto-download after success and the manual
/// re-download command.
pub async fn download_task_video(
  task: &crate::doubao_manager::DoubaoVideoTask,
) -> Result<crate::doubao_manager::DoubaoVideoTask, String> {
  let urls = candidate_download_urls(task);
  if urls.is_empty() {
    return Err("没有可下载的视频链接".to_string());
  }
  let settings = crate::doubao_manager::get_settings();
  let root = match settings.download_dir.as_deref().filter(|d| !d.is_empty()) {
    Some(dir) => PathBuf::from(dir),
    None => crate::app_dirs::doubao_dir().join("downloads"),
  };
  let dir = root.join("video");
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败: {e}"))?;
  let target = dir.join(task_filename(&task.prompt, task.vid.as_deref(), &task.id));
  let mut errors = Vec::new();
  for (index, url) in urls.iter().enumerate() {
    match download_video(url, &target).await {
      Ok(()) => break,
      Err(error) => {
        crate::doubao_executor::debug_log(&format!(
          "视频第 {}/{} 条下载链失败：{error}",
          index + 1,
          urls.len()
        ));
        errors.push(format!("第 {} 条链：{error}", index + 1));
      }
    }
    if index + 1 == urls.len() {
      return Err(format!("所有视频下载链均失败（{}）", errors.join("；")));
    }
  }
  let stem = target
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or("video")
    .to_string();
  let mut thumb_path = None;
  if let Some(cover) = task.cover_url.as_deref().filter(|u| !u.is_empty()) {
    thumb_path = fetch_cover(cover, &dir, &stem)
      .await
      .map(|path| path.to_string_lossy().into_owned());
    if thumb_path.is_none() {
      crate::doubao_executor::debug_log("封面图取回失败，卡片将显示占位图");
    }
  }
  let local_file = target.to_string_lossy().into_owned();
  let task = crate::doubao_manager::update_task(&task.id, |t| {
    t.local_file = Some(local_file.clone());
    t.local_thumb = thumb_path.clone();
    // 成功后收起「自动下载失败」，否则一行里同时挂着失败原因和已保存路径
    t.error_message = None;
  })
  .ok_or_else(|| "任务已被删除".to_string())?;
  Ok(task)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_task_filename_sanitizes_and_truncates() {
    let name = task_filename(
      "一只 猫：在/跳舞\r\n很可爱",
      Some("1234567890abcdef"),
      "task",
    );
    assert_eq!(name, "一只 猫：在_跳舞__很可爱_1234567890ab.mp4");
  }

  #[test]
  fn test_task_filename_falls_back() {
    assert_eq!(task_filename("", None, "tid"), "video_tid.mp4");
    assert_eq!(task_filename("   ", Some("v1"), "tid"), "video_v1.mp4");
  }

  #[test]
  fn test_candidate_download_urls_preserve_priority_and_skip_duplicates() {
    let task = crate::doubao_manager::DoubaoVideoTask {
      result_url: Some("https://example.com/main".into()),
      backup_result_url: Some("https://example.com/backup".into()),
      fallback_result_url: Some("https://example.com/main".into()),
      ..Default::default()
    };
    assert_eq!(
      candidate_download_urls(&task),
      vec!["https://example.com/main", "https://example.com/backup"]
    );
  }
}
