// SPDX-License-Identifier: AGPL-3.0-only
// Enterprise cloud: Azure, Bedrock and Vertex settings, gateways, the company policy, routing and usage.
import { invoke } from "@tauri-apps/api/core";

export type EntAuth = "key" | "entra" | "profile" | "adc";

export interface EntView {
  auth: EntAuth;
  api_version: string;
  region: string;
  project: string;
  profile: string;
  tenant: string;
  client_id: string;
  authority: string;
  signed_in: boolean;
  account: string;
  header_names: string[];
  has_ca: boolean;
  problem: string | null;
  /** Set by the company policy file. */
  locked: boolean;
}

export interface EntInput {
  base_url?: string;
  auth: EntAuth;
  api_version: string;
  region: string;
  project: string;
  profile: string;
  tenant: string;
  client_id: string;
  authority: string;
}

export interface PolicyLock { what: string; why: string }
export interface PolicyView { active: boolean; path: string; locks: PolicyLock[]; force_redact: boolean; local_only: boolean }

export interface DeviceCode { user_code: string; verification_uri: string; message: string; expires_in: number }

export interface Route {
  on: boolean;
  local_model: string | null;
  cloud_model: string | null;
  long_chars: number;
  private_words: string[];
  hard_words: string[];
}
export type RouteScope = "assistant" | "project";

export interface UsageRow {
  month: string;
  provider: string;
  project: string;
  requests: number;
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  usd: number;
}

export const enterpriseApi = {
  setCloudEnterprise: <V>(id: string, settingsIn: EntInput) => invoke<V>("set_cloud_enterprise", { id, settingsIn }),
  entraStart: (id: string) => invoke<DeviceCode>("cloud_entra_start", { id }),
  entraSignOut: <V>(id: string) => invoke<V>("cloud_entra_sign_out", { id }),
  /** Values left empty keep what was saved. */
  setCloudHeaders: <V>(id: string, headers: [string, string][]) => invoke<V>("set_cloud_headers", { id, headers }),
  setCloudCa: <V>(id: string, pem: string) => invoke<V>("set_cloud_ca", { id, pem }),
  policyView: () => invoke<PolicyView>("policy_view"),
  route: (scope: RouteScope, id: string) => invoke<Route>("cloud_route", { scope, id }),
  setRoute: (scope: RouteScope, id: string, route: Route) => invoke<Route>("set_cloud_route", { scope, id, route }),
  usage: () => invoke<UsageRow[]>("cloud_usage"),
  saveUsageCsv: (path: string) => invoke<void>("cloud_usage_csv", { path }),
};
