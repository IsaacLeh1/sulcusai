// SPDX-License-Identifier: AGPL-3.0-only
// The model name in the status bar, as a menu: pick the default model from
// the installed ones, turn Auto on or off, or open Models.
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { ModelCard } from "../api";
import { t } from "../i18n";

export function ModelPicker({ label, installed, current, onPick, onModels, auto, onAuto }: {
  label: ReactNode;
  installed: ModelCard[];
  current: string | null | undefined;
  onPick: (id: string) => void;
  onModels: () => void;
  /** Auto: a message goes to the installed model clearly better at it. */
  auto: boolean;
  onAuto: (on: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <span className="model-picker" ref={ref}>
      <button className="model-picker-btn" onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open} title={t("Change the default model")}>
        {label} <span aria-hidden className="chev">▴</span>
      </button>
      {open && (
        <div className="model-menu" role="menu">
          <div className="model-menu-head small muted">{t("Default model for new chats")}</div>
          {installed.length === 0 && <p className="small muted pad">{t("No model installed")}</p>}
          {installed.map((m) => (
            <button
              key={m.id}
              role="menuitemradio"
              aria-checked={m.id === current}
              className={m.id === current ? "active" : ""}
              onClick={() => {
                setOpen(false);
                if (m.id !== current) onPick(m.id);
              }}
            >
              <span className="model-check" aria-hidden>{m.id === current ? "✓" : ""}</span>
              <span className="grow ellipsis">{m.name}</span>
              {m.params_b > 0 && <span className="muted small">{m.params_b}B</span>}
            </button>
          ))}
          {installed.length > 1 && (
            <label className="model-menu-auto small" title={t("A chat on the default model hands a message to another installed model when it's clearly better at it (code, writing, translation) and runs well on this PC. A chat whose model you picked keeps it.")}>
              <input type="checkbox" checked={auto} onChange={(e) => onAuto(e.target.checked)} />
              <span>{t("Auto: the best installed model for each message")}</span>
            </label>
          )}
          <button
            className="model-menu-foot"
            onClick={() => {
              setOpen(false);
              onModels();
            }}
          >
            {t("Models")} <span className="muted small">· {t("install, remove or compare")}</span>
          </button>
        </div>
      )}
    </span>
  );
}
