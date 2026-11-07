// SPDX-License-Identifier: AGPL-3.0-only
// One set of building blocks so every page looks and behaves the same.
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as RPointerEvent, type ReactNode } from "react";
import { t } from "../i18n";
import { relativeTime, timeLeft } from "../ui/time";
import { setUiPrefs, uiPrefs } from "../ui/prefs";
import { Modal } from "./Modal";

export function EmptyState({ title, text, action }: { title: string; text?: string; action?: { label: string; run: () => void } }) {
  return (
    <div className="ui-empty">
      <div className="ui-empty-mark" aria-hidden="true" />
      <h3>{title}</h3>
      {text && <p className="muted">{text}</p>}
      {action && (
        <button className="btn primary" onClick={action.run}>
          {action.label}
        </button>
      )}
    </div>
  );
}

export function Skeleton({ lines = 3, block }: { lines?: number; block?: boolean }) {
  return (
    <div className="ui-skeleton" aria-busy="true" aria-label={t("Loading")}>
      {block && <div className="ui-sk-block" />}
      {Array.from({ length: lines }, (_, i) => (
        <div key={i} className="ui-sk-line" style={{ width: `${90 - i * 15}%` }} />
      ))}
    </div>
  );
}

/** An error in plain words, with the technical text one click away for a report. */
export function ErrorNote({ text, details, onRetry }: { text: string; details?: string; onRetry?: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="error-box ui-error" role="alert">
      <span>{text}</span>
      <span className="ui-error-actions">
        {onRetry && (
          <button className="link-btn small" onClick={onRetry}>
            {t("Try again")}
          </button>
        )}
        {details && (
          <button
            className="link-btn small"
            onClick={() => navigator.clipboard?.writeText(details).then(() => setCopied(true)).catch(() => {})}
          >
            {copied ? t("Copied") : t("Copy details")}
          </button>
        )}
      </span>
    </div>
  );
}

export function Tip({ text, children }: { text: string; children: ReactNode }) {
  return (
    <span className="ui-tip" data-tip={text} aria-label={text}>
      {children}
    </span>
  );
}

/** A switch where the whole row is the click target, with a sentence of help under the name. */
export function ToggleRow({ label, help, checked, onChange, disabled, why }: { label: string; help?: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; why?: string }) {
  return (
    <label className={`ui-toggle-row${disabled ? " disabled" : ""}`} title={disabled ? why : undefined}>
      <span className="ui-toggle-text">
        <span>{label}</span>
        {help && <span className="muted small">{help}</span>}
      </span>
      <input type="checkbox" role="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
    </label>
  );
}

export function SliderRow({ label, value, min, max, step = 1, reset, onChange, unit = "" }: { label: string; value: number; min: number; max: number; step?: number; reset?: number; onChange: (v: number) => void; unit?: string }) {
  return (
    <label className="ui-slider-row">
      <span>{label}</span>
      <input type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <output>
        {value}
        {unit}
      </output>
      {reset !== undefined && value !== reset && (
        <button className="link-btn small" onClick={(e) => (e.preventDefault(), onChange(reset))}>
          {t("Reset")}
        </button>
      )}
    </label>
  );
}

export function TimeAgo({ when }: { when: number | string | Date }) {
  const [, tick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 60_000);
    return () => clearInterval(id);
  }, []);
  const ms = typeof when === "number" ? (when < 1e12 ? when * 1000 : when) : new Date(when).getTime();
  return <time title={new Date(ms).toLocaleString()}>{relativeTime(when)}</time>;
}

export function Breadcrumbs({ trail }: { trail: { label: string; go?: () => void }[] }) {
  const back = trail.length > 1 ? trail[trail.length - 2].go : undefined;
  useEffect(() => {
    if (!back) return;
    const key = (e: KeyboardEvent) => {
      if (e.altKey && e.key === "ArrowLeft") (e.preventDefault(), back());
    };
    // The mouse's back button.
    const mouse = (e: MouseEvent) => {
      if (e.button === 3) (e.preventDefault(), back());
    };
    window.addEventListener("keydown", key);
    window.addEventListener("mouseup", mouse);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("mouseup", mouse);
    };
  }, [back]);
  return (
    <nav className="ui-crumbs" aria-label={t("You are here")}>
      {trail.map((c, i) => (
        <span key={i}>
          {i > 0 && <span className="ui-crumb-sep" aria-hidden="true">›</span>}
          {c.go && i < trail.length - 1 ? (
            <button className="link-btn" onClick={c.go}>
              {c.label}
            </button>
          ) : (
            <span aria-current={i === trail.length - 1 ? "page" : undefined}>{c.label}</span>
          )}
        </span>
      ))}
    </nav>
  );
}

/** A long job: what it's doing, how far, time left, and Cancel. */
export function JobProgress({ label, fraction, startedAt, onCancel }: { label: string; fraction: number | null; startedAt?: number; onCancel?: () => void }) {
  const left = fraction != null && startedAt ? timeLeft(startedAt, fraction) : null;
  return (
    <div className="ui-job">
      <div className="ui-job-head">
        <span className="ellipsis">{label}</span>
        <span className="muted small">
          {fraction != null ? `${Math.round(fraction * 100)}%` : t("Working…")}
          {left ? ` · ${left}` : ""}
        </span>
        {onCancel && (
          <button className="link-btn small" onClick={onCancel}>
            {t("Cancel")}
          </button>
        )}
      </div>
      <div className={`ui-bar${fraction == null ? " busy" : ""}`} role="progressbar" aria-valuenow={fraction != null ? Math.round(fraction * 100) : undefined} aria-valuemin={0} aria-valuemax={100}>
        <span style={{ width: fraction != null ? `${fraction * 100}%` : undefined }} />
      </div>
    </div>
  );
}

/** A side panel with a drag handle; double-click resets it, and the size is remembered by id. */
export function ResizablePanel({ id, side = "left", initial = 280, min = 160, max = 640, collapsed, children }: { id: string; side?: "left" | "right"; initial?: number; min?: number; max?: number; collapsed?: boolean; children: ReactNode }) {
  const [width, setWidth] = useState(() => uiPrefs().panelSizes[id] ?? initial);
  const save = (w: number) => setUiPrefs({ panelSizes: { ...uiPrefs().panelSizes, [id]: w } });
  const drag = (e: RPointerEvent) => {
    e.preventDefault();
    const start = e.clientX;
    const from = width;
    let last = from;
    const move = (m: PointerEvent) => {
      const d = side === "left" ? m.clientX - start : start - m.clientX;
      last = Math.min(max, Math.max(min, from + d));
      setWidth(last);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      save(last);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div className={`ui-panel ${side}${collapsed ? " collapsed" : ""}`} style={{ width: collapsed ? undefined : width }}>
      {children}
      {!collapsed && (
        <div
          className="ui-panel-handle"
          role="separator"
          aria-orientation="vertical"
          title={t("Drag to resize, double-click to reset")}
          onPointerDown={drag}
          onDoubleClick={() => (setWidth(initial), save(initial))}
        />
      )}
    </div>
  );
}

export function OfflineBanner() {
  const [online, setOnline] = useState(navigator.onLine);
  useEffect(() => {
    const up = () => setOnline(true);
    const down = () => setOnline(false);
    window.addEventListener("online", up);
    window.addEventListener("offline", down);
    return () => {
      window.removeEventListener("online", up);
      window.removeEventListener("offline", down);
    };
  }, []);
  if (online) return null;
  return (
    <div className="ui-offline" role="status">
      {t("You're offline. Chats with models on this PC, your files and the Studio still work; web search, cloud models and people will come back when the network does.")}
    </div>
  );
}

/** A dialog with one main button: Esc closes, Enter does the main thing. */
export function Dialog({ title, children, main, onClose, danger }: { title: string; children: ReactNode; main?: { label: string; run: () => void; disabled?: boolean }; onClose: () => void; danger?: boolean }) {
  // Modal keeps Escape, focus and Enter (for the main button) the same as every other dialog.
  return (
    <Modal title={title} onClose={onClose}>
      <div className="ui-dialog-body">{children}</div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>
          {main ? t("Cancel") : t("Close")}
        </button>
        {main && (
          <button className={danger ? "btn danger-fill" : "btn primary"} disabled={main.disabled} onClick={main.run}>
            {main.label}
          </button>
        )}
      </div>
    </Modal>
  );
}

export interface MenuItem {
  label: string;
  run: () => void;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  why?: string;
}

/**
 * A right-click menu at the pointer, kept on screen, with a search box when
 * it's long. Dangerous items always go last, in red, after a line.
 */
export function PopMenu({ x, y, items, onClose }: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [at, setAt] = useState({ x, y });
  const [sel, setSel] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  const shown = useMemo(() => {
    const f = items.filter((i) => i.label.toLowerCase().includes(q.toLowerCase()));
    return [...f.filter((i) => !i.danger), ...f.filter((i) => i.danger)];
  }, [items, q]);
  useLayoutEffect(() => {
    const r = box.current?.getBoundingClientRect();
    if (!r) return;
    setAt({ x: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)), y: Math.max(4, Math.min(y, window.innerHeight - r.height - 4)) });
  }, [x, y]);
  useEffect(() => {
    const away = (e: MouseEvent) => !box.current?.contains(e.target as Node) && onClose();
    window.addEventListener("mousedown", away);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", away);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);
  const pick = (i: MenuItem | undefined) => {
    if (!i || i.disabled) return;
    onClose();
    i.run();
  };
  const firstDanger = shown.findIndex((i) => i.danger);
  return (
    <div
      className="ui-menu"
      role="menu"
      ref={box}
      style={{ left: at.x, top: at.y }}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
        else if (e.key === "ArrowDown") (e.preventDefault(), setSel((s) => Math.min(shown.length - 1, s + 1)));
        else if (e.key === "ArrowUp") (e.preventDefault(), setSel((s) => Math.max(0, s - 1)));
        else if (e.key === "Enter") (e.preventDefault(), pick(shown[sel]));
      }}
    >
      {items.length > 7 && <input autoFocus className="ui-menu-search" placeholder={t("Search")} value={q} onChange={(e) => (setQ(e.target.value), setSel(0))} />}
      {shown.map((i, n) => (
        <div key={i.label}>
          {n === firstDanger && n > 0 && <hr />}
          <button
            role="menuitem"
            autoFocus={items.length <= 7 && n === 0}
            className={`${i.danger ? "danger-text" : ""}${n === sel ? " sel" : ""}`}
            disabled={i.disabled}
            title={i.disabled ? i.why : undefined}
            onMouseEnter={() => setSel(n)}
            onClick={() => pick(i)}
          >
            <span>{i.label}</span>
            {i.shortcut && <kbd>{i.shortcut}</kbd>}
          </button>
        </div>
      ))}
      {shown.length === 0 && <p className="muted small">{t("Nothing matches")}</p>}
    </div>
  );
}

/** Plain names for technical labels; the technical one stays in the tooltip. */
export const PLAIN: Record<string, string> = {
  MTP: "Faster replies (draft ahead)",
  RPC: "Lend this PC's card",
  GGUF: "Model file",
  Harmony: "Reasoning format",
  LoRA: "Style add-on",
  VRAM: "Graphics memory",
  Quantization: "Model size",
  "Context length": "How much it remembers",
  Temperature: "Creativity",
};

export function PlainLabel({ tech }: { tech: string }) {
  const plain = PLAIN[tech];
  if (!plain) return <>{tech}</>;
  return <span title={tech}>{t(plain)}</span>;
}
