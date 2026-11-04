// SPDX-License-Identifier: AGPL-3.0-only
// Personal life commands (src-tauri/src/personal). Records are encrypted on
// the PC; only kind and date are plain.
import { invoke } from "@tauri-apps/api/core";

export type PersonalKind =
  | "package" | "booking" | "event" | "newsletter" | "birthday" | "application" | "trip" | "contact" | "family"
  | "food" | "workout" | "health" | "recipe" | "meal_plan" | "car" | "plant" | "pet" | "journal" | "meditation";

export interface PersonalRecord {
  id: string;
  kind: PersonalKind;
  at: number;
  data: Record<string, unknown>;
  updated_at: number;
}

export interface LooseTask {
  title: string;
  due: string | null;
}

export const personalEnabled = () => invoke<boolean>("personal_enabled");
export const setPersonalEnabled = (on: boolean) => invoke<void>("personal_set_enabled", { on });
export const listPersonal = (kind: PersonalKind) => invoke<PersonalRecord[]>("personal_list", { kind });
export const savePersonal = (kind: PersonalKind, at: number, data: Record<string, unknown>, id?: string) =>
  invoke<PersonalRecord>("personal_save", { id: id ?? null, kind, at, data });
export const deletePersonal = (id: string) => invoke<void>("personal_delete", { id });
export const personalFromMail = (subject: string, from: string, body: string, receivedMs: number) =>
  invoke<PersonalRecord | null>("personal_from_mail", { subject, from, body, receivedMs });
export const tasksFromText = (text: string) => invoke<LooseTask[]>("personal_tasks_from_text", { text });
export const profileCheck = (text: string) => invoke<string[]>("personal_profile_check", { text });

/** Records grouped by day (local), newest day first, for the journal and logs. */
export function byDay(records: PersonalRecord[]): [string, PersonalRecord[]][] {
  const m = new Map<string, PersonalRecord[]>();
  for (const r of [...records].sort((a, b) => b.at - a.at)) {
    const d = new Date(r.at);
    const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    m.set(key, [...(m.get(key) ?? []), r]);
  }
  return [...m.entries()];
}
