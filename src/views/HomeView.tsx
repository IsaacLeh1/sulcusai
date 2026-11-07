// SPDX-License-Identifier: AGPL-3.0-only
import type { Chat, InstallProgress } from "../api";
import { t } from "../i18n";
import type { View } from "../App";
import { EmptyState, JobProgress, TimeAgo } from "../components/UiKit";

/** The first page: what's recent, what's waiting and what's running, in one look. */
export function HomeView({ chats, progress, peopleUnread, openChat, newChat, setView }: { chats: Chat[]; progress: Record<string, InstallProgress>; peopleUnread: number; openChat: (id: string) => void; newChat: () => void; setView: (v: View) => void }) {
  const recent = [...chats].sort((a, b) => b.updated_at - a.updated_at).slice(0, 8);
  const jobs = Object.values(progress);
  return (
    <div className="page ui-home">
      <h1>{t("Home")}</h1>
      <div className="ui-home-grid">
        <section className="card">
          <h3>{t("Recent chats")}</h3>
          {recent.length === 0 ? (
            <EmptyState title={t("No chats yet")} text={t("Ask anything; it runs on this PC.")} action={{ label: t("New chat"), run: newChat }} />
          ) : (
            <ul className="ui-list">
              {recent.map((c) => (
                <li key={c.id}>
                  <button className="ui-row" onClick={() => openChat(c.id)}>
                    <span className="ellipsis">{c.title || t("New chat")}</span>
                    <span className="muted small">
                      <TimeAgo when={c.updated_at} />
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section className="card">
          <h3>{t("Groups and people")}</h3>
          {peopleUnread > 0 ? (
            <button className="ui-row" onClick={() => setView("people")}>
              <span>{t("{n} unread", { n: peopleUnread })}</span>
              <span className="muted small">{t("Open Social")}</span>
            </button>
          ) : (
            <p className="muted">{t("You're all caught up.")}</p>
          )}
        </section>
        <section className="card">
          <h3>{t("Running now")}</h3>
          {jobs.length === 0 ? (
            <p className="muted">{t("Nothing is running.")}</p>
          ) : (
            jobs.map((j) => <JobProgress key={j.model_id} label={j.model_id} fraction={j.total ? j.received / j.total : null} />)
          )}
        </section>
        <section className="card">
          <h3>{t("Ideas")}</h3>
          <ul className="ui-list">
            <li>
              <button className="ui-row" onClick={() => setView("studio")}>{t("Make a picture, song or video in Create")}</button>
            </li>
            <li>
              <button className="ui-row" onClick={() => setView("networks")}>{t("Turn on Networks to play LAN games with friends")}</button>
            </li>
            <li>
              <button className="ui-row" onClick={() => setView("library")}>{t("Find any file or note in Library")}</button>
            </li>
          </ul>
        </section>
      </div>
    </div>
  );
}
