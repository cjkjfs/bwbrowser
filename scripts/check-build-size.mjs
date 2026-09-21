import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

function walk(dir) {
  if (!existsSync(dir)) return [];
  let results = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const stat = statSync(full);
    if (stat.isDirectory()) {
      results = results.concat(walk(full));
    } else {
      results.push({ path: full, size: stat.size });
    }
  }
  return results;
}

const bundleDir = "src-tauri/target/release/bundle";
const releaseDir = "src-tauri/target/release";

console.log("=== Bundle output ===");
const bundleFiles = walk(bundleDir);
if (bundleFiles.length === 0) {
  console.log("  No bundle directory found - project not built yet");
} else {
  let total = 0;
  for (const f of bundleFiles) {
    console.log(`  ${(f.size / 1048576).toFixed(1)} MB  ${f.path}`);
    total += f.size;
  }
  console.log(`  ---`);
  console.log(`  Total bundle: ${(total / 1048576).toFixed(1)} MB`);
}

console.log("");
console.log("=== Main release executables (top-level only) ===");
const topExes = existsSync(releaseDir)
  ? readdirSync(releaseDir)
      .filter((f) => f.endsWith(".exe"))
      .map((f) => ({ path: join(releaseDir, f), size: statSync(join(releaseDir, f)).size }))
  : [];
if (topExes.length === 0) {
  console.log("  No main release exe found - not compiled yet");
} else {
  for (const f of topExes) {
    console.log(`  ${(f.size / 1048576).toFixed(1)} MB  ${f.path}`);
  }
}

console.log("");
console.log("=== External binaries (bundled into installer) ===");
const binDirs = ["src-tauri/binaries"];
for (const d of binDirs) {
  const bins = walk(d);
  for (const f of bins) {
    console.log(`  ${(f.size / 1048576).toFixed(1)} MB  ${f.path}`);
  }
}

console.log("");
console.log("=== Target directory total size ===");
const allTarget = walk("src-tauri/target");
let targetTotal = 0;
for (const f of allTarget) targetTotal += f.size;
console.log(`  ${(targetTotal / 1048576).toFixed(0)} MB  src-tauri/target/`);
