// video_matcher.rs - 视频画面匹配
// 特征提取：DINOv2 ViT-S/14 / ViT-B/14 ONNX（CutMatch 同款）

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 默认提取器
pub const DEFAULT_EXTRACTOR: &str = "dinov2_vits14";

/// 模型下载 CDN 基地址（VPS）
const MODEL_CDN_BASE: &str = "https://www.yacm.xin/dl/models";

/// 可选模型注册表：id -> (onnx 文件名, 下载地址, 特征维度, 显示名, 体积MB, 输入尺寸)
/// 与 CutMatch MODEL_INFO 对齐（12 个模型全量支持）
fn model_registry(id: &str) -> Option<(&'static str, String, usize, &'static str, u64, usize)> {
  let (filename, dim, display, mb, img) = match id {
    "dinov2_vits14" => (
      "dinov2_vits14.onnx",
      384,
      "DINOv2 ViT-S/14（快速，推荐）",
      85,
      518,
    ),
    "dinov2_vitb14" => (
      "dinov2_vitb14.onnx",
      768,
      "DINOv2 ViT-B/14（精度更高）",
      330,
      518,
    ),
    "dinov2_vitl14" => (
      "dinov2_vitl14.onnx",
      1024,
      "DINOv2 ViT-L/14（最高精度）",
      1200,
      518,
    ),
    "resnet18" => ("resnet18.onnx", 512, "ResNet-18（轻量极速）", 45, 224),
    "resnet50" => ("resnet50.onnx", 2048, "ResNet-50（均衡之选）", 98, 224),
    "resnet101" => ("resnet101.onnx", 2048, "ResNet-101（大型 CNN）", 170, 224),
    "resnet152" => ("resnet152.onnx", 2048, "ResNet-152（超大 CNN）", 230, 224),
    "efficientnet_b0" => (
      "efficientnet_b0.onnx",
      1280,
      "EfficientNet-B0（轻量高效）",
      21,
      224,
    ),
    "efficientnet_b4" => (
      "efficientnet_b4.onnx",
      1792,
      "EfficientNet-B4（精度更好）",
      77,
      380,
    ),
    "efficientnet_b7" => (
      "efficientnet_b7.onnx",
      2560,
      "EfficientNet-B7（顶级精度）",
      256,
      600,
    ),
    "vit_b_16" => (
      "vit_b_16.onnx",
      768,
      "ViT-B/16（Vision Transformer）",
      330,
      224,
    ),
    "vit_l_16" => (
      "vit_l_16.onnx",
      1024,
      "ViT-L/16（大型 Transformer）",
      1100,
      224,
    ),
    _ => return None,
  };
  Some((
    filename,
    format!("{MODEL_CDN_BASE}/{filename}"),
    dim,
    display,
    mb,
    img,
  ))
}
/// 匹配片段结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchSegment {
  /// 对标视频起始时间（秒）
  pub source_start: f32,
  /// 对标视频结束时间（秒）
  pub source_end: f32,
  /// 原视频起始时间（秒）
  pub target_start: f32,
  /// 原视频结束时间（秒）
  pub target_end: f32,
  /// 平均相似度（0-1，越接近1越相似）
  pub similarity: f32,
  /// 连续匹配帧数
  pub matched_frames: u32,
}

/// 视频匹配设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMatcherSettings {
  /// 缓存目录（空字符串表示使用默认）
  pub cache_dir: String,
  /// 特征提取模型：phash / dinov2
  pub extractor: String,
  /// 抽帧间隔（秒）
  pub interval_sec: f32,
  /// 相似度阈值
  pub threshold: f32,
  /// 最少连续匹配帧数
  pub min_frames: u32,
}

impl Default for VideoMatcherSettings {
  fn default() -> Self {
    Self {
      cache_dir: String::new(),
      extractor: DEFAULT_EXTRACTOR.to_string(),
      interval_sec: 1.0,
      threshold: 0.5,
      min_frames: 2,
    }
  }
}

/// 获取默认缓存目录：程序目录下的 cache/video-matcher/
fn default_cache_dir() -> PathBuf {
  crate::app_dirs::data_dir()
    .join("cache")
    .join("video-matcher")
}

/// 解析缓存目录：设置项 > 默认
pub fn resolve_cache_dir(settings: &VideoMatcherSettings) -> PathBuf {
  if !settings.cache_dir.is_empty() {
    PathBuf::from(&settings.cache_dir)
  } else {
    default_cache_dir()
  }
}

/// 视频 ID（基于路径+大小。不含 mtime：剪映等软件可能触碰文件导致缓存永远失效）
pub fn video_id_for(video_path: &str) -> String {
  use std::hash::{Hash, Hasher};
  // 正规化路径（小写盘符/正斜杠统一），避免同一文件因路径书写差异得到不同 id
  let normalized = video_path.replace('/', "\\").to_lowercase();
  // 只用路径 + 文件大小。不用 mtime：剪映等软件可能触碰文件导致 mtime 变化，
  // 缓存永远无法命中（视频内容本身不会变）
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  normalized.hash(&mut hasher);
  if let Ok(meta) = std::fs::metadata(video_path) {
    meta.len().hash(&mut hasher);
  }
  format!("{:016x}", hasher.finish())
}

/// 缓存文件路径
pub fn cache_path_for(cache_dir: &Path, video_id: &str, extractor: &str, interval: f32) -> PathBuf {
  cache_dir.join(format!("{}_{}_{:.2}.bin", video_id, extractor, interval))
}

/// 按特征大小回退查找缓存（video_id 算法变更后，旧文件按 内容规模+模型+间隔 匹配）。
/// 返回匹配到的缓存文件路径。
fn find_cache_fallback(
  cache_dir: &Path,
  video_path: &str,
  extractor: &str,
  interval: f32,
  feature_dim: usize,
) -> Option<PathBuf> {
  let expected_frame_bytes = feature_dim * 4 + 8; // feature + (ts + dim 头)
  let expected_size = std::fs::metadata(video_path).ok()?; // 仅确认文件存在
  let _ = expected_size;
  let prefix = format!("_{extractor}_{interval:.2}.bin");
  let mut best: Option<(u64, PathBuf)> = None;
  if let Ok(entries) = std::fs::read_dir(cache_dir) {
    for entry in entries.flatten() {
      let name = entry.file_name().to_string_lossy().to_string();
      if !name.ends_with(&prefix) {
        continue;
      }
      let Ok(meta) = entry.metadata() else { continue };
      let size = meta.len();
      // 帧数 = (size) / frame_bytes，验证是合法的该维度缓存
      if size % expected_frame_bytes as u64 != 0 || size < expected_frame_bytes as u64 {
        continue;
      }
      // 同规模缓存可能对应多个视频；此处仅当目录里只有一个该规模文件时采用
      match best {
        Some((existing_size, _)) if existing_size == size => {
          // 同规模歧义，放弃回退（宁可不命中也不读错数据）
          return None;
        }
        _ => best = Some((size, entry.path())),
      }
    }
  }
  best.map(|(_, p)| p)
}

/// 一帧的特征。phash 时为 64 维 0/1 向量；dinov2 时为 384 维 L2 归一化向量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameFeature {
  pub timestamp: f32,
  pub feature: Vec<f32>,
}

/// 从缓存加载特征（帧记录 = 4 bytes timestamp + 4 bytes dim + dim*4 bytes f32）
fn load_cache(cache_file: &Path) -> Option<Vec<FrameFeature>> {
  if !cache_file.exists() {
    return None;
  }
  let bytes = std::fs::read(cache_file).ok()?;
  let mut features = Vec::new();
  let mut i = 0usize;
  while i + 8 <= bytes.len() {
    let timestamp = f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let dim = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
    i += 8;
    if i + dim * 4 > bytes.len() {
      break;
    }
    let mut feature = Vec::with_capacity(dim);
    for d in 0..dim {
      let off = i + d * 4;
      feature.push(f32::from_le_bytes([
        bytes[off],
        bytes[off + 1],
        bytes[off + 2],
        bytes[off + 3],
      ]));
    }
    i += dim * 4;
    features.push(FrameFeature { timestamp, feature });
  }
  Some(features)
}

/// 保存特征到缓存
fn save_cache(cache_file: &Path, features: &[FrameFeature]) -> Result<(), String> {
  if let Some(parent) = cache_file.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let mut bytes = Vec::new();
  for f in features {
    bytes.extend_from_slice(&f.timestamp.to_le_bytes());
    bytes.extend_from_slice(&(f.feature.len() as u32).to_le_bytes());
    for v in &f.feature {
      bytes.extend_from_slice(&v.to_le_bytes());
    }
  }
  // 原子写入：先写临时文件再 rename，避免共享盘上多用户同时首缓存时
  // 互相读到半截文件（rename 在同一目录内是原子的，Windows/Unix/SMB 均成立）
  let tmp = cache_file.with_extension("tmp");
  std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
  std::fs::rename(&tmp, cache_file).map_err(|e| e.to_string())
}

/// 余弦相似度（向量均已归一化时即点积）
fn feature_similarity(a: &[f32], b: &[f32]) -> f32 {
  let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
  dot.clamp(0.0, 1.0)
}

/// 进度事件载荷
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatcherProgress {
  pub stage: String, // "extract_source" | "extract_target" | "matching" | "exporting"
  pub percent: u32,
  pub message: String,
}

/// 模型目录（app data / models）
fn models_dir() -> PathBuf {
  crate::app_dirs::data_dir().join("models")
}

/// 指定模型的本地路径
fn model_path(model_id: &str) -> Option<PathBuf> {
  model_registry(model_id).map(|(filename, _, _, _, _, _)| models_dir().join(filename))
}

/// 尝试从安装目录捆绑的 resources 复制模型到 data_dir/models
fn try_copy_bundled_model(model_id: &str, target: &Path) -> bool {
  let Some(filename) = model_registry(model_id).map(|(f, _, _, _, _, _)| f) else {
    return false;
  };
  let exe_dir = std::env::current_exe()
    .ok()
    .and_then(|p| p.parent().map(|d| d.to_path_buf()));
  let Some(exe_dir) = exe_dir else {
    return false;
  };
  let candidates = [
    exe_dir.join("models").join(filename),
    // dev 模式：target/debug/models/
    exe_dir
      .parent()
      .map(|d| d.join(filename))
      .unwrap_or_default(),
  ];
  for cand in candidates {
    if cand.is_file()
      && std::fs::copy(&cand, target).is_ok()
      && target
        .metadata()
        .map(|m| m.len() > 1024 * 1024)
        .unwrap_or(false)
    {
      log::info!(
        "[video_matcher] 模型 {} 从捆绑资源复制完成: {}",
        model_id,
        cand.display()
      );
      return true;
    }
  }
  false
}

/// 下载模型（带进度事件）；已存在直接返回
async fn ensure_model<R: tauri::Runtime>(
  app: &tauri::AppHandle<R>,
  model_id: &str,
) -> Result<PathBuf, String> {
  use tauri::Emitter;

  let Some((filename, url, _, display_name, _, _)) = model_registry(model_id) else {
    return Err(format!("未知模型: {model_id}"));
  };

  let path = models_dir().join(filename);
  if path.exists()
    && std::fs::metadata(&path)
      .map(|m| m.len() > 1024 * 1024)
      .unwrap_or(false)
  {
    return Ok(path);
  }

  let dir = models_dir();
  std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

  // 优先从安装包捆绑的 resources 复制（免下载）
  if try_copy_bundled_model(model_id, &path) {
    return Ok(path);
  }

  let tmp = dir.join(format!("{filename}.tmp"));

  let _ = app.emit(
    "video-matcher-progress",
    MatcherProgress {
      stage: "download_model".to_string(),
      percent: 0,
      message: format!("下载模型 {display_name}..."),
    },
  );

  let response = reqwest::get(url)
    .await
    .map_err(|e| format!("模型下载请求失败: {e}"))?;
  if !response.status().is_success() {
    return Err(format!("模型下载失败: HTTP {}", response.status()));
  }
  let total = response.content_length().unwrap_or(0);

  use futures_util::StreamExt;
  let mut stream = response.bytes_stream();
  let mut file = tokio::fs::File::create(&tmp)
    .await
    .map_err(|e| e.to_string())?;
  let mut downloaded: u64 = 0;
  let mut last_emit = 0u64;

  while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(|e| e.to_string())?;
    tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
      .await
      .map_err(|e| e.to_string())?;
    downloaded += chunk.len() as u64;
    // 每 1MB 上报一次进度
    if total > 0 && downloaded - last_emit > 1024 * 1024 {
      last_emit = downloaded;
      let percent = (downloaded as f32 / total as f32 * 100.0) as u32;
      let _ = app.emit(
        "video-matcher-progress",
        MatcherProgress {
          stage: "download_model".to_string(),
          percent: percent / 2, // 模型下载占整体前 50%
          message: format!(
            "下载模型 {:.1}/{:.1} MB",
            downloaded as f32 / 1048576.0,
            total as f32 / 1048576.0
          ),
        },
      );
    }
  }
  use tokio::io::AsyncWriteExt;
  file.flush().await.map_err(|e| e.to_string())?;
  drop(file);
  std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;

  let _ = app.emit(
    "video-matcher-progress",
    MatcherProgress {
      stage: "download_model".to_string(),
      percent: 50,
      message: "模型下载完成".to_string(),
    },
  );

  Ok(path)
}

/// 当前推理设备（"gpu" / "cpu"），供状态查询
static ACTIVE_DEVICE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0); // 0=未初始化, 1=cpu, 2=gpu

/// 尝试带 GPU EP 创建会话；GPU 失败自动回退 CPU
fn build_session(
  env: &ort::environment::Environment,
  path: &Path,
) -> Result<(ort::session::Session, &'static str), String> {
  let mut builder = ort::session::Session::builder(env).map_err(|e| e.to_string())?;
  builder = builder.with_intra_threads(4).map_err(|e| e.to_string())?;

  // GPU 优先级：CUDA > DirectML > CoreML > CPU。GPU EP 初始化失败自动回退。
  // EP 均按 feature gate：默认 CPU，`--features directml/coreml/cuda` 时启用对应预编译 EP。
  #[cfg(all(target_os = "windows", feature = "cuda"))]
  {
    // 1) CUDA（NVIDIA 显卡 + CUDA 13 驱动）
    if let Ok(true) = <ort::ep::CUDA as ort::ep::ExecutionProvider>::is_available() {
      if let Ok(b) = ort::session::Session::builder(env) {
        if let Ok(b) = b.with_intra_threads(4) {
          if let Ok(mut b) = b.with_execution_providers([ort::ep::CUDA::default().build()]) {
            if let Ok(session) = b.commit_from_file(path) {
              return Ok((session, "gpu"));
            }
          }
        }
      }
    }
  }

  #[cfg(all(target_os = "windows", feature = "directml"))]
  {
    // 2) DirectML（系统自带，任何 DX12 显卡）
    if let Ok(true) = <ort::ep::DirectML as ort::ep::ExecutionProvider>::is_available() {
      let mut gpu_builder = ort::session::Session::builder(env).map_err(|e| e.to_string())?;
      gpu_builder = gpu_builder
        .with_intra_threads(4)
        .map_err(|e| e.to_string())?;
      if let Ok(session) = gpu_builder
        .with_execution_providers([ort::ep::DirectML::default().build()])
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
      {
        return Ok((session, "gpu"));
      }
    }
  }

  #[cfg(all(not(target_os = "windows"), feature = "cuda"))]
  {
    // 1) CUDA（NVIDIA）
    if let Ok(true) = <ort::ep::CUDA as ort::ep::ExecutionProvider>::is_available() {
      if let Ok(b) = ort::session::Session::builder(env) {
        if let Ok(b) = b.with_intra_threads(4) {
          if let Ok(mut b) = b.with_execution_providers([ort::ep::CUDA::default().build()]) {
            if let Ok(session) = b.commit_from_file(path) {
              return Ok((session, "gpu"));
            }
          }
        }
      }
    }
  }

  #[cfg(all(target_os = "macos", feature = "coreml"))]
  {
    // 2) CoreML（macOS）
    if let Ok(true) = <ort::ep::CoreML as ort::ep::ExecutionProvider>::is_available() {
      let mut gpu_builder = ort::session::Session::builder(env).map_err(|e| e.to_string())?;
      gpu_builder = gpu_builder
        .with_intra_threads(4)
        .map_err(|e| e.to_string())?;
      if let Ok(session) = gpu_builder
        .with_execution_providers([ort::ep::CoreML::default().build()])
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
      {
        return Ok((session, "gpu"));
      }
    }
  }

  let session = builder
    .commit_from_file(path)
    .map_err(|e| format!("加载 ONNX 模型失败: {e}"))?;
  Ok((session, "cpu"))
}

/// 获取或初始化 ONNX 会话（进程级单例，Box::leak 常驻）
fn onnx_session(model_id: &str) -> Result<&'static mut ort::session::Session, String> {
  use std::sync::atomic::{AtomicPtr, Ordering};
  static SESSION_PTR: AtomicPtr<ort::session::Session> = AtomicPtr::new(std::ptr::null_mut());
  static SESSION_MODEL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

  // 同一模型的会话复用；切换模型则重建（model_id 哈希作比对键）
  let model_key = {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    model_id.hash(&mut h);
    h.finish()
  };
  let existing = SESSION_PTR.load(Ordering::Acquire);
  if !existing.is_null() && SESSION_MODEL.load(Ordering::Acquire) == model_key {
    return Ok(unsafe { &mut *existing });
  }

  let Some(path) = model_path(model_id) else {
    return Err(format!("未知模型: {model_id}"));
  };
  if !path.exists() {
    return Err(format!("模型文件不存在: {}", path.to_string_lossy()));
  }
  // ONNX Runtime 全进程只允许一个 Environment，必须做成单例
  static ENV_PTR: AtomicPtr<ort::environment::Environment> = AtomicPtr::new(std::ptr::null_mut());
  let env_ptr = ENV_PTR.load(Ordering::Acquire);
  let env: &'static ort::environment::Environment = if !env_ptr.is_null() {
    unsafe { &*env_ptr }
  } else {
    let env = ort::init()
      .build()
      .map_err(|e| format!("初始化 ONNX 环境失败: {e}"))?;
    let leaked = Box::leak(Box::new(env)) as *mut ort::environment::Environment;
    ENV_PTR.store(leaked, Ordering::Release);
    unsafe { &*leaked }
  };
  let (session, device) = build_session(env, &path)?;
  log::info!("[video_matcher] ONNX 推理设备: {device}");
  ACTIVE_DEVICE.store(if device == "gpu" { 2 } else { 1 }, Ordering::Release);
  let leaked = Box::leak(Box::new(session)) as *mut ort::session::Session;
  SESSION_PTR.store(leaked, Ordering::Release);
  SESSION_MODEL.store(model_key, Ordering::Release);
  Ok(unsafe { &mut *leaked })
}

/// 用 DINOv2 对一批 RGB24 帧提取特征（CutMatch 同款预处理），带帧级进度
fn dinov2_extract_batch<R: tauri::Runtime>(
  app: &tauri::AppHandle<R>,
  model_id: &str,
  frames_rgb: &[Vec<u8>],
  img_size: usize,
  stage: &str,
  progress_base: u32,
  progress_span: u32,
) -> Result<Vec<Vec<f32>>, String> {
  use tauri::Emitter;

  let session = onnx_session(model_id)?;
  let img = img_size;
  let feature_dim = model_registry(model_id)
    .map(|(_, _, dim, _, _, _)| dim)
    .unwrap_or(384);

  let total = frames_rgb.len();
  let start = std::time::Instant::now();
  let mut outputs = Vec::with_capacity(total);

  // 批量推理（CutMatch batch_size=32 同思路）：一次喂多帧给 GPU，大幅提升利用率
  const BATCH: usize = 1; // DirectML 实测批量无收益（算子回退 CPU），单帧最省内存
  let mean = [0.485f32, 0.456, 0.406];
  let std = [0.229f32, 0.224, 0.225];
  let n = img * img;

  let mut batch_idx = 0usize;
  while batch_idx < total {
    let batch_end = (batch_idx + BATCH).min(total);
    let batch_len = batch_end - batch_idx;

    // 归一化整批帧（plane 分离布局）
    let mut input = vec![0f32; batch_len * 3 * n];
    for (bi, frame) in frames_rgb[batch_idx..batch_end].iter().enumerate() {
      let base = bi * 3 * n;
      for px in 0..n {
        let o = px * 3;
        input[base + px] = (frame[o] as f32 / 255.0 - mean[0]) / std[0];
        input[base + n + px] = (frame[o + 1] as f32 / 255.0 - mean[1]) / std[1];
        input[base + 2 * n + px] = (frame[o + 2] as f32 / 255.0 - mean[2]) / std[2];
      }
    }

    let array = ndarray::Array4::from_shape_vec((batch_len, 3, img, img), input)
      .map_err(|e| e.to_string())?;
    let value = ort::value::Tensor::from_array(array).map_err(|e| e.to_string())?;
    let t_infer = std::time::Instant::now();
    let outs = session
      .run(ort::inputs![value])
      .map_err(|e| format!("ONNX 推理失败: {e}"))?;
    let infer_ms = t_infer.elapsed().as_millis();
    let (_shape, data) = outs[0]
      .try_extract_tensor::<f32>()
      .map_err(|e| e.to_string())?;
    let data = data.to_vec();

    // 逐帧 L2 归一化并收集
    for bi in 0..batch_len {
      let row = &data[bi * feature_dim..(bi + 1) * feature_dim];
      let norm: f32 = row.iter().map(|v| v * v).sum::<f32>().sqrt();
      let feature: Vec<f32> = row
        .iter()
        .map(|v| if norm > 0.0 { v / norm } else { 0.0 })
        .collect();
      outputs.push(feature);
    }

    if batch_idx == 0 || (batch_idx / BATCH) % 10 == 0 {
      log::info!(
        "[video_matcher] 批推理 {batch_len}帧: {infer_ms}ms ({:.0}ms/帧) · 累计 {:.1}s",
        infer_ms as f32 / batch_len as f32,
        start.elapsed().as_secs_f32()
      );
    }

    // 帧级进度
    let done = batch_end;
    if total > 0 && (done % 3 == 0 || done == total) {
      let percent = progress_base + (done as f32 / total as f32 * progress_span as f32) as u32;
      let elapsed = start.elapsed().as_secs_f32();
      let per_frame = elapsed / done as f32;
      let remaining = per_frame * (total - done) as f32;
      let remain_str = if remaining >= 60.0 {
        format!(
          "{}分{}秒",
          (remaining / 60.0) as u32,
          (remaining % 60.0) as u32
        )
      } else {
        format!("{:.0}秒", remaining)
      };
      let _ = app.emit(
        "video-matcher-progress",
        MatcherProgress {
          stage: stage.to_string(),
          percent,
          message: format!("{stage} · 已提取 {done}/{total} 帧 · 剩余 {remain_str}"),
        },
      );
    }

    batch_idx = batch_end;
  }
  Ok(outputs)
}

/// 用 FFmpeg 提取视频帧并计算特征（DINOv2 模型），带帧级进度
#[allow(clippy::too_many_arguments)]
pub async fn extract_frame_hashes<R: tauri::Runtime>(
  app: &tauri::AppHandle<R>,
  video_path: &str,
  interval_sec: f32,
  cache_dir: &Path,
  extractor: &str,
  stage: &str,
  progress_base: u32,
  progress_span: u32,
) -> Result<Vec<FrameFeature>, String> {
  use tauri::Emitter;

  let vid = video_id_for(video_path);
  let cache_file = cache_path_for(cache_dir, &vid, extractor, interval_sec);

  // 尝试读缓存（精确键 → 回退：video_id 算法变更后按规模匹配旧文件）
  let feature_dim = model_registry(extractor)
    .map(|(_, _, dim, _, _, _)| dim)
    .unwrap_or(384);
  let cache_loaded = load_cache(&cache_file).or_else(|| {
    let fallback =
      find_cache_fallback(cache_dir, video_path, extractor, interval_sec, feature_dim)?;
    log::info!("[video_matcher] 缓存回退命中: {}", fallback.display());
    load_cache(&fallback)
  });
  if let Some(features) = cache_loaded {
    let _ = app.emit(
      "video-matcher-progress",
      MatcherProgress {
        stage: stage.to_string(),
        percent: progress_base + progress_span,
        message: format!("{}（缓存命中，{} 帧）", stage, features.len()),
      },
    );
    return Ok(features);
  }

  let ffmpeg = crate::app_dirs::data_dir()
    .join("video-tools")
    .join(if cfg!(windows) {
      "ffmpeg.exe"
    } else {
      "ffmpeg"
    });
  if !ffmpeg.exists() {
    return Err("FFmpeg not found".to_string());
  }

  // 确保模型文件存在
  ensure_model(app, extractor).await?;

  // FFmpeg 直接输出 imgSize x imgSize RGB24（与 CutMatch transforms.Resize 的拉伸行为
  // 一致），归一化在 Rust 完成。CNN 模型 224/380/600，ViT 系 518。
  // fps=1/x 精度保留 5 位小数（0.033 逐帧间隔若被截断成 0.03 会造成帧数与时间戳错位）
  let img_size = model_registry(extractor)
    .map(|(_, _, _, _, _, img)| img)
    .unwrap_or(518);
  let filter = format!(
    "fps=1/{:.5},scale={img}:{img}:flags=area",
    interval_sec,
    img = img_size
  );
  let frame_size = img_size * img_size * 3;

  let mut cmd = tokio::process::Command::new(&ffmpeg);
  cmd
    .arg("-i")
    .arg(video_path)
    .arg("-vf")
    .arg(&filter)
    .arg("-f")
    .arg("rawvideo")
    .arg("-pix_fmt")
    .arg("rgb24")
    .arg("-")
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());

  // 流式读取：抽帧是长耗时阶段（逐帧模式下几万帧），必须边读边报进度，不能等 FFmpeg 全部完成
  let mut child = cmd.spawn().map_err(|e| e.to_string())?;
  let mut stdout = child.stdout.take().ok_or("无法读取 FFmpeg 输出")?;
  let stderr_handle = child.stderr.take();

  let (frame_count_hint, _) = {
    // 先从 stderr 尽力解析时长用于进度提示（失败不影响流程）
    let mut hint = 0u64;
    if let Some(mut err_pipe) = stderr_handle {
      use tokio::io::AsyncReadExt;
      let mut buf = vec![0u8; 65536];
      // 读一次 stderr（FFmpeg 头信息足够）
      if let Ok(n) = err_pipe.read(&mut buf).await {
        let info = String::from_utf8_lossy(&buf[..n]);
        for line in info.lines() {
          if let Some(dur_str) = line.split("Duration:").nth(1) {
            if let Some(time_str) = dur_str.split(",").next() {
              let parts: Vec<&str> = time_str.trim().split(":").collect();
              if parts.len() == 3 {
                let h: f32 = parts[0].parse().unwrap_or(0.0);
                let m: f32 = parts[1].parse().unwrap_or(0.0);
                let s: f32 = parts[2].parse().unwrap_or(0.0);
                hint = ((h * 3600.0 + m * 60.0 + s) / interval_sec) as u64;
              }
            }
          }
        }
      }
    }
    (hint, ())
  };

  use tokio::io::AsyncReadExt;
  let mut raw: Vec<u8> = Vec::with_capacity(frame_count_hint as usize * frame_size);
  let mut chunk = vec![0u8; frame_size * 64]; // 64 帧一批
  let extract_started = std::time::Instant::now();
  let mut last_report = 0usize;
  loop {
    let n = stdout.read(&mut chunk).await.map_err(|e| e.to_string())?;
    if n == 0 {
      break;
    }
    raw.extend_from_slice(&chunk[..n]);

    // 抽帧阶段进度提示（基于已读字节估算帧数）
    let frames_done = raw.len() / frame_size;
    if frame_count_hint > 0 && frames_done - last_report >= 30 {
      last_report = frames_done;
      let percent = progress_base
        + (frames_done as f32 / frame_count_hint as f32 * progress_span as f32 * 0.3) as u32;
      let _ = app.emit(
        "video-matcher-progress",
        MatcherProgress {
          stage: stage.to_string(),
          percent: percent.min(progress_base + progress_span),
          message: format!("{stage} · 抽帧中 {frames_done}/{} 帧", frame_count_hint),
        },
      );
    }
  }
  let _ = child.wait().await;

  // 检查退出码（stderr 剩余内容在子进程退出后可读，简化处理忽略）
  let status = child
    .try_wait()
    .map_err(|e| e.to_string())?
    .ok_or("FFmpeg 未退出")?;
  if !status.success() {
    return Err("FFmpeg 抽帧失败".to_string());
  }

  let mut features = Vec::new();

  let frame_count = raw.len() / frame_size;
  log::info!(
    "[video_matcher] FFmpeg 抽帧完成: {frame_count} 帧 ({:.1}s), 期望 {frame_count_hint} 帧",
    extract_started.elapsed().as_secs_f32()
  );
  if frame_count == 0 {
    return Err("FFmpeg 未输出任何帧，请检查视频文件".to_string());
  }
  let rgb_frames: Vec<Vec<u8>> = (0..frame_count)
    .map(|i| raw[i * frame_size..(i + 1) * frame_size].to_vec())
    .collect();
  {
    // FFmpeg scale 保证每帧正好 imgSize x imgSize
    let batch_out = dinov2_extract_batch(
      app,
      extractor,
      &rgb_frames,
      img_size,
      stage,
      progress_base + progress_span * 3 / 10,
      progress_span * 7 / 10,
    )?;
    for (i, feature) in batch_out.into_iter().enumerate() {
      features.push(FrameFeature {
        timestamp: i as f32 * interval_sec,
        feature,
      });
    }
  }

  // 保存缓存
  let _ = save_cache(&cache_file, &features);

  Ok(features)
}

/// 查找两个视频的匹配片段（CutMatch reference_finder 同款算法）
/// 要点：
/// 1. 对标帧 → 原片最高分帧的 1:1 映射（frame_map）
/// 2. 按对标帧序遍历，源位置需单调递增（允许 ±2 秒内跳变、允许后退 max_jump/3）
/// 3. 未匹配帧可跳过少量（gap_tolerance/2）继续连段——容忍解说视频里的转场/字幕卡
/// 4. 相邻段（对标与原片都接近）自动合并
pub fn find_matching_segments(
  source_hashes: &[FrameFeature], // 对标
  target_hashes: &[FrameFeature], // 原片
  threshold: f32,
  min_frames: usize,
  interval_sec: f32,
) -> Vec<MatchSegment> {
  // ---- 1:1 帧映射 ----
  let mut frame_map: Vec<Option<(usize, f32)>> = vec![None; source_hashes.len()];
  for (si, sh) in source_hashes.iter().enumerate() {
    let mut best_ti = 0usize;
    let mut best_sim = 0f32;
    for (ti, th) in target_hashes.iter().enumerate() {
      let sim = feature_similarity(&sh.feature, &th.feature);
      if sim > best_sim {
        best_sim = sim;
        best_ti = ti;
      }
    }
    if best_sim >= threshold {
      frame_map[si] = Some((best_ti, best_sim));
    }
  }

  let matched = frame_map.iter().filter(|m| m.is_some()).count();
  log::info!(
    "[video_matcher] 帧映射: {}/{} 帧通过阈值 {}",
    matched,
    source_hashes.len(),
    threshold
  );
  // 相似度分布（帮助诊断阈值问题）
  let mut all_sims: Vec<f32> = source_hashes
    .iter()
    .map(|sh| {
      target_hashes
        .iter()
        .map(|th| feature_similarity(&sh.feature, &th.feature))
        .fold(0f32, f32::max)
    })
    .collect();
  all_sims.sort_by(|a, b| a.partial_cmp(b).unwrap());
  if !all_sims.is_empty() {
    let n = all_sims.len();
    log::info!(
      "[video_matcher] 最佳匹配相似度分布: min={:.3} p25={:.3} 中位={:.3} p75={:.3} max={:.3}",
      all_sims[0],
      all_sims[n / 4],
      all_sims[n / 2],
      all_sims[3 * n / 4],
      all_sims[n - 1]
    );
  }

  // CutMatch reference_finder.py 后处理逐行移植：
  // max_src_jump = max(3, int(2.0 / interval))  —— 约 2 秒的抽帧数，防跨场景合并
  // 跳帧容忍 = max_src_jump // 2                —— 未匹配帧（转场/字幕卡）允许跳过
  let max_src_jump: i64 = ((2.0 / interval_sec) as i64).max(3);
  let skip_tolerance = (max_src_jump / 2) as usize;

  let mut results: Vec<MatchSegment> = Vec::new();
  let mut i = 0usize;
  while i < source_hashes.len() {
    // CutMatch: if i not in frame_map: i += 1; continue
    let Some((src_global_start, score_start)) = frame_map[i] else {
      i += 1;
      continue;
    };

    // CutMatch 单视频源：local_src_start = src_global_start
    let local_src_start = src_global_start as i64;
    let ref_start_idx = i;
    let mut ref_end_idx = i;
    let mut local_src_end = local_src_start;
    let mut total_score = score_start;
    let mut count = 1usize;
    let mut last_src_global = src_global_start as i64;

    // 向前扩展连续段（CutMatch 原逻辑）
    let mut j = i + 1;
    let mut skip_count = 0usize;
    while j < source_hashes.len() {
      match frame_map[j] {
        Some((src_gj, score_j)) => {
          let src_delta = src_gj as i64 - last_src_global;
          // 源位置单调递增，允许小跳变（2 秒内），允许后退少许
          if src_delta >= -(max_src_jump / 3) && src_delta <= max_src_jump {
            ref_end_idx = j;
            local_src_end = src_gj as i64;
            total_score += score_j;
            count += 1;
            last_src_global = src_gj as i64;
            skip_count = 0;
            j += 1;
            continue;
          }
          // 源跳变太大，结束段
          break;
        }
        None => {
          // 未匹配帧，允许跳过少量
          skip_count += 1;
          if skip_count > skip_tolerance {
            break;
          }
          j += 1;
        }
      }
    }

    // CutMatch: avg_score；段时间用实际帧时间戳（min/max 处理反向）
    let s_start = local_src_start.min(local_src_end);
    let s_end = local_src_start.max(local_src_end);

    // ref_end_ts 用帧自己的时间戳（不加 interval —— 与 CutMatch 一致）
    let ref_start_ts = source_hashes[ref_start_idx].timestamp;
    let ref_end_ts = source_hashes[ref_end_idx].timestamp;
    let src_start_ts =
      target_hashes[s_start.min(target_hashes.len() as i64 - 1).max(0) as usize].timestamp;
    let src_end_ts =
      target_hashes[s_end.min(target_hashes.len() as i64 - 1).max(0) as usize].timestamp;

    // CutMatch 的 min_frames 在 GUI 默认 2；扩展失败（count=1）时仍计入由 min_frames 过滤
    if count as i64 >= min_frames as i64 {
      results.push(MatchSegment {
        source_start: ref_start_ts,
        source_end: ref_end_ts,
        target_start: src_start_ts,
        target_end: src_end_ts,
        similarity: total_score / count as f32,
        matched_frames: count as u32,
      });
    }

    i = ref_end_idx + 1;
  }

  // ---- 合并相邻段（CutMatch 原逻辑：对标间隔 ≤1s、同文件、源位置间隔 ≤1s）----
  let mut merged: Vec<MatchSegment> = Vec::new();
  for seg in results {
    if let Some(prev) = merged.last_mut() {
      let ref_gap = seg.source_start - prev.source_end;
      // 单视频源恒同文件
      let src_gap = seg.target_start - prev.target_end;
      const MERGE_REF_GAP: f32 = 1.0;
      const MERGE_SRC_GAP: f32 = 1.0;
      if ref_gap <= MERGE_REF_GAP && src_gap.abs() <= MERGE_SRC_GAP {
        prev.source_end = seg.source_end; // CutMatch: prev.ref_end = seg.ref_end
        prev.target_start = prev.target_start.min(seg.target_start);
        prev.target_end = prev.target_end.max(seg.target_end);
        let total = prev.matched_frames + seg.matched_frames;
        prev.similarity = (prev.similarity * prev.matched_frames as f32
          + seg.similarity * seg.matched_frames as f32)
          / total as f32;
        prev.matched_frames = total;
        continue;
      }
    }
    merged.push(seg);
  }

  merged.sort_by(|a, b| a.source_start.partial_cmp(&b.source_start).unwrap());
  log::info!(
    "[video_matcher] 聚类完成: {} 段 (合并后, 覆盖 {} 帧) | max_src_jump={max_src_jump} skip_tol={skip_tolerance}",
    merged.len(),
    merged.iter().map(|s| s.matched_frames).sum::<u32>()
  );
  merged
}

/// 设置文件路径
fn settings_path() -> PathBuf {
  crate::app_dirs::data_dir().join("video_matcher_settings.json")
}

/// Tauri 命令：获取设置
#[tauri::command]
pub fn video_matcher_get_settings() -> VideoMatcherSettings {
  let path = settings_path();
  if path.exists() {
    if let Ok(content) = std::fs::read_to_string(&path) {
      if let Ok(settings) = serde_json::from_str::<VideoMatcherSettings>(&content) {
        return settings;
      }
    }
  }
  VideoMatcherSettings::default()
}

/// Tauri 命令：保存设置
#[tauri::command]
pub fn video_matcher_save_settings(settings: VideoMatcherSettings) -> Result<(), String> {
  let path = settings_path();
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
  std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Tauri 命令：获取默认缓存目录
#[tauri::command]
pub fn video_matcher_default_cache_dir() -> String {
  default_cache_dir().to_string_lossy().to_string()
}

/// Tauri 命令：清除缓存
#[tauri::command]
pub fn video_matcher_clear_cache(cache_dir: Option<String>) -> Result<u32, String> {
  let dir = if let Some(d) = cache_dir {
    PathBuf::from(d)
  } else {
    default_cache_dir()
  };

  if !dir.exists() {
    return Ok(0);
  }

  let mut count = 0u32;
  if let Ok(entries) = std::fs::read_dir(&dir) {
    for entry in entries.flatten() {
      if let Ok(meta) = entry.metadata() {
        if meta.is_file() {
          if let Ok(name) = entry.file_name().into_string() {
            if name.ends_with(".bin") && std::fs::remove_file(entry.path()).is_ok() {
              count += 1;
            }
          }
        }
      }
    }
  }

  Ok(count)
}

/// Tauri 命令：查找两个视频的匹配片段
#[tauri::command]
pub async fn video_find_matches(
  app: tauri::AppHandle,
  source_path: String,
  target_path: String,
  threshold: Option<f32>,
  min_frames: Option<u32>,
  interval_sec: Option<f32>,
) -> Result<Vec<MatchSegment>, String> {
  use tauri::Emitter;

  let settings = video_matcher_get_settings();
  let interval = interval_sec.unwrap_or(settings.interval_sec);
  // 参数钳制：不信任调用方传入值（旧设置文件可能保存了 pHash 时代的 0.85/10 组合，
  // 该组合在 DINOv2 余弦相似度下会把匹配全部过滤掉）。合法域：阈值 0.4-0.75，min_frames 1-6
  let threshold = threshold.unwrap_or(settings.threshold).clamp(0.40, 0.75);
  let min_frames = min_frames.unwrap_or(settings.min_frames).clamp(1, 6) as usize;
  let extractor = settings.extractor.clone();
  let cache_dir = resolve_cache_dir(&settings);

  log::info!(
    "[video_matcher] 匹配开始 | 对标: {source_path} | 原片: {target_path} | 模型: {extractor} | 阈值: {threshold} | min_frames: {min_frames} | 间隔: {interval}s | 缓存目录: {}",
    cache_dir.display()
  );

  let emit = |app: &tauri::AppHandle, stage: &str, percent: u32, msg: &str| {
    let _ = app.emit(
      "video-matcher-progress",
      MatcherProgress {
        stage: stage.to_string(),
        percent,
        message: msg.to_string(),
      },
    );
  };

  let has_cache = |p: &str| {
    let vid = video_id_for(p);
    cache_path_for(&cache_dir, &vid, &extractor, interval).exists()
  };
  let src_cached = has_cache(&source_path);
  let tgt_cached = has_cache(&target_path);
  log::info!(
    "[video_matcher] 缓存状态: 对标={} 原片={}",
    if src_cached { "命中" } else { "未命中" },
    if tgt_cached { "命中" } else { "未命中" }
  );

  let src_name = std::path::Path::new(&source_path)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| "对标视频".to_string());
  let tgt_name = std::path::Path::new(&target_path)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| "原视频".to_string());

  emit(
    &app,
    "extract_source",
    5,
    &if src_cached {
      format!("对标「{src_name}」特征（缓存命中）")
    } else {
      format!("提取对标「{src_name}」特征...")
    },
  );
  let source_hashes = extract_frame_hashes(
    &app,
    &source_path,
    interval,
    &cache_dir,
    &extractor,
    &format!("对标: {src_name}"),
    5,
    40,
  )
  .await?;
  log::info!(
    "[video_matcher] 对标特征: {} 帧 (维度 {})",
    source_hashes.len(),
    source_hashes.first().map(|f| f.feature.len()).unwrap_or(0)
  );

  emit(
    &app,
    "extract_target",
    50,
    &if tgt_cached {
      format!("原片「{tgt_name}」特征（缓存命中）")
    } else {
      format!("提取原片「{tgt_name}」特征...")
    },
  );
  let target_hashes = extract_frame_hashes(
    &app,
    &target_path,
    interval,
    &cache_dir,
    &extractor,
    &format!("原片: {tgt_name}"),
    50,
    30,
  )
  .await?;
  log::info!(
    "[video_matcher] 原片特征: {} 帧 (维度 {})",
    target_hashes.len(),
    target_hashes.first().map(|f| f.feature.len()).unwrap_or(0)
  );

  if source_hashes.first().map(|f| f.feature.len())
    != target_hashes.first().map(|f| f.feature.len())
  {
    let msg = "特征维度不匹配——缓存可能来自不同模型，请清除缓存后重试".to_string();
    log::error!("[video_matcher] {msg}");
    return Err(msg);
  }

  emit(&app, "matching", 85, "正在匹配...");
  let segments = find_matching_segments(
    &source_hashes,
    &target_hashes,
    threshold,
    min_frames,
    interval,
  );

  log::info!(
    "[video_matcher] 匹配完成: {} 个片段 | 总帧覆盖 {}",
    segments.len(),
    segments.iter().map(|s| s.matched_frames).sum::<u32>()
  );
  for (i, s) in segments.iter().enumerate().take(30) {
    log::info!(
      "[video_matcher]   片段{}: 对标 {:.1}-{:.1}s 原片 {:.1}-{:.1}s sim={:.3} {}帧",
      i + 1,
      s.source_start,
      s.source_end,
      s.target_start,
      s.target_end,
      s.similarity,
      s.matched_frames
    );
  }

  emit(
    &app,
    "done",
    100,
    &format!("匹配完成，{} 个片段", segments.len()),
  );
  Ok(segments)
}

/// Tauri 命令：提取视频特征（供调试用）
#[tauri::command]
pub async fn video_extract_phash(
  app: tauri::AppHandle,
  video_path: String,
  interval_sec: Option<f32>,
) -> Result<Vec<(f32, String)>, String> {
  let settings = video_matcher_get_settings();
  let interval = interval_sec.unwrap_or(settings.interval_sec);
  let cache_dir = resolve_cache_dir(&settings);
  let extractor = settings.extractor.clone();

  let features = extract_frame_hashes(
    &app,
    &video_path,
    interval,
    &cache_dir,
    &extractor,
    "extract_source",
    0,
    100,
  )
  .await?;
  Ok(
    features
      .into_iter()
      .map(|f| {
        let brief = if f.feature.len() > 8 {
          format!("{:?}", &f.feature[..8])
        } else {
          format!("{:?}", f.feature)
        };
        (f.timestamp, brief)
      })
      .collect(),
  )
}

/// Tauri 命令：获取所有可选模型的状态
#[tauri::command]
pub fn video_matcher_model_status() -> Vec<serde_json::Value> {
  let ids = [
    "dinov2_vits14",
    "dinov2_vitb14",
    "dinov2_vitl14",
    "resnet18",
    "resnet50",
    "resnet101",
    "resnet152",
    "efficientnet_b0",
    "efficientnet_b4",
    "efficientnet_b7",
    "vit_b_16",
    "vit_l_16",
  ];
  ids
    .iter()
    .filter_map(|id| {
      let (filename, url, dim, display, approx_mb, img_size) = model_registry(id)?;
      let path = models_dir().join(filename);
      let exists = path.exists()
        && std::fs::metadata(&path)
          .map(|m| m.len() > 1024 * 1024)
          .unwrap_or(false);
      Some(serde_json::json!({
        "id": id,
        "name": display,
        "featureDim": dim,
        "approxSizeMb": approx_mb,
        "imgSize": img_size,
        "exists": exists,
        "path": path.to_string_lossy(),
        "sizeBytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        "downloadUrl": url,
      }))
    })
    .collect()
}

/// Tauri 命令：删除模型
#[tauri::command]
pub fn video_matcher_delete_model(model_id: String) -> Result<bool, String> {
  let Some(path) = model_path(&model_id) else {
    return Err(format!("未知模型: {model_id}"));
  };
  if path.exists() {
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    Ok(true)
  } else {
    Ok(false)
  }
}

/// 模型下载控制：正在下载的模型 id + 取消标志
static DOWNLOADING_MODEL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DOWNLOAD_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tauri 命令：查询指定模型是否正在下载
#[tauri::command]
pub fn video_matcher_is_downloading(model_id: String) -> bool {
  use std::hash::{Hash, Hasher};
  let mut h = std::collections::hash_map::DefaultHasher::new();
  model_id.hash(&mut h);
  DOWNLOADING_MODEL.load(std::sync::atomic::Ordering::Acquire) == h.finish()
}

/// Tauri 命令：取消模型下载
#[tauri::command]
pub fn video_matcher_cancel_download(model_id: String) -> bool {
  use std::hash::{Hash, Hasher};
  let mut h = std::collections::hash_map::DefaultHasher::new();
  model_id.hash(&mut h);
  if DOWNLOADING_MODEL.load(std::sync::atomic::Ordering::Acquire) == h.finish() {
    DOWNLOAD_CANCEL.store(true, std::sync::atomic::Ordering::Release);
    true
  } else {
    false
  }
}

/// Tauri 命令：手动下载模型（带进度事件，可取消）
#[tauri::command]
pub async fn video_matcher_download_model(
  app: tauri::AppHandle,
  model_id: String,
) -> Result<(), String> {
  use std::hash::{Hash, Hasher};
  use std::sync::atomic::Ordering;
  use tauri::Emitter;

  let Some((filename, url, _, display_name, _, _)) = model_registry(&model_id) else {
    return Err(format!("未知模型: {model_id}"));
  };

  let mut h = std::collections::hash_map::DefaultHasher::new();
  model_id.hash(&mut h);
  let key = h.finish();

  // 已在下载则直接返回
  if DOWNLOADING_MODEL
    .compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire)
    .is_err()
  {
    return Err("已有模型正在下载，请等待完成或先取消".to_string());
  }
  DOWNLOAD_CANCEL.store(false, Ordering::Release);

  let result = async {
    let path = models_dir().join(filename);
    if path.exists()
      && std::fs::metadata(&path)
        .map(|m| m.len() > 1024 * 1024)
        .unwrap_or(false)
    {
      return Ok(());
    }

    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if try_copy_bundled_model(&model_id, &path) {
      return Ok(());
    }

    let tmp = dir.join(format!("{filename}.tmp"));
    let _ = app.emit(
      "video-matcher-progress",
      MatcherProgress {
        stage: format!("model_download:{model_id}"),
        percent: 0,
        message: format!("开始下载 {display_name}"),
      },
    );

    let response = reqwest::get(&url)
      .await
      .map_err(|e| format!("下载请求失败: {e}"))?;
    if !response.status().is_success() {
      return Err(format!("下载失败: HTTP {}", response.status()));
    }
    let total = response.content_length().unwrap_or(0);

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&tmp)
      .await
      .map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut last_emit = 0u64;

    while let Some(chunk) = stream.next().await {
      if DOWNLOAD_CANCEL.load(Ordering::Acquire) {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err("已取消下载".to_string());
      }
      let chunk = chunk.map_err(|e| e.to_string())?;
      tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
        .await
        .map_err(|e| e.to_string())?;
      downloaded += chunk.len() as u64;
      if total > 0 && downloaded - last_emit > 1024 * 1024 {
        last_emit = downloaded;
        let percent = (downloaded as f32 / total as f32 * 100.0) as u32;
        let _ = app.emit(
          "video-matcher-progress",
          MatcherProgress {
            stage: format!("model_download:{model_id}"),
            percent,
            message: format!(
              "{display_name} · {:.1}/{:.1} MB",
              downloaded as f32 / 1048576.0,
              total as f32 / 1048576.0
            ),
          },
        );
      }
    }
    use tokio::io::AsyncWriteExt;
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;

    let _ = app.emit(
      "video-matcher-progress",
      MatcherProgress {
        stage: format!("model_download:{model_id}"),
        percent: 100,
        message: format!("{display_name} 下载完成"),
      },
    );
    Ok(())
  }
  .await;

  DOWNLOADING_MODEL.store(0, Ordering::Release);
  result
}

/// Tauri 命令：预热推理会话（后台初始化 ONNX + GPU EP，让设备徽章立即可见）
#[tauri::command]
pub async fn video_matcher_warmup(app: tauri::AppHandle) -> Result<(), String> {
  // 后台跑，不阻塞前端
  tauri::async_runtime::spawn(async move {
    let settings = video_matcher_get_settings();
    let extractor = settings.extractor.clone();
    // 模型不存在则静默跳过（首次匹配时会走下载流程）
    if model_path(&extractor).map(|p| p.exists()).unwrap_or(false) {
      match onnx_session(&extractor) {
        Ok(_) => log::info!("[video_matcher] 预热完成，设备就绪"),
        Err(e) => log::warn!("[video_matcher] 预热失败: {e}"),
      }
    }
    let _ = app;
  });
  Ok(())
}

/// Tauri 命令：查询当前推理设备（gpu/cpu，会话未初始化时 unknown）
#[tauri::command]
pub fn video_matcher_device() -> String {
  match ACTIVE_DEVICE.load(std::sync::atomic::Ordering::Acquire) {
    2 => "gpu".to_string(),
    1 => "cpu".to_string(),
    _ => "unknown".to_string(),
  }
}

/// 秒转微秒
fn sec_to_us(seconds: f32) -> i64 {
  (seconds * 1_000_000.0).round() as i64
}

/// 获取视频时长（秒），失败返回 None
async fn get_video_duration(ffmpeg: &Path, video_path: &str) -> Option<f32> {
  let output = tokio::process::Command::new(ffmpeg)
    .arg("-i")
    .arg(video_path)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .output()
    .await
    .ok()?;
  let info = String::from_utf8_lossy(&output.stderr);
  for line in info.lines() {
    if let Some(dur_str) = line.split("Duration:").nth(1) {
      if let Some(time_str) = dur_str.split(",").next() {
        let parts: Vec<&str> = time_str.trim().split(":").collect();
        if parts.len() == 3 {
          let h: f32 = parts[0].parse().ok()?;
          let m: f32 = parts[1].parse().ok()?;
          let s: f32 = parts[2].parse().ok()?;
          return Some(h * 3600.0 + m * 60.0 + s);
        }
      }
    }
  }
  None
}

/// Tauri 命令：导出剪映草稿
/// 对齐模式：每个匹配片段放在对标视频时间轴位置，素材按 1:1 原速引用原视频文件
#[tauri::command]
pub async fn video_export_jianying_draft(
  app: tauri::AppHandle,
  segments: Vec<MatchSegment>,
  source_video: String,
  output_dir: String,
  draft_name: Option<String>,
  reference_duration: Option<f32>,
) -> Result<String, String> {
  use tauri::Emitter;

  if segments.is_empty() {
    return Err("没有可导出的匹配片段".to_string());
  }
  if !Path::new(&source_video).exists() {
    return Err(format!("原视频文件不存在: {}", source_video));
  }

  let _ = app.emit(
    "video-matcher-progress",
    MatcherProgress {
      stage: "exporting".to_string(),
      percent: 10,
      message: "准备导出草稿...".to_string(),
    },
  );

  let draft_name = draft_name.unwrap_or_else(|| "视频匹配".to_string());
  let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
  let full_name = format!("{}_{}", draft_name, timestamp);

  let draft_dir = Path::new(&output_dir).join(&full_name);
  std::fs::create_dir_all(draft_dir.join("materials").join("video"))
    .map_err(|e| format!("创建草稿目录失败: {}", e))?;

  // 复制原视频到草稿目录（保证剪映打开不丢素材）
  let file_name = Path::new(&source_video)
    .file_name()
    .unwrap_or_default()
    .to_string_lossy()
    .to_string();
  let dst = draft_dir.join("materials").join("video").join(&file_name);
  let material_path = if dst.exists() {
    dst.to_string_lossy().replace('\\', "/")
  } else {
    std::fs::copy(&source_video, &dst)
      .map(|_| dst.to_string_lossy().replace('\\', "/"))
      .unwrap_or_else(|_| source_video.clone().replace('\\', "/"))
  };

  let _ = app.emit(
    "video-matcher-progress",
    MatcherProgress {
      stage: "exporting".to_string(),
      percent: 40,
      message: "复制素材完成".to_string(),
    },
  );

  let ffmpeg = crate::app_dirs::data_dir()
    .join("video-tools")
    .join(if cfg!(windows) {
      "ffmpeg.exe"
    } else {
      "ffmpeg"
    });
  let total_duration = get_video_duration(&ffmpeg, &material_path)
    .await
    .unwrap_or(0.0);

  let mat_id = uuid::Uuid::new_v4().to_string();
  let draft_id = uuid::Uuid::new_v4().to_string();

  // 素材路径（draft_content.json 要求绝对路径正斜杠）
  // 注意：Windows 的 canonicalize 返回 \\?\C:\... 前缀，剪映无法识别，必须去掉
  let abs_material = std::fs::canonicalize(&material_path)
    .map(|p| {
      p.to_string_lossy()
        .replace("\\\\?\\", "")
        .replace('\\', "/")
    })
    .unwrap_or_else(|_| material_path.clone());

  // ---------- 视频轨道片段 ----------
  let mut video_segments = Vec::new();
  let mut speed_materials = Vec::new();

  for (idx, seg) in segments.iter().enumerate() {
    let seg_id = uuid::Uuid::new_v4().to_string();
    let speed_id = uuid::Uuid::new_v4().to_string();

    // CutMatch export_from_segments 同款变速对齐：
    // 时间线位置/时长 = 对标段；素材截取完整原片段；
    // speed = 素材时长/对标时长（0.25~4.0 限幅），剪映按 speed 自动变速，
    // 保证每段画面节奏与对标一致（对标作者对素材做的加减速被还原）
    let timeline_start = seg.source_start;
    let timeline_dur = (seg.source_end - seg.source_start).max(0.04);
    let src_start = seg.target_start;
    let src_dur = (seg.target_end - seg.target_start).max(0.04);
    let raw_speed = src_dur / timeline_dur;
    let speed = raw_speed.clamp(0.25, 4.0);
    // 速度被限幅后，实际取用的素材时长 = 时间线时长 × speed（保持时间线时长恒定）
    let src_used_dur = timeline_dur * speed;

    log::info!(
      "[video_matcher] 片段{idx}: 对标 {timeline_start:.1}+{timeline_dur:.1}s 素材 {src_start:.1}+{src_used_dur:.1}s speed={speed:.2} (素材完整长 {src_dur:.1}s)"
    );

    speed_materials.push(serde_json::json!({
      "id": speed_id,
      "material_id": speed_id,
      "type": "speed",
      "speed": speed,
      "mode": 0,
      "curve_speed": null,
    }));

    video_segments.push(serde_json::json!({
      "id": seg_id,
      "material_id": mat_id,
      "target_timerange": {
        "start": sec_to_us(timeline_start),
        "duration": sec_to_us(timeline_dur),
      },
      "source_timerange": {
        "start": sec_to_us(src_start),
        "duration": sec_to_us(src_used_dur),
      },
      "extra_material_refs": [speed_id],
      "clip": {
        "alpha": 1.0,
        "rotation": 0,
        "scale": {"x": 1.0, "y": 1.0},
        "transform": {"x": 0.0, "y": 0.0},
        "flip": {"horizontal": false, "vertical": false},
      },
      "speed": speed,
      "volume": 1.0,
      "visible": true,
      "render_index": idx,
      "render_uniform_index": -1,
      "common_keyframes": [],
      "uniform_scale": {"on": true, "value": 1.0},
      "group_id": "",
      "track_render_index": 0,
      "enable_adjust": true,
      "enable_color_curves": true,
      "enable_color_match_adjust": false,
      "enable_color_wheels": true,
      "enable_lut": true,
      "enable_smart_color_adjust": false,
      "is_loop": false,
      "cartoon": false,
      "intensifies_audio": false,
      "last_nonzero_volume": 1.0,
    }));
  }

  // 尾部占位：对标真实总长 > 最后匹配段结束点时，剪映时间线会在尾部提前结束
  // （剪映总长由最后一个片段的 end 决定，顶层 duration 字段无效）。
  // 用素材的最后 1 秒画面循环追加，填满缺口，保证总长与对标一致。
  // 注意：source_video 参数是【原视频（素材）】而非对标——不能探测它当对标时长；
  // 对标总长由前端传入（对标播放器已加载的 duration）。
  let ref_dur = reference_duration
    .unwrap_or_else(|| segments.iter().map(|s| s.source_end).fold(0.0f32, f32::max));
  let last_end = segments.iter().map(|s| s.source_end).fold(0.0f32, f32::max);
  let gap = ref_dur - last_end;
  let freeze_dur = gap.max(0.0);
  if freeze_dur > 0.04 {
    let last = segments
      .iter()
      .max_by(|a, b| a.source_end.partial_cmp(&b.source_end).unwrap());
    if let Some(last) = last {
      let seg_id = uuid::Uuid::new_v4().to_string();
      // 冻结帧：source_timerange 取素材末端 0.04s 一帧，配合 speed 极慢拉满 freeze_dur
      // （剪映无直接 freeze 字段，用极长 source 片段 + 高 speed 的逆做法：取极短素材 + 极慢 speed）
      // 简化实现：直接引用素材结尾 1 秒循环播放（is_loop=true）铺满缺口
      let freeze_speed = 1.0f32;
      let tail_start = (last.target_end - 1.0).max(0.0);
      speed_materials.push(serde_json::json!({
        "id": seg_id,
        "material_id": seg_id,
        "type": "speed",
        "speed": freeze_speed,
        "mode": 0,
        "curve_speed": null,
      }));
      video_segments.push(serde_json::json!({
        "id": seg_id,
        "material_id": mat_id,
        "target_timerange": {
          "start": sec_to_us(last_end),
          "duration": sec_to_us(freeze_dur),
        },
        "source_timerange": {
          "start": sec_to_us(tail_start),
          "duration": sec_to_us(1.0_f32.min(freeze_dur)),
        },
        "extra_material_refs": [seg_id],
        "clip": {
          "alpha": 1.0,
          "rotation": 0,
          "scale": {"x": 1.0, "y": 1.0},
          "transform": {"x": 0.0, "y": 0.0},
          "flip": {"horizontal": false, "vertical": false},
        },
        "speed": freeze_speed,
        "volume": 0.0,
        "visible": true,
        "render_index": 999,
        "render_uniform_index": -1,
        "common_keyframes": [],
        "uniform_scale": {"on": true, "value": 1.0},
        "group_id": "",
        "track_render_index": 0,
        "enable_adjust": true,
        "enable_color_curves": true,
        "enable_color_match_adjust": false,
        "enable_color_wheels": true,
        "enable_lut": true,
        "enable_smart_color_adjust": false,
        "is_loop": true,
        "cartoon": false,
        "intensifies_audio": false,
        "last_nonzero_volume": 0.0,
      }));
      log::info!(
        "[video_matcher] 尾部占位: 对标 {ref_dur:.1}s > 最后匹配 {last_end:.1}s，追加 {freeze_dur:.1}s 静帧循环（素材 {tail_start:.1}s 起，静音）"
      );
    }
  }

  let draft_content = serde_json::json!({
    "id": draft_id,
    "name": full_name,
    "duration": sec_to_us(ref_dur),
    "fps": 30.0,
    "create_time": chrono::Utc::now().timestamp_millis(),
    "update_time": chrono::Utc::now().timestamp_millis(),
    "canvas_config": {
      "width": 1920,
      "height": 1080,
      "ratio": "16:9",
      "background": null,
    },
    "platform": {
      "app_source": "lv",
      "app_version": "12.0.0",
      "os": "win",
    },
    "tracks": [
      {
        "id": uuid::Uuid::new_v4().to_string(),
        "type": "video",
        "name": "主轨道",
        "segments": video_segments,
        "attribute": 0,
        "flag": 0,
        "is_default_name": true,
      },
    ],
    "materials": {
      "videos": [{
        "id": mat_id,
        "material_id": mat_id,
        "material_name": file_name,
        "path": abs_material,
        "media_path": "",
        "type": "video",
        "duration": sec_to_us(total_duration),
        "width": 1920,
        "height": 1080,
        "source_platform": 0,
        "category_name": "local",
        "local_material_id": "",
        "check_flag": 63487,
        "crop": {
          "upper_left_x": 0, "upper_left_y": 0,
          "upper_right_x": 1, "upper_right_y": 0,
          "lower_left_x": 0, "lower_left_y": 1,
          "lower_right_x": 1, "lower_right_y": 1,
        },
        "crop_ratio": "free",
        "crop_scale": 1.0,
        "audio_fade": null,
        "category_id": "",
        "file_size": std::fs::metadata(&material_path).map(|m| m.len()).unwrap_or(0),
      }],
      "audios": [],
      "texts": [],
      "stickers": [],
      "video_effects": [],
      "material_animations": [],
      "transitions": [],
      "masks": [],
      "canvases": [],
      "speeds": speed_materials,
      "placeholder_infos": [],
      "vocal_separations": [],
      "sound_channel_mappings": [],
      "smart_crops": [],
      "manual_deformations": [],
    },
    "keyframes": {
      "adjusts": [], "audios": [], "effects": [], "filters": [],
      "handwrites": [], "stickers": [],
    },
    "config": {
      "adjust_max_index": 1,
      "attachment_info": [],
      "combination_max_index": 1,
      "export_range": null,
      "extract_audio_last_index": 1,
      "lyrics_recognition_id": "",
      "lyrics_sync": true,
      "lyrics_taskinfo": [],
      "maintrack_adsorb": true,
      "material_save_mode": 0,
      "original_sound_last_index": 1,
      "record_audio_last_index": 1,
      "sticker_max_index": 1,
      "subtitle_recognition_id": "",
      "subtitle_sync": true,
      "subtitle_taskinfo": [],
      "system_font_list": [],
      "video_mute": false,
      "zoom_info_params": null,
    },
    "extra_info": {
      "generated_by": "BW Browser VideoMatcher",
      "segment_count": segments.len(),
    },
    "cover": null,
    "keyframe_graph_list": [],
    "draft_version": "3.0",
    "version": 3,
    "last_export_time": 0,
    "last_export_fps": 30.0,
    "last_export_width": 1920,
    "last_export_height": 1080,
    "free_render_index_mode_on": false,
  });

  std::fs::write(
    draft_dir.join("draft_content.json"),
    serde_json::to_string_pretty(&draft_content).map_err(|e| e.to_string())?,
  )
  .map_err(|e| format!("写入 draft_content.json 失败: {}", e))?;

  // draft_meta_info.json
  let meta_info = serde_json::json!({
    "draft_id": draft_id,
    "draft_name": full_name,
    "create_time": chrono::Utc::now().timestamp_millis(),
    "modify_time": chrono::Utc::now().timestamp_millis(),
    "draft_folder_path": draft_dir.to_string_lossy().replace('\\', "/"),
    "draft_root_path": draft_dir.to_string_lossy().replace('\\', "/"),
    "draft_materials": [{
      "id": mat_id,
      "type": 0,
      "value": [{
        "id": mat_id,
        "extra_info": file_name,
        "file_Path": abs_material,
        "metetype": "video",
      }],
    }],
    "draft_timeline_materials_size_": 0,
    "draft_cover": "",
    "draft_status": "editing",
    "resolution": "1080p",
    "frame_rate": 30.0,
    "tm_draft_create": chrono::Utc::now().timestamp(),
    "tm_draft_cloud_modified": 0,
  });
  std::fs::write(
    draft_dir.join("draft_meta_info.json"),
    serde_json::to_string_pretty(&meta_info).map_err(|e| e.to_string())?,
  )
  .map_err(|e| format!("写入 draft_meta_info.json 失败: {}", e))?;

  // 辅助配置文件
  let _ = std::fs::write(
    draft_dir.join("draft_settings"),
    r#"{"draft_version": "3.0", "auto_save_interval": 120}"#,
  );
  let _ = std::fs::write(
    draft_dir.join("draft_agency_config.json"),
    r#"{"agency_platform": "pc", "agency_id": "BWBrowser"}"#,
  );
  let _ = std::fs::write(draft_dir.join("draft_biz_config.json"), "");
  let _ = std::fs::write(
    draft_dir.join("attachment_pc_common.json"),
    serde_json::json!({"draft_id": draft_id, "attachments": []}).to_string(),
  );
  let _ = std::fs::write(
    draft_dir.join("draft_virtual_store.json"),
    r#"{"virtual_materials": []}"#,
  );

  let _ = app.emit(
    "video-matcher-progress",
    MatcherProgress {
      stage: "done".to_string(),
      percent: 100,
      message: "草稿导出完成".to_string(),
    },
  );

  Ok(draft_dir.to_string_lossy().to_string())
}
