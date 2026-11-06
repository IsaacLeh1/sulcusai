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

export interface Ingredient {
  name: string;
  qty: number;
  unit: string;
}

export interface PersonalOverview {
  deliveries: PersonalRecord[];
  trips: { start: number; end: number; bookings: string[] }[];
  birthdays: { name: string; date: string; days: number; turns: number | null }[];
  follow_ups: PersonalRecord[];
  today: { calories: number; protein_g: number; workout_min: number };
  care_due: string[];
  car_due: string[];
  journal_streak: number;
  journal_prompt: string;
  meditation_min_week: number;
}

export const scanMail = (limit?: number) => invoke<number>("personal_scan_mail", { limit: limit ?? null });
export const refineMail = (limit?: number) => invoke<number>("personal_refine_mail", { limit: limit ?? null });
export const personalOverview = () => invoke<PersonalOverview>("personal_overview");
export const leaveAt = (startMs: number, travelMin: number) => invoke<number>("personal_leave_at", { startMs, travelMin });
export const choreRota = (chores: string[], people: string[], week: number) =>
  invoke<{ chore: string; who: string }[]>("personal_rota", { chores, people, week });
export const scaleRecipe = (ingredients: Ingredient[], from: number, to: number) =>
  invoke<Ingredient[]>("personal_scale_recipe", { ingredients, from, to });
export const shoppingList = (meals: Ingredient[][]) => invoke<Ingredient[]>("personal_shopping_list", { meals });
export const breathing = (minutes: number, each: number) =>
  invoke<{ name: string; seconds: number }[]>("personal_breathing", { minutes, each });

/** "2 eggs", "300 g flour", "1.5 l milk" per line. Lines without a number keep qty 1. */
export function parseIngredients(text: string): Ingredient[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => {
      const m = l.match(/^(\d+(?:[.,]\d+)?)\s*(g|kg|ml|l|cup|cups|tbsp|tsp|oz|lb)?\s+(.+)$/i);
      if (!m) return { name: l, qty: 1, unit: "" };
      return { qty: Number(m[1].replace(",", ".")), unit: (m[2] ?? "").toLowerCase(), name: m[3] };
    });
}

/** Week number since the epoch, so the chore rota turns every Monday. */
export function weekIndex(ms: number): number {
  return Math.floor((ms / 86_400_000 + 3) / 7);
}
