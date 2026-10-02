#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cwd = fileURLToPath(new URL("../src-tauri", import.meta.url));

const steps = [
  ["cargo", ["fmt", "--all"]],
  ["cargo", ["clippy", "--all-targets", "--", "-D", "warnings", "-D", "clippy::all"]],
  ["cargo", ["test", "--lib", "--no-run"]],
];

for (const [command, args] of steps) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });

  if (result.error) {
    console.error(`[lint-staged-rust] failed to run ${command}: ${result.error.message}`);
    process.exit(1);
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}