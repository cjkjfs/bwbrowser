/**
 * BW Browser SDK: a thin client for the app's local REST API.
 *
 * The local API is off by default. Switch it on in the app under **Settings,
 * Integrations, Local API, "Enable Local API Server"**, and copy the port and
 * the authentication token from that screen.
 *
 * ```ts
 * import { BwbrowserClient } from "@bwbrowser/sdk";
 *
 * const client = new BwbrowserClient({ token: "..." });
 * await client.withProfile(profileId, { url: "https://example.com" }, async (session) => {
 *   console.log(session.cdpUrl);
 *   await client.agentClick(profileId, { locator: { role: "button", name: "Sign in" } });
 * });
 * ```
 */

export type { BwbrowserClientOptions, RunProfileOptions } from "./client.mts";
export {
  BwbrowserClient,
  DEFAULT_HOST,
  DEFAULT_PORT,
  RunSession,
} from "./client.mts";
export type { OperationKey } from "./coverage.mts";
export { OMITTED, OPERATIONS } from "./coverage.mts";
export type { BwbrowserApiErrorInit } from "./errors.mts";
export {
  BadGateway,
  BwbrowserApiError,
  BwbrowserConnectionError,
  BwbrowserError,
  Conflict,
  errorForStatus,
  Forbidden,
  NotFound,
  PaymentRequired,
  RateLimited,
  RequestTimeout,
  ServerError,
  ServiceUnavailable,
  Unauthorized,
  ValidationError,
} from "./errors.mts";
export type * from "./types.mts";
