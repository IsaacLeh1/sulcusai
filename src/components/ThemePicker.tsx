// SPDX-License-Identifier: AGPL-3.0-only
// Choosing the look: System, Light, Dark or OLED (true black). Settings shows
// cards with a small preview; the gear menu shows a compact row.
import { setTheme, useTheme, type ThemeChoice } from "../theme";
import { t, tx } from "../i18n";
import { FullCircleIcon, HalfCircleIcon, MoonIcon, SunIcon } from "./Icons";

const CHOICES: { id: ThemeChoice; label: string; icon: typeof SunIcon; note: string }[] = [
  { id: "system", label: tx("System"), icon: HalfCircleIcon, note: tx("Follows Windows") },
  { id: "light", label: tx("Light"), icon: SunIcon, note: tx("Bright and clear") },
  { id: "dark", label: tx("Dark"), icon: MoonIcon, note: tx("Easy on the eyes") },
  { id: "oled", label: tx("OLED black"), icon: FullCircleIcon, note: tx("True black: saves power on OLED screens") },
];

export function ThemePicker({ compact = false }: { compact?: boolean }) {
  const choice = useTheme();
  if (compact) {
    return (
      <div className="theme-row" role="radiogroup" aria-label={t("Appearance")}>
        {CHOICES.map((c) => (
          <button key={c.id} role="radio" aria-checked={choice === c.id} className={choice === c.id ? "active" : ""} onClick={() => setTheme(c.id)} title={t(c.label)}>
            <c.icon />
            <span className="sr-only">{t(c.label)}</span>
          </button>
        ))}
      </div>
    );
  }
  return (
    <div className="theme-cards" role="radiogroup" aria-label={t("Appearance")}>
      {CHOICES.map((c) => (
        <button key={c.id} role="radio" aria-checked={choice === c.id} className={`theme-card ${choice === c.id ? "active" : ""}`} onClick={() => setTheme(c.id)}>
          <span className={`theme-preview look-${c.id}`} aria-hidden>
            <i className="tp-side" />
            <i className="tp-main">
              <i className="tp-line" />
              <i className="tp-line short" />
              <i className="tp-pill" />
            </i>
          </span>
          <strong>{t(c.label)}</strong>
          <span className="muted small">{t(c.note)}</span>
        </button>
      ))}
    </div>
  );
}
