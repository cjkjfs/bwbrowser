import { readFile } from "node:fs/promises";
import path from "node:path";
import { allCoveredCommands } from "./coverage-map.mjs";

const root = path.resolve(import.meta.dirname, "..");
const source = await readFile(
  path.join(root, "src-tauri", "src", "lib.rs"),
  "utf8",
);
const match = source.match(
  /invoke_handler\(tauri::generate_handler!\[(.*?)\]\)/s,
);
const withoutComments = match[1].replace(/\/\/[^\n]*/g, "");
const registered = [
  ...withoutComments.matchAll(/([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*)\s*,/g),
].map((item) => item[1]);
const covered = new Set(allCoveredCommands());
const missing = registered.filter((c) => !covered.has(c));
const extra = [...covered].filter((c) => !registered.includes(c));
console.log("registered:", registered.length, "covered:", covered.size);
console.log("MISSING (" + missing.length + "):");
for (const m of missing.sort()) console.log("  " + m);
console.log("EXTRA (" + extra.length + "):");
for (const e of extra.sort()) console.log("  " + e);