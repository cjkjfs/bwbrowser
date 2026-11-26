//! 书签三路增量合并（2026-11-26）
//!
//! 背景：旧同步是「整文件覆盖」——启动时云端覆盖本地、关闭时本地覆盖云端，
//! 任何一次空/瘦内容都会把另一头的完整书签冲掉（2026-11-25 sqyacmyxgs 事故）。
//!
//! 新语义（用户要求：只同步增删改，不整体覆盖）：
//!   - 本地改动（新增/删除/改名/改 URL）永远保留；
//!   - 应用「云端相对基准的新增」；
//!   - 应用「云端相对基准的删除」（仅当本地相对基准未改动该节点）；
//!   - 应用「云端对本地未改动节点的修改」（改名/改 URL，以云端为准）；
//!   - 云端相对基准骤减（节点数腰斩以上）视为异常覆盖，忽略云端删除、本地为准。
//!
//! 基准（base）= 上次同步完成时双方共同的可见状态：
//!   启动下行后记录 base = 本次拉到的云端内容；
//!   关闭上行成功后记录 base = 合并结果。
//!   base 缺失时以本地为基准（即只并入云端新增，不做云端删除）。
//!
//! 节点身份：优先 guid（Chromium 稳定分配），退化用 url，再退化用 type+name。
//! checksum 一律去掉：Chromium 加载时缺失/不匹配只会触发 id 重排，不影响书签数据，
//! 且 JSON 解析失败才会触发 .bak 还原——我们输出的始终是合法 JSON。

use serde_json::Value;
use std::collections::HashMap;

pub struct MergeOutcome {
  pub json: String,
  pub cloud_added: usize,
  pub cloud_deleted: usize,
  pub cloud_updated: usize,
  /// 某一侧 JSON 解析失败，保守退回本地原样（调用方不得覆盖本地/云端）
  pub used_fallback: bool,
  /// 云端相对基准骤减（疑似被旧客户端用空壳覆盖），已忽略云端删除、本地为准
  pub cloud_shrink_anomaly: bool,
}

const ROOT_KEYS: [&str; 3] = ["bookmark_bar", "other", "synced"];

/// 解析并去掉顶层 checksum（用于合并输入与语义比较）
fn canon(s: &str) -> Option<Value> {
  let mut v: Value = serde_json::from_str(s).ok()?;
  if let Some(o) = v.as_object_mut() {
    o.remove("checksum");
  }
  Some(v)
}

/// 两个书签 JSON 是否语义相同（忽略 checksum 与键序）
pub fn canon_eq(a: &str, b: &str) -> bool {
  match (canon(a), canon(b)) {
    (Some(x), Some(y)) => x == y,
    _ => false,
  }
}

/// 参与节点比较的「形状」：去掉易变字段（id 会被 Chromium 重排、date_last_used 随使用变化）
fn shape(v: &Value) -> Value {
  let mut c = v.clone();
  if let Some(o) = c.as_object_mut() {
    o.remove("id");
    o.remove("date_last_used");
  }
  c
}

fn node_key(v: &Value) -> Option<String> {
  if let Some(g) = v.get("guid").and_then(|x| x.as_str()) {
    if !g.is_empty() {
      return Some(format!("g:{}", g));
    }
  }
  if let Some(u) = v.get("url").and_then(|x| x.as_str()) {
    if !u.is_empty() {
      return Some(format!("u:{}", u));
    }
  }
  let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
  let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
  if name.is_empty() {
    None
  } else {
    Some(format!("n:{}:{}", ty, name))
  }
}

fn is_folder(v: &Value) -> bool {
  v.get("type").and_then(|x| x.as_str()) == Some("folder")
}

fn children(v: &Value) -> Vec<Value> {
  v.get("children")
    .and_then(|x| x.as_array())
    .cloned()
    .unwrap_or_default()
}

fn keymap(children: &[Value]) -> HashMap<String, &Value> {
  children
    .iter()
    .filter_map(|v| node_key(v).map(|k| (k, v)))
    .collect()
}

/// 递归计节点数：顶层 JSON 无 children 字段，须先下钻 roots
fn count_v(v: &Value) -> usize {
  if let Some(roots) = v.get("roots").and_then(|r| r.as_object()) {
    return roots.values().map(count_v).sum();
  }
  1 + children(v).iter().map(count_v).sum::<usize>()
}

/// 合并一层 children。st = (added, deleted, updated) 累计器。
/// allow_deletions=false 时（云端骤减异常）跳过云端删除。
fn merge_children(
  base_children: &[Value],
  local_children: &[Value],
  cloud_children: &[Value],
  st: &mut (usize, usize, usize),
  allow_deletions: bool,
) -> Vec<Value> {
  let bmap = keymap(base_children);
  let cmap = keymap(cloud_children);

  let mut out: Vec<Value> = local_children.to_vec();

  // 1) 共有节点：云端修改 / 文件夹递归
  for item in out.iter_mut() {
    let key = match node_key(item) {
      Some(k) => k,
      None => continue,
    };
    let (bv, cv) = match (bmap.get(&key), cmap.get(&key)) {
      (Some(b), Some(c)) => (*b, *c),
      _ => continue,
    };
    if is_folder(item) && is_folder(bv) && is_folder(cv) {
      let mc = merge_children(
        &children(bv),
        &children(item),
        &children(cv),
        st,
        allow_deletions,
      );
      if let Some(o) = item.as_object_mut() {
        o.insert("children".into(), Value::Array(mc));
      }
      // 文件夹自身元数据：本地未改名而云端改名 → 采纳云端
      if item.get("name") == bv.get("name") && cv.get("name") != bv.get("name") {
        if let (Some(o), Some(n)) = (item.as_object_mut(), cv.get("name")) {
          o.insert("name".into(), n.clone());
          st.2 += 1;
        }
      }
    } else if item.get("type") == cv.get("type")
      && shape(item) == shape(bv)
      && shape(cv) != shape(bv)
    {
      // 本地未改动而云端改了 → 采纳云端版本
      *item = cv.clone();
      st.2 += 1;
    }
  }

  // 2) 云端新增（不在基准、也不在本地）
  let out_keys: std::collections::HashSet<String> = out.iter().filter_map(node_key).collect();
  for cv in cloud_children {
    if let Some(k) = node_key(cv) {
      if !bmap.contains_key(&k) && !out_keys.contains(&k) {
        out.push(cv.clone());
        st.0 += 1;
      }
    }
  }

  // 3) 云端删除（在基准、不在云端，且本地相对基准未改动）
  if allow_deletions {
    out.retain(|v| {
      if let Some(k) = node_key(v) {
        if bmap.contains_key(&k) && !cmap.contains_key(&k) {
          if let Some(bv) = bmap.get(&k) {
            if shape(v) == shape(bv) {
              st.1 += 1;
              return false;
            }
          }
        }
      }
      true
    });
  }

  out
}

fn fallback(local: &str) -> MergeOutcome {
  MergeOutcome {
    json: local.to_string(),
    cloud_added: 0,
    cloud_deleted: 0,
    cloud_updated: 0,
    used_fallback: true,
    cloud_shrink_anomaly: false,
  }
}

/// 三路合并：base（上次同步点）→ local（本地当前）+ cloud（云端当前）。
/// base 为 None 时以本地为基准（只并入云端新增，不执行云端删除）。
pub fn merge(base: Option<&str>, local: &str, cloud: &str) -> MergeOutcome {
  let lv = match canon(local) {
    Some(v) => v,
    None => return fallback(local),
  };
  let cv = match canon(cloud) {
    Some(v) => v,
    None => return fallback(local),
  };
  let bv = base.and_then(canon);

  // 云端相对基准骤减检测（基准 >20 节点且云端不足一半 → 疑似异常覆盖）
  let mut shrink_anomaly = false;
  if let Some(b) = &bv {
    let bc = count_v(b);
    let cc = count_v(&cv);
    if bc > 20 && cc * 2 < bc {
      shrink_anomaly = true;
    }
  }

  let mut st = (0usize, 0usize, 0usize);
  let mut out = lv.clone();

  if let Some(roots) = out.get_mut("roots").and_then(|r| r.as_object_mut()) {
    let empty = Value::Object(serde_json::Map::new());
    let c_roots = cv.get("roots").unwrap_or(&empty);
    let b_roots = bv.as_ref().and_then(|b| b.get("roots"));
    for k in ROOT_KEYS {
      let l_root = match roots.get(k) {
        Some(v) if is_folder(v) => v.clone(),
        Some(_) => continue, // 本地根存在但类型异常 → 保持本地
        None => {
          // 本地缺根而云端有 → 整体采纳云端根
          if let Some(cr) = c_roots.get(k) {
            if is_folder(cr) {
              roots.insert(k.to_string(), cr.clone());
              st.0 += 1;
            }
          }
          continue;
        }
      };
      let c_root = match c_roots.get(k) {
        Some(v) if is_folder(v) => v.clone(),
        _ => continue, // 云端该根缺失/异常 → 本地为准（不动）
      };
      let b_children = b_roots
        .and_then(|r| r.get(k))
        .filter(|v| is_folder(v))
        .map(children)
        .unwrap_or_default();
      let merged = merge_children(
        &b_children,
        &children(&l_root),
        &children(&c_root),
        &mut st,
        !shrink_anomaly,
      );
      if let Some(o) = roots.get_mut(k).and_then(|r| r.as_object_mut()) {
        o.insert("children".into(), Value::Array(merged));
      }
    }
  }

  // 顶层校验和去掉（Chromium 会自行重算）；version 缺失补 1
  if let Some(o) = out.as_object_mut() {
    o.remove("checksum");
    o.entry("version".to_string()).or_insert(Value::from(1));
  }

  MergeOutcome {
    json: serde_json::to_string(&out).unwrap_or_else(|_| local.to_string()),
    cloud_added: st.0,
    cloud_deleted: st.1,
    cloud_updated: st.2,
    used_fallback: false,
    cloud_shrink_anomaly: shrink_anomaly,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn folder(name: &str, children: Value) -> Value {
    serde_json::json!({
      "children": children, "date_added": "1", "date_last_used": "0",
      "date_modified": "1", "guid": format!("f-{}", name), "id": "1",
      "name": name, "type": "folder"
    })
  }
  fn url(name: &str, link: &str) -> Value {
    serde_json::json!({
      "date_added": "1", "date_last_used": "0", "guid": format!("u-{}", name),
      "id": "2", "name": name, "type": "url", "url": link
    })
  }
  fn tree(bar: Value, other: Value) -> String {
    serde_json::json!({
      "checksum": "abc", "roots": {
        "bookmark_bar": bar, "other": other,
        "synced": folder("移动设备书签", serde_json::json!([]))
      },
      "version": 1
    })
    .to_string()
  }

  #[test]
  fn 空壳不冲掉本地完整书签_云端新增并入() {
    // 造 30 个书签的大树（骤减检测阈值：基准 >20 节点且云端不足一半）
    let mut urls = Vec::new();
    for i in 0..30 {
      urls.push(url(&format!("N{}", i), &format!("https://site{}", i)));
    }
    let base = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", Value::Array(urls)),
    );
    let local = base.clone();
    // 云端被旧客户端冲成空壳
    let cloud = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([])),
    );
    let m = merge(Some(&base), &local, &cloud);
    assert!(m.cloud_shrink_anomaly);
    // 本地书签全部保留
    assert!(m.json.contains("https://site0") && m.json.contains("https://site29"));
    assert_eq!(m.cloud_deleted, 0);
  }

  #[test]
  fn 云端新增_本地未有的并入() {
    let base = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([url("A", "https://a")])),
    );
    let local = base.clone();
    let cloud = tree(
      folder("书签栏", serde_json::json!([])),
      folder(
        "其他书签",
        serde_json::json!([url("A", "https://a"), url("NEW", "https://new")]),
      ),
    );
    let m = merge(Some(&base), &local, &cloud);
    assert_eq!(m.cloud_added, 1);
    assert!(m.json.contains("https://new"));
  }

  #[test]
  fn 本地删除不被云端复活() {
    let base = tree(
      folder("书签栏", serde_json::json!([])),
      folder(
        "其他书签",
        serde_json::json!([url("A", "https://a"), url("B", "https://b")]),
      ),
    );
    let local = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([url("A", "https://a")])),
    );
    let m = merge(Some(&base), &local, &base);
    // 云端 == base：无云端改动；本地删除 B 应保留删除状态
    assert!(!m.json.contains("https://b"));
    assert_eq!(m.cloud_added, 0);
  }

  #[test]
  fn 云端删除_本地未动的节点被移除() {
    let base = tree(
      folder("书签栏", serde_json::json!([])),
      folder(
        "其他书签",
        serde_json::json!([url("A", "https://a"), url("B", "https://b")]),
      ),
    );
    let local = base.clone();
    let cloud = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([url("A", "https://a")])),
    );
    let m = merge(Some(&base), &local, &cloud);
    assert_eq!(m.cloud_deleted, 1);
    assert!(m.json.contains("https://a") && !m.json.contains("https://b"));
  }

  #[test]
  fn 云端改名_本地未动则采纳() {
    let base = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([url("旧名", "https://a")])),
    );
    let local = base.clone();
    let cloud = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([url("新名", "https://a")])),
    );
    let m = merge(Some(&base), &local, &cloud);
    assert_eq!(m.cloud_updated, 1);
    assert!(m.json.contains("新名"));
  }

  #[test]
  fn base缺失_只并云端新增不执行删除() {
    let local = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([url("A", "https://a")])),
    );
    let cloud = tree(
      folder("书签栏", serde_json::json!([])),
      folder("其他书签", serde_json::json!([])),
    );
    let m = merge(None, &local, &cloud);
    // 无基准 → 云端删除不可信，本地为准
    assert!(m.json.contains("https://a"));
    assert_eq!(m.cloud_deleted, 0);
  }
}
