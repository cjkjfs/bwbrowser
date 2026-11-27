/**
 * Single place to switch the whole app's server domain. Flip this and every
 * cloud-backed feature follows: API auth, cloud sync, remote MCP, and the
 * account/website portal links.
 *
 * Keep this in lockstep with the backend source of truth in
 * `src-tauri/src/cloud_domain.rs` (`cloud_host!()`).
 */
export const CLOUD_DOMAIN = "baowenku.com";

/** Website / account portal root, e.g. https://baowenku.com */
export const CLOUD_ROOT = `https://${CLOUD_DOMAIN}`;

/** Backend REST API + remote MCP endpoint, same host as the site, e.g. https://baowenku.com */
export const CLOUD_API_URL = `https://${CLOUD_DOMAIN}`;

/** Cloud sync service, same host as the site, e.g. https://baowenku.com */
export const CLOUD_SYNC_URL = `https://${CLOUD_DOMAIN}`;
