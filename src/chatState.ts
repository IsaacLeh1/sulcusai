// SPDX-License-Identifier: AGPL-3.0-only
/* What a chat shows while it replies, worked out from the events the core
 * sends, as plain functions so they can be tested without a window. */
import type { ChatEvents, PendingApproval } from "./api";

export type Status = "idle" | "loading" | "thinking" | "writing" | "working";

/** The reply being written, before it's saved as a message. */
export interface Streaming {
  id: string;
  content: string;
  thinking: string;
}

/** Where a reply is, apart from its text. */
export interface Live {
  status: Status;
  /** What's too hot while the heat guard pauses between steps. */
  cooling: string | null;
  /** A cloud provider was busy: when the next try goes, and which try. */
  retrying: { provider: string; at: number; attempt: number; of: number } | null;
  /** Waiting in line for another model, or for the graphics card. */
  queued: { place: number; model: string; card?: boolean } | null;
  /** Before a picture of something named: checking (or looking up) its look. */
  lookingUp: string | null;
  /** The picture reader is describing pictures for a model that can't see them. */
  readingPictures: boolean;
  handingOff: boolean;
  compacting: boolean;
}

export const QUIET: Live = { status: "idle", cooling: null, retrying: null, queued: null, lookingUp: null, readingPictures: false, handingOff: false, compacting: false };

/** `chat:status`. `now` and `pc` (what to call this PC when the heat
 * guard names nothing) come from the caller. */
export function onStatus(v: Live, p: ChatEvents["chat:status"], now: number, pc: string): Live {
  const queued = p.status === "queued" ? { place: p.place ?? 1, model: p.detail ?? "" } : p.status === "waiting_card" ? { place: p.place ?? 1, model: "", card: true } : null;
  const next = { ...v, queued };
  switch (p.status) {
    case "handoff":
      return { ...next, handingOff: true };
    case "compacting":
      return { ...next, compacting: true };
    case "retrying":
      return { ...next, retrying: { provider: p.provider ?? "", at: now + (p.seconds ?? 1) * 1000, attempt: p.attempt ?? 1, of: p.of ?? 1 } };
    case "idle":
      return { ...next, compacting: false, retrying: null, lookingUp: null, readingPictures: false };
    case "looking_up":
      return { ...next, lookingUp: p.detail ?? "", status: "working" };
    case "reading_pictures":
      return { ...next, readingPictures: true, status: "working" };
    case "cooling":
      return { ...next, cooling: p.detail ?? pc, status: "working" };
    default:
      return { ...next, status: "loading" };
  }
}

/** `chat:start`: a step of the reply begins. */
export function onStart(v: Live): Live {
  return { ...v, cooling: null, retrying: null, queued: null, lookingUp: null, readingPictures: false, status: "thinking" };
}

/** `chat:delta`: text arrived, so whatever it was waiting on is over. */
export function onDelta(v: Live, p: Pick<ChatEvents["chat:delta"], "content">): Live {
  return { ...v, retrying: null, status: p.content ? "writing" : v.status };
}

/** The bubble at `chat:start`: the same message tried again keeps what it
 * wrote (and what was still waiting to be drawn); another starts empty. */
export function startStreaming(s: Streaming | null, id: string, waiting: Streaming | null): Streaming {
  const w = waiting?.id === id ? waiting : null;
  return s && s.id === id ? { ...s, content: s.content + (w?.content ?? ""), thinking: s.thinking + (w?.thinking ?? "") } : { id, content: "", thinking: "" };
}

/** Text held back for a moment (to draw in batches), added to. */
export function held(h: Streaming | null, id: string, content: string, thinking: string): Streaming {
  return h && h.id === id ? { ...h, content: h.content + content, thinking: h.thinking + thinking } : { id, content, thinking };
}

/** Held text drawn into its bubble; text for another message is dropped
 * (its `chat:start` comes first and begins its own bubble). */
export function drawn(s: Streaming | null, h: Streaming): Streaming | null {
  return s && s.id === h.id ? { ...s, content: s.content + h.content, thinking: s.thinking + h.thinking } : s;
}

/** `agent:approval`: a card opens (once, even if it's sent again). */
export function withApproval(all: PendingApproval[], p: PendingApproval): PendingApproval[] {
  return [...all.filter((a) => a.call_id !== p.call_id), p];
}

/** `agent:approval_done`: that card closes, the others stay. */
export function withoutApproval(all: PendingApproval[], callId: string): PendingApproval[] {
  return all.filter((a) => a.call_id !== callId);
}
