// SPDX-License-Identifier: AGPL-3.0-only
// The address shield's commands (src-tauri/src/shield/live.rs).
import { invoke } from "@tauri-apps/api/core";

export interface ShieldSettings {
  enabled: boolean;
  via: "tor" | "own_list";
  per_site: boolean;
  rotate: boolean;
  rotate_minutes: number;
  proxies: string[];
  peers_relays_only: boolean;
  random_ports: boolean;
  port_change_hours: number;
  dns: "system" | "tor" | "encrypted";
  doh_provider: "cloudflare" | "quad9" | "custom";
  doh_custom: string;
  browser_common_fingerprint: boolean;
  browser_block_third_party_cookies: boolean;
  browser_clear_on_close: boolean;
}

export interface ShieldView {
  settings: ShieldSettings;
  /** What's wrong with what was typed or chosen, in plain words (English from the core). */
  problems: string[];
  /** false: the Tor helper in use predates the shield (no per-site routes yet); null: not started. */
  tor_isolates: boolean | null;
  /** Seconds until the next timed new address. */
  next_rotation: number | null;
}

export const shieldApi = {
  view: () => invoke<ShieldView>("shield_view"),
  set: (settings: ShieldSettings) => invoke<ShieldView>("set_shield", { settings }),
  newAddress: () => invoke<void>("shield_new_address"),
  wifiSettings: () => invoke<void>("shield_wifi_settings"),
};

/** The proxy list as typed, one per line (commas work too), blanks dropped. */
export function proxyLines(text: string): string[] {
  return text.split(/[\n,]/).map((l) => l.trim()).filter(Boolean);
}
