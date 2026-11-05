// SPDX-License-Identifier: AGPL-3.0-only
import { invoke } from "@tauri-apps/api/core";
import type { Message } from "./api";
import type { Usage } from "./api/work";

export interface LowTokens {
  global: boolean;
  /** The chat's own choice; null follows Settings. */
  chat: boolean | null;
  on: boolean;
}

export const lowTokensGet = (chatId: string | null) => invoke<LowTokens>("low_tokens_get", { chatId });
export const lowTokensSet = (chatId: string | null, on: boolean | null) => invoke<void>("low_tokens_set", { chatId, on });

export interface Savings {
  low: number;
  normal: number;
  /** Share fewer tokens per step than the chat's normal-mode steps; null with nothing to compare. */
  saved: number | null;
}

/** Tokens per model step in low token mode against the chat's other steps. */
export function lowTokenSavings(messages: Message[]): Savings | null {
  const sums = { low: [0, 0], normal: [0, 0] };
  for (const m of messages) {
    const u = m.role === "assistant" ? (m.meta?.usage as Usage | undefined) : undefined;
    if (!u) continue;
    const s = m.meta?.low_tokens ? sums.low : sums.normal;
    s[0] += (u.in ?? 0) + u.out;
    s[1] += 1;
  }
  if (sums.low[1] === 0) return null;
  const low = sums.low[0] / sums.low[1];
  const normal = sums.normal[1] ? sums.normal[0] / sums.normal[1] : 0;
  return { low: Math.round(low), normal: Math.round(normal), saved: normal > 0 ? Math.max(0, 1 - low / normal) : null };
}
