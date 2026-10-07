/**
 * Single place to switch the whole app's server domain. Flip this and every
 * cloud-backed feature follows: API auth, cloud sync, remote MCP, and the
 * account/website portal links.
 *
 * Keep this in lockstep with the backend source of truth in
 * `src-tauri/src/cloud_domain.rs` (`cloud_host!()`).
 */
export const CLOUD_DOMAIN = "yacm.xin";

/** Website / account portal root, e.g. https://yacm.xin */
export const CLOUD_ROOT = `https://${CLOUD_DOMAIN}`;

/** Website root over HTTP, for links that must skip the TLS handshake, e.g. http://yacm.xin */
export const CLOUD_HTTP_ROOT = `http://${CLOUD_DOMAIN}`;

/** Backend REST API + remote MCP endpoint, e.g. https://api.yacm.xin */
export const CLOUD_API_URL = `https://api.${CLOUD_DOMAIN}`;

/** Cloud sync service, e.g. https://sync.yacm.xin */
export const CLOUD_SYNC_URL = `https://sync.${CLOUD_DOMAIN}`;
