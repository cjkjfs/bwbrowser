//! 通用账号级开关（公司 KV 账号清单，默认全关）。
//!
//! 复用「Cookie 来源」那一套公司级云端 KV + 本地镜像的成熟思路，为
//! **密码管理工具**（`password_manager_accounts`）和**书签同步**
//! （`bookmark_sync_accounts`）各开一个 key。两份开关独立存在、互不影响，
//! 语义都是「云端名单里没有该账号 = 关闭」。
//!
//! 前端编辑账号弹窗读取/保存，VPS 后台 toggle_account_switch 也走同名 KV key，
//! 两边共用同一份名单。判定点（启动浏览器前）先 `pull_remote` 再读取，
//! 保证各设备即时生效。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 每个开关的 KV key。密码管理工具 / 书签同步 各一份独立清单。
#[derive(Debug, Clone, Copy)]
pub enum AccountFlagKey {
  /// 密码管理工具：启用 Chromium 原生保存密码 + 自动登录，并同步密码到云端
  PasswordManager,
  /// 书签同步：关闭浏览器把书签上传服务器，启动时从服务器下载到本地
  BookmarkSync,
}

impl AccountFlagKey {
  pub fn kv_key(self) -> &'static str {
    match self {
      AccountFlagKey::PasswordManager => "password_manager_accounts",
      AccountFlagKey::BookmarkSync => "bookmark_sync_accounts",
    }
  }

  fn log_tag(self) -> &'static str {
    match self {
      AccountFlagKey::PasswordManager => "password_manager",
      AccountFlagKey::BookmarkSync => "bookmark_sync",
    }
  }

  fn log_label(self) -> &'static str {
    match self {
      AccountFlagKey::PasswordManager => "密码管理工具",
      AccountFlagKey::BookmarkSync => "书签同步",
    }
  }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AccountFlagSettings {
  /// 最近一次编辑时间（epoch 秒），跨机同步的 last-write-wins 依据
  #[serde(default)]
  pub updated_at: u64,
  /// 开启该开关的云端账号 id
  #[serde(default)]
  pub enabled: Vec<i64>,
}

fn store_path(key: AccountFlagKey) -> PathBuf {
  crate::app_dirs::settings_dir().join(format!("{}.json", key.kv_key()))
}

fn load(key: AccountFlagKey) -> AccountFlagSettings {
  let Ok(raw) = std::fs::read_to_string(store_path(key)) else {
    return AccountFlagSettings::default();
  };
  serde_json::from_str(&raw).unwrap_or_default()
}

fn persist(key: AccountFlagKey, settings: &AccountFlagSettings) -> Result<(), String> {
  let dir = crate::app_dirs::settings_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
  let json = serde_json::to_string_pretty(settings)
    .map_err(|e| format!("序列化{}开关失败: {e}", key.log_label()))?;
  std::fs::write(store_path(key), json).map_err(|e| format!("写入{}开关失败: {e}", key.log_label()))
}

/// 从公司云端拉开关，远端更新则整包替换本地镜像。失败静默降级（用本地镜像）。
pub async fn pull_remote(key: AccountFlagKey) {
  let result = async {
    let Some((json, _server_ts)) =
      crate::bwbrowser_cloud::company_setting_get(key.kv_key()).await?
    else {
      return Ok::<(), String>(()); // 云端还没有记录 = 全关
    };
    let remote: AccountFlagSettings = serde_json::from_str(&json)
      .map_err(|e| format!("解析云端{}开关失败: {e}", key.log_label()))?;
    if remote.updated_at <= load(key).updated_at {
      return Ok(());
    }
    persist(key, &remote)?;
    crate::bwbrowser_cloud::log_bwbrowser(
      key.log_tag(),
      &format!(
        "已应用云端{}名单（{} 个账号）",
        key.log_label(),
        remote.enabled.len()
      ),
    );
    Ok(())
  }
  .await;
  if let Err(e) = result {
    crate::bwbrowser_cloud::log_bwbrowser(
      key.log_tag(),
      &format!("拉取云端{}名单失败（忽略）: {e}", key.log_label()),
    );
  }
}

/// 修改开关并推送云端；推送失败回滚本地镜像并报错（云端为准，本地不领先）。
async fn update_remote(
  key: AccountFlagKey,
  edit: impl FnOnce(&mut AccountFlagSettings) -> bool,
) -> Result<(), String> {
  // 吸收云端最新（仅当云端确实比本地新；失败静默降级用本地镜像）
  if let Ok(Some((json, _))) = crate::bwbrowser_cloud::company_setting_get(key.kv_key()).await {
    if let Ok(remote) = serde_json::from_str::<AccountFlagSettings>(&json) {
      if remote.updated_at > load(key).updated_at {
        let _ = persist(key, &remote);
      }
    }
  }
  let old = load(key);
  let mut settings = old.clone();
  if !edit(&mut settings) {
    return Ok(());
  }
  settings.updated_at = crate::proxy_manager::now_secs();
  persist(key, &settings)?;
  let json = serde_json::to_string(&settings)
    .map_err(|e| format!("序列化{}开关失败: {e}", key.log_label()))?;
  if let Err(e) = crate::bwbrowser_cloud::company_setting_save(key.kv_key(), &json).await {
    // 云端为准：推送失败回滚本地，避免本地领先云端
    let _ = persist(key, &old);
    return Err(format!("保存到云端失败: {e}"));
  }
  Ok(())
}

/// 某个账号在指定开关下的状态（读镜像，调用前需先 pull_remote）。
fn is_enabled_local(key: AccountFlagKey, account_id: i64) -> bool {
  load(key).enabled.contains(&account_id)
}

/// 判断某个账号是否开启某开关的公开读取入口（先拉云端再判）。
pub async fn account_flag_enabled(key: AccountFlagKey, account_id: i64) -> bool {
  pull_remote(key).await;
  is_enabled_local(key, account_id)
}

// ===== 密码管理工具 =====

/// 账号「密码管理工具」开关：开启=加入名单，关闭=移除。
#[tauri::command]
pub async fn set_account_password_manager(account_id: i64, enabled: bool) -> Result<(), String> {
  set_flag(AccountFlagKey::PasswordManager, account_id, enabled).await
}

/// 当前开启密码管理工具的账号 id 列表。
#[tauri::command]
pub async fn get_password_manager_accounts() -> Vec<i64> {
  pull_remote(AccountFlagKey::PasswordManager).await;
  load(AccountFlagKey::PasswordManager).enabled
}

// ===== 书签同步 =====

/// 账号「书签同步」开关：开启=加入名单，关闭=移除。
#[tauri::command]
pub async fn set_account_bookmark_sync(account_id: i64, enabled: bool) -> Result<(), String> {
  set_flag(AccountFlagKey::BookmarkSync, account_id, enabled).await
}

/// 当前开启书签同步的账号 id 列表。
#[tauri::command]
pub async fn get_bookmark_sync_accounts() -> Vec<i64> {
  pull_remote(AccountFlagKey::BookmarkSync).await;
  load(AccountFlagKey::BookmarkSync).enabled
}

async fn set_flag(key: AccountFlagKey, account_id: i64, enabled: bool) -> Result<(), String> {
  update_remote(key, |s| {
    if enabled {
      if s.enabled.contains(&account_id) {
        false
      } else {
        s.enabled.push(account_id);
        true
      }
    } else {
      let before = s.enabled.len();
      s.enabled.retain(|&id| id != account_id);
      s.enabled.len() != before
    }
  })
  .await?;
  crate::bwbrowser_cloud::log_bwbrowser(
    key.log_tag(),
    &format!(
      "账号 {account_id} {} {}",
      key.log_label(),
      if enabled { "开启" } else { "关闭" }
    ),
  );
  Ok(())
}
