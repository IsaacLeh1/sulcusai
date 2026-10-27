// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Bringing chats and memories over from ChatGPT or Claude (import.rs):
 * their export files, or memories pasted one a line. `memoriesOnly` shows
 * just the paste box (the Memory page). */
export function ImportFromOthers({ toast, memoriesOnly }: { toast: PushToast; memoriesOnly?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [pasted, setPasted] = useState("");
  const importFile = async () => {
    const path = await openFile({ multiple: false, filters: [{ name: t("ChatGPT or Claude export"), extensions: ["zip", "json"] }] });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      const r = await api.importChats(path);
      toast(
        r.chats || r.memories
          ? t("Brought {chats} chats ({messages} messages) and {memories} memories over from {from}. They're in the folder “From {from}”.", { chats: r.chats, messages: r.messages, memories: r.memories, from: r.from })
          : t("Nothing new: those chats are already here."),
        "success",
      );
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const importMemories = async () => {
    setBusy(true);
    try {
      const n = await api.importMemories(pasted);
      toast(n ? t("Added {n} memories.", { n }) : t("Those are already in your memories."), "success");
      setPasted("");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const memories = (
    <div className="form">
      <strong>{t("Memories from another assistant")}</strong>
      <span className="small muted">{t("Ask the other assistant to list everything it remembers about you, one item a line, and paste it here.")}</span>
      <textarea className="input" rows={4} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("Lives in Provo, Utah\nPrefers metric units\nWorks on a local AI app")} aria-label={t("Memories from another assistant")} />
      <div className="row">
        <span className="spacer" />
        <button className="btn small" disabled={busy || !pasted.trim()} onClick={importMemories}>{t("Add these memories")}</button>
      </div>
    </div>
  );
  if (memoriesOnly) return <section className="card">{memories}</section>;
  return (
    <section className="card">
      <h2>{t("Bring your chats over")}</h2>
      <p className="muted small">
        {t("From ChatGPT: Settings › Data controls › Export data. From Claude: Settings › Privacy › Export data. Each emails you a zip; choose it here. Every chat comes over with its title, dates and text (pictures and files aren't in the exports), into a folder of its own. Nothing leaves this PC.")}
      </p>
      <div className="row">
        <button className="btn small primary" disabled={busy} onClick={importFile}>{busy ? t("Bringing them over…") : t("Choose the export…")}</button>
      </div>
      {memories}
    </section>
  );
}
