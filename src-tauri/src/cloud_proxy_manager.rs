#![allow(dead_code, clippy::too_many_arguments)]

use crate::browser::ProxySettings;
use crate::bwbrowser_cloud::{bwbrowser_to_proxy_settings, BwbrowserProxy, BWBROWSER_AUTH};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// Prefix for proxy_id that directly encodes a proxy_node string.
/// Used when profile.proxy_id stores the raw proxy_node from cloud
/// instead of a local UUID or cloud numeric ID.
pub const NODE_PREFIX: &str = "node:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudProxyResult {
  pub proxy_id: String,
  pub source: String,
  pub settings: Option<ProxySettings>,
}

#[derive(Debug)]
struct CacheEntry {
  proxies: Vec<BwbrowserProxy>,
  fetched_at: Instant,
}

static PROXY_CACHE: Mutex<Option<CacheEntry>> = Mutex::new(None);

fn get_cached_proxies() -> Option<Vec<BwbrowserProxy>> {
  if let Ok(Some(entry)) = PROXY_CACHE.lock().as_deref() {
    if entry.fetched_at.elapsed() < CACHE_TTL {
      return Some(entry.proxies.clone());
    }
  }
  None
}

fn set_cache(proxies: Vec<BwbrowserProxy>) {
  if let Ok(mut slot) = PROXY_CACHE.lock() {
    *slot = Some(CacheEntry {
      proxies,
      fetched_at: Instant::now(),
    });
  }
}

pub fn invalidate_cache() {
  if let Ok(mut slot) = PROXY_CACHE.lock() {
    *slot = None;
  }
}

/// Fetch all proxies from cloud MySQL.
/// Uses 30s in-memory cache to avoid hammering the API.
/// Falls back to stale cache if cloud is unreachable.
pub async fn fetch_cloud_proxies() -> Result<Vec<BwbrowserProxy>, String> {
  if let Some(cached) = get_cached_proxies() {
    log::debug!(
      "cloud_proxy_manager: using cached proxies ({} entries)",
      cached.len()
    );
    return Ok(cached);
  }

  match BWBROWSER_AUTH.list_cloud_proxies(None).await {
    Ok(proxies) => {
      log::info!(
        "cloud_proxy_manager: fetched {} proxies from cloud",
        proxies.len()
      );
      set_cache(proxies.clone());
      Ok(proxies)
    }
    Err(e) => {
      log::warn!(
        "cloud_proxy_manager: cloud fetch failed ({}), trying stale cache",
        e
      );
      if let Ok(Some(entry)) = PROXY_CACHE.lock().as_deref() {
        log::info!(
          "cloud_proxy_manager: returning stale cache ({} entries)",
          entry.proxies.len()
        );
        return Ok(entry.proxies.clone());
      }
      Err(e)
    }
  }
}

/// Find a cloud proxy by its ID.
pub async fn find_cloud_proxy_by_id(proxy_id: i64) -> Option<BwbrowserProxy> {
  let proxies = fetch_cloud_proxies().await.ok()?;
  proxies.into_iter().find(|p| p.proxy_id == proxy_id)
}

/// Find a cloud proxy by host:port.
pub async fn find_cloud_proxy_by_hostport(host: &str, port: u16) -> Option<BwbrowserProxy> {
  let proxies = fetch_cloud_proxies().await.ok()?;
  proxies
    .into_iter()
    .find(|p| p.host == host && p.port as u16 == port)
}

/// Find a cloud proxy by its vless_uri (for VLESS/Trojan/SS matching).
pub async fn find_cloud_proxy_by_uri(uri: &str) -> Option<BwbrowserProxy> {
  let proxies = fetch_cloud_proxies().await.ok()?;
  let trimmed = uri.trim();
  for p in &proxies {
    let settings = bwbrowser_to_proxy_settings(p);
    if let Some(ref u) = settings.vless_uri {
      if u.trim() == trimmed {
        return Some(p.clone());
      }
    }
  }
  None
}

/// Parse a proxy_node string (the format stored in cloud MySQL `tiktok_accounts.proxy_node`)
/// into ProxySettings directly, without any local file lookup.
///
/// Formats:
/// - VLESS/Trojan: full URI (`vless://...` / `trojan://...`)
/// - HTTP/SOCKS5: `type:host:port:user:pass` or legacy `host:port:user:pass`
pub fn parse_proxy_node(node: &str) -> Option<ProxySettings> {
  let node = node.trim();
  if node.is_empty() {
    return None;
  }

  if node.starts_with("vless://") || node.starts_with("trojan://") {
    let proxy_type = if node.starts_with("vless://") {
      "vless"
    } else {
      "trojan"
    };
    return Some(ProxySettings {
      proxy_type: proxy_type.to_string(),
      host: String::new(),
      port: 0,
      username: None,
      password: None,
      vless_uri: Some(node.to_string()),
    });
  }

  let parts: Vec<&str> = node.split(':').collect();
  if parts.len() < 2 {
    return None;
  }

  const KNOWN_TYPES: &[&str] = &["http", "socks5"];
  let (proxy_type, host_idx) = {
    let first = parts[0].to_lowercase();
    if KNOWN_TYPES.contains(&first.as_str()) {
      (first, 1usize)
    } else {
      ("http".to_string(), 0usize)
    }
  };

  let remaining = &parts[host_idx..];
  if remaining.len() < 2 {
    return None;
  }

  let host = remaining[0].to_string();
  let port: u16 = remaining[1].parse().ok()?;
  let username = if remaining.len() >= 3 {
    Some(remaining[2].to_string())
  } else {
    None
  };
  let password = if remaining.len() >= 4 {
    Some(remaining[3..].join(":"))
  } else {
    None
  };

  Some(ProxySettings {
    proxy_type,
    host,
    port,
    username,
    password,
    vless_uri: None,
  })
}

/// Cloud-only proxy resolution.
/// Accepts multiple proxy_id formats:
/// - `node:<proxy_node>` — parse the embedded proxy_node string directly
/// - `cloud_<id>` or bare integer — look up in cloud MySQL by proxy ID
/// - local UUID (legacy) — match via local stored proxy's host:port/URI to cloud
pub async fn get_proxy_cloud_only(proxy_id: &str) -> Option<ProxySettings> {
  // Format 1: node:<proxy_node> — direct parse, no cloud or local lookup needed
  if let Some(node) = proxy_id.strip_prefix(NODE_PREFIX) {
    log::info!(
      "cloud_proxy_manager: resolving node: prefix, node_len={}",
      node.len()
    );
    if let Some(settings) = parse_proxy_node(node) {
      log::info!(
        "cloud_proxy_manager: parsed proxy from node: prefix, type={}, host={}, port={}",
        settings.proxy_type,
        settings.host,
        settings.port
      );
      return Some(settings);
    }
    log::warn!("cloud_proxy_manager: failed to parse proxy_node from node: prefix");
  }

  // Format 2: cloud numeric ID
  let cloud_id = if let Some(rest) = proxy_id.strip_prefix("cloud_") {
    rest.parse::<i64>().ok()
  } else {
    proxy_id.parse::<i64>().ok()
  };

  if let Some(cid) = cloud_id {
    if let Some(cloud_proxy) = find_cloud_proxy_by_id(cid).await {
      return Some(bwbrowser_to_proxy_settings(&cloud_proxy));
    }
  }

  // Format 3: legacy local UUID — match via local stored proxy to cloud
  let local = crate::proxy_manager::PROXY_MANAGER.get_stored_proxies();
  if let Some(local_proxy) = local.iter().find(|p| p.id == proxy_id) {
    let proxy_type = &local_proxy.proxy_settings.proxy_type;
    let cloud_proxy = if ["vless", "trojan", "ss"].contains(&proxy_type.as_str()) {
      let mut found = None;
      if let Some(ref uri) = local_proxy.proxy_settings.vless_uri {
        found = find_cloud_proxy_by_uri(uri).await;
      }
      if found.is_none() {
        found = find_cloud_proxy_by_hostport(
          &local_proxy.proxy_settings.host,
          local_proxy.proxy_settings.port,
        )
        .await;
      }
      found
    } else {
      find_cloud_proxy_by_hostport(
        &local_proxy.proxy_settings.host,
        local_proxy.proxy_settings.port,
      )
      .await
    };

    if let Some(cp) = cloud_proxy {
      return Some(bwbrowser_to_proxy_settings(&cp));
    }

    // Cloud unreachable but we have local data — use it as last resort
    log::warn!("cloud_proxy_manager: cloud unreachable for {proxy_id}, using local data");
    return Some(local_proxy.proxy_settings.clone());
  }

  None
}

/// Tauri command: list all proxies from cloud (MySQL source of truth).
#[tauri::command]
pub async fn cloud_list_proxies() -> Result<Vec<crate::bwbrowser_cloud::BwbrowserProxyItem>, String>
{
  crate::bwbrowser_cloud::bwbrowser_list_proxies(None).await
}

/// Tauri command: create or update a proxy in cloud MySQL. No local sync.
#[tauri::command]
pub async fn cloud_save_proxy(
  _app_handle: tauri::AppHandle,
  proxy_id: Option<i64>,
  name: String,
  proxy_type: String,
  host: String,
  port: u16,
  username: Option<String>,
  password: Option<String>,
  country: Option<String>,
  city: Option<String>,
  timezone: Option<String>,
  protocol_config: Option<String>,
) -> Result<i64, String> {
  let new_id = crate::bwbrowser_cloud::bwbrowser_sync_proxy(
    proxy_id,
    name,
    proxy_type,
    host,
    port,
    username,
    password,
    country,
    city,
    timezone,
    protocol_config,
  )
  .await?;

  invalidate_cache();
  Ok(new_id)
}

/// Tauri command: delete proxy from cloud MySQL. No local sync.
#[tauri::command]
pub async fn cloud_delete_proxy(
  _app_handle: tauri::AppHandle,
  proxy_id: i64,
) -> Result<(), String> {
  crate::bwbrowser_cloud::bwbrowser_delete_proxy(proxy_id).await?;
  invalidate_cache();
  Ok(())
}

/// Tauri command: force refresh proxy cache from cloud.
#[tauri::command]
pub async fn cloud_refresh_proxies(
  _app_handle: tauri::AppHandle,
) -> Result<crate::bwbrowser_cloud::SyncResult, String> {
  invalidate_cache();
  fetch_cloud_proxies().await?;
  Ok(crate::bwbrowser_cloud::SyncResult {
    cloud_total: 0,
    created: 0,
    skipped: 0,
    removed: 0,
  })
}

/// Tauri command: resolve proxy settings for a profile, cloud-only.
#[tauri::command]
pub async fn cloud_resolve_proxy(proxy_id: String) -> Result<Option<ProxySettings>, String> {
  Ok(get_proxy_cloud_only(&proxy_id).await)
}
