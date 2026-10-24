// audio_silence_remover.rs - 音频气口清除器
// 基于 FFmpeg 的静音检测与自动剪辑功能

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 音频处理统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProcessStats {
  pub original_duration: f32,
  pub removed_duration: f32,
  pub final_duration: f32,
  pub kept_segments: u32,
}

/// 音频处理参数
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProcessParams {
  /// 音量阈值 (dB), 默认 -28
  pub threshold_db: f32,
  /// 片段前保留时长 (秒), 默认 0.1
  pub keep_before: f32,
  /// 片段后保留时长 (秒), 默认 0.1
  pub keep_after: f32,
  /// 停顿阈值 (秒), 短于此值不剪, 默认 0.35
  pub min_silence_duration: f32,
  /// 剪辑节奏: slow/medium/fast/custom
  pub pace: String,
}

impl Default for AudioProcessParams {
  fn default() -> Self {
    Self {
      threshold_db: -28.0,
      keep_before: 0.1,
      keep_after: 0.1,
      min_silence_duration: 0.35,
      pace: "medium".to_string(),
    }
  }
}

/// 获取 FFmpeg 路径
fn ffmpeg_path() -> PathBuf {
  let app_dir = crate::app_dirs::data_dir();
  let tools_dir = app_dir.join("video-tools");
  if cfg!(windows) {
    tools_dir.join("ffmpeg.exe")
  } else {
    tools_dir.join("ffmpeg")
  }
}

/// FFmpeg 不存在时自动走视频下载的下载通道；返回可用的 ffmpeg 路径
async fn ensure_ffmpeg<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
  let ffmpeg = ffmpeg_path();
  if ffmpeg.exists() {
    return Ok(ffmpeg);
  }
  crate::video_downloader::download_ffmpeg(app.clone()).await?;
  let ffmpeg = ffmpeg_path();
  if ffmpeg.exists() {
    Ok(ffmpeg)
  } else {
    Err("FFmpeg 下载完成但仍未找到可执行文件".to_string())
  }
}

// ---- 参数持久化（过滤参数下次启动自动读取） ----

/// 音频处理设置（过滤参数 + 节奏）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioSettings {
  pub threshold_db: f32,
  pub keep_before: f32,
  pub keep_after: f32,
  pub min_silence_duration: f32,
  pub pace: String,
}

impl Default for AudioSettings {
  fn default() -> Self {
    Self {
      threshold_db: -28.0,
      keep_before: 0.1,
      keep_after: 0.1,
      min_silence_duration: 0.35,
      pace: "medium".to_string(),
    }
  }
}

fn audio_settings_path() -> PathBuf {
  crate::app_dirs::data_dir().join("audio_silence_settings.json")
}

/// Tauri 命令：读取音频处理设置
#[tauri::command]
pub fn audio_silence_get_settings() -> AudioSettings {
  let path = audio_settings_path();
  if path.exists() {
    if let Ok(content) = std::fs::read_to_string(&path) {
      if let Ok(s) = serde_json::from_str::<AudioSettings>(&content) {
        return s;
      }
    }
  }
  AudioSettings::default()
}

/// Tauri 命令：保存音频处理设置
#[tauri::command]
pub fn audio_silence_save_settings(settings: AudioSettings) -> Result<(), String> {
  let path = audio_settings_path();
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
  std::fs::write(path, json).map_err(|e| e.to_string())
}

/// 检测音频文件中的静音段
#[tauri::command]
pub async fn audio_detect_silence(
  app: tauri::AppHandle,
  input_path: String,
  threshold_db: f32,
  min_duration: f32,
) -> Result<Vec<SilenceSegment>, String> {
  let ffmpeg = ensure_ffmpeg(&app).await?;

  let mut cmd = tokio::process::Command::new(&ffmpeg);
  cmd
    .arg("-i")
    .arg(&input_path)
    .arg("-af")
    .arg(format!(
      "silencedetect=noise={}dB:d={}",
      threshold_db, min_duration
    ))
    .arg("-f")
    .arg("null")
    .arg("-")
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  let output = cmd.output().await.map_err(|e| e.to_string())?;
  let stderr = String::from_utf8_lossy(&output.stderr);

  let mut segments = Vec::new();
  let mut current_start: Option<f32> = None;

  for line in stderr.lines() {
    if line.contains("silence_start:") {
      if let Some(start_str) = line.split("silence_start:").nth(1) {
        if let Ok(start) = start_str.trim().parse::<f32>() {
          current_start = Some(start);
        }
      }
    } else if line.contains("silence_end:") {
      if let Some(start) = current_start {
        if let Some(end_str) = line.split("silence_end:").nth(1) {
          if let Some(end_part) = end_str.split_whitespace().next() {
            if let Ok(end) = end_part.parse::<f32>() {
              segments.push(SilenceSegment {
                start,
                end,
                duration: end - start,
              });
              current_start = None;
            }
          }
        }
      }
    }
  }

  Ok(segments)
}

/// 静音段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SilenceSegment {
  pub start: f32,
  pub end: f32,
  pub duration: f32,
}

/// 处理音频文件 - 剪掉气口
#[tauri::command]
pub async fn audio_remove_silence(
  app: tauri::AppHandle,
  input_path: String,
  output_path: String,
  params: AudioProcessParams,
) -> Result<AudioProcessStats, String> {
  let ffmpeg = ensure_ffmpeg(&app).await?;

  // 构建 FFmpeg 滤镜
  let filter = format!(
        "silenceremove=start_periods=1:start_silence={}:start_threshold={}dB:stop_periods=-1:stop_silence={}:stop_threshold={}dB",
        params.keep_before,
        params.threshold_db,
        params.min_silence_duration,
        params.threshold_db
    );

  let mut cmd = tokio::process::Command::new(&ffmpeg);
  cmd
    .arg("-y")
    .arg("-i")
    .arg(&input_path)
    .arg("-af")
    .arg(&filter)
    .arg(&output_path)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  let output = cmd.output().await.map_err(|e| e.to_string())?;

  if !output.status.success() {
    let error = String::from_utf8_lossy(&output.stderr);
    return Err(format!("FFmpeg error: {}", error));
  }

  // 获取统计信息
  let stats = get_audio_stats(&ffmpeg, &input_path, &output_path).await?;

  Ok(stats)
}

/// 获取音频统计信息
async fn get_audio_stats(
  ffmpeg: &PathBuf,
  input_path: &str,
  output_path: &str,
) -> Result<AudioProcessStats, String> {
  let original_duration = get_audio_duration(ffmpeg, input_path).await?;
  let final_duration = get_audio_duration(ffmpeg, output_path).await?;
  let removed_duration = original_duration - final_duration;

  Ok(AudioProcessStats {
    original_duration,
    removed_duration,
    final_duration,
    kept_segments: 0, // 简化版本,不统计片段数
  })
}

/// 获取音频时长
async fn get_audio_duration(ffmpeg: &PathBuf, path: &str) -> Result<f32, String> {
  let mut cmd = tokio::process::Command::new(ffmpeg);
  cmd
    .arg("-i")
    .arg(path)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  let output = cmd.output().await.map_err(|e| e.to_string())?;
  let stderr = String::from_utf8_lossy(&output.stderr);

  for line in stderr.lines() {
    if line.contains("Duration:") {
      if let Some(duration_str) = line.split("Duration:").nth(1) {
        if let Some(time_str) = duration_str.split(",").next() {
          let time_str = time_str.trim();
          let parts: Vec<&str> = time_str.split(":").collect();
          if parts.len() == 3 {
            let hours: f32 = parts[0].parse().unwrap_or(0.0);
            let minutes: f32 = parts[1].parse().unwrap_or(0.0);
            let seconds: f32 = parts[2].parse().unwrap_or(0.0);
            return Ok(hours * 3600.0 + minutes * 60.0 + seconds);
          }
        }
      }
    }
  }

  Err("Could not determine audio duration".to_string())
}

/// 批量处理音频文件
#[tauri::command]
#[allow(dead_code)]
pub async fn audio_batch_process(
  app: tauri::AppHandle,
  input_paths: Vec<String>,
  output_dir: String,
  params: AudioProcessParams,
) -> Result<Vec<AudioProcessResult>, String> {
  let mut results = Vec::new();

  for input_path in input_paths {
    let input = PathBuf::from(&input_path);
    let file_name = input
      .file_stem()
      .unwrap_or_default()
      .to_string_lossy()
      .to_string();
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_secs())
      .unwrap_or(0);
    let output_path = PathBuf::from(&output_dir).join(format!("{file_name}_{now}.wav"));

    match audio_remove_silence(
      app.clone(),
      input_path.clone(),
      output_path.to_string_lossy().to_string(),
      params.clone(),
    )
    .await
    {
      Ok(stats) => {
        results.push(AudioProcessResult {
          input_path,
          output_path: output_path.to_string_lossy().to_string(),
          success: true,
          stats: Some(stats),
          error: None,
        });
      }
      Err(e) => {
        results.push(AudioProcessResult {
          input_path,
          output_path: String::new(),
          success: false,
          stats: None,
          error: Some(e),
        });
      }
    }
  }

  Ok(results)
}

/// 音频处理结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AudioProcessResult {
  pub input_path: String,
  pub output_path: String,
  pub success: bool,
  pub stats: Option<AudioProcessStats>,
  pub error: Option<String>,
}

/// 使用 FFmpeg 渲染音频波形 PNG 并返回 base64 数据
#[tauri::command]
pub async fn audio_get_waveform(
  app: tauri::AppHandle,
  input_path: String,
  width: Option<u32>,
  height: Option<u32>,
) -> Result<String, String> {
  let ffmpeg = ensure_ffmpeg(&app).await?;

  let w = width.unwrap_or(900);
  let h = height.unwrap_or(80);

  let mut cmd = tokio::process::Command::new(&ffmpeg);
  cmd
    .arg("-y")
    .arg("-i")
    .arg(&input_path)
    .arg("-filter_complex")
    .arg(format!("showwavespic=s={}x{}:colors=#3b82f6", w, h))
    .arg("-frames:v")
    .arg("1")
    .arg("-f")
    .arg("image2pipe")
    .arg("-vcodec")
    .arg("png")
    .arg("-")
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  let output = cmd.output().await.map_err(|e| e.to_string())?;

  if !output.status.success() {
    let error = String::from_utf8_lossy(&output.stderr);
    return Err(format!("FFmpeg waveform error: {}", error));
  }

  if output.stdout.is_empty() {
    return Err("FFmpeg produced no waveform output".to_string());
  }

  Ok(base64::engine::general_purpose::STANDARD.encode(&output.stdout))
}
