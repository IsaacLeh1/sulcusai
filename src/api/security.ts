// SPDX-License-Identifier: AGPL-3.0-only
// The Security page's commands (src-tauri/src/security_page.rs). Kept apart
// from api.ts so the page is one self-contained piece.
import { invoke } from "@tauri-apps/api/core";

export type Severity = "critical" | "warning" | "notice" | "info";

export type FixCall =
  | { kind: "set_auto_lock"; minutes: number }
  | { kind: "set_phone_while_locked"; on: boolean }
  | { kind: "set_phone"; enabled: boolean }
  | { kind: "set_phone_link"; anywhere: boolean }
  | { kind: "remove_phone"; id: string }
  | { kind: "set_api_server"; enabled: boolean; port: number }
  | { kind: "set_ollama_server"; enabled: boolean; port: number }
  | { kind: "new_api_key" }
  | { kind: "set_tools_ask"; connector: string; tools: string[] }
  | { kind: "forget_site_key"; host: string };

export interface Fix {
  label: string;
  confirm: string;
  call: FixCall;
}

/** `title`, `detail` and the fix's texts are English templates with {name}
 * placeholders; `args` fills them after translation. */
export interface Finding {
  id: string;
  severity: Severity;
  title: string;
  detail: string;
  args: Record<string, string>;
  fix: Fix | null;
  /** The Settings section that has the control when there is no one-click fix. */
  open: string | null;
}

export interface SecurityDevice {
  id: string;
  name: string;
  added: number;
  last_seen: number | null;
  has_password: boolean;
}

export interface SecurityListener {
  id: string;
  name: string;
  port: number | null;
  enabled: boolean;
  running: boolean;
  local_only: boolean;
  problem: string | null;
}

export interface SecurityAddon {
  id: string;
  name: string;
  enabled: boolean;
  host: string | null;
  tools: number;
  auto_run: string[];
  ask: number;
  off: number;
  signed_in: boolean;
  has_secrets: boolean;
}

export interface SecurityKey {
  kind: "api_server" | "site";
  label: string;
  host: string | null;
  age_days: number;
}

export interface SecurityOverview {
  version: string;
  now: number;
  protection: { encryption: boolean; lock_enabled: boolean; hello_enabled: boolean; auto_lock_minutes: number; keep_working: boolean };
  phones: { page_on: boolean; anywhere: boolean; while_locked: boolean; devices: SecurityDevice[]; open_sessions: number };
  listeners: SecurityListener[];
  addons: SecurityAddon[];
  keys: SecurityKey[];
  programs?: { checked_at: number | null; ok: number; unchecked: number; problems: [string, string, boolean][] };
}

export interface SecurityReport {
  overview: SecurityOverview;
  findings: Finding[];
}

/** The signed security notice list (src-tauri/src/security_notice_feed.rs).
 * `no_key`: this build trusts no signing key yet; `waiting`: a key exists but
 * no notice has been accepted; `current`: a verified notice is in use. */
export interface SecurityNotice {
  status: "no_key" | "waiting" | "current";
  last_check: number;
  last_result: "new" | "same" | "none" | "rejected" | "failed" | null;
  last_error: string | null;
  sequence: number;
  issued: number;
  update_needed: boolean;
  lowest_safe_version: string;
  revoked: { kind: string; id: string; reason: string }[];
  messages: { severity: "info" | "warning" | "critical"; text: string }[];
}

export const securityApi = {
  notice: () => invoke<SecurityNotice>("security_notice_view"),
  checkNotice: () => invoke<SecurityNotice>("security_notice_check"),
  overview: () => invoke<SecurityReport>("security_overview"),
  /** Changes a setting. Called only after the person has said yes. */
  applyFix: (id: string, confirmed: boolean) => invoke<SecurityReport>("security_apply_fix", { id, confirmed }),
};
