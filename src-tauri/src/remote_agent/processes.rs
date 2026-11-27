//! 远程任务管理器（2026-11-26）：进程列表 + 结束进程
//!
//! viewer 指令与期望回复：
//! - `task_list` → `tasklist_result` { success, tasks[{pid,name,cpu,mem_mb,path}] }
//! - `task_kill` { pid } → `taskkill_result` { pid, ok, message }

use serde_json::json;

/// 进程列表按内存降序最多返回条数（控制消息体积，relay 入站上限 2MB）
const MAX_TASKS: usize = 150;

/// 枚举进程：sysinfo 需要刷新两次（间隔 ≥ MINIMUM_CPU_UPDATE_INTERVAL）才能取到有效 CPU 占用
pub fn task_list() -> String {
  let mut system = sysinfo::System::new_all();
  system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
  std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
  system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

  let mut tasks: Vec<serde_json::Value> = system
    .processes()
    .values()
    .map(|p| {
      let path = p
        .exe()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
      json!({
        "pid": p.pid().as_u32(),
        "name": p.name().to_string_lossy(),
        "cpu": (p.cpu_usage() * 10.0).round() / 10.0,
        "mem_mb": (p.memory() as f64 / 1048576.0 * 10.0).round() / 10.0,
        "path": path,
      })
    })
    .collect();

  tasks.sort_by(|a, b| {
    let am = a["mem_mb"].as_f64().unwrap_or(0.0);
    let bm = b["mem_mb"].as_f64().unwrap_or(0.0);
    bm.partial_cmp(&am).unwrap_or(std::cmp::Ordering::Equal)
  });
  tasks.truncate(MAX_TASKS);

  json!({
    "type": "tasklist_result",
    "success": true,
    "tasks": tasks,
  })
  .to_string()
}

/// 结束进程：Windows 用 taskkill /F /T（连子进程一起杀），类 Unix 用 kill -9
pub fn task_kill(pid: u32) -> String {
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    let result = std::process::Command::new("taskkill")
      .args(["/F", "/T", "/PID", &pid.to_string()])
      .creation_flags(0x08000000) // CREATE_NO_WINDOW，避免闪黑框
      .output();
    handle_kill_output(pid, result)
  }

  #[cfg(not(windows))]
  {
    let result = std::process::Command::new("kill")
      .args(["-9", &pid.to_string()])
      .output();
    handle_kill_output(pid, result)
  }
}

/// 统一处理 taskkill/kill 的输出并生成回复 JSON
fn handle_kill_output(pid: u32, result: std::io::Result<std::process::Output>) -> String {
  match result {
    Ok(out) if out.status.success() => {
      crate::remote_agent::log_remote_file(&format!("远程任务管理器：结束进程 {pid} 成功"));
      json!({
        "type": "taskkill_result",
        "pid": pid,
        "ok": true,
        "message": "已结束进程",
      })
      .to_string()
    }
    Ok(out) => {
      let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
      let msg = if msg.is_empty() {
        format!("结束进程 {pid} 失败")
      } else {
        msg
      };
      crate::remote_agent::log_remote_file(&format!("远程任务管理器：结束进程 {pid} 失败: {msg}"));
      json!({
        "type": "taskkill_result",
        "pid": pid,
        "ok": false,
        "message": msg,
      })
      .to_string()
    }
    Err(e) => {
      let msg = format!("执行结束命令失败: {e}");
      crate::remote_agent::log_remote_file(&format!("远程任务管理器：结束进程 {pid} 失败: {msg}"));
      json!({
        "type": "taskkill_result",
        "pid": pid,
        "ok": false,
        "message": msg,
      })
      .to_string()
    }
  }
}

#[cfg(test)]
mod tests {
  use super::super::protocol::ClientCommand;

  #[test]
  fn viewer_task_commands_deserialize() {
    for s in [
      r#"{"type":"task_list"}"#,
      r#"{"type":"task_kill","pid":123}"#,
    ] {
      assert!(
        serde_json::from_str::<ClientCommand>(s).is_ok(),
        "viewer 指令无法反序列化: {s}"
      );
    }
  }
}
