// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Chat } from "../api";
import { useWaitingApprovals } from "../approvals";
import { confirmFolder } from "../folders";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** The assistant needs a folder it can't reach: a popup, wherever the person
 * is in the app. They can allow the folder it suggests, pick another, or say
 * no (closing it says no too); the reply carries on either way. */
export function FolderPermission({ chats, onOpenChat }: { chats: Chat[]; onOpenChat: (id: string) => void }) {
  const [asks, setAsks] = useWaitingApprovals(chats, (p) => p.tool === "request_folder");
  // The folder each request will share, once the person picked another.
  const [picked, setPicked] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const p = asks[0];
  if (!p) return null;
  const chat = chats.find((c) => c.id === p.chat_id);
  const folder = picked[p.call_id] ?? p.preview.note ?? "";
  const answer = async (d: "allow" | "deny") => {
    if (d === "allow") {
      const ok = await confirmFolder(folder).catch(() => ({ go: true, system: false }));
      if (!ok.go) return;
    }
    setError(null);
    setAsks((all) => all.filter((a) => a.call_id !== p.call_id));
    await api.answerApproval(p.call_id, d).catch(() => {});
  };
  const pickAnother = async () => {
    const dir = await open({ directory: true, multiple: false, defaultPath: folder || undefined, title: t("Pick a folder for the assistant") });
    if (typeof dir !== "string") return;
    try {
      await api.chooseFolder(p.call_id, dir);
      setPicked((all) => ({ ...all, [p.call_id]: dir }));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  };
  return (
    <Modal title={t("Let the assistant use a folder?")} onClose={() => answer("deny")}>
      <p>
        {chat ? t("To work on this in “{title}”, the assistant wants a folder:", { title: chat.title }) : t("The assistant wants a folder to work on this:")}
      </p>
      {p.preview.detail && <p className="web-ask-reason">{p.preview.detail}</p>}
      <div className="folder-ask-path">
        <span className="ellipsis" title={folder}>
          📁 {folder}
        </span>
        <button className="btn small" onClick={pickAnother}>
          {t("Pick another…")}
        </button>
      </div>
      {error && <p className="error-text">{error}</p>}
      <p className="muted small">
        {t("If the folder doesn't exist yet, it's made for you. The assistant can open, change and run things in it, in any chat, until you stop sharing it (📁 under the message box). Changes can be undone from the chat.")}
      </p>
      <div className="modal-actions">
        {chat && (
          <button className="btn ghost" onClick={() => onOpenChat(p.chat_id)}>
            {t("Show the chat")}
          </button>
        )}
        <span className="spacer" />
        <button className="btn" onClick={() => answer("deny")}>{t("Not now")}</button>
        <button className="btn primary" onClick={() => answer("allow")} autoFocus>
          {t("Allow this folder")}
        </button>
      </div>
    </Modal>
  );
}
