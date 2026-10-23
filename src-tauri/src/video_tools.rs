// video_tools.rs - 视频工具：快速合并 / 快速拆分
// 复刻 CutMatch fast_merge.py 的 FastMerger 与 SplitWorker 逻辑

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn ffmpeg_path() -> PathBuf {
  crate::app_dirs::data_dir()
    .join("video-tools")
    .join(if cfg!(windows) {
      "ffmpeg.exe"
    } else {
      "ffmpeg"
    })
}

fn ffprobe_path() -> PathBuf {
  crate::app_dirs::data_dir()
    .join("video-tools")
    .join(if cfg!(windows) {
      "ffprobe.exe"
    } else {
      "ffprobe"
    })
}

/// 视频流信息（用于合并方式自动选择）
#[derive(Debug, Clone)]
struct VideoInfo {
  width: u32,
  height: u32,
  codec: String,
}

/// 用 ffprobe 获取视频流信息
async fn probe_video(path: &str) -> Result<VideoInfo, String> {
  let ffprobe = ffprobe_path();
  if !ffprobe.exists() {
    return Err("ffprobe not found".to_string());
  }
  let output = tokio::process::Command::new(&ffprobe)
    .args([
      "-v",
      "error",
      "-select_streams",
      "v:0",
      "-show_entries",
      "stream=width,height,codec_name",
      "-of",
      "json",
      path,
    ])
    .output()
    .await
    .map_err(|e| e.to_string())?;
  let json: serde_json::Value =
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
  let stream = &json["streams"][0];
  Ok(VideoInfo {
    width: stream["width"].as_u64().unwrap_or(0) as u32,
    height: stream["height"].as_u64().unwrap_or(0) as u32,
    codec: stream["codec_name"]
      .as_str()
      .unwrap_or_default()
      .to_string(),
  })
}

/// 用 ffmpeg 获取视频时长（秒）
async fn video_duration(path: &str) -> Result<f64, String> {
  let ffmpeg = ffmpeg_path();
  if !ffmpeg.exists() {
    return Err("FFmpeg not found".to_string());
  }
  let output = tokio::process::Command::new(&ffmpeg)
    .arg("-i")
    .arg(path)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .output()
    .await
    .map_err(|e| e.to_string())?;
  let info = String::from_utf8_lossy(&output.stderr);
  for line in info.lines() {
    if let Some(dur_str) = line.split("Duration:").nth(1) {
      if let Some(time_str) = dur_str.split(",").next() {
        let parts: Vec<&str> = time_str.trim().split(":").collect();
        if parts.len() == 3 {
          let h: f64 = parts[0].parse().unwrap_or(0.0);
          let m: f64 = parts[1].parse().unwrap_or(0.0);
          let s: f64 = parts[2].parse().unwrap_or(0.0);
          return Ok(h * 3600.0 + m * 60.0 + s);
        }
      }
    }
  }
  Err("无法解析视频时长".to_string())
}

// ============================================================
// 快速合并（FastMerger 移植）
// ============================================================

/// 自动选择合并方式（fast_merge._auto_select_method 移植）
async fn auto_select_method(files: &[String]) -> &'static str {
  // 逐个 probe，参数全部一致 → 无损
  let mut all_same = true;
  let mut first: Option<VideoInfo> = None;
  for f in files {
    match probe_video(f).await {
      Ok(info) => match &first {
        None => first = Some(info),
        Some(f0) => {
          if info.width != f0.width || info.height != f0.height || info.codec != f0.codec {
            all_same = false;
            break;
          }
        }
      },
      Err(_) => {
        all_same = false;
        break;
      }
    }
  }
  if all_same {
    "lossless"
  } else {
    "reencode"
  }
}

/// 无损合并：concat demuxer + stream copy
async fn merge_lossless(files: &[String], output: &str) -> Result<(), String> {
  // 写 concat 列表文件（单引号转义）
  let list_file = std::env::temp_dir().join(format!(
    "bwbrowser_merge_{}.txt",
    chrono::Utc::now().timestamp_millis()
  ));
  let mut list = String::new();
  for f in files {
    let abs = std::fs::canonicalize(f)
      .map(|p| p.to_string_lossy().to_string())
      .unwrap_or_else(|_| f.clone());
    let abs = abs.replace('\\', "/");
    let escaped = abs.replace('\'', "'\\''");
    list.push_str(&format!("file '{}'\n", escaped));
  }
  std::fs::write(&list_file, list).map_err(|e| e.to_string())?;

  let ffmpeg = ffmpeg_path();
  let result = tokio::process::Command::new(&ffmpeg)
    .args(["-y", "-f", "concat", "-safe", "0"])
    .arg("-i")
    .arg(&list_file)
    .args(["-c", "copy", "-movflags", "+faststart"])
    .arg(output)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .output()
    .await
    .map_err(|e| e.to_string())?;
  let _ = std::fs::remove_file(&list_file);

  if !result.status.success() {
    let err = String::from_utf8_lossy(&result.stderr);
    return Err(format!("无损合并失败: {}", &err[..err.len().min(400)]));
  }
  Ok(())
}

/// 重编码合并：concat filter（fast_merge._merge_concat_filter 移植）
async fn merge_reencode(files: &[String], output: &str) -> Result<(), String> {
  let ffmpeg = ffmpeg_path();
  let mut cmd = tokio::process::Command::new(&ffmpeg);
  cmd.arg("-y");
  for f in files {
    cmd.arg("-i").arg(f);
  }
  let n = files.len();
  let mut filter = String::new();
  for i in 0..n {
    filter.push_str(&format!("[{}:v:0][{}:a:0]", i, i));
  }
  filter.push_str(&format!("concat=n={}:v=1:a=1[v][a]", n));
  cmd
    .args(["-filter_complex", &filter, "-map", "[v]", "-map", "[a]"])
    .arg(output)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  let result = cmd.output().await.map_err(|e| e.to_string())?;
  if !result.status.success() {
    let err = String::from_utf8_lossy(&result.stderr);
    return Err(format!("重编码合并失败: {}", &err[..err.len().min(400)]));
  }
  Ok(())
}

/// Tauri 命令：快速合并（进度事件 video-tools-progress，stage=merge）
#[tauri::command]
pub async fn video_tools_merge(
  app: tauri::AppHandle,
  files: Vec<String>,
  output_path: String,
  method: Option<String>,
) -> Result<String, String> {
  use tauri::Emitter;

  if files.len() < 2 {
    return Err("至少需要 2 个视频文件才能合并".to_string());
  }
  for f in &files {
    if !Path::new(f).exists() {
      return Err(format!("文件不存在: {f}"));
    }
  }
  if Path::new(&output_path).exists() {
    return Err(format!("输出文件已存在: {output_path}"));
  }

  let emit = |pct: u32, msg: &str| {
    let _ = app.emit(
      "video-tools-progress",
      serde_json::json!({ "stage": "merge", "percent": pct, "message": msg }),
    );
  };

  emit(5, "分析视频参数...");

  // 解析方式（fast_merge.merge 移植）
  let resolved = match method.as_deref() {
    Some("lossless") => "lossless",
    Some("reencode") => "reencode",
    Some("auto") | None => {
      if output_path.to_lowercase().ends_with(".mkv") {
        // 无 mkvmerge 依赖，退回参数判断
        auto_select_method(&files).await
      } else {
        auto_select_method(&files).await
      }
    }
    Some(other) => return Err(format!("未知合并方式: {other}")),
  };
  emit(
    15,
    if resolved == "lossless" {
      "无损合并中..."
    } else {
      "重编码合并中（较慢）..."
    },
  );

  let result = if resolved == "lossless" {
    merge_lossless(&files, &output_path).await
  } else {
    merge_reencode(&files, &output_path).await
  };

  match result {
    Ok(()) => {
      emit(100, "合并完成");
      Ok(output_path)
    }
    // 无损失败自动回退重编码（fast_merge 同款回退）
    Err(_e) if resolved == "lossless" => {
      emit(40, "无损合并失败，回退重编码...");
      merge_reencode(&files, &output_path).await?;
      emit(100, "合并完成");
      Ok(output_path)
    }
    Err(e) => Err(e),
  }
}

// ============================================================
// 快速拆分（SplitWorker 移植）
// ============================================================

/// 拆分段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitSegment {
  pub start: f64,
  pub end: f64,
}

/// Tauri 命令：获取视频时长（前端用于均分/校验关键帧）
#[tauri::command]
pub async fn video_tools_get_duration(video_path: String) -> Result<f64, String> {
  video_duration(&video_path).await
}

/// Tauri 命令：列出文件夹下所有视频文件（递归，按文件名排序）
#[tauri::command]
pub fn video_tools_list_folder_videos(folder: String) -> Result<Vec<String>, String> {
  const VIDEO_EXTS: [&str; 8] = ["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "ts"];
  let dir = Path::new(&folder);
  if !dir.is_dir() {
    return Err(format!("不是文件夹: {folder}"));
  }
  let mut out = Vec::new();
  fn walk(dir: &Path, exts: &[&str], out: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
      let mut items: Vec<_> = entries.flatten().collect();
      items.sort_by_key(|e| e.file_name());
      for entry in items {
        let path = entry.path();
        if path.is_dir() {
          walk(&path, exts, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
          if exts.contains(&ext.to_lowercase().as_str()) {
            out.push(path.to_string_lossy().to_string());
          }
        }
      }
    }
  }
  walk(dir, &VIDEO_EXTS, &mut out);
  if out.is_empty() {
    return Err("文件夹下没有找到视频文件".to_string());
  }
  Ok(out)
}

/// Tauri 命令：快速拆分（关键帧时间点 → 相邻成段，copy 优先失败回退重编码）
#[tauri::command]
pub async fn video_tools_split(
  app: tauri::AppHandle,
  video_path: String,
  keyframes: Vec<f64>,
  output_dir: String,
) -> Result<Vec<String>, String> {
  use tauri::Emitter;

  if !Path::new(&video_path).exists() {
    return Err(format!("视频不存在: {video_path}"));
  }

  let duration = video_duration(&video_path).await?;

  // _start_split 移植：关键帧 → 相邻成段
  let mut points = keyframes.clone();
  points.sort_by(|a, b| a.partial_cmp(b).unwrap());
  let mut segments: Vec<SplitSegment> = Vec::new();
  let mut prev = 0.0f64;
  for t in points {
    if t > prev + 0.1 {
      segments.push(SplitSegment {
        start: prev,
        end: t,
      });
    }
    prev = t;
  }
  if duration > prev + 0.1 {
    segments.push(SplitSegment {
      start: prev,
      end: duration,
    });
  }
  if segments.len() < 2 {
    return Err("至少需要 1 个关键帧才能拆分为 2 段".to_string());
  }

  let _ = std::fs::create_dir_all(&output_dir);
  let base = Path::new(&video_path)
    .file_stem()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| "video".to_string());
  let ext = Path::new(&video_path)
    .extension()
    .map(|n| format!(".{}", n.to_string_lossy()))
    .unwrap_or_else(|| ".mp4".to_string());
  let pad = segments.len().to_string().len();

  let ffmpeg = ffmpeg_path();
  let mut results = Vec::new();
  let total = segments.len();

  for (i, seg) in segments.iter().enumerate() {
    let out_name = format!("{base}_{:0width$}{ext}", i + 1, width = pad);
    let out_path = Path::new(&output_dir).join(&out_name);
    let dur = seg.end - seg.start;

    let _ = app.emit(
      "video-tools-progress",
      serde_json::json!({
        "stage": "split",
        "percent": (i as f64 / total as f64 * 100.0) as u32,
        "message": format!("拆分段 {}/{}...", i + 1, total),
      }),
    );

    // 先尝试无损 copy（SplitWorker 移植）
    let copy = tokio::process::Command::new(&ffmpeg)
      .args(["-y", "-i"])
      .arg(&video_path)
      .args([
        "-ss",
        &format!("{:.3}", seg.start),
        "-t",
        &format!("{:.3}", dur),
      ])
      .args(["-c", "copy", "-avoid_negative_ts", "1"])
      .arg(&out_path)
      .stdout(std::process::Stdio::piped())
      .stderr(std::process::Stdio::piped())
      .output()
      .await
      .map_err(|e| e.to_string())?;

    if !copy.status.success() {
      // 回退重编码
      let reencode = tokio::process::Command::new(&ffmpeg)
        .args(["-y", "-i"])
        .arg(&video_path)
        .args([
          "-ss",
          &format!("{:.3}", seg.start),
          "-t",
          &format!("{:.3}", dur),
        ])
        .args(["-c:v", "libx264", "-preset", "fast", "-c:a", "aac"])
        .arg(&out_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| e.to_string())?;
      if !reencode.status.success() {
        let err = String::from_utf8_lossy(&reencode.stderr);
        return Err(format!(
          "拆分段 {} 失败: {}",
          i + 1,
          &err[..err.len().min(300)]
        ));
      }
    }
    results.push(out_path.to_string_lossy().to_string());
  }

  let _ = app.emit(
    "video-tools-progress",
    serde_json::json!({
      "stage": "split",
      "percent": 100,
      "message": format!("拆分完成，共 {total} 段"),
    }),
  );

  Ok(results)
}
