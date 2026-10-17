// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type DevModeView } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Settings → About: developer mode, which sends reports to the developer
 * from this PC. Turning it on shows exactly what it sends, and needs a yes. */
export function DevModeSetting({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<DevModeView | null>(null);
  const [asking, setAsking] = useState(false);
  const [understood, setUnderstood] = useState(false);
  const [sending, setSending] = useState(false);
  useEffect(() => {
    api.devModeView().then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const set = async (on: boolean) => {
    try {
      setV(await api.setDevMode(on, on && understood));
      setAsking(false);
      toast(on ? t("Developer mode is on. Thank you for helping improve SulcusAI.") : t("Developer mode is off. Nothing more is sent."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="dev-mode">
      <label className="check">
        <input
          type="checkbox"
          checked={v.on}
          onChange={(e) => {
            if (e.target.checked) {
              setUnderstood(false);
              setAsking(true);
            } else {
              set(false);
            }
          }}
        />
        <span>
          {t("Developer mode")}
          {v.on && <span className="badge subtle">{t("On")}</span>}
          <span className="muted small block">
            {v.on
              ? t("Sending reports to the developer to improve SulcusAI's engine: {n} sent so far{last}. Everything sent is in Settings → Logs (dev-sent.log).", {
                  n: v.reports,
                  last: v.last_sent ? t(", the last {when}", { when: new Date(v.last_sent).toLocaleString() }) : "",
                })
              : t("Sends reports from this PC to SulcusAI's developer, to help improve its engine. Off unless you turn it on.")}
          </span>
        </span>
      </label>
      {v.on && (
        <button
          className="btn ghost small"
          disabled={sending}
          onClick={async () => {
            setSending(true);
            try {
              setV(await api.sendDevReport());
              toast(t("Report sent."), "success");
            } catch (e) {
              toast(errorText(e), "error");
            } finally {
              setSending(false);
            }
          }}
        >
          {sending ? t("Sending…") : t("Send a report now")}
        </button>
      )}
      {asking && (
        <Modal title={t("Turn on developer mode?")} onClose={() => setAsking(false)}>
          <p>{t("Developer mode sends reports from this PC to SulcusAI's developer, who uses them to improve the app's engine: why pictures or models fail, how fast they run on different PCs, and what crashes.")}</p>
          <p><strong>{t("What it sends")}</strong></p>
          <ul className="small">
            <li>{t("This PC's setup: the SulcusAI version, Windows version, processor, memory and graphics cards, and a short random id to tell reports from this PC apart.")}</li>
            <li>{t("Crashes and freezes, as they happen, with where in the app they happened.")}</li>
            <li>{t("Every 30 minutes (and soon after an error), new lines from the app's log about the engine: models loaded, pictures, video and music made (which model, how long, on which card), downloads, updates and errors.")}</li>
          </ul>
          <p><strong>{t("What it never sends")}</strong></p>
          <ul className="small">
            <li>{t("What you say in chats, your prompts and descriptions, notes, mail, calendar, contacts, files or their contents.")}</li>
            <li>{t("Your Windows user name and folder (they're taken out), passwords, keys or settings about people and phones.")}</li>
          </ul>
          <p className="small muted">{t("Reports go only to the developer's own inbox. A copy of each one stays on this PC in Settings → Logs (dev-sent.log), so you can see exactly what was sent. Turn developer mode off at any time and nothing more is sent.")}</p>
          <label className="check">
            <input type="checkbox" checked={understood} onChange={(e) => setUnderstood(e.target.checked)} />
            <span>{t("I understand what's sent and agree that the developer uses it to improve SulcusAI.")}</span>
          </label>
          <div className="modal-actions">
            <button className="btn" onClick={() => setAsking(false)}>{t("Cancel")}</button>
            <button className="btn primary" disabled={!understood} onClick={() => set(true)}>{t("Turn on developer mode")}</button>
          </div>
        </Modal>
      )}
    </div>
  );
}
