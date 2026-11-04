// SPDX-License-Identifier: AGPL-3.0-only
// Find index, tidy plans, PDF citations, sheet plans and records (src-tauri/src/files).
import { invoke } from "@tauri-apps/api/core";

export interface FindHit {
  path: string;
  name: string;
  size: number;
  modified: number;
  /** The match is wrapped in [brackets]. */
  snippet: string;
}
export interface IndexReport {
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
}
export interface DupeGroup {
  paths: string[];
  size: number;
}
export interface TidyMove {
  from: string;
  to: string;
}
export type FindKind = "Pictures" | "Documents" | "Music" | "Videos" | "Archives" | "Installers" | "Other";

export const findFolders = () => invoke<string[]>("find_folders");
export const findAddFolder = (path: string) => invoke<IndexReport>("find_add_folder", { path });
export const findRemoveFolder = (path: string) => invoke<void>("find_remove_folder", { path });
export const findRefresh = () => invoke<IndexReport>("find_refresh");
export const findSearch = (query: string, kind?: FindKind) => invoke<FindHit[]>("find_search", { query, kind: kind ?? null });
export const findDuplicates = () => invoke<DupeGroup[]>("find_duplicates");
export const tidyPlan = (path: string) => invoke<TidyMove[]>("tidy_plan", { path });
export const tidyApply = (moves: TidyMove[]) => invoke<[number, string[]]>("tidy_apply", { moves });

/** A snippet split so the matched words can be shown in bold. */
export function snippetParts(s: string): { text: string; hit: boolean }[] {
  const out: { text: string; hit: boolean }[] = [];
  const re = /\[([^\]]*)\]/g;
  let last = 0;
  for (let m = re.exec(s); m; m = re.exec(s)) {
    if (m.index > last) out.push({ text: s.slice(last, m.index), hit: false });
    out.push({ text: m[1], hit: true });
    last = m.index + m[0].length;
  }
  if (last < s.length) out.push({ text: s.slice(last), hit: false });
  return out;
}
