// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../Toasts";
import type { Guide } from "./catalog";

export function openLink(url: string, toast: PushToast) {
  api.previewOpenExternal(url).catch((e) => toast(errorText(e), "error"));
}

/** One step-by-step guide, folded to its title until opened. */
export function SetupGuide({ guide, toast, open: startOpen = false }: { guide: Guide; toast: PushToast; open?: boolean }) {
  const [open, setOpen] = useState(startOpen);
  const [done, setDone] = useState<Set<number>>(new Set());
  const copy = (v: string) =>
    navigator.clipboard.writeText(v).then(
      () => toast(t("Copied."), "success"),
      () => toast(t("Couldn't copy. Select the text and press Ctrl+C."), "error"),
    );
  return (
    <div className={`setup-guide ${open ? "open" : ""}`} id={`guide-${guide.id}`}>
      <button className="setup-guide-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <strong>{t(guide.title)}</strong>
        <span className="muted small">{open ? t("Hide steps") : t("Show steps")}</span>
      </button>
      {open && (
        <>
          <p className="muted small">{t(guide.why)}</p>
          <ol className="setup-steps">
            {guide.steps.map((s, i) => (
              <li key={i} className={done.has(i) ? "done" : ""}>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={done.has(i)}
                    onChange={() => {
                      const next = new Set(done);
                      if (next.has(i)) next.delete(i);
                      else next.add(i);
                      setDone(next);
                    }}
                    aria-label={t("Step {n} done", { n: i + 1 })}
                  />
                </label>
                <span>
                  {t(s.text)}
                  {s.value && (
                    <span className="setup-value row">
                      <code>{s.value}</code>
                      <button className="btn ghost small" onClick={() => copy(s.value!)}>{t("Copy")}</button>
                    </span>
                  )}
                  {s.link && (
                    <button className="btn ghost small" onClick={() => openLink(s.link!, toast)}>
                      {t("Open {site}", { site: new URL(s.link).hostname })}
                    </button>
                  )}
                </span>
              </li>
            ))}
          </ol>
        </>
      )}
    </div>
  );
}
