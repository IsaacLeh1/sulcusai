// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type Chat, type Memory } from "../api";
import { Markdown } from "./Markdown";
import { t } from "../i18n";
import { MemoryList } from "./MemoryList";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** A chat's memories: what was saved from it (by the assistant, or added
 * here), to read, edit, delete or add to. */
export function ChatMemories({ chat, onClose, toast }: { chat: Chat; onClose: () => void; toast: PushToast }) {
  const [list, setList] = useState<Memory[] | null>(null);
  const load = () =>
    api
      .memories()
      .then((all) => setList(all.filter((m) => m.source_chat === chat.id)))
      .catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
  }, [chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <Modal title={t("Memories from this chat")} onClose={onClose}>
      <p className="muted small">{t("What the assistant remembered from this chat, and what you added here. Other chats recall them when they matter. Everything it remembers is on the Memory page.")}</p>
      {list && <MemoryList memories={list} add={{ projectId: chat.project_id, chatId: chat.id }} empty={t("Nothing remembered from this chat yet.")} toast={toast} onChanged={load} />}
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Close")}</button>
      </div>
    </Modal>
  );
}

/** Chats linked with this one to work together: each one's assistant knows
 * the others, reads what they've done and sends them a question or a piece
 * of work (`message_chat`), whose answer comes back. */
export function LinkedChats({ chat, onOpenChat, onClose, toast }: { chat: Chat; onOpenChat: (id: string) => void; onClose: () => void; toast: PushToast }) {
  const [linked, setLinked] = useState<Chat[]>([]);
  const [all, setAll] = useState<Chat[]>([]);
  const [pick, setPick] = useState("");
  const load = () => {
    api.linkedChats(chat.id).then(setLinked).catch((e) => toast(errorText(e), "error"));
    api.chats().then(setAll).catch(() => {});
  };
  useEffect(load, [chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const run = (f: () => Promise<unknown>) =>
    f()
      .then(load)
      .catch((e) => toast(errorText(e), "error"));
  const choices = all.filter((c) => c.id !== chat.id && !c.incognito && !linked.some((l) => l.id === c.id));
  return (
    <Modal title={t("Connected chats")} onClose={onClose}>
      <p className="muted small">{t("Connected chats work together: each one's assistant knows the others, can read what they've done, and can send them a question or a piece of work. The answer comes back to the chat that asked.")}</p>
      {linked.length === 0 ? (
        <p className="muted small">{t("No chats connected yet.")}</p>
      ) : (
        <ul className="linked-list">
          {linked.map((c) => (
            <li key={c.id}>
              <button
                className="link"
                onClick={() => {
                  onClose();
                  onOpenChat(c.id);
                }}
              >
                {c.title}
              </button>
              <button className="link danger" onClick={() => run(() => api.unlinkChats(chat.id, c.id))}>{t("Disconnect")}</button>
            </li>
          ))}
        </ul>
      )}
      <div className="row wrap">
        <select className="input" value={pick} onChange={(e) => setPick(e.target.value)} aria-label={t("Chat to connect")}>
          <option value="">{t("Pick a chat to connect…")}</option>
          {choices.map((c) => (
            <option key={c.id} value={c.id}>{c.title}</option>
          ))}
        </select>
        <button
          className="btn primary"
          disabled={!pick}
          onClick={() =>
            run(async () => {
              await api.linkChats(chat.id, pick);
              setPick("");
            })
          }
        >
          {t("Connect")}
        </button>
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Close")}</button>
      </div>
    </Modal>
  );
}

/** A plan step's line: its indent, whether it's done, and its words. */
const STEP = /^(\s*)[-*] \[( |x|X)\] (.*)$/;

/** A chat's notes: its own memory (what was asked, what's known about the
 * project, the earlier part summarized) and its plan, step by step. The
 * assistant keeps them up to date; the user reads them here, ticks steps
 * and edits them. */
export function ChatNotes({ chat, onClose, toast }: { chat: Chat; onClose: () => void; toast: PushToast }) {
  const [notes, setNotes] = useState<string | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const load = () =>
    api
      .chatNotes(chat.id)
      .then(setNotes)
      .catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    // The assistant changed them while they're open.
    const sub = on("chat:notes", (p) => p.chat_id === chat.id && load());
    return () => {
      sub.then((un) => un());
    };
  }, [chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const save = async (text: string) => {
    try {
      setNotes(await api.setChatNotes(chat.id, text));
      setDraft(null);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  // Ticking a step changes its line and saves.
  const toggle = (i: number) => {
    const lines = (notes ?? "").split("\n");
    const m = lines[i].match(STEP);
    if (!m) return;
    lines[i] = `${m[1]}- [${m[2] === " " ? "x" : " "}] ${m[3]}`;
    save(lines.join("\n"));
  };
  // The notes in pieces: plan steps as checkboxes, the rest as Markdown.
  const blocks: ({ kind: "text"; text: string } | { kind: "step"; i: number; indent: number; done: boolean; text: string })[] = [];
  (notes ?? "").split("\n").forEach((line, i) => {
    const m = line.match(STEP);
    if (m) blocks.push({ kind: "step", i, indent: m[1].length, done: m[2] !== " ", text: m[3] });
    else if (blocks.length && blocks[blocks.length - 1].kind === "text") (blocks[blocks.length - 1] as { text: string }).text += "\n" + line;
    else blocks.push({ kind: "text", text: line });
  });
  const steps = blocks.filter((b) => b.kind === "step") as { done: boolean }[];
  return (
    <Modal title={t("Chat notes")} onClose={onClose}>
      <p className="muted small">
        {t("This chat's own memory: what you asked for and what's left, what it knows about the project, and its plan, which it checks off step by step. The assistant reads them every time and keeps them up to date; when a long chat is summarized, the summary goes here too. You can edit them.")}
      </p>
      {notes === null ? null : draft !== null ? (
        <textarea className="input chat-notes-edit" rows={16} value={draft} onChange={(e) => setDraft(e.target.value)} aria-label={t("Chat notes")} autoFocus />
      ) : notes.trim() === "" ? (
        <p className="muted small">{t("Nothing yet. The assistant adds what matters as you work, or write your own with Edit.")}</p>
      ) : (
        <div className="chat-notes">
          {steps.length > 0 && <p className="small muted">{t("{done} of {n} steps done", { done: steps.filter((x) => x.done).length, n: steps.length })}</p>}
          {blocks.map((b, k) =>
            b.kind === "text" ? (
              b.text.trim() && <Markdown key={k} text={b.text} />
            ) : (
              <label key={k} className={`check plan-step ${b.done ? "done" : ""}`} style={{ marginLeft: `${b.indent * 0.5}em` }}>
                <input type="checkbox" checked={b.done} onChange={() => toggle(b.i)} />
                <span>{b.text}</span>
              </label>
            ),
          )}
        </div>
      )}
      <div className="modal-actions">
        {draft !== null ? (
          <>
            <button className="btn ghost" onClick={() => setDraft(null)}>{t("Cancel")}</button>
            <button className="btn primary" onClick={() => save(draft)}>{t("Save")}</button>
          </>
        ) : (
          <>
            <button className="btn" onClick={() => setDraft(notes ?? "")}>{t("Edit")}</button>
            <button className="btn primary" onClick={onClose}>{t("Close")}</button>
          </>
        )}
      </div>
    </Modal>
  );
}
