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

export interface FileRecord {
  id: string;
  kind: "inventory" | "form" | "scan";
  created: number;
  updated: number;
  data: Record<string, unknown>;
}
export interface RoomTotal {
  room: string;
  items: number;
  value: number;
}
export interface Passage {
  page: number;
  text: string;
  score: number;
}
export interface PageChange {
  page: number;
  removed: string[];
  added: string[];
}
export type SheetStep =
  | { step: "summary"; column: string }
  | { step: "group_total"; by: string; column: string }
  | { step: "filter"; column: string; contains: string };

export const recordsList = (kind: FileRecord["kind"], query?: string) => invoke<FileRecord[]>("records_list", { kind, query: query ?? null });
export const recordsDelete = (id: string) => invoke<boolean>("records_delete", { id });
export const inventorySave = (item: Record<string, unknown>, id?: string) => invoke<FileRecord>("inventory_save", { id: id ?? null, item });
export const inventoryTotals = () => invoke<RoomTotal[]>("inventory_totals");
export const inventoryExport = (path: string) => invoke<void>("inventory_export", { path });
export const scanSave = (path: string) => invoke<FileRecord>("scan_save", { path });
export const describePictures = (limit?: number) => invoke<number>("find_describe_pictures", { limit: limit ?? null });
export const pdfPassages = (path: string, question: string) => invoke<Passage[]>("pdf_passages", { path, question });
export const pdfCompare = (oldPath: string, newPath: string) => invoke<PageChange[]>("pdf_compare", { old: oldPath, new: newPath });
export const pdfRedact = (path: string, terms: string[], patterns: boolean) => invoke<[string, number]>("pdf_redact", { path, terms, patterns });
export const sheetPlan = (path: string, steps: SheetStep[]) => invoke<string[]>("sheet_plan", { path, sheet: null, steps });
export const sheetRun = (path: string, steps: SheetStep[]) => invoke<string>("sheet_run", { path, sheet: null, steps });

/** Plan lines typed one per line ("Total of Cost by Room", "Rows where Item contains lamp", "Summary of Cost"). */
export function parseSteps(text: string): SheetStep[] {
  const out: SheetStep[] = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    let m = /^total of (.+?) (?:by|for each) (.+)$/i.exec(line);
    if (m) {
      out.push({ step: "group_total", column: m[1].trim(), by: m[2].trim() });
      continue;
    }
    m = /^rows where (.+?) contains (.+)$/i.exec(line);
    if (m) {
      out.push({ step: "filter", column: m[1].trim(), contains: m[2].trim() });
      continue;
    }
    m = /^summary of (.+)$/i.exec(line);
    if (m) out.push({ step: "summary", column: m[1].trim() });
  }
  return out;
}

export interface PdfFormField {
  name: string;
  kind: "text" | "checkbox" | "choice" | "other";
  value: string;
}
export const pdfFormFields = (path: string) => invoke<PdfFormField[]>("pdf_form_fields", { path });
export const pdfFormFill = (path: string, values: Record<string, string>) => invoke<string>("pdf_form_fill", { path, values });
