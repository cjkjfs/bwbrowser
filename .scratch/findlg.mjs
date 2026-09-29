import { readFileSync } from "node:fs";
import { join } from "node:path";
const files = ["src-tauri/src/lib.rs","src-tauri/src/app_dirs.rs","src-tauri/src/logging.rs","src-tauri/src/bwbrowser_cloud.rs"];
for (const f of files) {
  const p = join("d:/bian/bwbrowser-main", f);
  let t; try { t = readFileSync(p, "utf8"); } catch { continue; }
  const lines = t.split("\n");
  lines.forEach((ln, i) => {
    if (/log_bwbrowser_debug\.log/.test(ln) || /fn log_bwbrowser\b/.test(ln) || /"bwbrowser_debug\.log"/.test(ln) || /BWBROWSER_DEBUG_LOG/.test(ln) || /log_browser\b/.test(ln)) {
      console.log(`${f}:${i + 1}: ${ln.trim().slice(0, 140)}`);
    }
  });
}