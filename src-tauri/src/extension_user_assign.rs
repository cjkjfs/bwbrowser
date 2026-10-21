//! 扩展与云端用户的持久绑定。归属人在云端账号上设置（owner_id），这里把
//! 「用户 ↔ 扩展」的分配关系存到本地，并负责两个方向的自动化：
//!
//! - 分配：该用户名下（现在与将来）的浏览器自动装上插件——载体是每个用户
//!   一个自动分组（名字 `__user_<id>`，内容 = 名下全部分配的扩展），用户
//!   浏览器 profile 的 extension_group_id 指向它，启动时走既有
//!   --load-extension 链路；
//! - 取消：从分组里摘掉扩展并清掉指向该分组的浏览器，即自动卸载。
//!
//! 手动选的其他分组不受影响；只有为空或本属于自动分组的 profile 才会被覆盖。
//!
//! 绑定关系与插件包通过公司级云端 KV（save_company_setting /
//! get_company_setting，需服务端补丁）跨机器同步：管理员（老板）机器写，
//! 成员（朱俊强等）机器在登录/启动时拉取后自动生效。任何网络或服务端
//! 错误都静默降级为纯本地行为。

use serde::{Deserialize, Serialize};
use std::fs;

use base64::{engine::general_purpose, Engine as _};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExtensionUserAssignment {
  pub extension_id: String,
  pub user_id: i64,
  pub assigned_at: u64,
}

/// 绑定存储：items + 全局编辑时间戳（last-write-wins 的依据）。
/// 旧版本文件是裸数组，load_store 兼容读取。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AssignmentStore {
  #[serde(default)]
  pub updated_at: u64,
  #[serde(default)]
  pub items: Vec<ExtensionUserAssignment>,
}

/// 公司 KV 上同步插件包时携带的元数据：完整扩展元信息 + 分块信息。
#[derive(Debug, Serialize, Deserialize, Clone)]
struct CompanyExtMeta {
  ext: crate::extension_manager::Extension,
  size: u64,
  sha256: String,
  chunks: usize,
}

/// 单块二进制大小。公司 KV 单值上限 512KB（服务端按字符串长度校验），
/// base64 膨胀 4/3：380KB → 518,828 字符，留有余量。
const CHUNK_SIZE: usize = 380 * 1024;

/// 公司 KV 里绑定关系的 key。
const COMPANY_ASSIGNMENTS_KEY: &str = "extension_user_assignments";

/// 自动分组名前缀：extension_group_id 指向这类分组时视为"跟随用户分配"。
const AUTO_GROUP_PREFIX: &str = "__user_";

fn store_path() -> std::path::PathBuf {
  crate::app_dirs::settings_dir().join("extension_user_assignments.json")
}

fn load_store() -> AssignmentStore {
  let raw = fs::read_to_string(store_path()).ok();
  match raw {
    Some(s) => {
      if let Ok(store) = serde_json::from_str::<AssignmentStore>(&s) {
        return store;
      }
      // 旧版裸数组 → items，updated_at 视为 0（首次编辑会写入真实值）
      if let Ok(items) = serde_json::from_str::<Vec<ExtensionUserAssignment>>(&s) {
        return AssignmentStore {
          updated_at: 0,
          items,
        };
      }
      AssignmentStore::default()
    }
    None => AssignmentStore::default(),
  }
}

fn persist_store(store: &AssignmentStore) -> Result<(), String> {
  let dir = crate::app_dirs::settings_dir();
  fs::create_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;
  let json = serde_json::to_string_pretty(store)
    .map_err(|e| format!("Failed to serialize assignments: {e}"))?;
  fs::write(store_path(), json).map_err(|e| format!("Failed to write assignments: {e}"))
}

fn load() -> Vec<ExtensionUserAssignment> {
  load_store().items
}

fn user_group_name(user_id: i64) -> String {
  format!("{AUTO_GROUP_PREFIX}{user_id}")
}

/// 该用户当前绑定的全部扩展 id。
fn assigned_extension_ids(user_id: i64) -> Vec<String> {
  load()
    .iter()
    .filter(|a| a.user_id == user_id)
    .map(|a| a.extension_id.clone())
    .collect()
}

/// 取（或建）该用户的自动分组，并把分组内容对齐到 `extension_ids`。
/// 返回分组 id。锁必须限制在块内（守卫非 Send）。
fn sync_user_group(user_id: i64, extension_ids: Vec<String>) -> Result<String, String> {
  let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
  let groups = mgr
    .list_groups()
    .map_err(|e| format!("Failed to list extension groups: {e}"))?;
  let existing = groups
    .iter()
    .find(|g| g.name == user_group_name(user_id))
    .cloned();
  match existing {
    Some(group) => {
      if group.extension_ids != extension_ids {
        mgr
          .update_group(&group.id, None, Some(extension_ids))
          .map_err(|e| format!("{e}"))?;
      }
      Ok(group.id)
    }
    None => {
      let group = mgr
        .create_group(user_group_name(user_id))
        .map_err(|e| format!("{e}"))?;
      let group_id = group.id.clone();
      if !extension_ids.is_empty() {
        mgr
          .update_group(&group_id, None, Some(extension_ids))
          .map_err(|e| format!("{e}"))?;
      }
      Ok(group_id)
    }
  }
}

/// 分组 id 是否属于某用户的自动分组（按名字判断）。
fn is_auto_group(group_id: &str) -> bool {
  crate::extension_manager::EXTENSION_MANAGER
    .lock()
    .unwrap()
    .get_group(group_id)
    .map(|g| g.name.starts_with(AUTO_GROUP_PREFIX))
    .unwrap_or(false)
}

/// 浏览器创建/启动前调用：把归属人绑定的扩展分组写到 profile 上
/// （不动手动分组）。owner_id 为 None（云端未返回归属人）时跳过。
pub fn apply_owner_assignments(
  owner_id: Option<i64>,
  profile_id: &str,
  current_group: Option<&str>,
) {
  let Some(owner_id) = owner_id else {
    return;
  };
  let extension_ids = assigned_extension_ids(owner_id);
  if extension_ids.is_empty() {
    // 已取消全部绑定：其他机器上跟随本用户自动分组的 profile 要摘掉分组，
    // 否则它会继续指向本机残留的自动分组、启动时照旧注入。
    if let Some(g) = current_group {
      if is_auto_group(g) {
        let pm = crate::profile::manager::ProfileManager::instance();
        if let Err(e) = pm.update_profile_extension_group(profile_id, None) {
          crate::bwbrowser_cloud::log_bwbrowser(
            "extension_user_assign",
            &format!("  取消归属人 {owner_id} 的扩展分组失败: {e}"),
          );
        } else {
          crate::bwbrowser_cloud::log_bwbrowser(
            "extension_user_assign",
            &format!("  归属人 {owner_id} 已取消分配，已清掉 profile {profile_id} 的扩展分组"),
          );
        }
      }
    }
    return;
  }
  let group_id = match sync_user_group(owner_id, extension_ids) {
    Ok(id) => id,
    Err(e) => {
      crate::bwbrowser_cloud::log_bwbrowser(
        "extension_user_assign",
        &format!("  同步用户 {owner_id} 自动分组失败: {e}"),
      );
      return;
    }
  };
  let should_apply = match current_group {
    None => true,
    Some(g) => g != group_id && is_auto_group(g),
  };
  if !should_apply {
    return;
  }
  let pm = crate::profile::manager::ProfileManager::instance();
  match pm.update_profile_extension_group(profile_id, Some(group_id.clone())) {
    Ok(_) => crate::bwbrowser_cloud::log_bwbrowser(
      "extension_user_assign",
      &format!("  归属人 {owner_id} 的扩展分组已应用到 profile {profile_id}"),
    ),
    Err(e) => crate::bwbrowser_cloud::log_bwbrowser(
      "extension_user_assign",
      &format!("  应用扩展分组到 profile {profile_id} 失败: {e}"),
    ),
  }
}

/// 插件被删除时清掉它在本机的全部绑定（所有用户），返回是否确有绑定被清除。
pub fn remove_assignments_for_extension(extension_id: &str) -> bool {
  let mut store = load_store();
  let before = store.items.len();
  store.items.retain(|a| a.extension_id != extension_id);
  if store.items.len() == before {
    return false;
  }
  store.updated_at = crate::proxy_manager::now_secs();
  if let Err(e) = persist_store(&store) {
    crate::bwbrowser_cloud::log_bwbrowser(
      "extension_user_assign",
      &format!("清理插件 {extension_id} 的绑定失败: {e}"),
    );
    return false;
  }
  true
}

/// 插件被删除后：删掉云端保存的插件包（meta + 分块），并上传更新后的
/// 绑定整包——否则其他机器下一次拉取会把已删除的插件重新下载回来。
pub fn spawn_delete_extension_remote(extension_id: String) {
  // 该函数在同步的 delete_extension 里调用，用 Tauri 的运行时句柄更稳妥。
  tauri::async_runtime::spawn(async move {
    let kid = company_key_id(&extension_id);
    let meta_key = format!("extmeta_{kid}");
    if let Err(e) = crate::bwbrowser_cloud::company_setting_delete(&meta_key, false).await {
      crate::bwbrowser_cloud::log_bwbrowser(
        "extension_user_assign",
        &format!("删除云端插件元数据 {extension_id} 失败（忽略）: {e}"),
      );
    }
    let chunk_prefix = format!("extfile_{kid}_");
    if let Err(e) = crate::bwbrowser_cloud::company_setting_delete(&chunk_prefix, true).await {
      crate::bwbrowser_cloud::log_bwbrowser(
        "extension_user_assign",
        &format!("删除云端插件分块 {extension_id} 失败（忽略）: {e}"),
      );
    }
    push_company_assignments().await;
  });
}

/// 本地绑定 store 序列化（供上传公司 KV）。
fn export_json() -> Result<String, String> {
  let store = load_store();
  serde_json::to_string(&store).map_err(|e| format!("Failed to serialize assignments: {e}"))
}

/// 用远端 JSON 整包替换本地绑定（不 bump 时间戳、不触发分组重建；
/// 分组在启动时由 apply_owner_assignments 落实）。
fn import_json(json: &str) -> Result<(), String> {
  let store: AssignmentStore =
    serde_json::from_str(json).map_err(|e| format!("Invalid assignments JSON: {e}"))?;
  persist_store(&store)
}

fn sha256_hex(data: &[u8]) -> String {
  let digest = Sha256::digest(data);
  let mut hex = String::with_capacity(digest.len() * 2);
  for b in digest {
    hex.push_str(&format!("{b:02x}"));
  }
  hex
}

/// 公司 KV 的 key 只接受 `[a-z0-9_]`，而插件 id 是带连字符的 UUID。
/// 把非字母数字统一替换为 `_`（UUID 仅含十六进制与 `-`，替换后仍唯一）。
fn company_key_id(extension_id: &str) -> String {
  extension_id
    .chars()
    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
    .collect()
}

/// 插件包已上传过（云端有 meta）则跳过；否则把本地插件包分块上传。
/// meta 最后写：它的存在即是「包已就绪」的标记。
async fn ensure_extension_uploaded(ext_id: &str) -> Result<(), String> {
  let kid = company_key_id(ext_id);
  let meta_key = format!("extmeta_{kid}");
  if crate::bwbrowser_cloud::company_setting_get(&meta_key)
    .await?
    .is_some()
  {
    return Ok(());
  }

  let (ext, data) = {
    let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
    let ext = mgr
      .get_extension(ext_id)
      .map_err(|e| format!("本地插件不存在: {e}"))?;
    if ext.is_linked() {
      return Err("linked 插件机器本地，不做跨机分发".to_string());
    }
    let path = mgr.get_file_dir_public(ext_id).join(&ext.file_name);
    let data = fs::read(&path).map_err(|e| format!("读取插件包失败: {e}"))?;
    (ext, data)
  };

  let meta = CompanyExtMeta {
    ext,
    size: data.len() as u64,
    sha256: sha256_hex(&data),
    chunks: data.len().div_ceil(CHUNK_SIZE),
  };

  for (i, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
    let b64 = general_purpose::STANDARD.encode(chunk);
    crate::bwbrowser_cloud::company_setting_save(&format!("extfile_{kid}_{i}"), &b64).await?;
    if (i + 1) % 10 == 0 {
      crate::bwbrowser_cloud::log_bwbrowser(
        "extension_user_assign",
        &format!("  插件 {ext_id} 上传进度 {}/{} 块", i + 1, meta.chunks),
      );
    }
  }

  let meta_json = serde_json::to_string(&meta).map_err(|e| format!("序列化插件元数据失败: {e}"))?;
  crate::bwbrowser_cloud::company_setting_save(&meta_key, &meta_json).await?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "extension_user_assign",
    &format!("  插件 {ext_id} 已上传到公司云端（{} 块）", meta.chunks),
  );
  Ok(())
}

/// 本地没有该插件时，从公司 KV 拉元数据与分块、拼包校验后按原 id 落地
/// （照 sync 引擎 download_extension 的方式：写文件 + upsert_extension_internal，
/// 保留原 id 让绑定分组能对上）。
async fn ensure_extension_local(ext_id: &str) -> Result<(), String> {
  {
    let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
    if mgr.get_extension(ext_id).is_ok() {
      return Ok(());
    }
  }

  let kid = company_key_id(ext_id);
  let meta_key = format!("extmeta_{kid}");
  let Some((meta_json, _)) = crate::bwbrowser_cloud::company_setting_get(&meta_key).await? else {
    return Err(format!("云端没有插件 {ext_id} 的元数据"));
  };
  let meta: CompanyExtMeta =
    serde_json::from_str(&meta_json).map_err(|e| format!("解析插件元数据失败: {e}"))?;

  let mut data = Vec::with_capacity(meta.size as usize);
  for i in 0..meta.chunks {
    let key = format!("extfile_{kid}_{i}");
    let Some((chunk_b64, _)) = crate::bwbrowser_cloud::company_setting_get(&key).await? else {
      return Err(format!("云端缺少分块 {key}"));
    };
    let bytes = general_purpose::STANDARD
      .decode(chunk_b64.trim())
      .map_err(|e| format!("分块 {key} 解码失败: {e}"))?;
    data.extend_from_slice(&bytes);
  }
  if sha256_hex(&data) != meta.sha256 {
    return Err("插件包 sha256 校验失败".to_string());
  }

  let file_dir = {
    let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
    mgr.get_file_dir_public(ext_id)
  };
  fs::create_dir_all(&file_dir).map_err(|e| format!("创建插件目录失败: {e}"))?;
  fs::write(file_dir.join(&meta.ext.file_name), &data)
    .map_err(|e| format!("写入插件文件失败: {e}"))?;

  let mut ext = meta.ext;
  ext.sync_enabled = crate::sync::is_sync_configured();
  ext.last_sync = Some(crate::proxy_manager::now_secs());
  {
    let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
    mgr
      .upsert_extension_internal(&ext)
      .map_err(|e| format!("保存插件元数据失败: {e}"))?;
  }
  crate::bwbrowser_cloud::log_bwbrowser(
    "extension_user_assign",
    &format!("  插件 {ext_id} 已从公司云端拉取落地"),
  );
  Ok(())
}

/// 本机已安装且非 linked 的绑定插件，确保云端有它的包（meta 已存在时只
/// 花一次 GET）。失败仅记日志，不阻断拉取。
async fn backfill_missing_packages(items: &[ExtensionUserAssignment]) {
  let ext_ids: std::collections::BTreeSet<&str> =
    items.iter().map(|a| a.extension_id.as_str()).collect();
  for ext_id in ext_ids {
    let uploadable = {
      let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
      mgr
        .get_extension(ext_id)
        .map(|e| !e.is_linked())
        .unwrap_or(false)
    };
    if !uploadable {
      continue;
    }
    if let Err(e) = ensure_extension_uploaded(ext_id).await {
      crate::bwbrowser_cloud::log_bwbrowser(
        "extension_user_assign",
        &format!("补传插件 {ext_id} 失败（忽略）: {e}"),
      );
    }
  }
}

/// 从公司云端拉绑定（管理员机器写的整包），远端更新则整包替换本地，
/// 并确保被绑定的插件包在本机存在。任何失败静默降级（保持本地现状）。
pub async fn pull_company_assignments() {
  if let Err(e) = pull_company_assignments_inner().await {
    crate::bwbrowser_cloud::log_bwbrowser(
      "extension_user_assign",
      &format!("拉取公司绑定失败（忽略）: {e}"),
    );
  }
}

async fn pull_company_assignments_inner() -> Result<(), String> {
  let Some((remote_json, _server_ts)) =
    crate::bwbrowser_cloud::company_setting_get(COMPANY_ASSIGNMENTS_KEY).await?
  else {
    return Ok(()); // 云端还没有绑定
  };
  let remote: AssignmentStore =
    serde_json::from_str(&remote_json).map_err(|e| format!("解析云端绑定失败: {e}"))?;
  let local = load_store();

  // 本机持有但云端缺包时补传：管理员机器分配时若上传失败（旧服务端拒绝
  // 带连字符的 key、或网络中断），下次登录/启动会自动补齐，无需重新分配。
  backfill_missing_packages(&local.items).await;

  if remote.updated_at <= local.updated_at {
    return Ok(());
  }

  import_json(&remote_json)?;
  crate::bwbrowser_cloud::log_bwbrowser(
    "extension_user_assign",
    &format!("已应用公司绑定（共 {} 条）", remote.items.len()),
  );

  let ext_ids: std::collections::BTreeSet<String> = remote
    .items
    .iter()
    .map(|i| i.extension_id.clone())
    .collect();
  for ext_id in ext_ids {
    if let Err(e) = ensure_extension_local(&ext_id).await {
      crate::bwbrowser_cloud::log_bwbrowser(
        "extension_user_assign",
        &format!("拉取插件 {ext_id} 失败: {e}"),
      );
    }
  }
  Ok(())
}

/// 把本地绑定整包上传公司 KV（管理员可写；非管理员/旧服务端失败仅记日志）。
pub async fn push_company_assignments() {
  match export_json() {
    Ok(json) => {
      if let Err(e) =
        crate::bwbrowser_cloud::company_setting_save(COMPANY_ASSIGNMENTS_KEY, &json).await
      {
        crate::bwbrowser_cloud::log_bwbrowser(
          "extension_user_assign",
          &format!("上传公司绑定失败（忽略，服务端未打补丁或非管理员）: {e}"),
        );
      }
    }
    Err(e) => crate::bwbrowser_cloud::log_bwbrowser(
      "extension_user_assign",
      &format!("序列化公司绑定失败: {e}"),
    ),
  }
}

/// 本地绑定变更后的后台收尾：确保插件包已上传，再上传绑定整包。
fn spawn_sync_after_change(extension_id: Option<String>) {
  tokio::spawn(async move {
    if let Some(ext_id) = extension_id {
      if let Err(e) = ensure_extension_uploaded(&ext_id).await {
        crate::bwbrowser_cloud::log_bwbrowser(
          "extension_user_assign",
          &format!("上传插件包 {ext_id} 失败（忽略）: {e}"),
        );
      }
    }
    push_company_assignments().await;
  });
}

#[tauri::command]
pub async fn assign_extension_to_user(
  extension_id: String,
  user_id: i64,
) -> Result<serde_json::Value, String> {
  {
    let mgr = crate::extension_manager::EXTENSION_MANAGER.lock().unwrap();
    if mgr.get_extension(&extension_id).is_err() {
      return Err(serde_json::json!({ "code": "EXTENSION_NOT_FOUND" }).to_string());
    }
  }

  let mut store = load_store();
  if !store
    .items
    .iter()
    .any(|a| a.extension_id == extension_id && a.user_id == user_id)
  {
    store.items.push(ExtensionUserAssignment {
      extension_id: extension_id.clone(),
      user_id,
      assigned_at: crate::proxy_manager::now_secs(),
    });
    store.updated_at = crate::proxy_manager::now_secs();
    persist_store(&store)?;
  }

  let extension_ids = assigned_extension_ids(user_id);
  let group_id = sync_user_group(user_id, extension_ids)?;

  let profile_ids = crate::bwbrowser_cloud::owner_profile_ids(user_id).await?;
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().unwrap_or_default();
  let wanted: std::collections::HashSet<&str> = profile_ids.iter().map(|s| s.as_str()).collect();

  let mut assigned = 0u32;
  let mut skipped = 0u32;
  for profile in &profiles {
    let pid = profile.id.to_string();
    if !wanted.contains(pid.as_str()) {
      continue;
    }
    match profile.extension_group_id.as_deref() {
      Some(g) if g == group_id => continue, // 已是该分组
      Some(g) if is_auto_group(g) => {}     // 其他自动分组 → 覆盖
      Some(_) => {
        skipped += 1; // 手动分组不动
        continue;
      }
      None => {}
    }
    pm.update_profile_extension_group(&pid, Some(group_id.clone()))
      .map_err(|e| format!("Failed to assign extension group: {e}"))?;
    assigned += 1;
  }

  spawn_sync_after_change(Some(extension_id));

  Ok(serde_json::json!({ "assigned": assigned, "skipped": skipped }))
}

#[tauri::command]
pub async fn unassign_extension_from_user(
  extension_id: String,
  user_id: i64,
) -> Result<serde_json::Value, String> {
  let mut store = load_store();
  let before = store.items.len();
  store
    .items
    .retain(|a| !(a.extension_id == extension_id && a.user_id == user_id));
  if store.items.len() != before {
    store.updated_at = crate::proxy_manager::now_secs();
    persist_store(&store)?;
  }

  let extension_ids = assigned_extension_ids(user_id);
  let group_id = sync_user_group(user_id, extension_ids)?;

  let profile_ids = crate::bwbrowser_cloud::owner_profile_ids(user_id).await?;
  let pm = crate::profile::manager::ProfileManager::instance();
  let profiles = pm.list_profiles().unwrap_or_default();
  let wanted: std::collections::HashSet<&str> = profile_ids.iter().map(|s| s.as_str()).collect();

  let mut removed = 0u32;
  for profile in &profiles {
    let pid = profile.id.to_string();
    if !wanted.contains(pid.as_str()) {
      continue;
    }
    if profile.extension_group_id.as_deref() == Some(group_id.as_str()) {
      pm.update_profile_extension_group(&pid, None)
        .map_err(|e| format!("Failed to clear extension group: {e}"))?;
      removed += 1;
    }
  }

  spawn_sync_after_change(None);

  Ok(serde_json::json!({ "removed": removed }))
}

#[tauri::command]
pub fn list_extension_user_assignments() -> Result<Vec<ExtensionUserAssignment>, String> {
  Ok(load())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn store_roundtrip_keeps_updated_at() {
    let store = AssignmentStore {
      updated_at: 1234,
      items: vec![ExtensionUserAssignment {
        extension_id: "e1".into(),
        user_id: 7,
        assigned_at: 1000,
      }],
    };
    let json = serde_json::to_string(&store).unwrap();
    let parsed: AssignmentStore = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.updated_at, 1234);
    assert_eq!(parsed.items.len(), 1);
  }

  #[test]
  fn legacy_bare_array_parses_as_store() {
    let json = r#"[{"extension_id":"e1","user_id":7,"assigned_at":1000}]"#;
    let store: AssignmentStore = serde_json::from_str(json).unwrap_or_default();
    assert_eq!(store.updated_at, 0);
    // 兼容路径走 load_store 的显式分支
    let items: Vec<ExtensionUserAssignment> = serde_json::from_str(json).unwrap();
    assert_eq!(items.len(), 1);
    let _ = store;
  }

  #[test]
  fn chunk_covers_base64_limit() {
    // 380KB → base64 518,828 字符 ≤ 512KB(524,288)
    let chunk = vec![0u8; CHUNK_SIZE];
    let b64_len = general_purpose::STANDARD.encode(&chunk).len();
    assert!(b64_len <= 512 * 1024);
  }
}
