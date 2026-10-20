// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText, type Chat, type SiteKeyAsk } from "../api";
import { useWaitingApprovals } from "../approvals";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** A site the assistant is building needs a data source that asks for an
 * API key: a popup, wherever the person is in the app, saying what the site
 * would connect to and what it needs. They paste the key (or keep one saved
 * earlier) and connect it, or say no; the reply carries on either way. */
export function KeyPermission({ chats, onOpenChat }: { chats: Chat[]; onOpenChat: (id: string) => void }) {
  const [info, setInfo] = useState<Record<string, SiteKeyAsk>>({});
  const [asks, setAsks] = useWaitingApprovals(
    chats,
    (p) => p.tool === "request_api_key",
    (p) => api.siteKeyAsk(p.call_id).then((a) => setInfo((all) => ({ ...all, [p.call_id]: a }))).catch(() => {}),
  );
  const [key, setKey] = useState("");
  const [replace, setReplace] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const p = asks[0];
  if (!p) return null;
  const a = info[p.call_id];
  const chat = chats.find((c) => c.id === p.chat_id);
  const platform = a?.platform ?? p.preview.note ?? "";
  const host = a?.host ?? p.preview.note ?? "";
  const typing = !a?.saved || replace;
  const close = () => {
    setKey("");
    setReplace(false);
    setError(null);
    setAsks((all) => all.filter((x) => x.call_id !== p.call_id));
  };
  const deny = async () => {
    close();
    await api.answerApproval(p.call_id, "deny").catch(() => {});
  };
  const connect = async () => {
    if (typing) {
      try {
        await api.siteKeyGive(p.call_id, key);
      } catch (e) {
        setError(errorText(e));
        return;
      }
    }
    close();
    await api.answerApproval(p.call_id, "allow").catch(() => {});
  };
  return (
    <Modal title={t("Connect {platform}?", { platform })} onClose={deny}>
      <p>
        {chat
          ? t("What you asked for in “{title}” needs to connect to an outside service:", { title: chat.title })
          : t("What you asked for needs to connect to an outside service:")}
      </p>
      <p className="web-ask-reason">
        {t("{platform} ({host}), for {reason}", { platform, host, reason: a?.reason ?? p.preview.detail ?? "" })}
      </p>
      <p>{t("It needs an API key: a code from your {platform} account that lets the site get its data.", { platform })}</p>
      {a?.signup && (
        <p>
          <a href={a.signup} target="_blank" rel="noreferrer">
            {t("Get a key from {platform} ↗", { platform })}
          </a>
        </p>
      )}
      {a?.saved && !replace ? (
        <div className="folder-ask-path">
          <span>{t("Use the key you saved for {host}", { host })}</span>
          <button className="btn small" onClick={() => setReplace(true)}>
            {t("Use another key…")}
          </button>
        </div>
      ) : (
        <label>
          {t("{platform} API key", { platform })}
          <input
            className="input"
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && key.trim()) connect();
            }}
            placeholder={t("Paste your key")} aria-label={t("Paste your key")}
            autoComplete="off"
            autoFocus
          />
        </label>
      )}
      {error && <p className="error-text">{error}</p>}
      <p className="muted small">
        {t("SulcusAI keeps the key on this PC, encrypted, and adds it only to this chat's site requests to {host}. The assistant never sees it, and it isn't written into the site's files. You can remove it in Settings → Connectivity.", { host })}
      </p>
      <div className="modal-actions">
        {chat && (
          <button className="btn ghost" onClick={() => onOpenChat(p.chat_id)}>
            {t("Show the chat")}
          </button>
        )}
        <span className="spacer" />
        <button className="btn" onClick={deny}>{t("Not now")}</button>
        <button className="btn primary" disabled={typing && !key.trim()} onClick={connect}>
          {t("Connect")}
        </button>
      </div>
    </Modal>
  );
}
