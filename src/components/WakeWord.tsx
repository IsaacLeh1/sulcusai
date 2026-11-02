// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type WakeView } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Settings → Voice: say a phrase to bring SulcusAI up, talking. */
export function WakeWord({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<WakeView | null>(null);
  const [phrase, setPhrase] = useState("");
  useEffect(() => {
    api
      .wakeWord()
      .then((w) => {
        setV(w);
        setPhrase(w.phrase);
      })
      .catch(() => {});
  }, []);
  if (!v) return null;
  const save = (on: boolean) => api.setWakeWord(on, phrase).then(setV).catch((e) => toast(errorText(e), "error"));

  return (
    <section className="card">
      <h2>{t("Wake word")}</h2>
      <p className="muted small">
        {t("Say a phrase and SulcusAI comes up with a new chat, listening, so you can ask without touching anything. While it's on, the microphone is listened to here on this PC and a speech model stays loaded; nothing heard is kept or sent anywhere.")}
      </p>
      <div className="toggle-row">
        <div>
          <strong>{t("Listen for the wake word")}</strong>
          <span className="small muted block">
            {v.on ? (v.problem ? v.problem : v.listening ? t("Listening for “{phrase}”.", { phrase: v.phrase }) : t("Starting…")) : t("Off.")}
          </span>
        </div>
        <label className="switch">
          <input type="checkbox" checked={v.on} onChange={(e) => save(e.target.checked)} aria-label={t("Listen for the wake word")} />
          <span />
        </label>
      </div>
      <label className="form small">
        {t("The phrase")}
        <div className="row">
          <input className="input" style={{ flex: 1 }} value={phrase} maxLength={40} onChange={(e) => setPhrase(e.target.value)} />
          <button className="btn" disabled={phrase.trim() === v.phrase} onClick={() => save(v.on)}>
            {t("Save")}
          </button>
        </div>
      </label>
      <p className="muted small">{t("Two or three words work best, said as a name the speech model can spell.")}</p>
    </section>
  );
}
