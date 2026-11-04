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
  set: (keep: number) => invoke<ClipboardView>("clipboard_set", { keep }),
  copy: (index: number) => invoke<void>("clipboard_copy", { index }),
  forget: (index: number | null) => invoke<ClipboardView>("clipboard_forget", { index }),
};

/** One line of a copy, short enough for a list. */
export function preview(text: string, max = 120): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** Under Clipboard history on the Features page, while it's on: how many to keep and the copies so far. */
export function ClipboardSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<ClipboardView | null>(null);
  useEffect(() => {
    clipboardApi.view().then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const act = (p: Promise<ClipboardView>) => p.then(setV).catch((e) => toast(errorText(e), "error"));

  return (
    <div className="feature-setup">
      {v.on && (
        <>
          <label className="row small">
            {t("How many to keep")}
            <select value={v.keep} onChange={(e) => act(clipboardApi.set(Number(e.target.value)))}>
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
    </div>
  );
}
