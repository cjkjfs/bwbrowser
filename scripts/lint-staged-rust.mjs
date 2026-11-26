#!/usr/bin/env node
import { execSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cwd = fileURLToPath(new URL("../src-tauri", import.meta.url));

// target\debug stays locked while the app / proxy exe is running; cargo then
// dies inside tauri-build with os error 5 (拒绝访问) and the whole commit is
// reverted even though the code is fine. The checks are meaningless under a
// lock, so skip them and let the next commit made with the app closed run them.
function appRunning() {
  if (process.platform !== "win32") return false;
  try {
    const out = execSync("tasklist", { encoding: "utf8" }).toLowerCase();
    return out.includes("bwbrowser.exe") || out.includes("bwbrowser-proxy");
  } catch {
    // tasklist unavailable / failed - fall through and run the cargo steps
    return false;
  }
}

if (appRunning()) {
  console.warn(
    "[lint-staged-rust] bwbrowser is running - target\\debug is locked, cargo would abort with os error 5. Skipping cargo fmt/clippy/test for this commit; run them with the app closed.",
  );
  process.exit(0);
}

const steps = [
  ["cargo", ["fmt", "--all"]],
  [
    "cargo",
    ["clippy", "--all-targets", "--", "-D", "warnings", "-D", "clippy::all"],
  ],
  ["cargo", ["test", "--lib", "--no-run"]],
];

for (const [command, args] of steps) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });

  if (result.error) {
    console.error(
      `[lint-staged-rust] failed to run ${command}: ${result.error.message}`,
    );
    process.exit(1);
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
