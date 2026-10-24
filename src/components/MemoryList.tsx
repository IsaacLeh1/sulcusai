// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText, type Memory } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Memories with Edit and Delete, and (with `add`) a box to add one: in a
 * project's page and a chat's Memories panel. `add.chatId` keeps what's
 * added there with that chat's memories. */
export function MemoryList({ memories, add, empty, toast, onChanged }: {
  memories: Memory[];
  add?: { projectId: string | null; chatId: string | null };
  empty: string;
  toast: PushToast;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [adding, setAdding] = useState("");
  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <>
      {add && (
        <form
          className="memory-add"
          onSubmit={(e) => {
            e.preventDefault();
            if (!adding.trim()) return;
            run(async () => {
              await api.addMemory(adding, add.projectId, add.chatId);
              setAdding("");
            });
          }}
        >
          <input className="input" placeholder={t("Add something to remember, e.g. I prefer metric units")} aria-label={t("Add something to remember, e.g. I prefer metric units")} value={adding} onChange={(e) => setAdding(e.target.value)} maxLength={500} />
          <button className="btn primary" type="submit" disabled={!adding.trim()}>{t("Add")}</button>
        </form>
      )}
      {memories.length === 0 ? (
        <p className="muted small">{empty}</p>
      ) : (
        <ul className="memory-list compact">
          {memories.map((m) => (
            <li key={m.id}>
              {editing === m.id ? (
                <form
                  className="memory-edit"
                  onSubmit={(e) => {
                    e.preventDefault();
                    run(async () => {
                      await api.updateMemory(m.id, draft);
                      setEditing(null);
                    });
                  }}
                >
                  <input className="input" value={draft} onChange={(e) => setDraft(e.target.value)} autoFocus maxLength={500} aria-label={t("Memory")} />
                  <button className="btn primary small" type="submit">{t("Save")}</button>
                  <button className="btn ghost small" type="button" onClick={() => setEditing(null)}>{t("Cancel")}</button>
                </form>
              ) : (
                <>
                  <span className="memory-text">{m.content}</span>
                  <button
                    className="link"
                    onClick={() => {
                      setEditing(m.id);
                      setDraft(m.content);
                    }}
                  >
                    {t("Edit")}
                  </button>
                  <button className="link danger" onClick={() => run(() => api.deleteMemory(m.id))}>{t("Delete")}</button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
