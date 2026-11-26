//! 编辑账号「一键导入本机浏览器数据」（2026-11-26）
//!
//! 从本机已装浏览器（Chrome / Edge / Brave 等任意 Chromium 内核 User Data）读取
//! 书签与密码，**添加式**导入到指定账号的指纹浏览器 profile：
//!   - 书签：按 URL 全局去重后并入目标 Bookmarks——保留源目录结构（同名文件夹
//!     合并、新文件夹整棵带入），绝不删除/覆盖任何现有书签；
//!   - 密码：读源目录 Login Data（解密密钥双路：Wayfern 便携 os_crypt_key /
//!     标准 Chromium Local State DPAPI），以 (origin_url, username_value) 去重
//!     写入目标 Login Data——直接复用 password_sync::export_passwords →
//!     import_passwords，天然「添加不覆盖」。
//!
//! 保护：该账号浏览器运行中拒绝导入（SQLite 锁库 + 退出时数据会被浏览器覆盖）。

use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ============================================================
// 目录探测与归一化
// ============================================================

/// 常见浏览器 User Data 候选目录（Windows 用 LOCALAPPDATA，其他平台用 XDG 路径）
fn default_candidates() -> Vec<(&'static str, PathBuf)> {
  let mut out = Vec::new();
  if let Ok(local) = std::env::var("LOCALAPPDATA") {
    let local = PathBuf::from(local);
    out.push((
      "Chrome",
      local.join("Google").join("Chrome").join("User Data"),
    ));
    out.push((
      "Edge",
      local.join("Microsoft").join("Edge").join("User Data"),
    ));
    out.push((
      "Brave",
      local
        .join("BraveSoftware")
        .join("Brave-Browser")
        .join("User Data"),
    ));
  } else if let Ok(home) = std::env::var("HOME") {
    let home = PathBuf::from(home);
    out.push(("Chrome", home.join(".config").join("google-chrome")));
    out.push(("Chromium", home.join(".config").join("chromium")));
    out.push(("Edge", home.join(".config").join("microsoft-edge")));
  }
  out
}

/// 用户可能选到三种层级：User Data 本身 / User Data\Default / 里面直接有
/// Bookmarks、Login Data 的 profile 目录。统一归一化到 User Data 层
/// （即 password_sync 约定的「含 Default 子目录」的目录）。
fn normalize_user_data_dir(input: &Path) -> Option<PathBuf> {
  let input = input.to_path_buf();
  // 已是 User Data 布局（含 Default 子目录）
  if input.join("Default").is_dir() {
    return Some(input);
  }
  // 选到了 profile 层：Default 目录本身，或里面直接放着 Bookmarks / Login Data
  let is_profile_layer = input.file_name().and_then(|n| n.to_str()) == Some("Default")
    || input.join("Bookmarks").is_file()
    || input.join("Login Data").is_file();
  if is_profile_layer {
    return input.parent().map(|p| p.to_path_buf());
  }
  None
}

/// 探测本机常见浏览器的默认数据目录（含数据可用性标记）
#[tauri::command]
pub fn bwbrowser_detect_import_dirs() -> Result<Vec<Value>, String> {
  Ok(
    default_candidates()
      .into_iter()
      .map(|(label, path)| {
        let has_default = path.join("Default").is_dir();
        let has_data = has_default
          && (path.join("Default").join("Bookmarks").is_file()
            || path.join("Default").join("Login Data").is_file());
        json!({
          "label": label,
          "path": path.to_string_lossy(),
          "has_default": has_default,
          "has_data": has_data,
        })
      })
      .collect(),
  )
}

// ============================================================
// 书签添加式合并
// ============================================================

/// Chromium 时间戳（1601-01-01 起的微秒，字符串）
fn chromium_now() -> String {
  let unix_micros = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_micros() as u64)
    .unwrap_or(0);
  (unix_micros + 11_644_473_600_000_000).to_string()
}

/// 目标书签树里已用到的最大节点 id（Chromium 节点 id 可能是字符串或数字）
fn max_node_id(v: &Value) -> u64 {
  let mut m = 0u64;
  if let Some(o) = v.as_object() {
    if let Some(id) = o.get("id") {
      let n = match id {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
      };
      if let Some(n) = n {
        m = m.max(n);
      }
    }
    for child in o.values() {
      m = m.max(max_node_id(child));
    }
  } else if let Some(arr) = v.as_array() {
    for child in arr {
      m = m.max(max_node_id(child));
    }
  }
  m
}

struct IdGen {
  next: u64,
}

impl IdGen {
  fn new(root: &Value) -> Self {
    IdGen {
      next: max_node_id(root).max(100),
    }
  }
  fn take(&mut self) -> String {
    self.next += 1;
    self.next.to_string()
  }
}

/// 收集整棵树里所有 url 节点的 URL（用于全局去重）。
/// 顶层 JSON 是 {"roots": {...}} 结构，必须先下钻 roots——只沿 children
/// 链走的话整个文档数出来是 0（同 bookmark_merge::count_v 的坑）。
fn collect_urls(v: &Value, out: &mut HashSet<String>) {
  if let Some(roots) = v.get("roots").and_then(|r| r.as_object()) {
    for r in roots.values() {
      collect_urls(r, out);
    }
    return;
  }
  if let Some(children) = v.get("children").and_then(|c| c.as_array()) {
    for c in children {
      collect_urls(c, out);
    }
  }
  if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
    if !url.is_empty() {
      out.insert(url.to_string());
    }
  }
}

/// 统计书签 JSON 里的去重 url 数（挑选书签源文件用）
fn count_url_nodes(v: &Value) -> usize {
  let mut set = HashSet::new();
  collect_urls(v, &mut set);
  set.len()
}

/// 克隆节点整棵子树并重编 id / guid（导入节点的 id、guid 必须与目标树不冲突）
fn reidentify(node: &Value, gen: &mut IdGen) -> Value {
  let mut n = node.clone();
  if let Some(o) = n.as_object_mut() {
    o.insert("id".into(), Value::from(gen.take()));
    o.insert("guid".into(), Value::from(uuid::Uuid::new_v4().to_string()));
    if let Some(children) = o.get_mut("children").and_then(|c| c.as_array_mut()) {
      *children = children.iter().map(|c| reidentify(c, gen)).collect();
    }
  }
  n
}

/// URL 已存在 → None（跳过）；否则克隆重编后加入 seen
fn dedupe_url_node(
  node: &Value,
  seen: &mut HashSet<String>,
  gen: &mut IdGen,
  st: &mut (usize, usize),
) -> Option<Value> {
  let url = node.get("url").and_then(|u| u.as_str()).unwrap_or("");
  if url.is_empty() || seen.contains(url) {
    if !url.is_empty() {
      st.1 += 1;
    }
    return None;
  }
  seen.insert(url.to_string());
  st.0 += 1;
  Some(reidentify(node, gen))
}

/// 新文件夹整棵带入，但内部 URL 仍逐个去重；空文件夹不带入
fn dedupe_folder_node(
  node: &Value,
  seen: &mut HashSet<String>,
  gen: &mut IdGen,
  st: &mut (usize, usize),
) -> Option<Value> {
  let mut kids: Vec<Value> = Vec::new();
  if let Some(arr) = node.get("children").and_then(|c| c.as_array()) {
    for c in arr {
      let ty = c.get("type").and_then(|t| t.as_str()).unwrap_or("");
      if ty == "folder" {
        if let Some(k) = dedupe_folder_node(c, seen, gen, st) {
          kids.push(k);
        }
      } else if ty == "url" {
        if let Some(k) = dedupe_url_node(c, seen, gen, st) {
          kids.push(k);
        }
      }
    }
  }
  if kids.is_empty() {
    return None;
  }
  let mut folder = node.clone();
  if let Some(o) = folder.as_object_mut() {
    o.insert("id".into(), Value::from(gen.take()));
    o.insert("guid".into(), Value::from(uuid::Uuid::new_v4().to_string()));
    o.insert("children".into(), Value::Array(kids));
  }
  Some(folder)
}

/// 把源 children 并入目标 children（同名文件夹递归合并，URL 全局去重）
fn merge_children_add_only(
  src_children: &[Value],
  dst_children: &mut Vec<Value>,
  seen: &mut HashSet<String>,
  gen: &mut IdGen,
  st: &mut (usize, usize),
) {
  for src in src_children {
    let ty = src.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "url" {
      if let Some(node) = dedupe_url_node(src, seen, gen, st) {
        dst_children.push(node);
      }
    } else if ty == "folder" {
      let name = src.get("name").and_then(|n| n.as_str()).unwrap_or("");
      let dst_folder = dst_children.iter_mut().find(|d| {
        d.get("type").and_then(|t| t.as_str()) == Some("folder")
          && d.get("name").and_then(|n| n.as_str()) == Some(name)
          && !name.is_empty()
      });
      if let Some(dst_folder) = dst_folder {
        // 同名文件夹已存在 → 递归合并进去
        if let Some(o) = dst_folder.as_object_mut() {
          let mut children: Vec<Value> = o
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
          let src_kids: Vec<Value> = src
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
          merge_children_add_only(&src_kids, &mut children, seen, gen, st);
          o.insert("children".into(), Value::Array(children));
        }
      } else if let Some(node) = dedupe_folder_node(src, seen, gen, st) {
        dst_children.push(node);
      }
    }
  }
}

fn skeleton_bookmarks() -> Value {
  let mk_root = |name: &str| {
    json!({
      "children": [],
      "date_added": chromium_now(),
      "date_last_used": "0",
      "date_modified": chromium_now(),
      "guid": uuid::Uuid::new_v4().to_string(),
      "id": "1",
      "name": name,
      "type": "folder",
    })
  };
  json!({
    "roots": {
      "bookmark_bar": mk_root("书签栏"),
      "other": mk_root("其他书签"),
      "synced": mk_root("移动设备书签"),
    },
    "version": 1,
  })
}

/// 书签添加式导入：读源 User Data\Default\Bookmarks，按 URL 去重并入目标
/// profile 的 Bookmarks（结构保留、绝不删除/覆盖）。返回 {found, added, skipped}。
fn import_bookmarks_into_target(profile_id: &str, src_userdata: &Path) -> Result<Value, String> {
  // 源书签文件：标准 Bookmarks 与新版内核的 AccountBookmarks（Google 账号书签，
  // 同为 roots 结构 JSON）都尝试解析，取 url 节点多的那份——有些内核会生成
  // 空的 Bookmarks 占位文件，真书签在 AccountBookmarks 里，不能只看文件存在
  let std_path = src_userdata.join("Default").join("Bookmarks");
  let alt_path = src_userdata.join("Default").join("AccountBookmarks");
  let mut best: Option<(usize, Value, String)> = None;
  for path in [std_path, alt_path] {
    if !path.is_file() {
      continue;
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
      continue;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
      continue; // 非标准书签 JSON（如加密账号书签）→ 跳过该文件
    };
    let n = count_url_nodes(&v);
    let name = path
      .file_name()
      .map(|f| f.to_string_lossy().into_owned())
      .unwrap_or_default();
    if best.as_ref().is_none_or(|(m, _, _)| n > *m) {
      best = Some((n, v, name));
    }
  }
  let Some((src_urls, src, src_name)) = best else {
    return Ok(json!({ "found": false, "added": 0, "skipped": 0 }));
  };

  let target_raw = crate::bwbrowser_cloud::BwbrowserAuthManager::read_local_bookmarks(profile_id)?;
  let mut target: Value = match target_raw {
    Some(s) => serde_json::from_str(&s).unwrap_or_else(|_| skeleton_bookmarks()),
    None => skeleton_bookmarks(),
  };

  let mut seen = HashSet::new();
  collect_urls(&target, &mut seen);
  let mut gen = IdGen::new(&target);
  let mut st = (0usize, 0usize);

  let empty = Map::new();
  let src_roots = src
    .get("roots")
    .and_then(|r| r.as_object())
    .unwrap_or(&empty);
  if let Some(dst_roots) = target.get_mut("roots").and_then(|r| r.as_object_mut()) {
    for key in ["bookmark_bar", "other", "synced"] {
      let Some(src_root) = src_roots.get(key) else {
        continue;
      };
      let Some(src_kids) = src_root.get("children").and_then(|c| c.as_array()) else {
        continue;
      };
      // 目标缺该根时补一个骨架根（Chromium 启动时按 roots 结构加载）
      if !dst_roots.get(key).map(|r| r.is_object()).unwrap_or(false) {
        let skeleton_root = skeleton_bookmarks()["roots"][key].clone();
        dst_roots.insert(key.to_string(), skeleton_root);
      }
      let dst_root = dst_roots.get_mut(key).unwrap();
      let mut kids: Vec<Value> = dst_root
        .get("children")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
      merge_children_add_only(src_kids, &mut kids, &mut seen, &mut gen, &mut st);
      if let Some(o) = dst_root.as_object_mut() {
        o.insert("children".into(), Value::Array(kids));
      }
    }
  }

  // checksum 一律去掉（Chromium 会自行重算），version 缺失补 1
  if let Some(o) = target.as_object_mut() {
    o.remove("checksum");
    o.entry("version".to_string()).or_insert(Value::from(1));
  }

  let out = serde_json::to_string(&target).map_err(|e| format!("序列化书签失败: {e}"))?;
  crate::bwbrowser_cloud::BwbrowserAuthManager::write_local_bookmarks(profile_id, &out)?;

  Ok(json!({
    "found": true,
    "added": st.0,
    "skipped": st.1,
    "source": src_name,
    "source_urls": src_urls,
  }))
}

// ============================================================
// 导入命令
// ============================================================

/// 一键导入本机浏览器数据（书签 / 密码）到指定账号的指纹浏览器 profile。
/// source_dir 可传 User Data 层或 Default/profile 层（自动归一化）。
/// 请求与失败路径都写日志（data_import），便于从 debug log 排查每次点击。
#[tauri::command]
pub async fn bwbrowser_import_local_browser_data(
  account_id: i64,
  account_name: String,
  source_dir: String,
  import_bookmarks: bool,
  import_passwords: bool,
) -> Result<Value, String> {
  crate::bwbrowser_cloud::log_bwbrowser(
    "data_import",
    &format!(
      "▶ 导入请求：账号 {account_name}（{account_id}）来源 {} 书签={import_bookmarks} 密码={import_passwords}",
      source_dir.trim(),
    ),
  );
  let result = import_impl(
    account_id,
    &account_name,
    &source_dir,
    import_bookmarks,
    import_passwords,
  );
  match &result {
    Ok(v) => crate::bwbrowser_cloud::log_bwbrowser(
      "data_import",
      &format!(
        "✓ 账号 {account_name}（{account_id}）导入完成：来源 {}，结果 {v}",
        source_dir.trim(),
      ),
    ),
    Err(e) => crate::bwbrowser_cloud::log_bwbrowser_error(
      "data_import",
      &format!(
        "✗ 账号 {account_name}（{account_id}）导入失败：来源 {}，{e}",
        source_dir.trim(),
      ),
    ),
  }
  result
}

fn import_impl(
  account_id: i64,
  account_name: &str,
  source_dir: &str,
  import_bookmarks: bool,
  import_passwords: bool,
) -> Result<Value, String> {
  if !import_bookmarks && !import_passwords {
    return Err("请至少勾选一种要导入的数据（书签或密码）".to_string());
  }

  // 1. 解析账号对应的本机 profile（account_profile_name 命名约定）
  let pm = crate::profile::manager::ProfileManager::instance();
  let profile_name = crate::bwbrowser_cloud::account_profile_name(account_name, account_id);
  let profiles = pm
    .list_profiles()
    .map_err(|e| format!("读取浏览器配置列表失败: {e}"))?;
  let profile = profiles
    .iter()
    .find(|p| p.name.to_lowercase() == profile_name.to_lowercase())
    .ok_or_else(|| {
      format!(
        "未找到该账号对应的本机浏览器环境（约定名 {profile_name}）。请先用该账号启动一次浏览器生成环境后再导入"
      )
    })?;

  // 2. 浏览器运行中拒绝导入（锁库 + 退出时覆盖）
  if profile
    .process_id
    .is_some_and(crate::proxy_storage::is_process_running)
  {
    return Err(
      "该账号的浏览器正在运行，请先关闭浏览器窗口后再导入（运行中导入的数据会在浏览器退出时被覆盖）"
        .to_string(),
    );
  }

  // 3. 源目录归一化与校验
  let src = normalize_user_data_dir(Path::new(source_dir.trim()))
    .ok_or_else(|| {
      "所选目录不是浏览器数据目录：需要包含 Default 子目录（如 Chrome 的 User Data）或直接是 Default / profile 目录".to_string()
    })?;
  if !src.join("Default").is_dir() {
    return Err(format!(
      "目录 {} 下没有 Default 子目录，不是可用的浏览器数据目录",
      src.to_string_lossy()
    ));
  }

  // 4. 目标 profile 目录
  let profiles_dir = pm.get_profiles_dir();
  let target_profile = profile.get_profile_data_path(&profiles_dir);
  if !target_profile.exists() {
    return Err(format!(
      "该账号的浏览器环境目录不存在（{}），请先启动一次该账号浏览器",
      target_profile.to_string_lossy()
    ));
  }
  let profile_id = profile.id.to_string();

  let mut result = Map::new();

  // 5. 书签导入（添加式合并）
  if import_bookmarks {
    let bm = import_bookmarks_into_target(&profile_id, &src)?;
    result.insert("bookmarks".into(), bm);
  }

  // 6. 密码导入（复用 password_sync：源解密 → 目标加密落库，添加式去重）
  if import_passwords {
    match crate::password_sync::export_passwords(&src) {
      Ok(Some(json)) => {
        let (imported, skipped) = crate::password_sync::import_passwords(&target_profile, &json)
          .map_err(|e| format!("写入目标密码库失败: {e}"))?;
        result.insert(
          "passwords".into(),
          json!({ "found": true, "imported": imported, "skipped": skipped }),
        );
      }
      Ok(None) => {
        result.insert(
          "passwords".into(),
          json!({ "found": false, "imported": 0, "skipped": 0 }),
        );
      }
      Err(e) => return Err(format!("读取源密码库失败: {e}")),
    }
  }

  Ok(Value::Object(result))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn url_node(name: &str, link: &str) -> Value {
    json!({
      "date_added": "1", "date_last_used": "0", "guid": format!("src-{}", link),
      "id": "9", "name": name, "type": "url", "url": link,
    })
  }

  #[test]
  fn 同名文件夹合并_url去重() {
    let mut target = skeleton_bookmarks();
    if let Some(o) = target["roots"]["bookmark_bar"].as_object_mut() {
      o["children"] = json!([
        {"type":"folder","name":"工作","children":[url_node("A","https://a")],"id":"5","guid":"f1"},
        url_node("B", "https://b"),
      ]);
    }
    let src = json!({
      "roots": {
        "bookmark_bar": {"type":"folder","name":"书签栏","children":[
          {"type":"folder","name":"工作","children":[
            url_node("A2","https://a"),       // 与目标重复 → 跳过
            url_node("C","https://c"),        // 新增
          ]},
          {"type":"folder","name":"娱乐","children":[url_node("D","https://d")]}, // 新文件夹整棵带入
          url_node("B2","https://b"),         // 重复 → 跳过
        ]},
        "other": {"type":"folder","name":"其他书签","children":[]},
        "synced": {"type":"folder","name":"移动设备书签","children":[]},
      },
      "version": 1,
    });

    let mut seen = HashSet::new();
    collect_urls(&target, &mut seen);
    assert_eq!(seen.len(), 2); // https://a + https://b

    let mut gen = IdGen::new(&target);
    let mut st = (0usize, 0usize);
    let bar = &src["roots"]["bookmark_bar"];
    let src_kids: Vec<Value> = bar["children"].as_array().unwrap().clone();
    let mut dst_kids: Vec<Value> = target["roots"]["bookmark_bar"]["children"]
      .as_array()
      .unwrap()
      .clone();
    merge_children_add_only(&src_kids, &mut dst_kids, &mut seen, &mut gen, &mut st);
    assert_eq!(st.0, 2); // 新增 c、d
    assert_eq!(st.1, 2); // 重复 a、b

    let urls = serde_json::to_string(&dst_kids).unwrap();
    assert!(urls.contains("https://c") && urls.contains("https://d"));
  }

  #[test]
  fn 空文件夹不带入_新节点重编id() {
    let target = skeleton_bookmarks();
    let mut seen = HashSet::new();
    collect_urls(&target, &mut seen);
    let mut gen = IdGen::new(&target);
    let mut st = (0usize, 0usize);

    let src_folder = json!({
      "type":"folder","name":"空","children":[
        {"type":"folder","name":"更深","children":[url_node("X","https://x")]},
      ],
    });
    let out = dedupe_folder_node(&src_folder, &mut seen, &mut gen, &mut st);
    assert!(out.is_some());
    let out = out.unwrap();
    // 新节点 id 与目标骨架的 id 不冲突
    let new_id = out["id"].as_str().unwrap().parse::<u64>().unwrap();
    assert!(new_id > 1);
    assert!(out.to_string().contains("https://x"));
  }

  #[test]
  fn 顶层roots必须下钻_否则计数与去重全为0() {
    // 回归测试：2026-11-26 实际事故——collect_urls 只沿 children 链走，
    // 顶层 {"roots":{...}} 数出 0 个 url，导致空占位 Bookmarks 被当成
    // 数据源、且目标去重集合恒为空（重复导入会产生大量重复书签）。
    let doc = json!({
      "roots": {
        "bookmark_bar": {"type":"folder","name":"书签栏","children":[
          url_node("A", "https://a"),
          {"type":"folder","name":"子","children":[url_node("B", "https://b")]},
        ]},
        "other": {"type":"folder","name":"其他书签","children":[]},
        "synced": {"type":"folder","name":"移动设备书签","children":[]},
      },
      "version": 1,
    });
    assert_eq!(count_url_nodes(&doc), 2);
    let mut seen = HashSet::new();
    collect_urls(&doc, &mut seen);
    assert_eq!(seen.len(), 2);
    assert!(seen.contains("https://a") && seen.contains("https://b"));
  }

  #[test]
  fn 空占位书签让位给有数据的账号书签() {
    // Bookmarks 存在但是空壳（新版内核常见）、AccountBookmarks 有数据
    // → 源选择必须取 url 多的后者
    let empty_bm = json!({
      "roots": {
        "bookmark_bar": {"children":[],"type":"folder","name":"书签栏"},
        "other": {"children":[],"type":"folder","name":"其他书签"},
        "synced": {"children":[],"type":"folder","name":"移动设备书签"},
      },
      "version": 1,
    });
    let mut acct = empty_bm.clone();
    acct["roots"]["bookmark_bar"]["children"] =
      json!([url_node("A", "https://a"), url_node("B", "https://b"),]);
    assert_eq!(count_url_nodes(&empty_bm), 0);
    assert_eq!(count_url_nodes(&acct), 2);
    assert!(count_url_nodes(&acct) > count_url_nodes(&empty_bm));
  }
}
