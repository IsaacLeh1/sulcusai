// SPDX-License-Identifier: AGPL-3.0-only
// "Run on its own": parts of SulcusAI that can start as small apps of their
// own (parts.rs), and the way back to the full app from one.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";

type PartView = { id: string; name: string; can_split: boolean; on: boolean; why: string };

export function PartsSettings({ toast }: { toast: PushToast }) {
  const [parts, setParts] = useState<PartView[]>([]);
  useEffect(() => {
    invoke<PartView[]>("parts_view").then(setParts).catch(() => setParts([]));
  }, []);
  return (
    <div className="social-parts">
      <h4>{t("Run on its own")}</h4>
      <p className="small muted">{t("A part that runs on its own gets its own Start menu entry and window, and opens without loading models, mail or media. Turning it off puts it back in the main app.")}</p>
      {parts.map((p) => (
        <div key={p.id} className="small">
          {p.can_split ? (
            <label className="row">
              <input
                type="checkbox"
                checked={p.on}
                onChange={(e) =>
                  invoke<PartView[]>("parts_set", { id: p.id, on: e.target.checked })
                    .then((x) => {
                      setParts(x);
                      toast(e.target.checked ? t("Added \"SulcusAI Social\" to the Start menu. Pin it to the taskbar from there.") : t("Removed the Start menu entry; it's back in the main app."));
                    })
                    .catch((err) => toast(errorText(err)))
                }
              />
              <strong>{t(p.name)}</strong>
            </label>
          ) : (
            <strong>{t(p.name)}</strong>
          )}
          <div className="muted">{t(p.why)}</div>
        </div>
      ))}
    </div>
  );
}

/** In the light social app: a button that brings in the rest of SulcusAI. */
export function OpenFullApp() {
  const [light, setLight] = useState(false);
  useEffect(() => {
    invoke<boolean>("parts_light").then(setLight).catch(() => setLight(false));
  }, []);
  if (!light) return null;
  return (
    <button
      className="social-pill"
      title={t("Open the full SulcusAI app")}
      onClick={() =>
        invoke("parts_open_full")
          .then(() => setLight(false))
          .catch(() => {})
      }
    >
      <svg width="18" height="18" viewBox="0 0 20 20" aria-hidden>
        <rect x="3" y="4" width="14" height="12" rx="2" fill="none" stroke="currentColor" strokeWidth="1.6" />
        <path d="M3 8h14" stroke="currentColor" strokeWidth="1.6" />
      </svg>
    </button>
  );
}
