// SPDX-License-Identifier: AGPL-3.0-only
import { t } from "../i18n";
import type { Finding, SecurityReport, Severity } from "../api/security";

const DAY = 24 * 60 * 60 * 1000;

export const SEVERITY_ORDER: Severity[] = ["critical", "warning", "notice", "info"];

/** What a finding says, in the window's language, with its values filled in. */
export function findingText(f: Finding): { title: string; detail: string } {
  return { title: t(f.title, f.args), detail: t(f.detail, f.args) };
}

/** Findings by how serious they are, worst group first; empty groups left out. */
export function groupFindings(findings: Finding[]): { severity: Severity; items: Finding[] }[] {
  return SEVERITY_ORDER.map((severity) => ({ severity, items: findings.filter((f) => f.severity === severity) })).filter((g) => g.items.length > 0);
}

/** How long ago a phone was used, from a time in milliseconds. */
export function lastUsed(at: number | null, now: number): string {
  if (at == null) return t("Never used");
  const days = Math.floor(Math.max(0, now - at) / DAY);
  if (days === 0) return t("Used today");
  if (days === 1) return t("Used yesterday");
  return t("Used {days} days ago", { days });
}

export function keyAge(days: number): string {
  if (days === 0) return t("First seen today");
  return days === 1 ? t("In use for 1 day") : t("In use for {days} days", { days });
}

/** The one place a fix is applied: only after the person says yes to its
 * question, and never otherwise. Returns the new report, or null when the
 * answer was no. */
export async function fixAfterAsking(
  finding: Finding,
  ask: (question: string) => Promise<boolean>,
  apply: (id: string, confirmed: boolean) => Promise<SecurityReport>,
): Promise<SecurityReport | null> {
  if (!finding.fix) return null;
  if (!(await ask(t(finding.fix.confirm, finding.args)))) return null;
  return apply(finding.id, true);
}
