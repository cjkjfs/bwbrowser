use std::sync::OnceLock;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HardwareSnapshot {
  cpu: CpuInfo,
  memory: MemoryInfo,
  disks: Vec<DiskInfo>,
  gpus: Vec<GpuInfo>,
}

#[derive(Debug, Clone, Serialize)]
struct CpuInfo {
  name: String,
  logical_cores: usize,
  physical_cores: Option<usize>,
  frequency_mhz: u64,
  usage_percent: f32,
}

#[derive(Debug, Clone, Serialize)]
struct MemoryInfo {
  total_bytes: u64,
  used_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct DiskInfo {
  name: String,
  mount_point: String,
  total_bytes: u64,
  available_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct GpuInfo {
  name: String,
  memory_bytes: Option<u64>,
}

pub(crate) fn snapshot() -> HardwareSnapshot {
  let mut system = sysinfo::System::new_all();
  system.refresh_all();
  std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
  snapshot_with_system(&mut system)
}

pub(crate) fn snapshot_json() -> String {
  serde_json::to_string(&snapshot()).unwrap_or_else(|_| "{}".to_string())
}

pub(crate) fn snapshot_with_system(system: &mut sysinfo::System) -> HardwareSnapshot {
  system.refresh_memory();
  system.refresh_cpu_usage();
  let cpu = system.cpus().first();
  let disks = sysinfo::Disks::new_with_refreshed_list();
  HardwareSnapshot {
    cpu: CpuInfo {
      name: cpu
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default(),
      logical_cores: system.cpus().len(),
      physical_cores: sysinfo::System::physical_core_count(),
      frequency_mhz: cpu.map(|c| c.frequency()).unwrap_or(0),
      usage_percent: (system.global_cpu_usage() * 10.0).round() / 10.0,
    },
    memory: MemoryInfo {
      total_bytes: system.total_memory(),
      used_bytes: system.used_memory(),
    },
    disks: disks
      .iter()
      .filter(|d| !d.is_removable() && d.total_space() > 0)
      .map(|d| DiskInfo {
        name: d.name().to_string_lossy().trim().to_string(),
        mount_point: d.mount_point().to_string_lossy().to_string(),
        total_bytes: d.total_space(),
        available_bytes: d.available_space(),
      })
      .collect(),
    gpus: gpus().clone(),
  }
}

fn gpus() -> &'static Vec<GpuInfo> {
  static GPUS: OnceLock<Vec<GpuInfo>> = OnceLock::new();
  GPUS.get_or_init(collect_gpus)
}

#[cfg(windows)]
fn collect_gpus() -> Vec<GpuInfo> {
  use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

  let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
    return Vec::new();
  };
  let mut result = Vec::new();
  for index in 0..32u32 {
    let Ok(adapter) = (unsafe { factory.EnumAdapters(index) }) else {
      break;
    };
    let Ok(desc) = (unsafe { adapter.GetDesc() }) else {
      continue;
    };
    let len = desc
      .Description
      .iter()
      .position(|&c| c == 0)
      .unwrap_or(desc.Description.len());
    let name = String::from_utf16_lossy(&desc.Description[..len])
      .trim()
      .to_string();
    if name.is_empty() {
      continue;
    }
    result.push(GpuInfo {
      name,
      memory_bytes: (desc.DedicatedVideoMemory > 0).then_some(desc.DedicatedVideoMemory as u64),
    });
  }
  result
}

#[cfg(not(windows))]
fn collect_gpus() -> Vec<GpuInfo> {
  Vec::new()
}
