// SPDX-License-Identifier: AGPL-3.0-only
// Settings card for the Layout assistant: say what to change in plain words,
// or pick it directly. Off until switched on; switching off restores the
// shipped look without forgetting the choices.
import { useState } from "react";
import { t, tx } from "../i18n";
import {
  ACCENTS,
  DEFAULT_LAYOUT,
  TEXT_MAX,
  TEXT_MIN,
  canUndoLayout,
  interpretLayout,
  setLayout,
  undoLayout,
  useLayout,
  type Accent,
  type Change,
  type LayoutPrefs,
} from "./layout";

const SWATCH: Record<Accent, string> = {
  violet: "#6447e8",
  blue: "#2f6fe0",
  green: "#1f8a5b",
  orange: "#c4650f",
  pink: "#c43c78",
  gray: "#4f5563",
};
const ACCENT_NAMES: Record<Accent, string> = {
  violet: tx("Violet"),
  blue: tx("Blue"),
  green: tx("Green"),
  orange: tx("Orange"),
  pink: tx("Pink"),
  gray: tx("Gray"),
};

function Pick<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <label className="form small">
      {t(label)}
      <select className="input" value={value} onChange={(e) => onChange(e.target.value as T)}>
        {options.map(([v, l]) => (
          <option key={v} value={v}>
            {t(l)}
          </option>
        ))}
      </select>
    </label>
  );
}

export function LayoutAssistant() {
  const p = useLayout();
  const [ask, setAsk] = useState("");
  const [said, setSaid] = useState<Change[] | null>(null);
  const change = (patch: Partial<LayoutPrefs>) => setLayout({ ...p, ...patch });

  const run = () => {
    if (!ask.trim()) return;
    const r = interpretLayout(ask, p);
    if (r.changes.length) setLayout(r.next);
    setSaid(r.changes);
    setAsk("");
  };

  return (
    <>
      <div className="toggle-row">
        <div>
          <strong>{t("Layout assistant")}</strong>
          <span className="small muted block">
            {t("Move the sidebar, change spacing, chat width, text size and colour by saying what you want. It only changes how this window looks.")}
          </span>
        </div>
        <label className="switch" title={p.on ? t("On") : t("Off")}>
          <input type="checkbox" checked={p.on} aria-label={t("Layout assistant")} onChange={(e) => change({ on: e.target.checked })} />
          <span />
        </label>
      </div>
      {p.on && (
        <>
          <form
            className="layout-ask"
            onSubmit={(e) => {
              e.preventDefault();
              run();
            }}
          >
            <input
              className="input"
              value={ask}
              onChange={(e) => setAsk(e.target.value)}
              placeholder={t("For example: sidebar on the right, compact, bigger text, green")}
              aria-label={t("What to change")}
            />
            <button className="btn small" type="submit" disabled={!ask.trim()}>
              {t("Change")}
            </button>
            <button className="btn ghost small" type="button" disabled={!canUndoLayout()} onClick={() => undoLayout()}>
              {t("Undo")}
            </button>
          </form>
          {said && (
            <p className="small muted" role="status">
              {said.length ? said.map((c) => t(c.en, c.vars)).join(", ") : t("Nothing to change there. Try words like right, compact, wide, plain, bigger text or a colour.")}
            </p>
          )}
          <div className="layout-grid">
            <Pick label={tx("Sidebar")} value={p.sidebar} options={[["left", tx("On the left")], ["right", tx("On the right")]]} onChange={(v) => change({ sidebar: v })} />
            <Pick label={tx("Spacing")} value={p.density} options={[["comfortable", tx("Roomy")], ["compact", tx("Compact")]]} onChange={(v) => change({ density: v })} />
            <Pick label={tx("Chat width")} value={p.chatWidth} options={[["narrow", tx("Narrow")], ["normal", tx("Normal")], ["wide", tx("Wide")]]} onChange={(v) => change({ chatWidth: v })} />
            <Pick label={tx("Messages")} value={p.chatStyle} options={[["bubbles", tx("In bubbles")], ["plain", tx("Plain, like a document")]]} onChange={(v) => change({ chatStyle: v })} />
            <label className="form small">
              {t("Text size {n}%", { n: p.textSize })}
              <input type="range" min={TEXT_MIN} max={TEXT_MAX} step={5} value={p.textSize} onChange={(e) => change({ textSize: Number(e.target.value) })} />
            </label>
            <div className="form small">
              {t("Colour")}
              <div className="layout-swatches" role="radiogroup" aria-label={t("Colour")}>
                {ACCENTS.map((a) => (
                  <button
                    key={a}
                    type="button"
                    role="radio"
                    aria-checked={p.accent === a}
                    className="layout-swatch"
                    style={{ background: SWATCH[a] }}
                    title={t(ACCENT_NAMES[a])}
                    aria-label={t(ACCENT_NAMES[a])}
                    onClick={() => change({ accent: a })}
                  />
                ))}
              </div>
            </div>
          </div>
          <button className="btn ghost small" type="button" onClick={() => setLayout({ ...DEFAULT_LAYOUT, on: true })}>
            {t("Back to the usual layout")}
          </button>
        </>
      )}
    </>
  );
}
