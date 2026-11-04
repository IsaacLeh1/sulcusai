// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type MatrixView } from "../api";
import { t } from "../i18n";
import { everyWhileVisible } from "../visible";
import type { PushToast } from "./Toasts";

/** Settings → Phone: texting the assistant from Matrix, through an account only the person can use. */
export function MatrixSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<MatrixView | null>(null);
  const [homeserver, setHomeserver] = useState("");
  const [token, setToken] = useState("");
  const [working, setWorking] = useState(false);

  useEffect(() => {
    api.matrixView().then(setV).catch(() => {});
  }, []);
  useEffect(() => {
    if (!v?.connected || v.linked_to !== null) return;
    return everyWhileVisible(() => api.matrixView().then(setV).catch(() => {}), 3000);
  }, [v?.connected, v?.linked_to]);
  if (!v) return null;

  const connect = async () => {
    setWorking(true);
    try {
      setV(await api.matrixConnect(homeserver, token));
      setToken("");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWorking(false);
    }
  };

  return (
    <section className="card">
      <h2>{t("Text your assistant on Matrix")}</h2>
      <p className="muted small">
        {t("Write to your assistant from Element or another Matrix app, by text, picture or voice message, and get its answers and the pictures it makes. It answers with the models on this PC, so SulcusAI needs to stay open and unlocked here. When something needs your OK, reply Allow or Deny.")}{" "}
        {t("Use a room with encryption off: the assistant's account can't read encrypted rooms, and your homeserver sees these messages.")}
      </p>
      {!v.connected && (
        <>
          <ol className="small">
            <li>{t("Make a separate Matrix account for the assistant, on any homeserver.")}</li>
            <li>{t("Signed in as that account in Element, copy its access token (Settings › Help & About) and paste it here with the homeserver.")}</li>
          </ol>
          <div className="row wrap">
            <input
              className="input"
              placeholder={t("Homeserver, like matrix.org")}
              aria-label={t("Homeserver")}
              value={homeserver}
              onChange={(e) => setHomeserver(e.target.value)}
            />
            <input
              className="input"
              style={{ flex: 1 }}
              type="password"
              autoComplete="off"
              placeholder={t("Access token")}
              aria-label={t("Access token")}
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <button className="btn primary" disabled={working || !token.trim() || !homeserver.trim()} onClick={connect}>
              {working ? t("Checking…") : t("Connect")}
            </button>
          </div>
        </>
      )}
      {v.connected && v.linked_to === null && v.code && (
        <p>
          {t("Now, from your own account, invite {account} to a room without encryption and send this code there. Only that account, in that room, is answered.", { account: v.account ?? "" })}{" "}
          <code className="big-code">{v.code}</code>
        </p>
      )}
      {v.connected && v.linked_to !== null && (
        <>
          <p className="small">{t("{account} answers {name}. /new starts a new chat, /stop stops an answer.", { account: v.account ?? "", name: v.linked_to })}</p>
          <div className="toggle-row">
            <div>
              <strong>{t("Send scheduled task results")}</strong>
              <span className="small muted block">{t("When a scheduled task finishes, its answer comes to Matrix too.")}</span>
            </div>
            <label className="switch">
              <input
                type="checkbox"
                checked={v.tasks}
                onChange={(e) => api.matrixSetTasks(e.target.checked).then(setV).catch((err) => toast(errorText(err), "error"))}
                aria-label={t("Send scheduled task results")}
              />
              <span />
            </label>
          </div>
        </>
      )}
      {v.problem && <p className="error-text small">{v.problem}</p>}
      {v.connected && (
        <button
          className="btn"
          onClick={() =>
            api
              .matrixDisconnect()
              .then(() => api.matrixView().then(setV))
              .catch((e) => toast(errorText(e), "error"))
          }
        >
          {t("Disconnect Matrix")}
        </button>
      )}
    </section>
  );
}
