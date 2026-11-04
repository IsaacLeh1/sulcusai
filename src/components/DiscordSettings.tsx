// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type DiscordView } from "../api";
import { t } from "../i18n";
import { everyWhileVisible } from "../visible";
import type { PushToast } from "./Toasts";

/** Settings → Phone: texting the assistant from Discord, through a bot only the person can use. */
export function DiscordSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<DiscordView | null>(null);
  const [token, setToken] = useState("");
  const [working, setWorking] = useState(false);

  useEffect(() => {
    api.discordView().then(setV).catch(() => {});
  }, []);
  // Waiting for the code: the link shows once the person sends it.
  useEffect(() => {
    if (!v?.connected || v.linked_to !== null) return;
    return everyWhileVisible(() => api.discordView().then(setV).catch(() => {}), 3000);
  }, [v?.connected, v?.linked_to]);
  if (!v) return null;

  const connect = async () => {
    setWorking(true);
    try {
      setV(await api.discordConnect(token));
      setToken("");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWorking(false);
    }
  };

  return (
    <section className="card">
      <h2>{t("Text your assistant on Discord")}</h2>
      <p className="muted small">
        {t("Write to your assistant in Discord direct messages, by text, picture or voice message, and get its answers and the pictures it makes. It answers with the models on this PC, so SulcusAI needs to stay open and unlocked here. When something needs your OK, reply Allow or Deny.")}{" "}
        {t("Discord's servers see these messages, so keep private things to the app itself.")}
      </p>
      {!v.connected && (
        <>
          <ol className="small">
            <li>{t("In the Discord developer portal, make a New Application and open Bot.")}</li>
            <li>{t("Press Reset Token, copy the token and paste it here.")}</li>
          </ol>
          <div className="row">
            <input
              className="input"
              style={{ flex: 1 }}
              type="password"
              autoComplete="off"
              placeholder={t("Bot token")}
              aria-label={t("Bot token")}
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <button className="btn primary" disabled={working || !token.trim()} onClick={connect}>
              {working ? t("Checking…") : t("Connect")}
            </button>
          </div>
        </>
      )}
      {v.connected && v.linked_to === null && v.code && (
        <p>
          {t("Add {bot} to a server of yours, then send it this code in a direct message from your own account. Only that account can use the bot.", { bot: v.bot ?? "" })}{" "}
          {v.invite && (
            <a href={v.invite} target="_blank" rel="noreferrer">
              {t("Add the bot to a server")}
            </a>
          )}{" "}
          <code className="big-code">{v.code}</code>
        </p>
      )}
      {v.connected && v.linked_to !== null && (
        <>
          <p className="small">{t("{bot} answers {name} in direct messages. /new starts a new chat, /stop stops an answer.", { bot: v.bot ?? "", name: v.linked_to || t("you") })}</p>
          <div className="toggle-row">
            <div>
              <strong>{t("Send scheduled task results")}</strong>
              <span className="small muted block">{t("When a scheduled task finishes, its answer comes to Discord too.")}</span>
            </div>
            <label className="switch">
              <input
                type="checkbox"
                checked={v.tasks}
                onChange={(e) => api.discordSetTasks(e.target.checked).then(setV).catch((err) => toast(errorText(err), "error"))}
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
              .discordDisconnect()
              .then(() => api.discordView().then(setV))
              .catch((e) => toast(errorText(e), "error"))
          }
        >
          {t("Disconnect the bot")}
        </button>
      )}
    </section>
  );
}
