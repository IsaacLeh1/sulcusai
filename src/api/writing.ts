// SPDX-License-Identifier: AGPL-3.0-only
// Language and writing: partner practice, writing help, scam check, PC health, social posts.
import { invoke } from "@tauri-apps/api/core";

export type WritingTool = "partner" | "writing_help" | "scam_check" | "pc_health" | "social";

export interface WordMark { word: string; mark: "ok" | "accent" | "wrong" | "extra" }
export interface PartnerScore { score: number; words: WordMark[]; missing: string[] }
export interface PartnerProgress { language: string; rounds: number; total: number; streak: number; best_streak: number; recent: number[] }
export interface PartnerRound { english: string; reference: string; hint: string; level: number }

export interface WritingIssue { start: number; end: number; kind: string; message: string; fix: string | null }
export interface WritingReport {
  words: number;
  sentences: number;
  avg_sentence: number;
  ease: number;
  grade: number;
  long_sentences: number;
  tone: "formal" | "friendly" | "neutral" | "urgent" | "unsure";
  issues: WritingIssue[];
}

export interface ScamSign { kind: string; why: string; weight: number }
export interface ScamVerdict { score: number; level: "no_signs" | "careful" | "likely_scam"; signs: ScamSign[] }

export interface HealthFinding { level: "ok" | "watch" | "act"; area: string; text: string }
export interface PcHealth { findings: HealthFinding[]; overall: "ok" | "watch" | "act" }

export interface SocialShown { kind: "bluesky" | "mastodon"; name: string; limit: number }
export type SocialAccount =
  | { kind: "bluesky"; handle: string; app_password: string; server: string }
  | { kind: "mastodon"; instance: string; token: string };

export const writingApi = {
  switches: () => invoke<Record<WritingTool, boolean>>("writing_switches"),
  setSwitch: (tool: WritingTool, on: boolean) => invoke<void>("writing_set_switch", { tool, on }),
  partnerRound: (language: string, level: number, topic: string) => invoke<PartnerRound>("partner_round", { language, level, topic }),
  partnerAnswer: (language: string, level: number, reference: string, answer: string) =>
    invoke<{ score: PartnerScore; progress: PartnerProgress; next_level: number }>("partner_answer", { language, level, reference, answer }),
  partnerProgress: () => invoke<PartnerProgress[]>("partner_progress"),
  check: (text: string) => invoke<WritingReport>("writing_check", { text }),
  fix: (text: string) => invoke<string>("writing_fix", { text }),
  rewrite: (text: string, tone: string) => invoke<string>("writing_rewrite", { text, tone }),
  scamCheck: (text: string, sender: string | null) => invoke<ScamVerdict>("scam_check", { text, sender }),
  pcHealth: () => invoke<PcHealth>("pc_health"),
  socialAccounts: () => invoke<SocialShown[]>("social_accounts"),
  socialAdd: (account: SocialAccount) => invoke<void>("social_add", { account }),
  socialRemove: (index: number) => invoke<void>("social_remove", { index }),
  socialPost: (index: number, text: string) => invoke<string>("social_post", { index, text }),
};

/** How many characters are left before a post's limit (negative: over). */
export function charsLeft(text: string, limit: number): number {
  return limit - Array.from(text.trim()).length;
}

/** Splits text into plain and marked pieces, for highlighting issues. */
export function pieces(text: string, issues: WritingIssue[]): { text: string; issue: WritingIssue | null }[] {
  // Offsets come from Rust as UTF-8 bytes; map them onto the JS string.
  const bytes = new TextEncoder().encode(text);
  const dec = new TextDecoder();
  const out: { text: string; issue: WritingIssue | null }[] = [];
  let at = 0;
  for (const i of [...issues].sort((a, b) => a.start - b.start)) {
    if (i.start < at || i.end > bytes.length) continue;
    if (i.start > at) out.push({ text: dec.decode(bytes.slice(at, i.start)), issue: null });
    out.push({ text: dec.decode(bytes.slice(i.start, i.end)), issue: i });
    at = i.end;
  }
  if (at < bytes.length) out.push({ text: dec.decode(bytes.slice(at)), issue: null });
  return out;
}
