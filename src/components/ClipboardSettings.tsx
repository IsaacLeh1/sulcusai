// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

export interface ClipboardItem {
  text: string;
  at: number;
}

export interface ClipboardView {
  on: boolean;
  keep: number;
  items: ClipboardItem[];
}

export const clipboardApi = {
  view: () => invoke<ClipboardView>("clipboard_view"),
  set: (on: boolean, keep: number) => invoke<ClipboardView>("clipboard_set", { on, keep }),
  copy: (index: number) => invoke<void>("clipboard_copy", { index }),
  forget: (index: number | null) => invoke<ClipboardView>("clipboard_forget", { index }),
};

/** One line of a copy, short enough for a list. */
export function preview(text: string, max = 120): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** Settings: clipboard history, off until turned on. */
export function ClipboardSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<ClipboardView | null>(null);
  useEffect(() => {
    clipboardApi.view().then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const act = (p: Promise<ClipboardView>) => p.then(setV).catch((e) => toast(errorText(e), "error"));

  return (
    <section className="card">
      <h2>{t("Clipboard history")}</h2>
      <div className="toggle-row">
        <div>
          <strong>{t("Keep what I copy")}</strong>
          <span className="small muted block">
            {t("Keeps the last texts you copied on this PC, encrypted, so you can copy them again. Copies that password managers mark as private are skipped. Turning it off forgets them.")}
          </span>
        </div>
        <label className="switch">
          <input type="checkbox" checked={v.on} onChange={(e) => act(clipboardApi.set(e.target.checked, v.keep))} aria-label={t("Keep what I copy")} />
          <span />
        </label>
      </div>
      {v.on && (
        <>
          <label className="row small">
            {t("How many to keep")}
            <select value={v.keep} onChange={(e) => act(clipboardApi.set(true, Number(e.target.value)))}>
              {[10, 20, 50, 100].map((n) => (
                <option key={n} value={n}>{n}</option>
              ))}
            </select>
          </label>
          {v.items.length === 0 ? (
            <p className="muted small">{t("Nothing copied since it was turned on.")}</p>
          ) : (
            <ul className="clipboard-list">
              {v.items.map((item, i) => (
                <li key={`${item.at}-${i}`} className="row">
                  <span className="small" style={{ flex: 1 }} title={item.text}>{preview(item.text)}</span>
                  <button className="btn small" onClick={() => clipboardApi.copy(i).then(() => toast(t("Copied."), "success")).catch((e) => toast(errorText(e), "error"))}>
                    {t("Copy")}
                  </button>
                  <button className="btn ghost small" onClick={() => act(clipboardApi.forget(i))} aria-label={t("Forget this copy")}>
                    {t("Forget")}
                  </button>
                </li>
              ))}
            </ul>
          )}
          {v.items.length > 0 && (
            <button className="btn" onClick={() => act(clipboardApi.forget(null))}>{t("Forget all")}</button>
          )}
        </>
      )}
    </section>
  );
}
