// SPDX-License-Identifier: AGPL-3.0-only
// Calls and texts for accepting the legal documents and feature notices
// (src-tauri/src/consent.rs). The app shows its own copy of each document,
// bundled at build time, so accepting works offline and matches the version
// the app was built with.
import { invoke } from "@tauri-apps/api/core";

export type ConsentKind = "document" | "notice";

export interface ConsentStatus {
  id: string;
  kind: ConsentKind;
  version: number;
  accepted: number | null;
  accepted_at: number | null;
  due: boolean;
  at_start: boolean;
  file: string | null;
  with: string[];
}

export interface Exported {
  folder: string;
  readable: string;
  backup: string;
}

export interface EraseRequest {
  keep_models: boolean;
  at: number;
}

/** Typed by the person to confirm deleting everything (consent.rs ERASE_PHRASE). */
export const ERASE_PHRASE = "delete everything";
const NEEDED_PREFIX = "consent:";

export const consentApi = {
  status: () => invoke<ConsentStatus[]>("consent_status"),
  accept: (shown: { id: string; version: number }[]) => invoke<ConsentStatus[]>("consent_accept", { shown }),
  exportEverything: (dir: string, password: string) => invoke<Exported>("export_everything_cmd", { dir, password }),
  eraseEverything: (keepModels: boolean, confirm: string) => invoke<void>("erase_everything", { keepModels, confirm }),
  eraseStatus: () => invoke<EraseRequest | null>("erase_status"),
  cancelErase: () => invoke<void>("cancel_erase_everything"),
};

export const DOCUMENT_TITLES: Record<string, string> = {
  terms: "Terms of Use",
  privacy: "Privacy Policy",
  guidelines: "Community Guidelines",
  copyright: "Copyright Policy",
  likeness: "Consent for faces, voices and LoRAs",
  market: "Market Data Disclaimer",
  relay: "Running a relay or server",
};

const SOURCE = "https://github.com/IsaacLeh1/sulcusai/blob/main/docs/legal/";

/** The published copy, for reading in a browser or sharing. */
export function documentLink(file: string): string {
  return SOURCE + encodeURIComponent(file);
}

const bundled = import.meta.glob("../../docs/legal/*.md", { query: "?raw", import: "default" }) as Record<string, () => Promise<string>>;

/** The full text of a document as this copy of the app shipped it. */
export async function documentText(file: string): Promise<string | null> {
  const load = bundled[`../../docs/legal/${file}`];
  return load ? load() : null;
}

/** What to accept before the app opens, in the order they're listed. */
export function dueAtStart(all: ConsentStatus[]): ConsentStatus[] {
  return all.filter((s) => s.at_start && s.due);
}

/** A notice plus the documents that come with it, as they need accepting now. */
export function dueFor(all: ConsentStatus[], id: string): ConsentStatus[] {
  const own = all.find((s) => s.id === id);
  if (!own) return [];
  const withIt = own.with.map((w) => all.find((s) => s.id === w)).filter((s): s is ConsentStatus => !!s);
  return [own, ...withIt].filter((s) => s.due);
}

/** The feature whose notice a command is waiting for, from its error. */
export function neededNotice(error: unknown): string | null {
  const text = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return text.startsWith(NEEDED_PREFIX) ? text.slice(NEEDED_PREFIX.length) : null;
}

/** Whether a change of version means the person has seen this before. */
export function isUpdate(s: ConsentStatus): boolean {
  return s.accepted !== null && s.accepted < s.version;
}
