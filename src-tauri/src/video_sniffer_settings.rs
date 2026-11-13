//! 视频嗅探开关（云端为准，默认关闭）。
//!
//! 账号开关是「嗅探/开始/下载按钮」是否出现的**唯一依据**，与扩展分组/绑定/
//! 归属人彻底解耦：开关开启的账号，其 profile 启动时由 browser_runner 直接
//! 注入内置嗅探扩展（`ExtensionManager::linked_sniffer_path`），
//! `is_profile_sniffer_allowed` 统一判定「该 profile 是否允许注入」。
//! 爆文库（VPS 登录）profile 走 `is_vps_enabled`。
//!
//! **默认关闭**：云端名单里没有记录 = 关闭。存储只有公司级云端 KV 一份
//! （key `video_sniffer_enabled_accounts`），本地 JSON 只是它的镜像缓存——每
//! 个判定点都先 pull 一次云端再判定，保证开关即时生效；写操作推送失败会回滚
//! 本地并报错，让本地永远跟云端一致。任何网络错误都不影响浏览器启动本身。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// 公司 KV 的 key（与扩展分配同一命名空间）
const COMPANY_KEY: &str = "video_sniffer_enabled_accounts";

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct VideoSnifferSettings {
  /// 最近一次编辑时间（epoch 秒），跨机同步的 last-write-wins 依据
  #[serde(default)]
  pub updated_at: u64,
  /// 开启了视频嗅探的云端账号 id（不在名单里 = 关闭）
  #[serde(default)]
  pub enabled: BTreeSet<i64>,
  /// 爆文库（VPS 登录）浏览器是否注入嗅探扩展
  #[serde(default)]
  pub vps_enabled: bool,
}

/// 前端读取的开关状态快照。
#[derive(Debug, Serialize, Clone)]
pub struct VideoSnifferState {
  /// 开启了嗅探的云端账号 id（升序）
  pub accounts: Vec<i64>,
  /// 爆文库是否开启
  pub vps: bool,
}

fn store_path() -> PathBuf {
  crate::app_dirs::settings_dir().join("video_sniffer_accounts.json")
}

/// 本地镜像缓存（判定点都已先 pull 云端，这里只是磁盘落点）。
fn load() -> VideoSnifferSettings {
  let Ok(raw) = std::fs::read_to_string(store_path()) else {
    return VideoSnifferSettings::default();
  };
  serde_json::from_str(&raw).unwrap_or_default()
}

fn persist(settings: &VideoSnifferSettings) -> Result<(), String> {
  let dir = crate::app_dirs::settings_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
  let json =
    serde_json::to_string_pretty(settings).map_err(|e| format!("序列化嗅探开关失败: {e}"))?;
  std::fs::write(store_path(), json).map_err(|e| format!("写入嗅探开关失败: {e}"))
}

/// 该云端账号的视频嗅探是否开启（云端名单里没有 = 关闭）。
pub fn is_account_enabled(account_id: i64) -> bool {
  load().enabled.contains(&account_id)
}

/// 统一判定：某 profile 是否应注入嗅探扩展。账号开关是唯一依据——只要账号
/// 开着就注入，不依赖扩展分组/绑定/归属人。云端账号（名字 `_<账号id>` 结尾）
/// 看账号名单；「VPS登录」专用 profile 看爆文库开关；其它本地 profile 默认注
/// 入（嗅探本就该随程序内置可用）。
pub fn is_profile_sniffer_allowed(profile_name: &str) -> bool {
  match profile_name
    .rsplit('_')
    .next()
    .and_then(|s| s.parse::<i64>().ok())
  {
    Some(account_id) => is_account_enabled(account_id),
    None if profile_name.to_lowercase() == "vps登录" => is_vps_enabled(),
    None => true,
  }
}

/// 爆文库（VPS 登录）浏览器是否注入嗅探扩展（默认关闭）。
pub fn is_vps_enabled() -> bool {
  load().vps_enabled
}

/// 从公司云端拉开关，远端更新则整包替换本地镜像。失败静默降级（用本地镜像）。
pub async fn pull_remote() {
  let result = async {
    let Some((json, _server_ts)) = crate::bwbrowser_cloud::company_setting_get(COMPANY_KEY).await?
    else {
      return Ok::<(), String>(()); // 云端还没有记录 = 全关
    };
    let remote: VideoSnifferSettings =
      serde_json::from_str(&json).map_err(|e| format!("解析云端嗅探开关失败: {e}"))?;
    if remote.updated_at <= load().updated_at {
      return Ok(());
    }
    persist(&remote)?;
    crate::bwbrowser_cloud::log_bwbrowser(
      "video_sniffer",
      &format!(
        "已应用云端嗅探开关（开启 {} 个账号，爆文库 {}）",
        remote.enabled.len(),
        if remote.vps_enabled { "开" } else { "关" }
      ),
    );
    Ok(())
  }
  .await;
  if let Err(e) = result {
    crate::bwbrowser_cloud::log_bwbrowser(
      "video_sniffer",
      &format!("拉取云端嗅探开关失败（忽略）: {e}"),
    );
  }
}

/// 修改开关并推送云端；推送失败回滚本地镜像并报错（云端为准，本地不领先）。
/// `edit` 收到 &mut settings 做具体修改，返回 true 表示确有变化。
async fn update_remote(edit: impl FnOnce(&mut VideoSnifferSettings) -> bool) -> Result<(), String> {
  let old = load();
  let mut settings = old.clone();
  if !edit(&mut settings) {
    return Ok(());
  }
  settings.updated_at = crate::proxy_manager::now_secs();
  persist(&settings)?;
  let json = serde_json::to_string(&settings).map_err(|e| format!("序列化嗅探开关失败: {e}"))?;
  if let Err(e) = crate::bwbrowser_cloud::company_setting_save(COMPANY_KEY, &json).await {
    // 云端为准：推送失败回滚本地，避免本地领先云端
    let _ = persist(&old);
    return Err(format!("保存到云端失败: {e}"));
  }
  Ok(())
}

/// 当前登录用户的 id（未登录返回 None）。
fn current_user_id() -> Option<i64> {
  crate::bwbrowser_cloud::BWBROWSER_AUTH.get_user_id()
}

/// 把开关立刻落到爆文库（VPS 登录）profile 的分组上。
fn reapply_vps_group() {
  let Some(user_id) = current_user_id() else {
    return;
  };
  let pm = crate::profile::manager::ProfileManager::instance();
  let Ok(profiles) = pm.list_profiles() else {
    return;
  };
  // 与 bwbrowser_open_vps_login 的查找约定一致：专用 profile 固定叫「VPS登录」
  let Some(profile) = profiles.iter().find(|p| p.name.to_lowercase() == "vps登录") else {
    return;
  };
  crate::extension_user_assign::apply_owner_assignments(
    Some(user_id),
    None,
    &profile.id.to_string(),
    profile.extension_group_id.as_deref(),
  );
}

/// 编辑账号里「开启视频嗅探」：写云端 + 镜像本地。按钮是否出现由账号开关
/// 唯一决定，启动时按开关注入拓展；此处无需再动扩展绑定/分组。
#[tauri::command]
pub async fn set_account_video_sniffer(
  account_id: i64,
  enabled: bool,
  _owner_id: Option<i64>,
) -> Result<(), String> {
  update_remote(|s| {
    if enabled {
      s.enabled.insert(account_id)
    } else {
      s.enabled.remove(&account_id)
    }
  })
  .await?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "video_sniffer",
    &format!(
      "账号 {account_id} 视频嗅探 {}",
      if enabled { "开启" } else { "关闭" }
    ),
  );
  Ok(())
}

/// 爆文库（VPS 登录）嗅探开关：写云端 + 镜像本地 + 立即对齐分组。
#[tauri::command]
pub async fn set_vps_video_sniffer(enabled: bool) -> Result<(), String> {
  update_remote(|s| {
    let changed = s.vps_enabled != enabled;
    s.vps_enabled = enabled;
    changed
  })
  .await?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "video_sniffer",
    &format!("爆文库视频嗅探 {}", if enabled { "开启" } else { "关闭" }),
  );
  reapply_vps_group();
  Ok(())
}

/// 开关状态快照（先拉云端，保证各机器看到的是最新值）。
#[tauri::command]
pub async fn get_video_sniffer_state() -> VideoSnifferState {
  pull_remote().await;
  let settings = load();
  VideoSnifferState {
    accounts: settings.enabled.into_iter().collect(),
    vps: settings.vps_enabled,
  }
}
