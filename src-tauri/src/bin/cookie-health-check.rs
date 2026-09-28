// Manual trigger for the cookie health checker.
//
//   cargo run --bin cookie_health_check -- <profile_id>
//   cargo run --bin cookie_health_check -- cdp <platform> <cookie_json_file>
//
// Point it at real user data with BWBROWSER_DATA_DIR when the profile lives in
// a non-default data directory, e.g.:
//
//   BWBROWSER_DATA_DIR=C:\Users\me\AppData\Local\BwBrowser \
//     cargo run --bin cookie_health_check -- <profile_id>

use bwbrowser_lib::cookie_health::{self, HealthStatus};

fn main() {
  env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

  let args: Vec<String> = std::env::args().skip(1).collect();
  let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");

  if args.first().map(|s| s.as_str()) == Some("dump") {
    let profile_id = match args.get(1) {
      Some(p) => p.as_str(),
      None => {
        eprintln!("usage: cookie_health_check dump <profile_id> [domain_suffix]");
        std::process::exit(2);
      }
    };
    let suffix = args.get(2).map(|s| s.as_str()).unwrap_or("douyin");
    let header = cookie_health::dump_platform_cookies(profile_id, suffix);
    println!("{header}");
    return;
  }

  if args.first().map(|s| s.as_str()) == Some("cdp") {
    let (platform, file) = match (args.get(1), args.get(2)) {
      (Some(p), Some(f)) => (p.as_str(), f.as_str()),
      _ => {
        eprintln!("usage: cookie_health_check cdp <platform> <cookie_json_file>");
        std::process::exit(2);
      }
    };
    let raw = std::fs::read_to_string(file).expect("failed to read cookie json file");
    let cookies: Vec<serde_json::Value> =
      serde_json::from_str(&raw).expect("cookie json must be an array of CDP cookies");
    match rt.block_on(cookie_health::probe_cdp_cookies(platform, &cookies, None)) {
      Some((status, message)) => {
        println!(
          "[{:<8}] {} — {}",
          format!("{:?}", status),
          platform,
          message
        );
      }
      None => {
        println!("No probe rule for platform: {platform}");
      }
    }
    return;
  }

  let profile_id = args.first().map(|s| s.as_str()).unwrap_or_else(|| {
    eprintln!("usage: cookie_health_check <profile_id>");
    std::process::exit(2);
  });

  let results = rt.block_on(cookie_health::check_profile_health(profile_id, true));

  if results.is_empty() {
    println!("No results for {profile_id} (cooldown active, no cookies, or profile missing)");
    return;
  }

  for r in &results {
    println!(
      "[{:<8}] {} — {}",
      format!("{:?}", r.status),
      r.domain,
      r.message
    );
  }

  let valid = results
    .iter()
    .filter(|r| r.status == HealthStatus::Valid)
    .count();
  println!("Summary: {valid}/{} platforms valid", results.len());
}
