//! Cookie 来源账号清单（云端为准，默认全关）。
//!
//! 与视频嗅探开关（`video_sniffer_settings`）同一套思路：公司级云端 KV 一份，
//! 本地 JSON 只是镜像缓存。用于给视频下载圈定「哪些账号可作为 Cookie 来源」，
//! 并维护优先级顺序——排在前面的账号优先拉取 Cookie，取不到再依次回退到下一个。
//!
//! **默认关闭**：云端名单里没有记录 = 不把该账号作为 Cookie 来源。
//! 判定点（下载链路）先 `pull_remote` 一次再读取，保证开关即时生效；
//! 写操作推送失败回滚本地并报错，让本地永远跟云端一致。
//!
//! 与 `DownloadSettings.cookie_account_id`（下载设置里的旧单选）的关系：
//! 这里是有序的「多账号清单」，语义更贴合「设多个账号、按顺序兜底下载很稳」；
//! 下载链路读取时取「本模块清单 ∩ 兼容旧字段」为准。清单为空则退回旧的
//! `cookie_account_id`；两者都无则走 VPS 登录导出兜底。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 公司 KV 的 key（与扩展分配、嗅探开关同一命名空间）
const COMPANY_KEY: &str = "cookie_source_accounts";

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CookieSourceSettings {
  /// 最近一次编辑时间（epoch 秒），跨机同步的 last-write-wins 依据
  #[serde(default)]
  pub updated_at: u64,
  /// 可作为 Cookie 来源的云端账号 id（保持加入顺序，排前面的优先级更高）
  #[serde(default)]
  pub accounts: Vec<i64>,
}

/// 前端读取的状态（可为空）。
#[derive(Debug, Serialize, Clone)]
pub struct CookieSourceState {
  /// 可作为 Cookie 来源的云端账号 id（优先级降序）
  pub accounts: Vec<i64>,
}

fn store_path() -> PathBuf {
  crate::app_dirs::settings_dir().join("cookie_source_accounts.json")
}

/// 本地镜像缓存（判定点都已先 pull 云端，这里只是磁盘落点）。
fn load() -> CookieSourceSettings {
  let Ok(raw) = std::fs::read_to_string(store_path()) else {
    return CookieSourceSettings::default();
  };
  serde_json::from_str(&raw).unwrap_or_default()
}

fn persist(settings: &CookieSourceSettings) -> Result<(), String> {
  let dir = crate::app_dirs::settings_dir();
  std::fs::create_dir_all(&dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
  let json = serde_json::to_string_pretty(settings)
    .map_err(|e| format!("序列化 Cookie 来源清单失败: {e}"))?;
  std::fs::write(store_path(), json).map_err(|e| format!("写入 Cookie 来源清单失败: {e}"))
}

/// 供下载链路使用：返回应当依次尝试的账号 id（优先级降序，先开优先）。
/// = 云端 Cookie 来源清单 + 兼容旧字段 `cookie_account_id`（若不在清单则追加到末尾），
/// 并去重。调用前会先 pull 一次云端，保证各机器看到的是最新清单。
pub async fn download_candidate_accounts(legacy: Option<i64>) -> Vec<i64> {
  pull_remote().await;
  let mut v = load().accounts;
  if let Some(aid) = legacy {
    if !v.contains(&aid) {
      v.push(aid);
    }
  }
  v
}

/// 从公司云端拉开关，远端更新则整包替换本地镜像。失败静默降级（用本地镜像）。
pub async fn pull_remote() {
  let result = async {
    let Some((json, _server_ts)) = crate::bwbrowser_cloud::company_setting_get(COMPANY_KEY).await?
    else {
      return Ok::<(), String>(()); // 云端还没有记录 = 全关
    };
    let remote: CookieSourceSettings =
      serde_json::from_str(&json).map_err(|e| format!("解析云端 Cookie 来源清单失败: {e}"))?;
    if remote.updated_at <= load().updated_at {
      return Ok(());
    }
    persist(&remote)?;
    crate::bwbrowser_cloud::log_bwbrowser(
      "cookie_source",
      &format!(
        "已应用云端 Cookie 来源清单（{} 个账号）",
        remote.accounts.len()
      ),
    );
    Ok(())
  }
  .await;
  if let Err(e) = result {
    crate::bwbrowser_cloud::log_bwbrowser(
      "cookie_source",
      &format!("拉取云端 Cookie 来源清单失败（忽略）: {e}"),
    );
  }
}

/// 修改开关并推送云端；推送失败回滚本地镜像并报错（云端为准，本地不领先）。
/// `edit` 收到 &mut settings 做具体修改，返回 true 表示确有变化。
async fn update_remote(edit: impl FnOnce(&mut CookieSourceSettings) -> bool) -> Result<(), String> {
  let old = load();
  let mut settings = old.clone();
  if !edit(&mut settings) {
    return Ok(());
  }
  settings.updated_at = crate::proxy_manager::now_secs();
  persist(&settings)?;
  let json =
    serde_json::to_string(&settings).map_err(|e| format!("序列化 Cookie 来源清单失败: {e}"))?;
  if let Err(e) = crate::bwbrowser_cloud::company_setting_save(COMPANY_KEY, &json).await {
    // 云端为准：推送失败回滚本地，避免本地领先云端
    let _ = persist(&old);
    return Err(format!("保存到云端失败: {e}"));
  }
  Ok(())
}

/// 编辑账号里「作为下载 Cookie 来源」开关。
/// 开启：若不在清单则追加到末尾（保持优先顺序）；关闭：从清单移除。
#[tauri::command]
pub async fn set_account_cookie_source(account_id: i64, enabled: bool) -> Result<(), String> {
  update_remote(|s| {
    if enabled {
      if s.accounts.contains(&account_id) {
        false
      } else {
        s.accounts.push(account_id);
        true
      }
    } else {
      let before = s.accounts.len();
      s.accounts.retain(|&id| id != account_id);
      s.accounts.len() != before
    }
  })
  .await?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "cookie_source",
    &format!(
      "账号 {account_id} 下载 Cookie 来源 {}",
      if enabled { "开启" } else { "关闭" }
    ),
  );
  Ok(())
}

/// Cookie 来源账号清单状态快照（先拉云端，保证各机器看到的是最新值）。
#[tauri::command]
pub async fn get_cookie_source_accounts() -> CookieSourceState {
  pull_remote().await;
  CookieSourceState {
    accounts: load().accounts,
  }
}

/// 「VPS登录」（爆文库）在 Cookie 来源清单里的特殊标记 id。
/// 真实云端账号 id 从 1 开始，0 不会与任何账号冲突；
/// 前端用 `accounts.includes(0)` 判断爆文库开关状态。
pub const VPS_COOKIE_SOURCE_ID: i64 = 0;

/// 爆文库（VPS 登录）「Cookie 来源」开关：
/// 开启 = 把标记 0 追加进清单末尾（与账号同一套「先开优先」规则，
/// 想让 VPS 优先就先开 VPS 再开账号）；关闭 = 从清单移除。
/// 下载链路遍历候选遇到 0 时结束账号循环，直接走「VPS登录」老链路提取。
#[tauri::command]
pub async fn set_vps_cookie_source(enabled: bool) -> Result<(), String> {
  update_remote(|s| {
    if enabled {
      if s.accounts.contains(&VPS_COOKIE_SOURCE_ID) {
        false
      } else {
        s.accounts.push(VPS_COOKIE_SOURCE_ID);
        true
      }
    } else {
      let before = s.accounts.len();
      s.accounts.retain(|&id| id != VPS_COOKIE_SOURCE_ID);
      s.accounts.len() != before
    }
  })
  .await?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "cookie_source",
    &format!(
      "爆文库（VPS登录）下载 Cookie 来源 {}",
      if enabled { "开启" } else { "关闭" }
    ),
  );
  Ok(())
}
