// SPDX-License-Identifier: AGPL-3.0-only
// Sign in with Microsoft or Google (for mail and calendars), and the app
// registrations that make it possible.
import { useEffect, useState } from "react";
import { api, errorText, type OAuthClients, type OAuthProvider } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** The two sign-in buttons; `start` does the sign-in and the connecting. */
export function SignInButtons({ start, toast, done }: { start: (p: OAuthProvider) => Promise<unknown>; toast: PushToast; done: (p: OAuthProvider) => void }) {
  const [busy, setBusy] = useState<OAuthProvider | null>(null);
  const go = async (p: OAuthProvider) => {
    setBusy(p);
    try {
      await start(p);
      done(p);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="signin">
      <div className="row wrap">
        <button className="btn signin-btn" disabled={!!busy} onClick={() => go("microsoft")}>
          <span aria-hidden className="signin-logo ms">
            <i /><i /><i /><i />
          </span>
          {t("Sign in with Microsoft")}
        </button>
        <button className="btn signin-btn" disabled={!!busy} onClick={() => go("google")}>
          <span aria-hidden className="signin-logo g">G</span>
          {t("Sign in with Google")}
        </button>
      </div>
      {busy && <p className="small muted">{t("Finish signing in in your browser. This page continues by itself (you have 5 minutes).")}</p>}
      {!busy && <p className="small muted">{t("Opens the sign-in page in your browser; SulcusAI never sees your password.")}</p>}
    </div>
  );
}

/** Settings › Accounts: the client IDs from the Microsoft and Google registrations. */
export function SignInApps({ toast }: { toast: PushToast }) {
  const [ids, setIds] = useState<OAuthClients | null>(null);
  const [saved, setSaved] = useState<OAuthClients | null>(null);
  useEffect(() => {
    api.oauthClients().then((v) => {
      setIds(v);
      setSaved(v);
    }).catch(() => {});
  }, []);
  if (!ids) return null;
  const changed = JSON.stringify(ids) !== JSON.stringify(saved);
  const save = async () => {
    try {
      await api.setOauthClients(ids);
      setSaved(ids);
      toast(t("Saved. Sign in with Microsoft or Google from Mail or Calendar."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <section className="card">
      <h2>{t("Microsoft and Google sign-in")}</h2>
      <p className="muted small">
        {t("Signing in to Outlook.com, Microsoft 365 or Gmail needs this copy of SulcusAI registered with Microsoft and Google. Registered builds have these filled in; otherwise add your own registration's IDs here.")}
      </p>
      <div className="form">
        <label>
          {t("Microsoft application (client) ID")}
          <input className="input" value={ids.microsoft} onChange={(e) => setIds({ ...ids, microsoft: e.target.value })} placeholder="00000000-0000-0000-0000-000000000000" spellCheck={false} />
          <span className="small muted">
            {t("Azure portal › App registrations › New registration: any account type (\"personal Microsoft accounts\" included); platform \"Mobile and desktop applications\" with redirect")}{" "}
            <code>http://localhost</code>; {t("API permissions IMAP.AccessAsUser.All and SMTP.Send (Office 365 Exchange Online) and Calendars.ReadWrite (Microsoft Graph).")}
          </span>
        </label>
        <label>
          {t("Google OAuth client ID")}
          <input className="input" value={ids.google} onChange={(e) => setIds({ ...ids, google: e.target.value })} placeholder="….apps.googleusercontent.com" spellCheck={false} />
        </label>
        <label>
          {t("Google client secret")}
          <input className="input" value={ids.google_secret} onChange={(e) => setIds({ ...ids, google_secret: e.target.value })} spellCheck={false} />
          <span className="small muted">
            {t("Google Cloud console › APIs & Services › Credentials › Create OAuth client ID › Desktop app. Enable the Gmail and Google Calendar APIs, and add the scopes https://mail.google.com/ and …/auth/calendar on the consent screen. (A desktop app's \"secret\" isn't secret.)")}
          </span>
        </label>
        <div className="row">
          <button className="btn primary" disabled={!changed} onClick={save}>{t("Save")}</button>
        </div>
      </div>
    </section>
  );
}
