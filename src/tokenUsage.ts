// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import type { Message } from "./api";
import type { Usage } from "./api/work";

const KEY = "sulcusai.chat.hideTokens";
const EVENT = "sulcusai-tokens-shown";

export function tokensShown(): boolean {
  try {
    return localStorage.getItem(KEY) !== "1";
  } catch {
    return true;
  }
}

export function setTokensShown(on: boolean) {
  try {
    localStorage.setItem(KEY, on ? "0" : "1");
  } catch {
    /* storage blocked: the switch still applies until the window closes */
  }
  window.dispatchEvent(new Event(EVENT));
}

/** Every open chat follows the switch at once, not after a reload. */
export function useTokensShown(): boolean {
  const [on, setOn] = useState(tokensShown);
  useEffect(() => {
    const sync = () => setOn(tokensShown());
    window.addEventListener(EVENT, sync);
    return () => window.removeEventListener(EVENT, sync);
  }, []);
  return on;
}

function add(a: Usage | undefined, b: Usage): Usage {
  if (!a) return { ...b };
  const sum = (x: number | null, y: number | null) => (x == null && y == null ? null : (x ?? 0) + (y ?? 0));
  return {
    in: sum(a.in, b.in),
    out: a.out + b.out,
    thinking: sum(a.thinking, b.thinking),
    cache_read: sum(a.cache_read, b.cache_read),
    // One guessed step makes the whole figure a guess.
    source: a.source === "estimate" || b.source === "estimate" ? "estimate" : "provider",
  };
}

/** One figure per reply: a reply that called tools is several model steps,
 *  each with its own usage, and the person asked one question. The sum goes
 *  on the reply's last written message. */
export function replyUsage(messages: Message[]): Map<string, Usage> {
  const out = new Map<string, Usage>();
  let sum: Usage | undefined;
  let last: string | undefined;
  let steps = 0;
  const close = () => {
    if (sum && last) out.set(last, { ...sum, steps });
    sum = undefined;
    last = undefined;
    steps = 0;
  };
  for (const m of messages) {
    if (m.role === "user") close();
    if (m.role !== "assistant" || m.meta?.compacted) continue;
    const u = m.meta?.usage as Usage | undefined;
    if (u) {
      sum = add(sum, u);
      steps += 1;
    }
    if (m.content) last = m.id;
  }
  close();
  return out;
}

/** The chat's whole use so far, for the figure in the chat's header. */
export interface Total {
  in: number;
  out: number;
  estimate: boolean;
}

export function chatTotal(messages: Message[]): (Total & { byModel: [string, Total][] }) | null {
  let total: Usage | undefined;
  const models = new Map<string, Usage>();
  for (const m of messages) {
    const u = m.role === "assistant" ? (m.meta?.usage as Usage | undefined) : undefined;
    if (!u) continue;
    total = add(total, u);
    const key = u.model ?? "";
    models.set(key, add(models.get(key), u));
  }
  const plain = (u: Usage): Total => ({ in: u.in ?? 0, out: u.out, estimate: u.source === "estimate" });
  return total ? { ...plain(total), byModel: [...models].map(([k, u]) => [k, plain(u)]) } : null;
}
