// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type NewPhone, type PhoneView } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Settings → Phone: chat, tasks and notes from a phone on the same Wi-Fi. */
export function PhoneSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PhoneView | null>(null);
  const [adding, setAdding] = useState<{ name: string; result: NewPhone | null } | null>(null);

  useEffect(() => {
    api.phoneView().then(setV).catch(() => {});
  }, []);
  if (!v) return null;

  const run = async (f: () => Promise<PhoneView>) => {
    try {
      setV(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>{t("Phone")}</h2>
      <p className="muted small">
        {t("Use SulcusAI from your phone's browser while it's on the same Wi-Fi as this PC: chat (it runs here, on this PC's models), add and tick off tasks, read notes, and answer the assistant's questions.")}{" "}
        {t("Nothing is installed on the phone, and everything sent between them is encrypted with a key only the two share. Use it on a home or office network you trust.")}
      </p>
      <div className="toggle-row">
        <div>
          <strong>{t("Phone page")}</strong>
          <span className="small muted block">
            {v.enabled ? (v.running ? (v.address ? t("On at {address}. SulcusAI needs to stay open (and unlocked) here.", { address: v.address }) : t("On. SulcusAI needs to stay open (and unlocked) here.")) : v.problem ?? t("Starting…")) : t("Off.")}
          </span>
        </div>
        <label className="switch" title={v.enabled ? t("On") : t("Off")}>
          <input type="checkbox" checked={v.enabled} onChange={(e) => run(() => api.setPhone(e.target.checked))} aria-label={t("Phone page")} />
          <span />
        </label>
      </div>
      {v.phones.length > 0 && (
        <ul className="teach-list small">
          {v.phones.map((p) => (
            <li key={p.id}>
              <span>
                <strong>{p.name}</strong>{" "}
                <span className="muted">· {p.last_seen ? t("last used {date}", { date: new Date(p.last_seen).toLocaleString() }) : t("not used yet")}</span>
              </span>
              <button className="btn ghost small" onClick={() => run(() => api.removePhone(p.id))}>
                {t("Remove")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {v.enabled && v.running && (
        <button className="btn" onClick={() => setAdding({ name: "", result: null })}>
          {t("Add a phone…")}
        </button>
      )}
      {v.enabled && <p className="muted small">{t("The first time, Windows may ask whether SulcusAI may use private networks: allow it, or the phone can't reach this PC.")}</p>}
      {adding && (
        <Modal title={t("Add a phone")} onClose={() => setAdding(null)}>
          {!adding.result ? (
            <div className="form">
              <label>
                {t("Name it")}
                <input className="input" value={adding.name} autoFocus placeholder={t("e.g. My iPhone")} maxLength={40} onChange={(e) => setAdding({ ...adding, name: e.target.value })} />
              </label>
              <div className="modal-actions">
                <button
                  className="btn primary"
                  onClick={async () => {
                    try {
                      const r = await api.addPhone(adding.name);
                      setV(r.view);
                      setAdding({ ...adding, result: r });
                    } catch (e) {
                      toast(errorText(e), "error");
                    }
                  }}
                >
                  {t("Show the code")}
                </button>
              </div>
            </div>
          ) : (
            <div className="phone-qr">
              <p className="small">{t("Point the phone's camera at this code and open the link. Then add the page to its home screen to keep it handy.")}</p>
              <div className="qr" dangerouslySetInnerHTML={{ __html: adding.result.qr }} />
              <p className="muted small">{t("This code is the phone's key: don't share a picture of it. It isn't shown again; remove the phone and add it again to get a new one.")}</p>
              <div className="modal-actions">
                <button className="btn" onClick={() => setAdding(null)}>
                  {t("Done")}
                </button>
              </div>
            </div>
          )}
        </Modal>
      )}
    </section>
  );
}
