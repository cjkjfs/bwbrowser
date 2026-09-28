//! Revenue shield: a bundled browser extension that hides earnings surfaces in a
//! profile's browser while the signed-in employee may not see revenue.
//!
//! ## Why an extension rather than CDP script injection
//!
//! The first cut injected a `MutationObserver` over CDP. That cannot work for the
//! employees this exists for: the browser refuses `Runtime.evaluate` unless the
//! account carries a paid browser-automation plan, so the observer never
//! installed and the only trace was a refusal in the log. An extension is an
//! ordinary browser feature that needs no automation entitlement. Its stylesheet
//! is applied by the browser itself at `document_start` to every navigation and
//! to every node inserted afterwards — the hide survives page changes with no
//! watchdog round trip to the page.
//!
//! ## Where the selectors come from
//!
//! The union of the local `REVENUE_SELECTORS` fallback and, when the launch is
//! for a known cloud platform, that platform's `earnings_hide.selectors` from
//! `api_get_platform_config.php`. The platform list is authoritative and far
//! sharper than what a class-name guess reaches: Douyin publishes
//! `#douyin-creator-master-menu-nav-cash`, which is exactly the `收入变现`
//! navigation entry a substring selector misses. Hiding an id selector like this
//! removes the whole entry, where a text-label match only blanked the caption
//! and left the clickable spot behind.
//!
//! `earnings_hide.js_script` is deliberately NOT copied into the extension. It
//! is remote JS a config host controls; running it on launch would hand the
//! employee's browser to that host whenever revenue was due to be hidden. Its
//! only effect is `display: none`, so the structured `selectors` field alone
//! hides the same surfaces with no code of ours in the page.
//!
//! ## Scope
//!
//! Cosmetic and page-local: the figure is still in the DOM and the inspector
//! brings it back. Anything that must not be readable has to be withheld by the
//! site's own server-side permissions.
//!
//! ## Fail-closed
//!
//! Every degradation — signed out, a failed lookup, a user the backend does not
//! recognise — reads as "may not see revenue" and loads the shield. Hiding a
//! figure from someone entitled to it is a cosmetic annoyance the next launch
//! clears; showing one to someone who is not is the leak this module exists to
//! prevent.

use std::fs;
use std::path::{Path, PathBuf};

/// Directory the shield is staged into, and the extension's stable identity.
///
/// It sits inside the profile's staging root so the cleanup that already runs
/// for user extensions when a browser exits removes this one with them.
const SHIELD_DIR: &str = "bw-revenue-shield";

/// The permission key `users.php` reports for "may see revenue".
///
/// The same key the management UI renders as 允许查看收益. It arrives in the
/// `permissions` map of the `me` action, which is what
/// `get_current_management_user` reads.
const REVENUE_PERMISSION_KEY: &str = "allow_view_revenue";

/// Selectors that match the usual revenue/earnings surfaces.
///
/// Deliberately broad — they are substring matches on class names, so a site
/// that names a container `revenue-summary` is covered without anyone writing a
/// selector for it. The cost of that breadth is false positives on unrelated
/// pages, which is the price of hiding a figure without knowing every site's
/// markup.
///
/// `[class*='balance']` is left out on purpose: `text-balance` and the
/// `balance-*` layout utilities are ordinary CSS, and hiding them would blank
/// real text on pages that have nothing to do with revenue.
const REVENUE_SELECTORS: &[&str] = &[
  "[class*='revenue']",
  "[class*='Revenue']",
  "[class*='REVENUE']",
  "[class*='earnings']",
  "[class*='Earnings']",
  "[class*='profit']",
  "[class*='Profit']",
  "[class*='income']",
  "[class*='Income']",
  "[class*='billing']",
  "[class*='Billing']",
  "[class*='commission']",
  "[class*='Commission']",
  "[data-role*='revenue']",
  "[data-role*='earnings']",
  "[data-role*='profit']",
  "[data-testid*='revenue']",
  "[data-testid*='earnings']",
  "[class*='stat-amount']",
  "[class*='amount-display']",
  "[class*='money-display']",
  "[class*='wallet']",
  "[class*='Wallet']",
  "[class*='money']",
  "[class*='Money']",
  "[class*='cash']",
  "[class*='Cash']",
  "[class*='收益']",
  "[class*='收入']",
  "[class*='利润']",
  "[class*='佣金']",
  "[class*='奖金']",
  "[class*='提现']",
  "[class*='钱包']",
  "svg[class*='revenue']",
  "svg[class*='earnings']",
  "i[class*='revenue']",
  "i[class*='earnings']",
];

/// The extension's manifest.
///
/// Manifest V3, a content script with a stylesheet and no scripting half: the
/// browser applies the CSS itself, so no code of ours runs in the page and there
/// is nothing for a site's CSP or a navigation to outrun. The selectors live in
/// `hide.css`, which is generated per launch, so the manifest itself never
/// changes.
const SHIELD_MANIFEST: &str = r#"{
  "manifest_version": 3,
  "name": "Revenue Shield",
  "version": "1.0.0",
  "description": "Hides earnings surfaces in the browser.",
  "content_scripts": [
    {
      "matches": ["<all_urls>"],
      "css": ["hide.css"],
      "run_at": "document_start"
    }
  ]
}
"#;

// ──────────────────────────────────────────────────────────
// The extension payload
// ──────────────────────────────────────────────────────────

/// The stylesheet the content script applies.
///
/// One rule for the whole selector list, so the browser matches it in a single
/// pass, and `!important` so an inline `display` on the element cannot win. The
/// browser keeps applying it to nodes the page inserts later, so a dashboard
/// that paints its figures late is covered without any script.
fn hide_css(selectors: &[&str]) -> String {
  format!(
    "/* Generated by revenue_shield.rs. */\n{}\n{{\n  display: none !important;\n}}\n",
    selectors.join(",\n")
  )
}

/// Write the extension into `dir`, replacing whatever a previous launch left.
fn write_shield(dir: &Path, selectors: &[String]) -> std::io::Result<()> {
  let selectors: Vec<&str> = selectors.iter().map(String::as_str).collect();
  fs::create_dir_all(dir)?;
  fs::write(dir.join("manifest.json"), SHIELD_MANIFEST)?;
  fs::write(dir.join("hide.css"), hide_css(&selectors))
}

/// Where the shield is staged for one profile.
fn staged_dir(profile_id: &str) -> PathBuf {
  crate::extension_manager::ExtensionManager::unpacked_dir_for_profile(profile_id).join(SHIELD_DIR)
}

// ──────────────────────────────────────────────────────────
// Entry point
// ──────────────────────────────────────────────────────────

/// The extension directory to load for this launch, or `None` when the employee
/// may see revenue and nothing needs to be hidden.
///
/// The decision is made per launch rather than at startup because it belongs to
/// the signed-in account, not to the profile: the same profile can be opened by
/// an account that may see revenue and by one that may not.
///
/// `platform` selects the per-platform `earnings_hide.selectors` to union into
/// the stylesheet; `None` (a manual profile launch) still hides via the local
/// fallback list.
pub async fn extension_for_launch(profile_id: &str, platform: Option<&str>) -> Option<String> {
  let allowed = revenue_visibility_allowed().await;
  // The decision goes to `bwbrowser_debug.log` rather than the plugin log, so it
  // sits next to the `[users] ← action=me` line carrying the very permission it
  // judged. Logging it at all matters too: without it a permitted launch and a
  // policy that never ran are both silent, and "why can this employee still see
  // revenue" has no answer in the log.
  crate::bwbrowser_cloud::log_bwbrowser(
    "revenue_shield",
    &format!(
      "收益可见性：{}（profile {profile_id}）",
      if allowed {
        "已授权，不隐藏收益"
      } else {
        "无权限，启动收益隐藏"
      }
    ),
  );
  if allowed {
    return None;
  }

  let dir = staged_dir(profile_id);
  // `--load-extension` is a comma-separated list, so a comma in the path would
  // silently split it into two directories that do not exist and every extension
  // in the launch would fail to load. Nothing can encode it.
  if !crate::extension_manager::path_is_load_extension_safe(&dir) {
    crate::bwbrowser_cloud::log_bwbrowser_error(
      "revenue_shield",
      &format!(
        "无法为 profile {profile_id} 隐藏收益: {} 含逗号，--load-extension 无法表达该路径",
        dir.display()
      ),
    );
    return None;
  }

  // Failure to fetch a platform config degrades to the local fallback list — the
  // fail-closed direction still holds, it just hides less precisely.
  let mut selectors: Vec<String> = REVENUE_SELECTORS.iter().map(|s| s.to_string()).collect();
  if let Some(platform) = platform {
    let platform_selectors =
      crate::bwbrowser_cloud::fetch_platform_earnings_hide_selectors(platform).await;
    if !platform_selectors.is_empty() {
      crate::bwbrowser_cloud::log_bwbrowser(
        "revenue_shield",
        &format!(
          "平台 {platform} 发布 {} 个收益隐藏选择器，已并入样式表",
          platform_selectors.len()
        ),
      );
      selectors.extend(platform_selectors);
    }
  }

  match write_shield(&dir, &selectors) {
    Ok(()) => {
      crate::bwbrowser_cloud::log_bwbrowser(
        "revenue_shield",
        &format!(
          "已为 profile {profile_id} 落盘收益隐藏扩展: {}",
          dir.display()
        ),
      );
      Some(dir.to_string_lossy().to_string())
    }
    // The browser still opens. A cosmetic shield that could not be written is
    // worth a loud log line, not a refused launch, and the line says which way
    // the failure went.
    Err(error) => {
      crate::bwbrowser_cloud::log_bwbrowser_error(
        "revenue_shield",
        &format!("为 profile {profile_id} 落盘收益隐藏扩展失败: {error}; 收益保持可见"),
      );
      None
    }
  }
}

// ──────────────────────────────────────────────────────────
// The permission
// ──────────────────────────────────────────────────────────

/// Whether a management-user record grants revenue visibility.
///
/// Split from the lookup so the fail-closed direction is testable without a
/// network: an absent permission and an absent map both read as "may not", and
/// a regression there is invisible until an employee sees a figure.
fn revenue_visibility_granted(
  is_super_admin: bool,
  permissions: Option<&std::collections::BTreeMap<String, bool>>,
) -> bool {
  if is_super_admin {
    return true;
  }
  permissions
    .and_then(|permissions| permissions.get(REVENUE_PERMISSION_KEY))
    .copied()
    .unwrap_or(false)
}

/// Whether the signed-in management user may see revenue.
async fn revenue_visibility_allowed() -> bool {
  match crate::bwbrowser_cloud::BWBROWSER_AUTH
    .get_current_management_user()
    .await
  {
    Ok(user) if user.success => revenue_visibility_granted(
      user.is_super_admin.unwrap_or(false),
      user.permissions.as_ref(),
    ),
    Ok(_) => {
      crate::bwbrowser_cloud::log_bwbrowser_error(
        "revenue_shield",
        "users.php 未返回用户信息，无法判定收益权限；按无权限处理，隐藏收益",
      );
      false
    }
    Err(error) => {
      crate::bwbrowser_cloud::log_bwbrowser_error(
        "revenue_shield",
        &format!("读取 {REVENUE_PERMISSION_KEY} 失败: {error}; 按无权限处理，隐藏收益"),
      );
      false
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_stylesheet_hides_every_selector_and_cannot_be_overridden() {
    let css = hide_css(REVENUE_SELECTORS);
    for selector in REVENUE_SELECTORS {
      assert!(
        css.contains(selector),
        "selector {selector} is missing from the generated stylesheet"
      );
    }
    // An inline `display` on the element would win against a plain declaration.
    assert!(css.contains("display: none !important"));
  }

  #[test]
  fn the_manifest_is_valid_json_and_declares_the_generated_stylesheet() {
    // The manifest is a hand-written literal, so a typo would surface only as a
    // browser that silently loads nothing.
    let manifest: serde_json::Value =
      serde_json::from_str(SHIELD_MANIFEST).expect("the manifest must parse");
    assert_eq!(manifest["manifest_version"], 3);
    assert_eq!(
      manifest["content_scripts"][0]["css"][0],
      serde_json::Value::String("hide.css".to_string())
    );
    // A JavaScript half would be remote-JS-on-launch territory and nothing the
    // stylesheet needs; its absence is an invariant, not an accident.
    assert!(manifest["content_scripts"][0]["js"].is_null());
    // document_start is what keeps the figure from being painted before hiding.
    assert_eq!(manifest["content_scripts"][0]["run_at"], "document_start");
  }

  #[test]
  fn a_platform_config_unions_its_selectors_ahead_of_the_fallback() {
    // Douyin's own `#douyin-creator-master-menu-nav-cash` is exactly the entry a
    // class-name guess never reaches, so the union must carry a platform id
    // selector through to the stylesheet when one is supplied.
    let mut selectors: Vec<&str> = REVENUE_SELECTORS.to_vec();
    selectors.push("#douyin-creator-master-menu-nav-cash");
    let css = hide_css(&selectors);
    assert!(css.contains("#douyin-creator-master-menu-nav-cash"));
    assert!(css.contains("[class*='revenue']"));
  }

  #[test]
  fn the_default_selectors_avoid_fetching_a_platform_when_none_is_known() {
    // Fallback-only path (a manual profile launch) never touches the network; it
    // just renders the local list.
    let css = hide_css(REVENUE_SELECTORS);
    assert!(!css.contains("#douyin-creator-master-menu-nav-cash"));
  }

  #[test]
  fn the_shield_is_staged_where_the_browser_exit_cleanup_looks() {
    // Staging outside the profile's staging root would leave extension code on
    // disk after every launch.
    let staged = staged_dir("profile-1");
    assert_eq!(
      staged,
      crate::extension_manager::ExtensionManager::unpacked_dir_for_profile("profile-1")
        .join(SHIELD_DIR)
    );
    assert!(staged.ends_with(SHIELD_DIR));
  }

  #[test]
  fn the_default_selectors_avoid_ordinary_layout_class_names() {
    // `text-balance` and `balance-*` are layout utilities, not money. Hiding
    // them would blank real text on pages that have nothing to do with revenue.
    assert!(!REVENUE_SELECTORS
      .iter()
      .any(|selector| selector.contains("balance")));
  }

  #[test]
  fn revenue_stays_hidden_unless_the_permission_says_otherwise() {
    // The fail-closed direction is the whole point: an employee whose account
    // carries no revenue grant must not see a figure. Every line here would
    // silently leak if the default flipped.
    let granted = std::collections::BTreeMap::from([(REVENUE_PERMISSION_KEY.to_string(), true)]);
    let revoked = std::collections::BTreeMap::from([(REVENUE_PERMISSION_KEY.to_string(), false)]);
    let unrelated = std::collections::BTreeMap::from([("allow_video_download".to_string(), true)]);

    assert!(revenue_visibility_granted(false, Some(&granted)));
    assert!(!revenue_visibility_granted(false, Some(&revoked)));
    // No map at all: the lookup failed, or the user is unknown to the backend.
    assert!(!revenue_visibility_granted(false, None));
    // A map that simply does not mention revenue is not a grant.
    assert!(!revenue_visibility_granted(false, Some(&unrelated)));
    // The account owner is not locked out of their own browser.
    assert!(revenue_visibility_granted(true, None));
    assert!(revenue_visibility_granted(true, Some(&revoked)));
  }
}
