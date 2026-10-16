// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type HandoffsView } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Settings → Handoffs between chats: notes chats left for picking up their
 * work, and which other chats were let in. */
export function HandoffSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<HandoffsView | null>(null);
  useEffect(() => {
    api.handoffsView().then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const run = async (f: () => Promise<HandoffsView>) => {
    try {
      setV(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const name = (p: string) => p.split(/[\\/]/).pop() ?? p;
  return (
    <section className="card">
      <h2>{t("Handoffs between chats")}</h2>
      <p className="muted small">
        {v.together
          ? t("“Chats work together” is on (Features), so chats can read each other's notes and handoffs freely.")
          : t("A chat sometimes leaves a note for picking up its work later (like HANDOFF.md). While “Chats work together” is off, other chats don't read it unless you say so: ask in your message (“pick up where the other chat left off”) or allow it on the card that comes up.")}
      </p>
      {v.notes.length === 0 ? (
        <p className="muted small">{t("No handoff notes yet.")}</p>
      ) : (
        <ul className="teach-list small">
          {v.notes.map((n) => (
            <li key={n.key}>
              <span className="ellipsis">
                <strong title={n.path}>{name(n.path)}</strong>{" "}
                <span className="muted">· {n.by ? t("left by “{chat}”", { chat: n.by }) : t("left before this was kept")} · {new Date(n.at).toLocaleDateString()}</span>
                <span className="muted block ellipsis">
                  {n.allowed.length > 0 ? t("Also allowed: {chats}", { chats: n.allowed.map((c) => `“${c}”`).join(", ") }) : t("No other chat may read it.")}
                </span>
              </span>
              <span className="row">
                {n.allowed.length > 0 && <button className="btn ghost small" onClick={() => run(() => api.handoffRevoke(n.key))}>{t("Take back")}</button>}
                <button className="btn ghost small" onClick={() => run(() => api.handoffForget(n.key))}>{t("Forget")}</button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
