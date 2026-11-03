// SPDX-License-Identifier: AGPL-3.0-only
// The password vault's commands (src-tauri/src/vault_cmds.rs). Kept apart from
// api.ts so the page is one self-contained piece. Passwords come back only
// from `reveal` and `generate`.
import { invoke } from "@tauri-apps/api/core";

export interface VaultStatus {
  enabled: boolean;
  set_up: boolean;
  unlocked: boolean;
  notice_accepted: boolean;
}

export interface VaultEntry {
  id: string;
  origin: string;
  name: string;
  username: string;
  created_ms: number;
  changed_ms: number;
  /** False for http and non-web addresses: kept, never filled. */
  fillable: boolean;
}

export interface VaultEntryInput {
  origin: string;
  name: string;
  username: string;
  /** Empty when editing keeps the saved password. */
  password: string;
  notes: string;
}

export interface Revealed {
  password: string;
  notes: string;
}

export type Strength = "very_weak" | "weak" | "fair" | "strong";

export interface VaultChecks {
  weak: [string, Strength][];
  reused: string[][];
  old: string[];
}

export interface GenOptions {
  length: number;
  lower: boolean;
  upper: boolean;
  digits: boolean;
  symbols: boolean;
  avoid_ambiguous: boolean;
}

export type ImportFormat = "other" | "sulcus";

export interface MergeReport {
  added: number;
  updated: number;
  unchanged: number;
  skipped: number;
}

export interface ImportResult {
  source: string | null;
  report: MergeReport;
}

export const vaultApi = {
  status: () => invoke<VaultStatus>("vault_status"),
  /** Turning on needs `accepted`: the person has read the notice. */
  setEnabled: (on: boolean, accepted: boolean) => invoke<VaultStatus>("vault_set_enabled", { on, accepted }),
  /** Returns the recovery key, to be shown once. */
  setup: (master: string) => invoke<string>("vault_setup", { master }),
  unlock: (master: string) => invoke<void>("vault_unlock", { master }),
  recover: (recoveryKey: string, newMaster: string) => invoke<void>("vault_recover", { recoveryKey, newMaster }),
  lock: () => invoke<void>("vault_lock"),
  changeMaster: (current: string, newMaster: string) => invoke<void>("vault_change_master", { current, newMaster }),
  newRecoveryKey: (master: string) => invoke<string>("vault_new_recovery_key", { master }),
  list: () => invoke<VaultEntry[]>("vault_list"),
  add: (input: VaultEntryInput) => invoke<{ id: string; result: "added" | "updated" | "unchanged" }>("vault_add", { input }),
  edit: (id: string, input: VaultEntryInput) => invoke<void>("vault_edit", { id, input }),
  remove: (id: string) => invoke<void>("vault_delete", { id }),
  reveal: (id: string) => invoke<Revealed>("vault_reveal", { id }),
  generate: (options: GenOptions) => invoke<string>("vault_generate", { options }),
  import: (path: string, format: ImportFormat, password?: string) => invoke<ImportResult>("vault_import", { path, format, password: password ?? null }),
  export: (path: string, password: string) => invoke<void>("vault_export", { path, password }),
  check: () => invoke<VaultChecks>("vault_check"),
  /** Keeps the window out of screenshots and screen sharing while the page is open. */
  hideFromCapture: (on: boolean) => invoke<void>("vault_hide_from_capture", { on }),
};
