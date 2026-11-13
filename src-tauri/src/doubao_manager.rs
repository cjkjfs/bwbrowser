use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A doubao video account: one doubao web login riding a BW environment.
/// The login session itself lives in that environment's browser data dir;
/// this record carries the pool bookkeeping (identity, quota, state).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DoubaoAccount {
  pub id: String,
  pub display_name: String,
  pub doubao_user_id: Option<String>,
  pub doubao_nickname: Option<String>,
  pub login_account: Option<String>,
  /// The BW environment (profile) this account browses through.
  pub profile_id: Option<String>,
  /// Server-side account id on the cookie-sync backend, once matched.
  pub server_id: Option<i64>,
  /// Cloud account display fields mirrored at load time: device id (手机编号)
  /// and the assigned proxy node.
  pub phone: Option<String>,
  pub proxy_node: Option<String>,
  pub status: String,
  pub enabled: bool,
  pub last_verified_at: Option<u64>,
  pub last_error: Option<String>,
  pub video_quota_used: u32,
  /// Business date (yyyy-mm-dd) `video_quota_used` counts against; resets at
  /// the configured quota reset time, not midnight.
  pub video_quota_date: Option<String>,
  pub video_limited_until: Option<u64>,
  pub created_at: u64,
  pub updated_at: u64,
}

impl Default for DoubaoAccount {
  fn default() -> Self {
    Self {
      id: String::new(),
      display_name: String::new(),
      doubao_user_id: None,
      doubao_nickname: None,
      login_account: None,
      profile_id: None,
      server_id: None,
      phone: None,
      proxy_node: None,
      status: "active".to_string(),
      enabled: true,
      last_verified_at: None,
      last_error: None,
      video_quota_used: 0,
      video_quota_date: None,
      video_limited_until: None,
      created_at: 0,
      updated_at: 0,
    }
  }
}

/// One seedance video generation task. Status flow:
/// queued -> running -> (awaiting_confirm) -> completed | failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DoubaoVideoTask {
  pub id: String,
  pub account_id: Option<String>,
  pub prompt: String,
  pub model: String,
  pub ratio: String,
  pub duration: u32,
  /// "t2v" (no images) or "i2v" (1-9 images); never stored both ways.
  pub mode: String,
  pub image_paths: Vec<String>,
  pub status: String,
  pub conversation_id: Option<String>,
  pub section_id: Option<String>,
  pub question_id: Option<String>,
  pub remote_task_id: Option<String>,
  pub vid: Option<String>,
  pub result_url: Option<String>,
  pub backup_result_url: Option<String>,
  pub fallback_result_url: Option<String>,
  pub cover_url: Option<String>,
  pub local_file: Option<String>,
  pub local_thumb: Option<String>,
  pub error_message: Option<String>,
  pub created_at: u64,
  pub updated_at: u64,
  pub completed_at: Option<u64>,
}

impl Default for DoubaoVideoTask {
  fn default() -> Self {
    Self {
      id: String::new(),
      account_id: None,
      prompt: String::new(),
      model: "seedance_v2.0_mini".to_string(),
      ratio: "1:1".to_string(),
      duration: 5,
      mode: "t2v".to_string(),
      image_paths: Vec::new(),
      status: "queued".to_string(),
      conversation_id: None,
      section_id: None,
      question_id: None,
      remote_task_id: None,
      vid: None,
      result_url: None,
      backup_result_url: None,
      fallback_result_url: None,
      cover_url: None,
      local_file: None,
      local_thumb: None,
      error_message: None,
      created_at: 0,
      updated_at: 0,
      completed_at: None,
    }
  }
}

/// Pool-wide settings. Values are clamped (not rejected) on save so the
/// settings page can never wedge the pool with an out-of-range value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DoubaoSettings {
  pub max_concurrency: u32,
  pub quota_enabled: bool,
  pub daily_quota: u32,
  /// "HH:MM" local reset time for the daily quota window.
  pub quota_reset_time: String,
  /// "least_used" or "round_robin".
  pub scheduler_strategy: String,
  pub default_model: String,
  pub default_duration: u32,
  pub default_ratio: String,
  /// Seconds to wait for a generation to finish before giving up.
  pub video_timeout: u32,
  /// Where generated videos land; empty = `<data>/doubao/downloads`.
  pub download_dir: Option<String>,
  pub sync_enabled: bool,
  pub sync_api_url: String,
  pub sync_username: String,
  pub sync_password: String,
}

impl Default for DoubaoSettings {
  fn default() -> Self {
    Self {
      max_concurrency: 1,
      quota_enabled: true,
      daily_quota: 5,
      quota_reset_time: "00:00".to_string(),
      scheduler_strategy: "least_used".to_string(),
      default_model: "seedance_v2.0_mini".to_string(),
      default_duration: 5,
      default_ratio: "9:16".to_string(),
      video_timeout: 420,
      download_dir: None,
      sync_enabled: false,
      sync_api_url: String::new(),
      sync_username: String::new(),
      sync_password: String::new(),
    }
  }
}

pub const MODELS: [&str; 3] = ["seedance_v2.0_std", "seedance_v2.0", "seedance_v2.0_mini"];
pub const RATIOS: [&str; 6] = ["1:1", "3:4", "4:3", "9:16", "16:9", "21:9"];
pub const DURATIONS: [u32; 3] = [5, 10, 15];
pub const MAX_I2V_IMAGES: usize = 9;

/// One reference image in the pool's material library. Files are copied into
/// `<data>/doubao/materials/` so tasks keep working after the original moves.
/// `category` groups the pool ("角色"/"场景"/…，空=未分类) and `cloud_url` is the
/// server URL once the file has been pushed to the cloud pool — empty means
/// 还没上传.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DoubaoMaterial {
  pub id: String,
  pub name: String,
  pub path: String,
  pub size: u64,
  pub created_at: u64,
  pub category: String,
  pub cloud_url: String,
}

const MATERIAL_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const MAX_MATERIAL_BYTES: u64 = 50 * 1024 * 1024;

fn materials_dir() -> PathBuf {
  crate::app_dirs::doubao_dir().join("materials")
}

fn materials_index_path() -> PathBuf {
  crate::app_dirs::doubao_dir().join("materials.json")
}

pub fn list_materials() -> Vec<DoubaoMaterial> {
  read_json(&materials_index_path()).unwrap_or_default()
}

fn persist_materials(materials: &[DoubaoMaterial]) -> Result<(), String> {
  write_json(&materials_index_path(), materials).map_err(|e| e.to_string())
}

/// Import (copy) images into the library. The whole batch is validated before
/// anything is copied, so a bad path aborts without leaving orphans. Same
/// name+size entries already in the library are skipped as duplicates.
pub fn import_materials(paths: &[String], category: &str) -> Result<Vec<DoubaoMaterial>, String> {
  let category = category.trim();
  let mut sources: Vec<(PathBuf, String, u64)> = Vec::new();
  for raw in paths {
    let src = PathBuf::from(raw.trim());
    let ext = src
      .extension()
      .and_then(|e| e.to_str())
      .map(|e| e.to_lowercase())
      .unwrap_or_default();
    if !MATERIAL_EXTENSIONS.contains(&ext.as_str()) {
      return Err(format!(
        "不支持的素材类型: {}（仅支持 png/jpg/jpeg/webp）",
        src.display()
      ));
    }
    let meta = fs::metadata(&src).map_err(|e| format!("读取 {} 失败: {e}", src.display()))?;
    if !meta.is_file() {
      return Err(format!("不是文件: {}", src.display()));
    }
    if meta.len() > MAX_MATERIAL_BYTES {
      return Err(format!("{} 超过 50MB 大小限制", src.display()));
    }
    let name = src
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or("material")
      .to_string();
    sources.push((src, name, meta.len()));
  }
  fs::create_dir_all(materials_dir()).map_err(|e| format!("创建素材目录失败: {e}"))?;
  let mut materials = list_materials();
  let now = crate::proxy_manager::now_secs();
  let mut imported = Vec::new();
  for (src, name, size) in sources {
    if materials.iter().any(|m| m.name == name && m.size == size) {
      continue;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let dest = materials_dir().join(format!("{id}_{name}"));
    fs::copy(&src, &dest).map_err(|e| format!("复制 {name} 失败: {e}"))?;
    let material = DoubaoMaterial {
      id: id.clone(),
      name,
      path: dest.to_string_lossy().to_string(),
      size,
      created_at: now,
      category: category.to_string(),
      cloud_url: String::new(),
    };
    materials.push(material.clone());
    imported.push(material);
  }
  if !imported.is_empty() {
    persist_materials(&materials)?;
  }
  Ok(imported)
}

pub fn delete_material(id: &str) -> bool {
  let mut materials = list_materials();
  let Some(pos) = materials.iter().position(|m| m.id == id) else {
    return false;
  };
  let removed = materials.remove(pos);
  let _ = fs::remove_file(&removed.path);
  persist_materials(&materials).is_ok()
}

fn update_material(id: &str, edit: impl FnOnce(&mut DoubaoMaterial)) -> bool {
  let mut materials = list_materials();
  let Some(material) = materials.iter_mut().find(|m| m.id == id) else {
    return false;
  };
  edit(material);
  persist_materials(&materials).is_ok()
}

pub fn set_material_category(id: &str, category: &str) -> bool {
  let category = category.trim().to_string();
  update_material(id, |material| material.category = category)
}

/// Material uploads ride next to the account endpoint the pool already talks
/// to, so the sibling file of `.../tk/simprint_accounts.php` is the target.
fn materials_cloud_endpoint(api_url: &str) -> Option<String> {
  let trimmed = api_url.trim().trim_end_matches('/');
  if trimmed.is_empty() {
    return None;
  }
  let scheme_end = trimmed.find("://").map(|i| i + 3).unwrap_or(0);
  let tail = &trimmed[scheme_end..];
  match tail.rfind('/') {
    Some(offset) => Some(format!(
      "{}/simprint_materials.php",
      &trimmed[..scheme_end + offset]
    )),
    None => Some(format!("{}/simprint_materials.php", trimmed)),
  }
}

fn material_mime(name: &str) -> &'static str {
  match name
    .rsplit('.')
    .next()
    .unwrap_or_default()
    .to_lowercase()
    .as_str()
  {
    "png" => "image/png",
    "webp" => "image/webp",
    "gif" => "image/gif",
    _ => "image/jpeg",
  }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoubaoMaterialUpload {
  pub id: String,
  pub name: String,
  pub ok: bool,
  pub message: String,
  pub cloud_url: String,
}

async fn upload_one_material(
  client: &reqwest::Client,
  endpoint: &str,
  settings: &DoubaoSettings,
  material: &DoubaoMaterial,
) -> Result<String, String> {
  let bytes = fs::read(&material.path).map_err(|e| format!("读取素材失败: {e}"))?;
  let part = reqwest::multipart::Part::bytes(bytes)
    .file_name(material.name.clone())
    .mime_str(material_mime(&material.name))
    .map_err(|e| e.to_string())?;
  let form = reqwest::multipart::Form::new()
    .text("action", "upload")
    .text("platform", "doubao")
    .text("username", settings.sync_username.clone())
    .text("password", settings.sync_password.clone())
    .text("category", material.category.clone())
    .text("name", material.name.clone())
    .part("file", part);
  let reply = client
    .post(endpoint)
    .multipart(form)
    .send()
    .await
    .map_err(|e| format!("上传失败: {e}"))?;
  let body: serde_json::Value = reply
    .json()
    .await
    .map_err(|e| format!("云端返回的不是 JSON: {e}"))?;
  if body["success"].as_bool() != Some(true) {
    return Err(
      body["message"]
        .as_str()
        .filter(|m| !m.is_empty())
        .unwrap_or("云端拒绝了该素材")
        .to_string(),
    );
  }
  Ok(
    body["url"]
      .as_str()
      .or_else(|| body["path"].as_str())
      .unwrap_or_default()
      .to_string(),
  )
}

/// Push chosen materials to the cloud pool. Per-file failures come back in the
/// list so one bad image never aborts the batch.
pub async fn upload_materials_to_cloud(
  ids: &[String],
) -> Result<Vec<DoubaoMaterialUpload>, String> {
  let settings = get_settings();
  if !settings.sync_enabled {
    return Err("云端同步未开启：请先在「设置」里开启同步并填写云端地址、账号和密码".to_string());
  }
  let endpoint = materials_cloud_endpoint(&settings.sync_api_url)
    .ok_or_else(|| "云端地址为空：请先在「设置」里填写同步接口地址".to_string())?;
  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(180))
    .build()
    .map_err(|e| format!("创建上传客户端失败: {e}"))?;
  let library = list_materials();
  let mut results = Vec::with_capacity(ids.len());
  for id in ids {
    let name = library
      .iter()
      .find(|m| m.id == *id)
      .map(|m| m.name.clone())
      .unwrap_or_default();
    let outcome = match library.iter().find(|m| m.id == *id) {
      None => Err("素材已被删除".to_string()),
      Some(material) => match upload_one_material(&client, &endpoint, &settings, material).await {
        Ok(url) => {
          let saved = if url.is_empty() {
            material.cloud_url.clone()
          } else {
            url.clone()
          };
          update_material(&material.id, |m| m.cloud_url = saved.clone());
          Ok(saved)
        }
        Err(message) => Err(message),
      },
    };
    results.push(match outcome {
      Ok(cloud_url) => DoubaoMaterialUpload {
        id: id.clone(),
        name,
        ok: true,
        message: "已传回云端".to_string(),
        cloud_url,
      },
      Err(message) => DoubaoMaterialUpload {
        id: id.clone(),
        name,
        ok: false,
        message,
        cloud_url: String::new(),
      },
    });
  }
  Ok(results)
}

fn accounts_dir() -> PathBuf {
  crate::app_dirs::doubao_dir().join("accounts")
}

fn tasks_dir() -> PathBuf {
  crate::app_dirs::doubao_dir().join("tasks")
}

fn task_path(id: &str) -> PathBuf {
  tasks_dir().join(format!("{id}.json"))
}

fn settings_path() -> PathBuf {
  crate::app_dirs::doubao_dir().join("settings.json")
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Option<T> {
  fs::read_to_string(path)
    .ok()
    .and_then(|c| serde_json::from_str(&c).ok())
}

fn write_json<T: Serialize + ?Sized>(
  path: &Path,
  value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }
  let content = serde_json::to_string_pretty(value)?;
  crate::app_dirs::write_owner_only(path, content.as_bytes())?;
  Ok(())
}

fn list_json<T: serde::de::DeserializeOwned>(dir: &PathBuf) -> Vec<T> {
  let Ok(entries) = fs::read_dir(dir) else {
    return Vec::new();
  };
  let mut items: Vec<(u64, T)> = entries
    .filter_map(|entry| {
      let entry = entry.ok()?;
      let path = entry.path();
      if path.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
      }
      let item: T = read_json(&path)?;
      let modified = entry
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
      Some((modified, item))
    })
    .collect();
  items.sort_by_key(|(modified, _)| *modified);
  items.into_iter().map(|(_, item)| item).collect()
}

pub fn list_accounts() -> Vec<DoubaoAccount> {
  list_json(&accounts_dir())
}

pub fn persist_account(account: &DoubaoAccount) -> Result<(), Box<dyn std::error::Error>> {
  write_json(
    &accounts_dir().join(format!("{}.json", account.id)),
    account,
  )
}

pub fn delete_account(id: &str) -> bool {
  fs::remove_file(accounts_dir().join(format!("{id}.json"))).is_ok()
}

pub fn list_tasks() -> Vec<DoubaoVideoTask> {
  list_json(&tasks_dir())
}

pub fn persist_task(task: &DoubaoVideoTask) -> Result<(), Box<dyn std::error::Error>> {
  write_json(&tasks_dir().join(format!("{}.json", task.id)), task)
}

pub fn delete_task(id: &str) -> bool {
  fs::remove_file(task_path(id)).is_ok()
}

/// 生成结果删除：任务记录 +（可选）它下载过的视频与封面。
/// 返回真正删掉的文件数，前端据此给轻提示。
pub fn delete_result(id: &str, delete_files: bool) -> usize {
  let mut removed = 0;
  if delete_files {
    if let Some(task) = read_json::<DoubaoVideoTask>(&task_path(id)) {
      let root = download_root();
      for path in [task.local_file, task.local_thumb].into_iter().flatten() {
        if let Some(target) = removable_download(&path, &root) {
          if fs::remove_file(target).is_ok() {
            removed += 1;
          }
        }
      }
    }
  }
  delete_task(id);
  removed
}

/// 只允许删下载目录里的文件：任务记录里的路径被改过也伤不到别的资料。
fn removable_download(path: &str, root: &Path) -> Option<PathBuf> {
  let target = Path::new(path).canonicalize().ok()?;
  let root = root.canonicalize().ok()?;
  target.starts_with(&root).then_some(target)
}

fn download_root() -> PathBuf {
  match get_settings()
    .download_dir
    .as_deref()
    .filter(|dir| !dir.is_empty())
  {
    Some(dir) => PathBuf::from(dir),
    None => crate::app_dirs::doubao_dir().join("downloads"),
  }
}

pub fn get_settings() -> DoubaoSettings {
  read_json(&settings_path()).unwrap_or_default()
}

pub fn persist_settings(settings: &DoubaoSettings) -> Result<(), Box<dyn std::error::Error>> {
  write_json(&settings_path(), settings)
}

fn clamp_settings(mut settings: DoubaoSettings) -> DoubaoSettings {
  settings.max_concurrency = settings.max_concurrency.clamp(1, 10);
  settings.daily_quota = settings.daily_quota.clamp(1, 100);
  settings.video_timeout = settings.video_timeout.clamp(60, 3600);
  let valid_time = {
    let parts: Vec<&str> = settings.quota_reset_time.split(':').collect();
    matches!(parts.as_slice(), [h, m]
      if h.len() == 2 && m.len() == 2
      && h.chars().all(|c| c.is_ascii_digit())
      && m.chars().all(|c| c.is_ascii_digit())
      && h.parse::<u8>().is_ok_and(|v| v < 24)
      && m.parse::<u8>().is_ok_and(|v| v < 60))
  };
  if !valid_time {
    settings.quota_reset_time = "00:00".to_string();
  }
  if !matches!(
    settings.scheduler_strategy.as_str(),
    "least_used" | "round_robin"
  ) {
    settings.scheduler_strategy = "least_used".to_string();
  }
  if !MODELS.contains(&settings.default_model.as_str()) {
    settings.default_model = "seedance_v2.0_mini".to_string();
  }
  if !DURATIONS.contains(&settings.default_duration) {
    settings.default_duration = 5;
  }
  if !RATIOS.contains(&settings.default_ratio.as_str()) {
    settings.default_ratio = "9:16".to_string();
  }
  settings
}

// ============================================================
// Scheduler-facing helpers (quota window, account selection)
// ============================================================

pub fn find_task(id: &str) -> Option<DoubaoVideoTask> {
  list_tasks().into_iter().find(|t| t.id == id)
}

/// Read-modify-persist one task; bumps `updated_at`.
pub fn update_task(
  task_id: &str,
  apply: impl FnOnce(&mut DoubaoVideoTask),
) -> Option<DoubaoVideoTask> {
  let mut task = find_task(task_id)?;
  apply(&mut task);
  task.updated_at = crate::proxy_manager::now_secs();
  persist_task(&task).ok()?;
  Some(task)
}

pub fn assign_task_account(task_id: &str, account_id: Option<String>) {
  let _ = update_task(task_id, |task| task.account_id = account_id);
}

/// Business-day split for the daily quota: the day `video_quota_used` counts
/// against, plus the next reset instant (unix secs). Matches DoubaoManager's
/// Asia/Shanghai-fixed window — a reset at 06:00 means 03:00 usage still
/// counts against yesterday's business day.
pub fn quota_window(
  now_local: chrono::DateTime<chrono_tz::Tz>,
  reset_value: &str,
) -> (String, u64) {
  let (hour, minute) = reset_value
    .split_once(':')
    .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
    .filter(|(h, m)| *h < 24 && *m < 60)
    .unwrap_or((0, 0));
  let reset_today = now_local
    .date_naive()
    .and_hms_opt(hour, minute, 0)
    .and_then(|t| {
      use chrono::TimeZone;
      chrono_tz::Asia::Shanghai.from_local_datetime(&t).single()
    });
  let Some(reset_today) = reset_today else {
    return (now_local.format("%Y-%m-%d").to_string(), 0);
  };
  if now_local < reset_today {
    let business = now_local.date_naive() - chrono::Duration::days(1);
    (
      business.format("%Y-%m-%d").to_string(),
      reset_today.timestamp().max(0) as u64,
    )
  } else {
    let next = reset_today + chrono::Duration::days(1);
    (
      now_local.format("%Y-%m-%d").to_string(),
      next.timestamp().max(0) as u64,
    )
  }
}

/// 豆包把恢复时间写在自己的额度提示里（「预计 11 月 9 日 10:31 恢复」）。
/// 读到就按豆包说的时间锁账号——免费池是按周算的，锁到本日重置点等于每天
/// 白跑一次；读不到由调用方按 `quota_window` 兜底。提示里不写年份，
/// 日期比今天早就落到明年。
pub fn parse_recovery_deadline(
  message: &str,
  now_local: chrono::DateTime<chrono_tz::Tz>,
) -> Option<u64> {
  use chrono::{Datelike, TimeZone};
  let at = message.find("预计")?;
  let tail = &message[at + "预计".len()..];
  if !(tail.contains('月') && tail.contains('日')) {
    return None;
  }
  let (month, tail) = next_number(tail)?;
  let (day, tail) = next_number(tail)?;
  let (hour, minute) = match next_number(tail) {
    Some((hour, rest)) => match next_number(rest) {
      Some((minute, _)) => (hour, minute),
      None => (hour, 0),
    },
    None => (0, 0),
  };
  if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
    return None;
  }
  for year in [now_local.year(), now_local.year() + 1] {
    let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
      continue;
    };
    let Some(local) = date
      .and_hms_opt(hour, minute, 0)
      .and_then(|t| chrono_tz::Asia::Shanghai.from_local_datetime(&t).single())
    else {
      continue;
    };
    if local > now_local {
      return Some(local.timestamp().max(0) as u64);
    }
  }
  None
}

/// `(number, rest)` of the first digit run in `s`; the CJK separators between
/// the numbers are whatever is left over, so the next call just scans again.
fn next_number(s: &str) -> Option<(u32, &str)> {
  let start = s.find(|c: char| c.is_ascii_digit())?;
  let digits = &s[start..];
  let end = digits
    .find(|c: char| !c.is_ascii_digit())
    .unwrap_or(digits.len());
  Some((digits[..end].parse().ok()?, &digits[end..]))
}

/// Zero out quotas for accounts whose stored business date isn't today's.
/// A limit that has not expired yet stays: 豆包的免费池是按周算的，跨过每日重置
/// 放行只会让账号天天白跑一次。
pub fn reset_daily_quotas(business_date: &str) {
  let now = crate::proxy_manager::now_secs();
  for mut account in list_accounts() {
    if account.video_quota_date.as_deref() != Some(business_date) {
      account.video_quota_used = 0;
      account.video_quota_date = Some(business_date.to_string());
      if account.video_limited_until.is_none_or(|until| until <= now) {
        account.video_limited_until = None;
      }
      let _ = persist_account(&account);
    }
  }
}

/// Pick the next account allowed to generate: enabled + active, and (when the
/// quota gate is on) under the daily quota with no standing limit.
pub fn choose_available_account(
  daily_quota: u32,
  strategy: &str,
  quota_enabled: bool,
) -> Option<DoubaoAccount> {
  let now = crate::proxy_manager::now_secs();
  let mut candidates: Vec<DoubaoAccount> = list_accounts()
    .into_iter()
    .filter(|a| a.enabled && a.status == "active")
    // 豆包亲口说额度用完了就锁到它给的时间，这条不受每日额度开关影响
    .filter(|a| a.video_limited_until.is_none_or(|until| until <= now))
    .filter(|a| !quota_enabled || a.video_quota_used < daily_quota)
    .collect();
  if strategy == "round_robin" {
    candidates.sort_by_key(|a| a.updated_at);
  } else {
    candidates.sort_by_key(|a| (a.video_quota_used, a.updated_at));
  }
  candidates.into_iter().next()
}

pub fn mark_account_limited(account_id: &str, limited_until: u64, daily_quota: u32, reason: &str) {
  let _ = update_account(account_id, |account| {
    account.video_quota_used = daily_quota;
    account.video_limited_until = Some(limited_until);
    account.last_error = Some(reason.to_string());
  });
}

pub fn increment_account_quota(account_id: &str) {
  let _ = update_account(account_id, |account| {
    account.video_quota_used = account.video_quota_used.saturating_add(1);
  });
}

pub fn mark_account_inactive(account_id: &str, message: &str) {
  let _ = update_account(account_id, |account| {
    account.status = "inactive".to_string();
    account.last_error = Some(message.to_string());
  });
}

pub fn mark_account_active(account_id: &str) {
  let _ = update_account(account_id, |account| {
    account.status = "active".to_string();
    account.last_error = None;
  });
}

fn update_account(
  account_id: &str,
  apply: impl FnOnce(&mut DoubaoAccount),
) -> Option<DoubaoAccount> {
  let mut account = list_accounts().into_iter().find(|a| a.id == account_id)?;
  apply(&mut account);
  account.updated_at = crate::proxy_manager::now_secs();
  persist_account(&account).ok()?;
  Some(account)
}

#[tauri::command]
pub fn doubao_list_accounts() -> Vec<DoubaoAccount> {
  list_accounts()
}

#[tauri::command]
pub fn doubao_delete_account(id: String) -> bool {
  delete_account(&id)
}

#[derive(Debug, Serialize)]
pub struct DoubaoCloudLoadResult {
  pub loaded: u32,
  pub created: u32,
  pub updated: u32,
  pub bound: u32,
  pub skipped: u32,
}

/// Same rule as the launch flow: derived name first, legacy pure account name
/// only when globally unique (the launch flow renames it in place later).
fn match_cloud_profile(
  profiles: &[crate::profile::types::BrowserProfile],
  account_name: &str,
  account_id: i64,
) -> Option<String> {
  let derived = crate::bwbrowser_cloud::account_profile_name(account_name, account_id);
  profiles
    .iter()
    .find(|p| p.name.to_lowercase() == derived.to_lowercase())
    .or_else(|| {
      let legacy: Vec<_> = profiles
        .iter()
        .filter(|p| p.name.to_lowercase() == account_name.trim().to_lowercase())
        .collect();
      legacy.first().copied().filter(|_| legacy.len() == 1)
    })
    .map(|p| p.id.to_string())
}

/// Pull the cloud account list into the pool. Each cloud account becomes one
/// DoubaoAccount keyed by `server_id`, bound to the local BW environment the
/// launch flow uses for it (profile derived from 「账号名_id」). Re-loading is
/// idempotent: existing pool records are matched by server_id and only their
/// name/login/binding are refreshed; quota, status and enabled stay untouched.
#[tauri::command]
pub async fn doubao_load_cloud_accounts() -> Result<DoubaoCloudLoadResult, String> {
  if crate::bwbrowser_cloud::BWBROWSER_AUTH
    .get_credentials()
    .is_none()
  {
    return Err(serde_json::json!({ "code": "CLOUD_NOT_SIGNED_IN" }).to_string());
  }
  let list = crate::bwbrowser_cloud::BWBROWSER_AUTH
    .list_cloud_accounts(
      1,
      500,
      Some("doubao".to_string()),
      None,
      None,
      None,
      None,
      None,
      None,
      None,
    )
    .await?;
  let cloud_accounts: Vec<_> = list.accounts.unwrap_or_default();
  let profiles = crate::profile::manager::ProfileManager::instance()
    .list_profiles()
    .map_err(|e| format!("读取浏览器配置列表失败: {e}"))?;
  let existing = list_accounts();
  let now = crate::proxy_manager::now_secs();
  let mut result = DoubaoCloudLoadResult {
    loaded: cloud_accounts.len() as u32,
    created: 0,
    updated: 0,
    bound: 0,
    skipped: 0,
  };
  for cloud in &cloud_accounts {
    if cloud.launch_allowed == Some(false) {
      result.skipped += 1;
      continue;
    }
    let profile_id = match_cloud_profile(&profiles, &cloud.account_name, cloud.id);
    if profile_id.is_some() {
      result.bound += 1;
    }
    let display_name = {
      let name = cloud.account_name.trim();
      if name.is_empty() {
        format!("account_{}", cloud.id)
      } else {
        name.to_string()
      }
    };
    if let Some(local) = existing.iter().find(|a| a.server_id == Some(cloud.id)) {
      let mut next = local.clone();
      next.display_name = display_name;
      next.login_account = cloud.login_account.clone();
      next.phone = cloud.device_id.clone();
      next.proxy_node = cloud.proxy_node.clone();
      if let Some(pid) = profile_id {
        next.profile_id = Some(pid);
      }
      next.updated_at = now;
      persist_account(&next)
        .map_err(|e| crate::backend_error_with_detail("INTERNAL_ERROR", e.to_string()))?;
      result.updated += 1;
    } else {
      let account = DoubaoAccount {
        id: uuid::Uuid::new_v4().to_string(),
        display_name,
        login_account: cloud.login_account.clone(),
        phone: cloud.device_id.clone(),
        proxy_node: cloud.proxy_node.clone(),
        profile_id,
        server_id: Some(cloud.id),
        created_at: now,
        updated_at: now,
        ..Default::default()
      };
      persist_account(&account)
        .map_err(|e| crate::backend_error_with_detail("INTERNAL_ERROR", e.to_string()))?;
      result.created += 1;
    }
  }
  // Prune stale pool entries: cloud-loaded records whose server id is no
  // longer in the platform-filtered cloud list (e.g. loaded before the
  // platform filter existed, or the cloud account was removed/re-platformed).
  let loaded_ids: std::collections::HashSet<i64> = cloud_accounts.iter().map(|c| c.id).collect();
  for stale in existing
    .iter()
    .filter(|a| a.server_id.is_some_and(|id| !loaded_ids.contains(&id)))
  {
    let _ = delete_account(&stale.id);
  }
  Ok(result)
}

#[derive(Debug, Serialize)]
pub struct DoubaoAccountHealthView {
  pub account_id: String,
  pub status: String,
  pub message: String,
  pub checked_at: u64,
}

/// Last persisted doubao cookie-health verdict per pool account (via its bound
/// profile). Reads the doubao-specific record: the profile-level aggregate is
/// "any platform valid", which would show a dead doubao session as 登录有效.
#[tauri::command]
pub fn doubao_account_health_states() -> Vec<DoubaoAccountHealthView> {
  list_accounts()
    .iter()
    .map(|a| {
      let record = a
        .profile_id
        .as_deref()
        .and_then(|pid| crate::cookie_health::platform_health(pid, "doubao"));
      match record {
        Some(r) => DoubaoAccountHealthView {
          account_id: a.id.clone(),
          status: crate::cookie_health::health_status_str(&r.status).to_string(),
          message: r.message.clone(),
          checked_at: r.checked_at,
        },
        None => DoubaoAccountHealthView {
          account_id: a.id.clone(),
          status: "unknown".to_string(),
          message: "未检测，点「检测」核对豆包登录态".to_string(),
          checked_at: 0,
        },
      }
    })
    .collect()
}

/// Run a fresh cookie-health probe for one pool account's bound profile and
/// persist the verdict.
#[tauri::command]
pub async fn doubao_check_account_health(
  account_id: String,
) -> Result<DoubaoAccountHealthView, String> {
  let Some(account) = list_accounts().into_iter().find(|a| a.id == account_id) else {
    return Err(serde_json::json!({ "code": "PROFILE_NOT_FOUND" }).to_string());
  };
  let now = crate::proxy_manager::now_secs();
  let Some(profile) = crate::doubao_login::bind_existing_profile(&account) else {
    return Ok(DoubaoAccountHealthView {
      account_id,
      status: "unknown".to_string(),
      message: "环境尚未创建，点「扫码登录」或「打开」后会自动创建".to_string(),
      checked_at: now,
    });
  };
  // 探测本身已把各平台结论分别落盘，这里只取豆包那一条
  let profile_id = profile.id.to_string();
  let _ = crate::cookie_health::check_profile_health(&profile_id, true).await;
  Ok(
    match crate::cookie_health::platform_health(&profile_id, "doubao") {
      Some(r) => DoubaoAccountHealthView {
        account_id,
        status: crate::cookie_health::health_status_str(&r.status).to_string(),
        message: r.message,
        checked_at: r.checked_at,
      },
      None => DoubaoAccountHealthView {
        account_id,
        status: "unknown".to_string(),
        message: "本地无 Cookie，未检测到有效会话".to_string(),
        checked_at: now,
      },
    },
  )
}

#[tauri::command]
pub fn doubao_list_materials() -> Vec<DoubaoMaterial> {
  list_materials()
}

#[tauri::command]
pub fn doubao_import_materials(
  paths: Vec<String>,
  category: Option<String>,
) -> Result<Vec<DoubaoMaterial>, String> {
  import_materials(&paths, category.as_deref().unwrap_or(""))
}

#[tauri::command]
pub fn doubao_set_material_category(id: String, category: String) -> bool {
  set_material_category(&id, &category)
}

#[tauri::command]
pub async fn doubao_upload_materials(
  ids: Vec<String>,
) -> Result<Vec<DoubaoMaterialUpload>, String> {
  upload_materials_to_cloud(&ids).await
}

#[tauri::command]
pub fn doubao_delete_material(id: String) -> bool {
  delete_material(&id)
}

#[tauri::command]
pub fn doubao_list_tasks() -> Vec<DoubaoVideoTask> {
  list_tasks()
}

#[tauri::command]
pub fn doubao_create_task(
  mut account_id: Option<String>,
  prompt: String,
  model: String,
  ratio: String,
  duration: u32,
  image_paths: Vec<String>,
) -> Result<DoubaoVideoTask, String> {
  let prompt = prompt.trim().to_string();
  if prompt.is_empty() {
    return Err(serde_json::json!({ "code": "NAME_CANNOT_BE_EMPTY" }).to_string());
  }
  if image_paths.len() > MAX_I2V_IMAGES {
    return Err(serde_json::json!({ "code": "INTERNAL_ERROR" }).to_string());
  }
  if let Some(account_id) = account_id.as_deref().filter(|id| !id.is_empty()) {
    let usable = list_accounts()
      .into_iter()
      .find(|a| a.id == account_id)
      .is_some_and(|a| a.enabled && a.status == "active");
    if !usable {
      return Err("指定的豆包账号不可用".to_string());
    }
  } else {
    account_id = None;
  }
  let settings = get_settings();
  let model = if MODELS.contains(&model.as_str()) {
    model
  } else {
    settings.default_model
  };
  let ratio = if RATIOS.contains(&ratio.as_str()) {
    ratio
  } else {
    settings.default_ratio
  };
  let duration = if DURATIONS.contains(&duration) {
    duration
  } else {
    settings.default_duration
  };
  let now = crate::proxy_manager::now_secs();
  let task = DoubaoVideoTask {
    id: uuid::Uuid::new_v4().to_string(),
    account_id,
    prompt,
    model,
    ratio,
    duration,
    mode: if image_paths.is_empty() { "t2v" } else { "i2v" }.to_string(),
    image_paths,
    status: "queued".to_string(),
    created_at: now,
    updated_at: now,
    ..Default::default()
  };
  persist_task(&task)
    .map_err(|e| crate::backend_error_with_detail("INTERNAL_ERROR", e.to_string()))?;
  crate::doubao_scheduler::schedule(&task.id, false);
  Ok(task)
}

#[tauri::command]
pub fn doubao_delete_task(id: String) -> bool {
  delete_task(&id)
}

#[tauri::command]
pub fn doubao_delete_result(id: String, delete_files: bool) -> usize {
  delete_result(&id, delete_files)
}

#[tauri::command]
pub fn doubao_get_settings() -> DoubaoSettings {
  get_settings()
}

#[tauri::command]
pub fn doubao_save_settings(settings: DoubaoSettings) -> Result<DoubaoSettings, String> {
  let settings = clamp_settings(settings);
  persist_settings(&settings)
    .map_err(|e| crate::backend_error_with_detail("INTERNAL_ERROR", e.to_string()))?;
  Ok(settings)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_materials_cloud_endpoint_is_the_sibling_file() {
    assert_eq!(
      materials_cloud_endpoint("https://www.yacm.xin/tk/simprint_accounts.php"),
      Some("https://www.yacm.xin/tk/simprint_materials.php".to_string())
    );
    assert_eq!(
      materials_cloud_endpoint("http://127.0.0.1:8080"),
      Some("http://127.0.0.1:8080/simprint_materials.php".to_string())
    );
    assert_eq!(materials_cloud_endpoint("  "), None);
  }

  #[test]
  fn test_material_mime_falls_back_to_jpeg() {
    assert_eq!(material_mime("a.PNG"), "image/png");
    assert_eq!(material_mime("a.webp"), "image/webp");
    assert_eq!(material_mime("a"), "image/jpeg");
  }

  #[test]
  fn test_download_removal_stays_inside_the_root() {
    let root = std::env::temp_dir().join(format!("bwb-dl-{}", uuid::Uuid::new_v4()));
    let video = root.join("video").join("a.mp4");
    fs::create_dir_all(video.parent().unwrap()).unwrap();
    fs::write(&video, b"x").unwrap();
    let outside = std::env::temp_dir().join(format!("bwb-out-{}.mp4", uuid::Uuid::new_v4()));
    fs::write(&outside, b"x").unwrap();

    assert!(removable_download(&video.to_string_lossy(), &root).is_some());
    assert!(removable_download(&outside.to_string_lossy(), &root).is_none());
    assert!(removable_download("Z:/missing/a.mp4", &root).is_none());

    let _ = fs::remove_file(outside);
    let _ = fs::remove_dir_all(&root);
  }

  #[test]
  fn test_clamp_settings_fixes_out_of_range_values() {
    let settings = clamp_settings(DoubaoSettings {
      max_concurrency: 99,
      daily_quota: 0,
      video_timeout: 1,
      quota_reset_time: "25:99".to_string(),
      scheduler_strategy: "bogus".to_string(),
      default_model: "bogus".to_string(),
      default_duration: 7,
      default_ratio: "2:1".to_string(),
      ..Default::default()
    });
    assert_eq!(settings.max_concurrency, 10);
    assert_eq!(settings.daily_quota, 1);
    assert_eq!(settings.video_timeout, 60);
    assert_eq!(settings.quota_reset_time, "00:00");
    assert_eq!(settings.scheduler_strategy, "least_used");
    assert_eq!(settings.default_model, "seedance_v2.0_mini");
    assert_eq!(settings.default_duration, 5);
    assert_eq!(settings.default_ratio, "9:16");
  }

  #[test]
  fn test_clamp_settings_keeps_valid_values() {
    let settings = clamp_settings(DoubaoSettings {
      max_concurrency: 3,
      daily_quota: 20,
      video_timeout: 600,
      quota_reset_time: "06:30".to_string(),
      scheduler_strategy: "round_robin".to_string(),
      default_model: "seedance_v2.0".to_string(),
      default_duration: 10,
      default_ratio: "9:16".to_string(),
      ..Default::default()
    });
    assert_eq!(
      settings,
      DoubaoSettings {
        max_concurrency: 3,
        daily_quota: 20,
        video_timeout: 600,
        quota_reset_time: "06:30".to_string(),
        scheduler_strategy: "round_robin".to_string(),
        default_model: "seedance_v2.0".to_string(),
        default_duration: 10,
        default_ratio: "9:16".to_string(),
        ..Default::default()
      }
    );
  }

  #[test]
  fn test_account_and_task_roundtrip() {
    let account = DoubaoAccount {
      id: "a1".to_string(),
      display_name: "豆包一号".to_string(),
      profile_id: Some("p1".to_string()),
      ..Default::default()
    };
    let json = serde_json::to_string(&account).unwrap();
    let parsed: DoubaoAccount = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, account);
    assert_eq!(parsed.status, "active");

    let task = DoubaoVideoTask {
      id: "t1".to_string(),
      prompt: "a cat".to_string(),
      image_paths: vec!["x.png".to_string()],
      mode: "i2v".to_string(),
      ..Default::default()
    };
    let parsed: DoubaoVideoTask =
      serde_json::from_str(&serde_json::to_string(&task).unwrap()).unwrap();
    assert_eq!(parsed, task);
    assert_eq!(parsed.status, "queued");
  }

  #[test]
  fn test_quota_window_rolls_back_before_reset_time() {
    use chrono::TimeZone;
    let tz = chrono_tz::Asia::Shanghai;
    let before = tz.with_ymd_and_hms(2026, 11, 4, 5, 59, 0).unwrap();
    let (business, next_reset) = quota_window(before, "06:00");
    assert_eq!(business, "2026-11-03");
    assert_eq!(
      next_reset,
      tz.with_ymd_and_hms(2026, 11, 4, 6, 0, 0)
        .unwrap()
        .timestamp() as u64
    );

    let after = tz.with_ymd_and_hms(2026, 11, 4, 6, 0, 0).unwrap();
    let (business, next_reset) = quota_window(after, "06:00");
    assert_eq!(business, "2026-11-04");
    assert_eq!(
      next_reset,
      tz.with_ymd_and_hms(2026, 11, 5, 6, 0, 0)
        .unwrap()
        .timestamp() as u64
    );
  }

  #[test]
  fn test_quota_window_falls_back_to_midnight_on_bad_value() {
    use chrono::TimeZone;
    let tz = chrono_tz::Asia::Shanghai;
    let now = tz.with_ymd_and_hms(2026, 11, 4, 5, 59, 0).unwrap();
    let (business, _) = quota_window(now, "25:99");
    assert_eq!(business, "2026-11-04");
  }

  #[test]
  fn test_recovery_deadline_reads_the_doubao_sentence() {
    use chrono::TimeZone;
    let tz = chrono_tz::Asia::Shanghai;
    let now = tz.with_ymd_and_hms(2026, 11, 5, 15, 0, 0).unwrap();
    let message = "近 7 天免费额度已用完，预计 11 月 9 日 10:31 恢复。开通订阅畅享更多额度。";
    assert_eq!(
      parse_recovery_deadline(message, now),
      Some(
        tz.with_ymd_and_hms(2026, 11, 9, 10, 31, 0)
          .unwrap()
          .timestamp() as u64
      )
    );
    // 不写年份：日期已经过去就落到明年
    assert_eq!(
      parse_recovery_deadline("额度已用完，预计 1 月 3 日恢复。", now),
      Some(
        tz.with_ymd_and_hms(2027, 1, 3, 0, 0, 0)
          .unwrap()
          .timestamp() as u64
      )
    );
  }

  #[test]
  fn test_recovery_deadline_gives_up_when_there_is_no_date() {
    use chrono::TimeZone;
    let tz = chrono_tz::Asia::Shanghai;
    let now = tz.with_ymd_and_hms(2026, 11, 5, 15, 0, 0).unwrap();
    assert_eq!(
      parse_recovery_deadline("今日额度已用完，预计明天恢复。", now),
      None
    );
    assert_eq!(
      parse_recovery_deadline("近 7 天免费额度已用完。", now),
      None
    );
  }

  fn profile(name: &str) -> crate::profile::types::BrowserProfile {
    crate::profile::types::BrowserProfile {
      id: uuid::Uuid::new_v4(),
      name: name.to_string(),
      browser: "wayfern".to_string(),
      version: "1.0".to_string(),
      release_type: "stable".to_string(),
      ..Default::default()
    }
  }

  #[test]
  fn test_match_cloud_profile_prefers_derived_name() {
    let p1 = profile("张三_11");
    let profiles = vec![p1.clone(), profile("张三")];
    assert_eq!(
      match_cloud_profile(&profiles, "张三", 11),
      Some(p1.id.to_string())
    );
  }

  #[test]
  fn test_match_cloud_profile_legacy_name_only_when_unique() {
    let p1 = profile("张三");
    let profiles = vec![p1.clone()];
    assert_eq!(
      match_cloud_profile(&profiles, "张三", 11),
      Some(p1.id.to_string())
    );
    let duplicated = vec![profile("张三"), profile("张三")];
    assert_eq!(match_cloud_profile(&duplicated, "张三", 11), None);
  }

  #[test]
  fn test_match_cloud_profile_no_match() {
    let profiles = vec![profile("其他_9")];
    assert_eq!(match_cloud_profile(&profiles, "张三", 11), None);
  }
}
