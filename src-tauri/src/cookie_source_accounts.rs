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
//! 下载 Cookie 提取完整优先级：前台运行中的浏览器窗口（video_downloader）→
//! 本清单（云端勾选顺序）。旧「下载设置单选 cookie_account_id」已移除：
//! 代理跟随/云端解析兜底改用 `first_cookie_account()`（清单第一个账号），
//! 嗅探识别按钮绑定改由 `sync_sniffer_binding_from_cookie_sources()` 跟随本清单。
//! 旧「VPS登录」（标记 0）兜底链路已随爆文库入口重构整体移除。

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
/// 调用前会先 pull 一次云端，保证各机器看到的是最新清单。
/// 自动过滤历史遗留的 VPS 标记 0（该兜底链路已移除，云端旧数据可能还残留）。
pub async fn download_candidate_accounts() -> Vec<i64> {
  pull_remote().await;
  load().accounts.into_iter().filter(|id| *id > 0).collect()
}

/// 云端清单快照（先 pull 再返回本地镜像的账号列表）。
pub async fn cookie_source_accounts_snapshot() -> Vec<i64> {
  download_candidate_accounts().await
}

/// 云端清单第一个真实账号 id，无则 None。
/// 供「旧单选 cookie_account_id」移除后的云端解析/代理跟随兜底使用。
pub async fn first_cookie_account() -> Option<i64> {
  cookie_source_accounts_snapshot()
    .await
    .into_iter()
    .find(|id| *id > 0)
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
/// 本 key 允许公司成员（含员工）写入，保存前先吸收云端最新，防止多员工并发覆盖。
/// `edit` 收到 &mut settings 做具体修改，返回 true 表示确有变化。
async fn update_remote(edit: impl FnOnce(&mut CookieSourceSettings) -> bool) -> Result<(), String> {
  // 吸收云端最新（仅当云端确实比本地新；失败静默降级用本地镜像）
  if let Ok(Some((json, _))) = crate::bwbrowser_cloud::company_setting_get(COMPANY_KEY).await {
    if let Ok(remote) = serde_json::from_str::<CookieSourceSettings>(&json) {
      if remote.updated_at > load().updated_at {
        let _ = persist(&remote);
      }
    }
  }
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
  // 识别按钮绑定跟随清单（账号管理勾谁，谁的浏览器有识别按钮）
  crate::extension_user_assign::sync_sniffer_binding_from_cookie_sources().await;
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
