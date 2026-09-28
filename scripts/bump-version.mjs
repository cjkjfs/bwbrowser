#!/usr/bin/env node
// One-shot version bumper: shows the last version you entered, asks for the new
// one, then updates every version field in the project.
//
// Usage:
//   node scripts/bump-version.mjs            # interactive (recommended)
//   node scripts/bump-version.mjs 2.3.0      # non-interactive, for CI/scripts
//
// The actual file edits are delegated to scripts/set-version.mjs (package.json,
// tauri.conf.json, Cargo.toml) plus a Cargo.lock fix here. Your last entry is
// kept in scripts/.bump-state.json so the next run starts from where you left
// off. This script only writes version files — it never commits, tags, or pushes.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const statePath = join(repoRoot, "scripts", ".bump-state.json");
const setVersionPath = join(repoRoot, "scripts", "set-version.mjs");

const semverRe = /^\d+\.\d+\.\d+(?:[-+].+)?$/;

function readJson(path) {
  try {
    return existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : null;
  } catch {
    return null;
  }
}

function readCurrentVersion() {
  try {
    return (
      readFileSync(join(repoRoot, "package.json"), "utf8").match(
        /"version"\s*:\s*"([^"]+)"/,
      )?.[1] ?? null
    );
  } catch {
    return null;
  }
}

function prompt(rl, question) {
  return new Promise((resolve) => rl.question(question, resolve));
}

function writeState(version) {
  writeFileSync(
    statePath,
    `${JSON.stringify({ lastVersion: version, updatedAt: new Date().toISOString() }, null, 2)}\n`,
    "utf8",
  );
}

function applyVersion(version) {
  console.log(`\n本次更新内容：`);
  console.log(`  - package.json`);
  console.log(`  - src-tauri/tauri.conf.json`);
  console.log(`  - src-tauri/Cargo.toml`);
  console.log(`  - src-tauri/Cargo.lock`);

  const set = spawnSync(process.execPath, [setVersionPath, version], {
    cwd: repoRoot,
    stdio: "inherit",
  });
  if (set.status !== 0) {
    console.error("set-version.mjs 执行失败，未写状态文件。");
    process.exit(1);
  }

  // set-version.mjs leaves Cargo.lock behind; keep it consistent so a --locked
  // build (like CI) still resolves to the same package version.
  const lockPath = join(repoRoot, "src-tauri", "Cargo.lock");
  let lock = readFileSync(lockPath, "utf8");
  const lockRe = /(name = "bwbrowser"\nversion = ")[^"]+(")/;
  if (lockRe.test(lock)) {
    lock = lock.replace(lockRe, `$1${version}$2`);
    writeFileSync(lockPath, lock, "utf8");
    console.log(`  Updated src-tauri/Cargo.lock`);
  } else {
    console.warn("  Cargo.lock 中未找到 bwbrowser 包条目，跳过。");
  }

  writeState(version);
  console.log(`\n✓ 已一键更新到 ${version}，并已记住该版本。`);
}

function printManualHint() {
  console.log("（本脚本未执行 git 操作，tag 与推送请自行处理：");
  console.log(
    "  git add -A && git commit -m \"v<version>\" && git tag v<version> && git push && git push --tags）",
  );
}

async function main() {
  const versionArg = process.argv[2]?.trim();

  if (versionArg) {
    if (!semverRe.test(versionArg)) {
      console.error(`无效版本号: "${versionArg}"（应形如 0.30.0）`);
      process.exit(1);
    }
    applyVersion(versionArg);
    printManualHint();
    return;
  }

  const state = readJson(statePath);
  const lastEntered = state?.lastVersion ?? null;
  const current = readCurrentVersion();
  const defaultVersion = lastEntered ?? current;

  console.log(lastEntered ? `上次输入版本: ${lastEntered}` : "上次输入版本: （暂无记录，首次运行）");
  console.log(`当前代码版本: ${current ?? "未知"}`);

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  const hint = defaultVersion ? ` [回车使用 ${defaultVersion}]` : "";
  const answer = (await prompt(rl, `请输入新版本号 (x.y.z)${hint}: `)).trim();
  const version = answer || defaultVersion;

  if (!version) {
    console.error("未输入版本号，已取消。");
    rl.close();
    process.exit(1);
  }
  if (!semverRe.test(version)) {
    console.error(`无效版本号: "${version}"（应形如 0.30.0）`);
    rl.close();
    process.exit(1);
  }

  const go = ((await prompt(rl, `\n将项目版本统一更新为 ${version}，继续？(Y/n) `)) ?? "").trim();
  rl.close();
  if (go.toLowerCase().startsWith("n")) {
    console.log("已取消，未做任何改动。");
    process.exit(0);
  }

  applyVersion(version);
  printManualHint();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});