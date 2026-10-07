fn main() {
  println!("cargo:rerun-if-changed=build.rs");

  // The harness binary links `bwbrowser` as a dependency, but a dependency's
  // `rustc-link-arg` directives do not reach this package's binary. Without a
  // manifest of its own, bwbrowser-e2e.exe dies at load with
  // STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) on a machine whose comctl32 is
  // v5.82: that version has no TaskDialogIndirect, which only the v6
  // side-by-side assembly exports. Reuse the app's manifest so the harness
  // loads exactly the way the app does.
  #[cfg(target_os = "windows")]
  embed_windows_manifest();
}

#[cfg(target_os = "windows")]
fn embed_windows_manifest() {
  use std::path::PathBuf;

  let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
  let manifest_path = PathBuf::from(&manifest_dir)
    .join("..")
    .join("..")
    .join("src-tauri")
    .join("app.manifest");

  if !manifest_path.exists() {
    println!("cargo:warning=app.manifest not found, skipping manifest embedding");
    return;
  }

  // Use the path directly (avoid canonicalize which adds \\?\ prefix that mt.exe rejects)
  let manifest_str = manifest_path.to_str().unwrap().replace('/', "\\");
  println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
  println!("cargo:rustc-link-arg=/MANIFESTINPUT:{manifest_str}");
  println!("cargo:rerun-if-changed={}", manifest_path.display());
}