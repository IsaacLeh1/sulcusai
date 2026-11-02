// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type CloudAccount } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Settings → Google Drive and OneDrive: files the assistant can find and read, where they are. */
export function CloudFiles({ toast }: { toast: PushToast }) {
  const [accounts, setAccounts] = useState<CloudAccount[] | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  useEffect(() => {
    api.cloudFiles().then(setAccounts).catch(() => setAccounts([]));
  }, []);
  if (!accounts) return null;

  const connect = async (provider: "google" | "microsoft") => {
    setWorking(provider);
    try {
      setAccounts(await api.cloudFilesConnect(provider));
      toast(t("Connected. Ask about your files in any chat."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWorking(null);
    }
  };

  return (
    <section className="card">
      <h2>{t("Google Drive and OneDrive")}</h2>
      <p className="muted small">
        {t("Let the assistant find and read your files in Google Drive or OneDrive when you ask (\"what did the lease say about pets?\"). It can only read them, and they stay where they are: a file is read when it's needed and not kept. Sign-in happens in your browser; SulcusAI never sees your password.")}
      </p>
      {accounts.map((a) => (
        <div key={a.id} className="toggle-row">
          <div>
            <strong>{a.provider === "google" ? t("Google Drive") : t("OneDrive")}</strong>
            <span className="small muted block">{a.email}</span>
          </div>
          <button className="btn" onClick={() => api.cloudFilesRemove(a.id).then(setAccounts).catch((e) => toast(errorText(e), "error"))}>
            {t("Disconnect")}
          </button>
        </div>
      ))}
      <div className="row">
        <button className="btn" disabled={working !== null} onClick={() => connect("google")}>
          {working === "google" ? t("Waiting for the browser…") : t("Connect Google Drive")}
        </button>
        <button className="btn" disabled={working !== null} onClick={() => connect("microsoft")}>
          {working === "microsoft" ? t("Waiting for the browser…") : t("Connect OneDrive")}
        </button>
      </div>
    </section>
  );
}
