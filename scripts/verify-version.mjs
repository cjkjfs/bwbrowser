#!/usr/bin/env node
// Verify package.json / tauri.conf.json / Cargo.toml all carry the given
// version. Used by 一键推送.bat before tagging so a version tag can never
// point at code that still reports an older version.
//
// Usage: node scripts/verify-version.mjs <x.y.z>
import { readFileSync } from "node:fs";

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+(?:[-+].+)?$/.test(version)) {
  console.error("Usage: node scripts/verify-version.mjs <x.y.z>");
  process.exit(2);
}

const checks = [
  ["package.json", /"version"\s*:\s*"([^"]+)"/],
  ["src-tauri/tauri.conf.json", /"version"\s*:\s*"([^"]+)"/],
  ["src-tauri/Cargo.toml", /^version\s*=\s*"([^"]+)"/m],
];

let ok = true;
for (const [path, re] of checks) {
  let actual = "(missing)";
  try {
    actual = readFileSync(path, "utf8").match(re)?.[1] ?? "(missing)";
  } catch {
    // leave "(missing)"
  }
  const pass = actual === version;
  console.log(`${pass ? "OK   " : "FAIL "} ${path}: ${actual}`);
  if (!pass) ok = false;
}

if (!ok) {
  console.error(`Version mismatch: files do not agree on ${version}.`);
  process.exit(1);
}
console.log(`All version files agree on ${version}.`);
