// SPDX-License-Identifier: AGPL-3.0-only
// Messages typed in a chat and not sent yet: kept while SulcusAI runs, so
// switching chats or pages doesn't lose them, and, with "Keep unsent
// messages after a restart" on, saved (encrypted) until sent or cleared.
import { api } from "./api";

const drafts = new Map<string, string>();
let keep = false;
const timers = new Map<string, number>();

/** Loads the saved ones once at start; chats open already pick theirs up. */
export async function loadDrafts(): Promise<void> {
  try {
    keep = await api.draftsKeep();
    if (!keep) return;
    const saved = (await api.drafts()) ?? {};
    for (const [id, text] of Object.entries(saved)) if (!drafts.has(id)) drafts.set(id, text);
    window.dispatchEvent(new Event("drafts:loaded"));
  } catch {
    // Without them, typing still works.
  }
}

export function draftOf(chatId: string): string {
  return drafts.get(chatId) ?? "";
}

export function setDraft(chatId: string, text: string): void {
  if ((drafts.get(chatId) ?? "") === text) return;
  if (text) drafts.set(chatId, text);
  else drafts.delete(chatId);
  if (!keep) return;
  // Saved a moment after typing stops.
  window.clearTimeout(timers.get(chatId));
  timers.set(chatId, window.setTimeout(() => api.draftSet(chatId, text).catch(() => {}), 800));
}

/** The setting changed: from now on they're saved, or not (and forgotten). */
export function keepDrafts(on: boolean): void {
  keep = on;
  if (on) for (const [id, text] of drafts) api.draftSet(id, text).catch(() => {});
}
