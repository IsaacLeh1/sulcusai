// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useEffect, useState, type ReactNode } from "react";
import { api, type ChatHit } from "../api";
import { t } from "../i18n";

/** The searched-for words marked in an excerpt. */
function marked(text: string, query: string): ReactNode {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean).map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  if (!words.length) return text;
  const parts = text.split(new RegExp(`(${words.join("|")})`, "gi"));
  return parts.map((p, i) => (i % 2 ? <mark key={i}>{p}</mark> : <Fragment key={i}>{p}</Fragment>));
}

/** Chats with every word of `query`, in their names or what was said. */
export function ChatSearchResults({ query, onOpen }: { query: string; onOpen: (chatId: string) => void }) {
  const [hits, setHits] = useState<ChatHit[] | null>(null);
  useEffect(() => {
    let live = true;
    // Typed a little further before searching every chat again.
    const timer = window.setTimeout(() => {
      api
        .searchChats(query)
        .then((h) => live && setHits(h))
        .catch(() => live && setHits([]));
    }, 250);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [query]);
  if (hits === null) return <p className="muted small pad">{t("Searching…")}</p>;
  if (!hits.length) return <p className="muted small pad">{t("No chat has all of those words.")}</p>;
  return (
    <div className="chat-list" role="list" aria-label={t("Chats found")}>
      {hits.map((h) => (
        <button key={h.chat_id} className="chat-item search-hit" role="listitem" onClick={() => onOpen(h.chat_id)}>
          <span className="ellipsis">{marked(h.title, query)}</span>
          {h.snippet && <span className="search-snippet small muted">{marked(h.snippet, query)}</span>}
        </button>
      ))}
    </div>
  );
}
