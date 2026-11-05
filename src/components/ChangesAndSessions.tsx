import { useEffect, useState } from "react";
import { api, errorText, type ChangeView, type SessionNode } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** "Changes" (every file the assistant touched in this chat, with undo per
 *  batch or reply) and, when this chat started any, "Sessions". */
export function ChangesAndSessions({ chatId, onError }: { chatId: string; onError: (text: string) => void }) {
  const [open, setOpen] = useState<"changes" | "sessions" | null>(null);
  const [sessions, setSessions] = useState<SessionNode[]>([]);
  useEffect(() => {
    api.sessionsTree(chatId).then(setSessions, () => setSessions([]));
  }, [chatId, open]);
  return (
    <>
      <button className="globe" onClick={() => setOpen("changes")} title={t("Every file changed in this chat, with a diff and undo")}>
        {t("Changes")}
      </button>
      {sessions.length > 0 && (
        <button className="globe" onClick={() => setOpen("sessions")} title={t("Sessions started from this chat")}>
          {t("Sessions")}
        </button>
      )}
      {open === "changes" && <ChangesWindow chatId={chatId} onError={onError} onClose={() => setOpen(null)} />}
      {open === "sessions" && <SessionsWindow chatId={chatId} nodes={sessions} onError={onError} onChange={() => api.sessionsTree(chatId).then(setSessions, () => {})} onClose={() => setOpen(null)} />}
    </>
  );
}

function ChangesWindow({ chatId, onError, onClose }: { chatId: string; onError: (text: string) => void; onClose: () => void }) {
  const [rows, setRows] = useState<ChangeView[] | null>(null);
  const load = () => api.chatChanges(chatId).then(setRows, (e) => onError(errorText(e)));
  useEffect(() => {
    load();
  }, [chatId]);
  const groups = new Map<string, ChangeView[]>();
  for (const r of rows ?? []) {
    const key = r.batch ? `b:${r.batch}` : `t:${r.turn_id}`;
    groups.set(key, [...(groups.get(key) ?? []), r]);
  }
  const undo = async (key: string) => {
    try {
      const notes = key.startsWith("b:") ? await api.undoBatch(chatId, key.slice(2)) : await api.undoTurn(chatId, key.slice(2));
      if (notes.length) onError(notes.join(" "));
      load();
    } catch (e) {
      onError(errorText(e));
    }
  };
  return (
    <Modal title={t("Changes made in this chat")} onClose={onClose}>
      {rows && rows.length === 0 && <p>{t("No files have been changed in this chat.")}</p>}
      {[...groups.entries()].map(([key, list]) => (
        <section key={key}>
          <h4>
            {key.startsWith("b:") ? t("Batch") : t("Reply")} {new Date(list[0].created_at).toLocaleString()}
            {list[0].by && <em> ({list[0].by})</em>}{" "}
            {list.some((r) => !r.undone) ? (
              <button className="link" onClick={() => undo(key)}>
                {t("Undo")}
              </button>
            ) : (
              <em>{t("Undone")}</em>
            )}
          </h4>
          {list.map((r) => (
            <details key={r.id}>
              <summary>
                {r.action} {r.path}
                {r.other ? ` to ${r.other}` : ""}
              </summary>
              {r.diff && <pre className="small">{r.diff}</pre>}
            </details>
          ))}
        </section>
      ))}
    </Modal>
  );
}

function SessionsWindow({ chatId, nodes, onError, onChange, onClose }: { chatId: string; nodes: SessionNode[]; onError: (text: string) => void; onChange: () => void; onClose: () => void }) {
  const stopAll = async () => {
    try {
      await api.sessionsStopAll(chatId);
      onChange();
    } catch (e) {
      onError(errorText(e));
    }
  };
  return (
    <Modal title={t("Sessions")} onClose={onClose}>
      <button onClick={stopAll} disabled={!nodes.some((n) => ["queued", "running", "paused"].includes(n.state))}>
        {t("Stop all")}
      </button>
      <ul>
        {nodes.map((n) => (
          <li key={n.id} style={{ marginLeft: `${(n.depth - 1) * 1.5}em` }}>
            <strong>{n.title}</strong> <em>{t(n.state)}</em>
            <div className="small">
              {t("Model")}: {n.model ?? t("same as its parent")} · {t("Effort")}: {n.effort ?? t("default")} · {t("Tokens")}: ~{n.tokens}
              {n.branch && ` · ${n.branch}`}
            </div>
            <div className="small">{t("Why")}: {n.reason}</div>
            {n.report && <details><summary>{t("Report")}</summary><pre className="small">{n.report}</pre></details>}
            {n.log.length > 0 && <details><summary>{t("Log")}</summary><pre className="small">{n.log.join("\n")}</pre></details>}
          </li>
        ))}
      </ul>
    </Modal>
  );
}
