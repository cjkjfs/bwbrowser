//! Registry of running standard-Chromium instances.
//!
//! The Wayfern launch path tracks its processes in WayfernManager; standard
//! Chromium launches have no fingerprint injection or proxy worker, so all they
//! need to remember is where the CDP port landed and which PID to kill. This
//! registry holds exactly that, in memory, per process.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// The only Playwright chromium build npmmirror still ships win64 zips for;
/// mirrored from `CHROMIUM_PINNED_VERSION` in src/lib/browser-utils.ts.
pub const PINNED_VERSION: &str = "1200";

/// sha256 of the pinned `chromium-win64.zip`. npmmirror (the only mirror
/// serving it) publishes no `.sha256` sidecar — nor do the Playwright/Google
/// origins behind it — so the downloader verifies against this digest instead.
/// It was taken from a download proven byte-identical to the official
/// Chrome-for-Testing 143.0.7499.4 win64 zip the Playwright CDN redirects
/// revision 1200 to.
pub const PINNED_WIN64_ZIP_SHA256: &str =
  "4b4d412c65ffa6486eebdeb7ca05186c9598f90a60fb67e201fb8afa27776ab6";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChromiumInstance {
  pub profile_path: String,
  pub process_id: u32,
  pub cdp_port: u16,
  /// The binary the launch actually used. A profile whose metadata says
  /// Wayfern but is riding the default-kernel override still has to resolve
  /// its executable for "open URL in existing window", and the on-disk
  /// metadata cannot answer that.
  pub executable_path: String,
}

pub struct ChromiumManager {
  instances: Mutex<HashMap<String, ChromiumInstance>>,
}

impl ChromiumManager {
  fn new() -> Self {
    Self {
      instances: Mutex::new(HashMap::new()),
    }
  }

  pub fn instance() -> &'static ChromiumManager {
    &CHROMIUM_MANAGER
  }

  pub fn register(&self, instance: ChromiumInstance) {
    self
      .instances
      .lock()
      .expect("chromium registry poisoned")
      .insert(instance.profile_path.clone(), instance);
  }

  pub fn unregister_by_pid(&self, pid: u32) {
    self
      .instances
      .lock()
      .expect("chromium registry poisoned")
      .retain(|_, instance| instance.process_id != pid);
  }

  pub fn get_cdp_port(&self, profile_path: &str) -> Option<u16> {
    let instances = self.instances.lock().expect("chromium registry poisoned");
    instances
      .values()
      .find(|instance| paths_match(&instance.profile_path, profile_path))
      .map(|instance| instance.cdp_port)
  }

  pub fn find_by_profile(&self, profile_path: &str) -> Option<ChromiumInstance> {
    let instances = self.instances.lock().expect("chromium registry poisoned");
    instances
      .values()
      .find(|instance| paths_match(&instance.profile_path, profile_path))
      .cloned()
  }

  /// Terminate the instance's process tree and wait for it to leave the
  /// process table. Returns false if it is still running after the wait.
  pub async fn stop(&self, pid: u32) -> bool {
    terminate_process_tree(pid);

    let started = std::time::Instant::now();
    let limit = Duration::from_secs(10);
    loop {
      if !crate::proxy_storage::is_process_running(pid) {
        self.unregister_by_pid(pid);
        return true;
      }
      if started.elapsed() >= limit {
        return false;
      }
      tokio::time::sleep(Duration::from_millis(200)).await;
    }
  }
}

fn paths_match(a: &str, b: &str) -> bool {
  let canonicalize = |p: &str| {
    std::fs::canonicalize(p)
      .map(|c| c.to_string_lossy().to_lowercase())
      .unwrap_or_else(|_| p.to_lowercase())
  };
  canonicalize(a) == canonicalize(b)
}

/// taskkill /T /F: Chromium spawns helper processes; killing only the root
/// leaves renderers behind, so the tree flag is mandatory here (the Wayfern
/// path escalates to the same thing when a graceful stop is not enough).
fn terminate_process_tree(pid: u32) {
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let _ = std::process::Command::new("taskkill")
      .args(["/PID", &pid.to_string(), "/T", "/F"])
      .creation_flags(CREATE_NO_WINDOW)
      .output();
  }
  #[cfg(unix)]
  {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
  }
}

lazy_static::lazy_static! {
  static ref CHROMIUM_MANAGER: ChromiumManager = ChromiumManager::new();
}

/// Poll the DevTools endpoint until the freshly spawned browser answers, or
/// fail fast if its process dies before that can happen.
pub async fn wait_for_cdp_ready(
  pid: u32,
  port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let url = format!("http://127.0.0.1:{port}/json/version");
  let client = reqwest::Client::new();
  let started = std::time::Instant::now();
  let limit = Duration::from_secs(60);
  loop {
    if !crate::proxy_storage::is_process_running(pid) {
      return Err(
        format!(
          "Chromium process {pid} exited before the CDP endpoint on port {port} became ready"
        )
        .into(),
      );
    }
    if let Ok(resp) = client.get(&url).send().await {
      if resp.status().is_success() {
        log::info!("Chromium CDP ready on port {port}");
        return Ok(());
      }
    }
    if started.elapsed() >= limit {
      return Err(
        format!("Chromium CDP endpoint on port {port} did not become ready within 60 seconds")
          .into(),
      );
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
  }
}
