// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type NewPhone, type PhoneView } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Settings → Phone: chat, tasks and notes from a phone on the same Wi-Fi. */
export function PhoneSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PhoneView | null>(null);
  const [adding, setAdding] = useState<{ name: string; password: string; again: string; result: NewPhone | null } | null>(null);

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
            {v.enabled
              ? v.running
                ? v.while_locked && v.keep_working
                  ? v.address
                    ? t("On at {address}. SulcusAI needs to stay open here.", { address: v.address })
                    : t("On. SulcusAI needs to stay open here.")
                  : v.address
                    ? t("On at {address}. SulcusAI needs to stay open (and unlocked) here.", { address: v.address })
                    : t("On. SulcusAI needs to stay open (and unlocked) here.")
                : v.problem ?? t("Starting…")
              : t("Off.")}
          </span>
        </div>
        <label className="switch" title={v.enabled ? t("On") : t("Off")}>
          <input type="checkbox" checked={v.enabled} onChange={(e) => run(() => api.setPhone(e.target.checked))} aria-label={t("Phone page")} />
          <span />
        </label>
      </div>
      <div className="toggle-row">
        <div>
          <strong>{t("Paired phones work while it's locked")}</strong>
          <span className="small muted block">
            {v.keep_working
              ? t("Phones you've paired can still read and send chats, tasks and notes while SulcusAI is locked on this PC. Other phones and browsers still can't.")
              : t("Phones you've paired can still read and send chats, tasks and notes while SulcusAI is locked on this PC. This needs Keep working while locked (Settings → Security); with a full lock, nothing can be read until you unlock.")}
          </span>
        </div>
        <label className="switch" title={v.while_locked ? t("On") : t("Off")}>
          <input type="checkbox" checked={v.while_locked} onChange={(e) => run(() => api.setPhoneWhileLocked(e.target.checked))} aria-label={t("Paired phones work while it's locked")} />
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
                {!p.has_password && <span className="warn-text block">{t("Paired before phones had passwords, so it no longer works. Pair it again to set one.")}</span>}
              </span>
              <button className="btn ghost small" onClick={() => run(() => api.removePhone(p.id))}>
                {t("Remove")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {v.enabled && v.running && (
        <button className="btn" onClick={() => setAdding({ name: "", password: "", again: "", result: null })}>
          {v.phones.length > 0 ? t("Pair a different phone…") : t("Add a phone…")}
        </button>
      )}
      {v.enabled && <p className="muted small">{t("Only one phone can be paired, and it asks for its password each time the page is opened. Pairing another phone unpairs this one.")}</p>}
      {v.enabled && <p className="muted small">{t("The first time, Windows may ask whether SulcusAI may use private networks: allow it, or the phone can't reach this PC.")}</p>}
      {adding && (
        <Modal title={t("Add a phone")} onClose={() => setAdding(null)}>
          {!adding.result ? (
            <div className="form">
              <label>
                {t("Name it")}
                <input className="input" value={adding.name} autoFocus placeholder={t("e.g. My iPhone")} aria-label={t("e.g. My iPhone")} maxLength={40} onChange={(e) => setAdding({ ...adding, name: e.target.value })} />
              </label>
              <label>
                {t("A password for this phone")}
                <input className="input" type="password" autoComplete="new-password" value={adding.password} onChange={(e) => setAdding({ ...adding, password: e.target.value })} />
                <span className="muted small">{t("At least 6 characters. The phone asks for it each time you open the page, so a lost phone or a copied link isn't enough. It isn't your PC's PIN; pick a different one.")}</span>
              </label>
              <label>
                {t("Type it again")}
                <input className="input" type="password" autoComplete="new-password" value={adding.again} onChange={(e) => setAdding({ ...adding, again: e.target.value })} />
              </label>
              {v.phones.length > 0 && <p className="warn-text small">{t("{name} will be unpaired: only one phone can be paired.", { name: v.phones[0].name })}</p>}
              <div className="modal-actions">
                <button
                  className="btn primary"
                  disabled={adding.password.length < 6 || adding.password !== adding.again}
                  onClick={async () => {
                    try {
                      const r = await api.addPhone(adding.name, adding.password);
                      setV(r.view);
                      setAdding({ ...adding, password: "", again: "", result: r });
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
              <p className="muted small">{t("The page reaches this PC over your network without HTTPS. Its messages are encrypted, but on a network you don't trust, someone could change the page itself and catch the password. Use it on your home or office Wi-Fi.")}</p>
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
