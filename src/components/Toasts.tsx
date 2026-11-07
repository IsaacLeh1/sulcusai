// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { t } from "../i18n";
import { chime } from "../ui/sounds";

export type ToastKind = "info" | "success" | "error";
export interface ToastAction {
  label: string;
  run: () => void;
}
export interface Toast {
  id: number;
  text: string;
  kind: ToastKind;
  action?: ToastAction;
  details?: string;
}
export type PushToast = (text: string, kind?: ToastKind, extra?: { action?: ToastAction; details?: string; ms?: number }) => void;

/** Anything outside the React tree (helpers, menus) can raise a toast through this event. */
export function toastEvent(text: string, kind: ToastKind = "info", extra?: { action?: ToastAction; details?: string; ms?: number }) {
  window.dispatchEvent(new CustomEvent("sulcus:toast", { detail: { text, kind, extra } }));
}

/**
 * Deletes after a 30 s grace period instead of asking first, so a slip costs
 * one click. The work runs when the time is up or the window closes.
 */
export function undoable(text: string, commit: () => void, undo?: () => void) {
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    window.removeEventListener("beforeunload", finish);
    commit();
  };
  const timer = setTimeout(finish, 30_000);
  window.addEventListener("beforeunload", finish);
  toastEvent(text, "success", {
    ms: 30_000,
    action: {
      label: t("Undo"),
      run: () => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        window.removeEventListener("beforeunload", finish);
        undo?.();
      },
    },
  });
}

export function useToasts() {
  const [list, setList] = useState<Toast[]>([]);
  const next = useRef(1);
  const dismiss = useCallback((id: number) => setList((l) => l.filter((x) => x.id !== id)), []);
  const push = useCallback<PushToast>(
    (text, kind = "info", extra) => {
      const id = next.current++;
      setList((l) => [...l.slice(-4), { id, text, kind, action: extra?.action, details: extra?.details }]);
      chime(kind === "error" ? "error" : kind === "success" ? "done" : "note");
      // Errors stay until dismissed; everything else fades out.
      if (kind !== "error") setTimeout(() => dismiss(id), extra?.ms ?? 5000);
    },
    [dismiss],
  );
  useEffect(() => {
    const take = (e: Event) => {
      const d = (e as CustomEvent<{ text: string; kind: ToastKind; extra?: Parameters<PushToast>[2] }>).detail;
      push(d.text, d.kind, d.extra);
    };
    window.addEventListener("sulcus:toast", take);
    return () => window.removeEventListener("sulcus:toast", take);
  }, [push]);
  return useMemo(() => ({ list, push, dismiss }), [list, push, dismiss]);
}

export function Toasts({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((item) => (
        <div key={item.id} className={`toast ${item.kind}`}>
          <span className="toast-mark" aria-hidden="true">{item.kind === "success" ? "✓" : item.kind === "error" ? "!" : "i"}</span>
          <span>{item.text}</span>
          {item.details && (
            <button className="link-btn small" onClick={() => navigator.clipboard?.writeText(item.details ?? "").catch(() => {})}>
              {t("Copy details")}
            </button>
          )}
          {item.action && (
            <button
              className="link-btn small"
              onClick={() => {
                item.action?.run();
                onDismiss(item.id);
              }}
            >
              {item.action.label}
            </button>
          )}
          <button className="icon-btn" aria-label={t("Dismiss")} onClick={() => onDismiss(item.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
