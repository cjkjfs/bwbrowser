#!/usr/bin/env node
// Set the project version in package.json, tauri.conf.json, and Cargo.toml.
// Usage: node scripts/set-version.mjs <version>
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!version) {
  console.error("Usage: node scripts/set-version.mjs <version>");
  process.exit(1);
}

const semverRe = /^\d+\.\d+\.\d+(?:[-+].+)?$/;
if (!semverRe.test(version)) {
  console.error(`Error: "${version}" is not a valid semver version (e.g. 0.30.0)`);
  process.exit(1);
}

const files = [
  {
    path: "package.json",
    re: /"version"\s*:\s*"[^"]+"/,
    replacement: `"version": "${version}"`,
  },
  {
    path: "src-tauri/tauri.conf.json",
    re: /"version"\s*:\s*"[^"]+"/,
    replacement: `"version": "${version}"`,
  },
  {
    path: "src-tauri/Cargo.toml",
    re: /^version\s*=\s*"[^"]+"/m,
    replacement: `version = "${version}"`,
  },
];

for (const { path, re, replacement } of files) {
  let content = readFileSync(path, "utf8");
  if (!re.test(content)) {
    console.error(`Error: Could not find version field in ${path}`);
    process.exit(1);
  }
  content = content.replace(re, replacement);
  writeFileSync(path, content, "utf8");
  console.log(`  Updated ${path}`);
}

console.log(`\nVersion set to ${version}`);
