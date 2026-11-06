// SPDX-License-Identifier: AGPL-3.0-only
// Links in chats open through the app's link handling, after a check the
// first time a site comes up, so a look-alike address gets a second look.
import { invoke } from "@tauri-apps/api/core";
import { api } from "./api";
import { confirmed } from "./confirm";
import { t } from "./i18n";

let known: Set<string> | null = null;

export function hostOf(href: string): string {
  try {
    return new URL(href).hostname.toLowerCase();
  } catch {
    return "";
  }
}

export async function openChecked(href: string): Promise<void> {
  const host = hostOf(href);
  if (!host || !/^https?:\/\//i.test(href)) return;
  if (!known) known = new Set(await invoke<string[]>("social_known_sites").catch(() => []));
  if (!known.has(host)) {
    if (!(await confirmed(t("Open {host}? You haven't opened this site from a chat before.\n\n{url}", { host, url: href })))) return;
    known.add(host);
    invoke("social_trust_site", { host }).catch(() => {});
  }
  api.openWeb(href);
}
