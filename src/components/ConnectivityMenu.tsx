// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import type { Connectivity } from "../api";
import { Modal } from "./Modal";

export const LEVELS: { id: Connectivity; icon: string; label: string; short: string; detail: string }[] = [
  {
    id: "offline",
    icon: "🔒",
    label: "Offline",
    short: "Offline",
    detail: "Nothing leaves this PC. Model downloads you start are the only exception.",
  },
  {
    id: "web",
    icon: "🌐",
    label: "Local AI + Web",
    short: "Web",
    detail: "Adds web search, the browser, and email/calendar sync. All AI still runs on this PC.",
  },
  {
    id: "cloud",
    icon: "☁",
    label: "Cloud",
    short: "Cloud",
    detail: "Adds cloud AI models and cloud sync. Prompts sent to cloud models leave this PC.",
  },
];

export function levelInfo(level: Connectivity) {
  return LEVELS.find((l) => l.id === level)!;
}

/** Status-bar switch: one click to see and change the level. */
export function ConnectivityMenu({ level, onChange }: { level: Connectivity; onChange: (l: Connectivity) => void }) {
  const [open, setOpen] = useState(false);
  const [confirmCloud, setConfirmCloud] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const info = levelInfo(level);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  const pick = (l: Connectivity) => {
    setOpen(false);
    if (l === "cloud" && level !== "cloud") setConfirmCloud(true);
    else onChange(l);
  };

  return (
    <div className="conn" ref={ref}>
      <button className={`conn-pill ${level}`} onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open}>
        <span aria-hidden>{info.icon}</span> {info.short}
      </button>
      {open && (
        <div className="conn-menu" role="menu">
          {LEVELS.map((l) => (
            <button key={l.id} role="menuitemradio" aria-checked={l.id === level} className={l.id === level ? "active" : ""} onClick={() => pick(l.id)}>
              <span className="conn-icon" aria-hidden>{l.icon}</span>
              <span>
                <strong>{l.label}</strong>
                <span className="small muted block">{l.detail}</span>
              </span>
            </button>
          ))}
        </div>
      )}
      {confirmCloud && <CloudConfirm onCancel={() => setConfirmCloud(false)} onConfirm={() => { setConfirmCloud(false); onChange("cloud"); }} />}
    </div>
  );
}

export function CloudConfirm({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  return (
    <Modal title="Turn on cloud features?" onClose={onCancel}>
      <p>With Cloud on, you can choose cloud AI models and cloud sync. When you use them:</p>
      <ul>
        <li>Your messages and any files you attach are sent to the provider you picked.</li>
        <li>Each cloud feature shows a ☁ badge, and you turn providers on one at a time.</li>
        <li>Local models stay the default unless you pick a cloud one.</li>
      </ul>
      <p className="muted small">Cloud providers arrive in a later update; this setting is ready for them.</p>
      <div className="modal-actions">
        <button className="btn" onClick={onCancel}>Keep it local</button>
        <button className="btn primary" onClick={onConfirm}>Turn on Cloud</button>
      </div>
    </Modal>
  );
}
