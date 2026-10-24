// SPDX-License-Identifier: AGPL-3.0-only
// People: link with someone else's SulcusAI by a code and chat with them
// directly. Messages go app to app; they never go to a model.
import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type PeopleView as View, type PersonAllow, type PersonAsk, type PersonContact, type PersonMessage, type SharedChat, type SharedView } from "../api";
import { Markdown } from "../components/Markdown";
import { rememberSent, SmartInput, useTypingHelpers } from "../components/SmartInput";
import { PeopleConnect } from "../components/PeopleConnect";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { bytes } from "../format";
import { t, tx } from "../i18n";
import { PaperclipIcon } from "../components/Icons";
import { ModelSelect } from "../components/ModelChoices";

function seen(ms: number | null): string {
  if (!ms) return t("not seen yet");
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return t("seen just now");
  if (s < 3600) return t("seen {n} min ago", { n: Math.round(s / 60) });
  if (s < 86400) return t("seen {n} h ago", { n: Math.round(s / 3600) });
  return t("seen {date}", { date: new Date(ms).toLocaleDateString() });
}

/** The longest question for someone's assistant (as their app takes it). */
const ASK_MOST = 1000;

const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const day = (ms: number) => new Date(ms).toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });

function Avatar({ name, online }: { name: string; online?: boolean }) {
  return (
    <span className="person-avatar" aria-hidden>
      {(name.trim()[0] ?? "?").toUpperCase()}
      {online !== undefined && <span className={`presence ${online ? "on" : ""}`} />}
    </span>
  );
}

export function PeopleView({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<View | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [naming, setNaming] = useState<string | null>(null);
  // Answers are numbered in the order they were asked for: one that comes
  // back after a newer one (a slow refresh finishing after a change) is
  // dropped, so it can't undo what was just saved.
  const asked = useRef(0);
  const shown = useRef(0);
  const savingName = useRef(false);
  const take = useCallback((n: number, x: View) => {
    if (n < shown.current) return;
    shown.current = n;
    setV(x);
  }, []);
  const apply = useCallback((x: View) => take(++asked.current, x), [take]);

  const load = useCallback(() => {
    const n = ++asked.current;
    api.peopleView().then((x) => take(n, x)).catch((e) => toast(errorText(e), "error"));
  }, [toast, take]);
  useEffect(() => {
    load();
    const subs = [on("people:changed", load), on("people:message", load), on("people:shared", load)];
    // Keeps "online" and who's using the codes current.
    const timer = window.setInterval(load, 5000);
    return () => {
      window.clearInterval(timer);
      subs.forEach((s) => s.then((un) => un()));
    };
  }, [load]);

  // Open the latest conversation at first.
  useEffect(() => {
    if (v && !openId && v.contacts.length > 0) setOpenId(v.contacts[0].id);
    if (v && openId && !v.contacts.some((c) => c.id === openId)) setOpenId(v.contacts[0]?.id ?? null);
  }, [v, openId]);

  if (!v) return null;
  const current = v.contacts.find((c) => c.id === openId) ?? null;

  // Enter saves, and so does leaving the box: only once.
  const saveName = async () => {
    if (savingName.current) return;
    const name = naming?.trim();
    setNaming(null);
    if (!name || name === v.name) return;
    savingName.current = true;
    // Shown at once; refreshes asked for before this can't put the old name back.
    take(++asked.current, { ...v, name });
    try {
      apply(await api.setPeopleName(name));
    } catch (e) {
      toast(errorText(e), "error");
      load();
    } finally {
      savingName.current = false;
    }
  };

  return (
    <div className="people-page">
      <aside className="people-list">
        <div className="row people-head">
          <h2>{t("People")}</h2>
          <button className="btn primary small" onClick={() => setConnecting(true)}>
            {t("+ Connect")}
          </button>
        </div>
        {v.contacts.length === 0 && (
          <p className="muted small">{t("No one yet. To connect with someone, one of you shows a code and the other types it in.")}</p>
        )}
        {v.contacts.map((c) => (
          <button key={c.id} className={`person-item ${openId === c.id ? "active" : ""}`} onClick={() => setOpenId(c.id)}>
            <Avatar name={c.name} online={c.online} />
            <span className="person-text">
              <span className="row">
                <strong className="ellipsis">{c.name}</strong>
                {c.unread > 0 && (
                  <span className="badge" aria-label={t("{n} unread", { n: c.unread })}>
                    {c.unread}
                  </span>
                )}
              </span>
              <span className="small muted ellipsis">
                {c.last ? (c.last.from_me ? t("You: {text}", { text: preview(c.last) }) : preview(c.last)) : c.online ? t("Online") : c.via_relay ? t("Through the relay") : seen(c.last_seen)}
              </span>
            </span>
          </button>
        ))}
        <div className="people-me small muted">
          {naming !== null ? (
            <input
              className="input"
              value={naming}
              autoFocus
              maxLength={60}
              aria-label={t("Your name, as others see it")}
              onChange={(e) => setNaming(e.target.value)}
              onBlur={saveName}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveName();
                if (e.key === "Escape") setNaming(null);
              }}
            />
          ) : (
            <span>
              {t("Others see you as {name}.", { name: v.name })}{" "}
              <button className="link small" onClick={() => setNaming(v.name)}>
                {t("Change")}
              </button>
            </span>
          )}
        </div>
      </aside>
      <section className="people-chat">
        {current ? (
          <Conversation key={current.id} contact={current} relaySet={v.relay_set} cloud={v.cloud} onChanged={load} toast={toast} />
        ) : (
          <div className="people-empty">
            <h2>{t("Chat with people directly")}</h2>
            <p className="muted">
              {t("Connect with someone else's SulcusAI by a code, then send each other messages and files. They go straight between your two PCs, encrypted, and never to a model.")}
            </p>
            <button className="btn primary" onClick={() => setConnecting(true)}>
              {t("Connect with someone")}
            </button>
          </div>
        )}
      </section>
      {connecting && (
        <PeopleConnect
          v={v}
          onView={apply}
          onClose={() => setConnecting(false)}
          onLinked={(id) => {
            setConnecting(false);
            load();
            if (id) setOpenId(id);
          }}
          toast={toast}
        />
      )}
    </div>
  );
}

function preview(m: PersonMessage): string {
  if (m.deleted) return t("Message deleted");
  if (m.text.trim()) return m.text.replace(/\s+/g, " ").slice(0, 80);
  return m.file ? m.file.name : "";
}

/** One conversation: messages, and the box to write in. */
function Conversation({ contact: c, relaySet, cloud, onChanged, toast }: { contact: PersonContact; relaySet: boolean; cloud: boolean; onChanged: () => void; toast: PushToast }) {
  const [messages, setMessages] = useState<PersonMessage[] | null>(null);
  const [text, setText] = useState("");
  const helpers = useTypingHelpers();
  const [file, setFile] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [menu, setMenu] = useState(false);
  const [address, setAddress] = useState<string | null>(null);
  const [tab, setTab] = useState<"messages" | "shared">("messages");
  const [asks, setAsks] = useState<PersonAsk[]>([]);
  const [allowing, setAllowing] = useState(false);
  // "Also ask {name}'s assistant" for the message being written (one message at a time).
  const [alsoAsk, setAlsoAsk] = useState(false);
  // A message of this user's being changed.
  const [editing, setEditing] = useState<{ id: string; text: string } | null>(null);
  const [watching, setWatching] = useState<SharedChat | null>(null);
  // A chat they stopped sharing (or that went with the connection) closes.
  useEffect(() => {
    if (watching && !c.shared.some((x) => x.id === watching.id)) setWatching(null);
  }, [c.shared, watching]);
  const end = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    api.peopleMessages(c.id).then(setMessages).catch(() => {});
    api.peopleAsks(c.id).then(setAsks).catch(() => {});
  }, [c.id]);
  useEffect(() => {
    load();
    const subs = [
      on("people:message", (p) => p.contact_id === c.id && load()),
      on("people:changed", load),
      on("people:ask", (p) => p.contact_id === c.id && api.peopleAsks(c.id).then(setAsks).catch(() => {})),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [load, c.id]);
  // Reading them here clears the unread count.
  useEffect(() => {
    if (messages) onChanged();
  }, [messages?.length]); // eslint-disable-line react-hooks/exhaustive-deps
  // New messages scroll into view (unless the user scrolled up to read).
  const count = (messages?.length ?? 0) + asks.length + asks.reduce((n, a) => n + a.thread.length + (a.answer ? 1 : 0), 0);
  const atBottom = useRef(true);
  useEffect(() => {
    if (atBottom.current) end.current?.scrollIntoView({ block: "end" });
  }, [count]);

  const question = text.trim();
  const tooLong = alsoAsk && question.length > ASK_MOST;
  const send = async () => {
    if (sending || (!text.trim() && !file)) return;
    if (tooLong) return toast(t("Questions for {name}'s assistant can be up to {n} characters: it answers questions about {name}, it doesn't do work.", { name: c.name, n: ASK_MOST.toLocaleString() }), "error");
    setSending(true);
    try {
      const m = await api.peopleSend(c.id, text, file);
      // A refresh may already have brought it.
      setMessages((all) => (all?.some((x) => x.id === m.id) ? all : [...(all ?? []), m]));
      if (alsoAsk && question) {
        try {
          const a = await api.peopleAsk(c.id, question);
          setAsks((all) => (all.some((x) => x.id === a.id) ? all : [...all, a]));
        } catch (e) {
          toast(errorText(e), "error");
        }
      }
      setText("");
      setFile(null);
      setAlsoAsk(false);
      atBottom.current = true;
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSending(false);
    }
  };
  const saveEdit = async () => {
    if (!editing) return;
    try {
      await api.peopleEdit(c.id, editing.id, editing.text);
      setEditing(null);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const deleteMessage = async (m: PersonMessage) => {
    if (!window.confirm(t("Delete this message? It's deleted for {name} too, as soon as your app reaches theirs.", { name: c.name }))) return;
    try {
      await api.peopleDelete(c.id, m.id);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const pick = async () => {
    const path = await openFile({ multiple: false, directory: false });
    if (typeof path === "string") setFile(path);
  };
  const saveFile = async (m: PersonMessage) => {
    if (!m.file) return;
    const to = await save({ defaultPath: m.file.name });
    if (!to) return;
    try {
      await api.peopleSaveFile(c.id, m.id, to);
      toast(t("Saved {name}.", { name: m.file.name }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const remove = async () => {
    setMenu(false);
    if (!window.confirm(t("Remove {name}? Your messages with them are deleted from this PC, and they can't reach you until you connect again.", { name: c.name }))) return;
    try {
      await api.peopleRemove(c.id);
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const saveAddress = async () => {
    if (!address?.trim()) return;
    try {
      await api.peopleSetAddress(c.id, address);
      setAddress(null);
      toast(t("Trying {name} at the new address…", { name: c.name }), "info");
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const waiting = (messages ?? []).filter((m) => m.from_me && !m.delivered).length;
  const fileName = file?.split(/[\\/]/).pop();

  return (
    <>
      <header className="people-chat-head">
        <Avatar name={c.name} online={c.online} />
        <div className="person-text">
          <strong>{c.name}</strong>
          <span className="small muted">
            {c.online ? (c.via_pipe ? t("Online, through SulcusAI's relay (encrypted)") : t("Online")) : c.via_relay ? t("Through the relay") : seen(c.last_seen)}
            {!c.online && !c.via_relay && c.error && <span className="warn-text"> · {t("Can't reach them right now.")}</span>}
          </span>
        </div>
        <div className="seg people-tabs" role="tablist">
          <button role="tab" aria-selected={tab === "messages"} className={tab === "messages" ? "active" : ""} onClick={() => setTab("messages")}>
            {t("Messages")}
          </button>
          <button role="tab" aria-selected={tab === "shared"} className={tab === "shared" ? "active" : ""} onClick={() => setTab("shared")}>
            {t("Shared chats")}
            {c.shared.length > 0 && <span className="badge subtle">{c.shared.length}</span>}
          </button>
        </div>
        <div className="people-menu">
          <button className="icon-btn" aria-label={t("More")} aria-expanded={menu} onClick={() => setMenu((m) => !m)}>
            ⋯
          </button>
          {menu && (
            <div className="menu-pop" role="menu" onKeyDown={(e) => e.key === "Escape" && setMenu(false)}>
              <button
                role="menuitem"
                onClick={() => {
                  setMenu(false);
                  setAllowing(true);
                }}
              >
                {t("Your assistant for {name}…", { name: c.name })}
              </button>
              <button
                role="menuitem"
                onClick={() => {
                  setMenu(false);
                  setAddress(c.address?.replace(/:47816$/, "") ?? "");
                }}
              >
                {t("Change their address")}
              </button>
              <button
                role="menuitem"
                disabled={!relaySet && !c.offer_relay}
                title={!relaySet ? t("Set your relay first, in Settings → Your devices.") : !cloud ? t("Relays work at the Cloud level.") : undefined}
                onClick={async () => {
                  setMenu(false);
                  try {
                    await api.peopleSetRelayOffer(c.id, !c.offer_relay);
                    toast(c.offer_relay ? t("{name} can't use your relay anymore.", { name: c.name }) : t("{name} can now reach you through your relay when you're not connected directly.", { name: c.name }), "success");
                    onChanged();
                  } catch (e) {
                    toast(errorText(e), "error");
                  }
                }}
              >
                {c.offer_relay ? "✓ " : ""}
                {t("Let {name} reach me through my relay", { name: c.name })}
              </button>
              <button
                role="menuitem"
                title={t("They see when you've read their messages. You see theirs only if they turn this on for you.")}
                onClick={async () => {
                  setMenu(false);
                  try {
                    await api.peopleSetReadReceipts(c.id, !c.read_receipts);
                    toast(c.read_receipts ? t("{name} won't see when you've read their messages.", { name: c.name }) : t("{name} will see when you've read their messages.", { name: c.name }), "success");
                    onChanged();
                  } catch (e) {
                    toast(errorText(e), "error");
                  }
                }}
              >
                {c.read_receipts ? "✓ " : ""}
                {t("Let {name} see when I've read their messages", { name: c.name })}
              </button>
              <button role="menuitem" className="danger" onClick={remove}>
                {t("Remove {name}", { name: c.name })}
              </button>
            </div>
          )}
        </div>
      </header>
      {tab === "shared" ? (
        watching ? (
          <SharedChatView key={watching.id} contactId={c.id} contactName={c.name} chat={watching} onBack={() => setWatching(null)} toast={toast} />
        ) : (
          <SharedPanel contact={c} onOpen={setWatching} toast={toast} />
        )
      ) : (
      <>
      <div
        className="people-messages"
        ref={box}
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
        }}
      >
        {messages && messages.length === 0 && (
          <p className="muted small people-note">{t("Say hello. Messages go straight to {name}'s SulcusAI and never to a model.", { name: c.name })}</p>
        )}
        {timeline(messages ?? [], asks).map((item, i, all) => {
          const prev = all[i - 1];
          const newDay = !prev || day(prev.at) !== day(item.at);
          if (item.kind === "ask") {
            // Asked along with a message ("Also ask"): the message above says it.
            const along = prev?.kind === "message" && prev.m.from_me === item.a.from_me && prev.m.text.trim() === item.a.question.trim() && item.at - prev.at < 60_000;
            return (
              <div key={item.a.id}>
                {newDay && <div className="people-day small muted">{day(item.at)}</div>}
                <AskCard a={item.a} contact={c} toast={toast} along={along} />
              </div>
            );
          }
          const m = item.m;
          if (m.deleted) {
            return (
              <div key={m.id}>
                {newDay && <div className="people-day small muted">{day(m.at)}</div>}
                <div className={`pmsg ${m.from_me ? "mine" : ""}`}>
                  <div className="pbubble gone small muted">{m.from_me ? t("You deleted this message") : t("{name} deleted this message", { name: c.name })}</div>
                </div>
              </div>
            );
          }
          if (editing?.id === m.id) {
            return (
              <div key={m.id}>
                {newDay && <div className="people-day small muted">{day(m.at)}</div>}
                <div className="pmsg mine">
                  <div className="pbubble pedit">
                    <textarea
                      className="input"
                      rows={Math.min(8, Math.max(2, editing.text.split("\n").length))}
                      value={editing.text}
                      autoFocus
                      aria-label={t("Change your message")}
                      onChange={(e) => setEditing({ ...editing, text: e.target.value })}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                          e.preventDefault();
                          saveEdit();
                        } else if (e.key === "Escape") setEditing(null);
                      }}
                    />
                    <div className="row">
                      <button className="btn primary small" onClick={saveEdit} disabled={!editing.text.trim() && !m.file}>
                        {t("Save")}
                      </button>
                      <button className="btn ghost small" onClick={() => setEditing(null)}>
                        {t("Cancel")}
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            );
          }
          return (
            <div key={m.id}>
              {newDay && <div className="people-day small muted">{day(m.at)}</div>}
              <div className={`pmsg ${m.from_me ? "mine" : ""}`}>
                <div className="pbubble">
                  {m.text && <span className="ptext">{m.text}</span>}
                  {m.file && (
                    <span className="pfile">
                      <PaperclipIcon />
                      <span className="ellipsis">{m.file.name}</span>
                      <span className="muted small">{bytes(m.file.size)}</span>
                      <button className="btn ghost small" onClick={() => saveFile(m)}>
                        {t("Save…")}
                      </button>
                    </span>
                  )}
                </div>
                <span className="small muted pmeta">
                  {time(m.at)}
                  {m.edited && " · " + t("edited")}
                  {m.from_me && (m.read ? " · ✓✓ " + t("Read") : m.delivered ? " · ✓ " + t("Delivered") : " · " + t("Waiting to send"))}
                  {m.from_me && (
                    <span className="pactions">
                      {" · "}
                      <button className="link small" onClick={() => setEditing({ id: m.id, text: m.text })}>
                        {t("Edit")}
                      </button>
                      {" · "}
                      <button className="link small" onClick={() => deleteMessage(m)}>
                        {t("Delete")}
                      </button>
                    </span>
                  )}
                </span>
              </div>
            </div>
          );
        })}
        <div ref={end} />
      </div>
      {waiting > 0 && !c.online && !c.via_relay && (
        <p className="small muted people-note">
          {t("{name} isn't reachable right now. Your messages wait here and go as soon as either of your apps reaches the other.", { name: c.name })}
        </p>
      )}
      {c.via_relay && (messages ?? []).some((m) => m.from_me && !m.delivered && (m.file?.size ?? 0) > 1_000_000) && (
        <p className="small muted people-note">{t("Files over 1 MB don't go through the relay; they wait until you're connected directly.")}</p>
      )}
      <div className="composer-wrap">
        <div className="composer">
          {fileName && (
            <div className="row small">
              <span className="chip"><PaperclipIcon /> {fileName}</span>
              <button className="link small" onClick={() => setFile(null)}>
                {t("Remove")}
              </button>
            </div>
          )}
          <SmartInput
            rows={2}
            value={text}
            onValueChange={setText}
            complete={helpers.complete}
            correct={helpers.correct}
            placeholder={t("Message {name}", { name: c.name })}
            aria-label={t("Message {name}", { name: c.name })}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />
          <div className="composer-bar">
            <button className="icon-btn" onClick={pick} title={t("Send a file (up to 16 MB)")} aria-label={t("Send a file")}>
              <PaperclipIcon />
            </button>
            <label className="check small" title={t("Their assistant answers on their PC, about them only (like whether they're free), if they allow it. Their model loads only for this.")}>
              <input type="checkbox" checked={alsoAsk} onChange={(e) => setAlsoAsk(e.target.checked)} />
              {t("Also ask {name}'s assistant", { name: c.name })}
            </label>
            <span className={`small ${tooLong ? "warn-text" : "muted"}`}>
              {alsoAsk
                ? tooLong
                  ? t("{n} characters at most for their assistant", { n: ASK_MOST.toLocaleString() })
                  : t("Their assistant answers here, if {name} allows it.", { name: c.name })
                : t("To {name} only. Never sent to a model.", { name: c.name })}
            </span>
            <button className="btn primary small push-right" disabled={sending || (!text.trim() && !file) || tooLong} onClick={send}>
              {t("Send")}
            </button>
          </div>
        </div>
      </div>
      </>
      )}
      {allowing && <AllowModal contact={c} onClose={() => setAllowing(false)} toast={toast} />}
      {address !== null && (
        <Modal title={t("Where to reach {name}", { name: c.name })} onClose={() => setAddress(null)}>
          <p className="muted small">{t("If their address changed, ask them for the one shown under Connect → Show my code, and type it here.")}</p>
          <input
            className="input"
            value={address}
            autoFocus
            placeholder="26.10.4.7"
            onChange={(e) => setAddress(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && saveAddress()}
          />
          <div className="adv-actions">
            <button className="btn primary" disabled={!address.trim()} onClick={saveAddress}>
              {t("Save")}
            </button>
            <button className="btn ghost" onClick={() => setAddress(null)}>
              {t("Cancel")}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

/** What they share with this user (while connected), and what this user shares with them. */
function SharedPanel({ contact: c, onOpen, toast }: { contact: PersonContact; onOpen: (chat: SharedChat) => void; toast: PushToast }) {
  const stop = async (chatId: string) => {
    try {
      const choices = await api.peopleChatSharing(chatId);
      await api.peopleShareChat(chatId, choices.filter((p) => p.shared && p.id !== c.id).map((p) => p.id));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="people-shared">
      <section>
        <h3>{t("{name} shares with you", { name: c.name })}</h3>
        {!c.online ? (
          <p className="muted small">{t("{name} isn't connected right now. Chats they share show here while you're both connected.", { name: c.name })}</p>
        ) : c.shared.length === 0 ? (
          <p className="muted small">{t("Nothing yet. When {name} shares a chat with you, it shows here and you can watch it live.", { name: c.name })}</p>
        ) : (
          <ul className="shared-list">
            {c.shared.map((s) => (
              <li key={s.id}>
                <button className="shared-item" onClick={() => onOpen(s)}>
                  <strong className="ellipsis">{s.title}</strong>
                  <span className="small muted">
                    {s.busy ? <span className="live-dot">{t("Working now")}</span> : t("Updated {time}", { time: new Date(s.updated_at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) })}
                    {s.model && " · " + t("by {model}", { model: s.model })}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section>
        <h3>{t("You share with {name}", { name: c.name })}</h3>
        {c.sharing.length === 0 ? (
          <p className="muted small">{t("Nothing. To share a chat, open it and click Share at the top.")}</p>
        ) : (
          <ul className="shared-list">
            {c.sharing.map((s) => (
              <li key={s.chat_id} className="row">
                <span className="ellipsis">{s.title}</span>
                <button className="btn ghost small push-right" onClick={() => stop(s.chat_id)}>
                  {t("Stop sharing")}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

const LIVE_STATUS: Record<string, string> = {
  loading: tx("Loading the model…"),
  thinking: tx("Thinking…"),
  writing: tx("Writing…"),
  working: tx("Using a tool…"),
};

/** A chat someone shares, live and read-only. */
/** A chat someone shares with this user, live: from People or the sidebar.
 * This user can take part: what they write goes into that chat, and their
 * own model, here, answers it there. */
export function SharedChatView({ contactId, contactName, chat, onBack, toast }: { contactId: string; contactName: string; chat: SharedChat; onBack?: () => void; toast: PushToast }) {
  const c = { id: contactId, name: contactName };
  const [v, setV] = useState<SharedView | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const helpers = useTypingHelpers();
  // This user's model answering here: what it's doing, and its words so far.
  const [mine, setMine] = useState<{ status: string; content: string } | null>(null);
  // What this user just wrote, until it comes back in their chat.
  const [pending, setPending] = useState<string | null>(null);
  const end = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  useEffect(() => {
    let alive = true;
    api
      .peopleWatch(c.id, chat.id)
      .then((x) => alive && setV(x))
      .catch((e) => {
        if (!alive) return;
        if (onBack) {
          toast(errorText(e), "error");
          onBack();
        } else setFailed(errorText(e));
      });
    const here = (p: { contact_id: string; chat_id: string }) => p.contact_id === c.id && p.chat_id === chat.id;
    const subs = [
      on("people:snapshot", (p) => {
        if (!here(p)) return;
        setV(p.view);
        setPending((t) => (t && p.view.messages.some((m) => m.mine && m.role === "user" && m.text === t) ? null : t));
      }),
      on("people:live", (p) => here(p) && setV((x) => (x ? { ...x, live: p.live } : x))),
      on("people:post_live", (p) => {
        if (!here(p)) return;
        if (p.status === "done") setMine(null);
        else setMine((m) => ({ status: p.status, content: (m?.content ?? "") + (p.delta ?? "") }));
      }),
    ];
    return () => {
      alive = false;
      subs.forEach((s) => s.then((un) => un()));
      api.peopleUnwatch(c.id, chat.id).catch(() => {});
    };
  }, [c.id, chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const send = async () => {
    const text = input.trim();
    if (!text || mine) return;
    setInput("");
    rememberSent(text);
    setPending(text);
    setMine({ status: "loading", content: "" });
    try {
      await api.peopleSharedPost(c.id, chat.id, text);
    } catch (e) {
      setMine(null);
      setPending(null);
      setInput(text);
      toast(errorText(e), "error");
    }
  };
  const size = (v?.messages.length ?? 0) + (v?.live?.content.length ?? 0) + (mine?.content.length ?? 0);
  useEffect(() => {
    if (atBottom.current) end.current?.scrollIntoView({ block: "end" });
  }, [size]);

  const info = v?.chat ?? chat;
  return (
    <>
      <div className="shared-head">
        {onBack && (
          <button className="link small" onClick={onBack}>
            {t("← Shared chats")}
          </button>
        )}
        <strong className="ellipsis">{info.title}</strong>
        <span className="small muted">
          {t("{name}'s chat, shared with you", { name: c.name })}
          {info.model && " · " + t("their replies by {model} on their PC", { model: info.model })}
        </span>
      </div>
      {failed && <p className="muted small people-note">{failed}</p>}
      <div
        className="people-messages shared-messages"
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
        }}
      >
        {!v && <p className="muted small people-note">{t("Opening…")}</p>}
        {v?.messages.map((m) =>
          m.role === "step" ? (
            <div key={m.id} className="shared-step small muted">
              {m.text}
            </div>
          ) : m.role === "note" ? (
            <div key={m.id} className="people-day small muted">
              {t("The earlier part of this chat was summarized here.")}
            </div>
          ) : (
            <div key={m.id} className={`pmsg ${m.mine ? "mine" : ""}`}>
              <span className="small muted pmeta">
                {m.role === "user"
                  ? m.mine
                    ? t("You")
                    : m.by || c.name
                  : m.mine
                    ? t("Your assistant") + (m.model ? " · " + m.model : "")
                    : m.by
                      ? t("{name}'s assistant", { name: m.by }) + (m.model ? " · " + m.model : "")
                      : t("{name}'s assistant", { name: c.name }) + (info.model ? " · " + info.model : "")}
              </span>
              <div className="pbubble">{m.role === "user" ? <span className="ptext">{m.text}</span> : <Markdown text={m.text} />}</div>
            </div>
          ),
        )}
        {v?.live && (
          <div className="pmsg">
            <span className="small muted pmeta live-dot">{t(LIVE_STATUS[v.live.status] ?? "Working…")}</span>
            {v.live.content && (
              <div className="pbubble">
                <Markdown text={v.live.content} />
              </div>
            )}
          </div>
        )}
        {pending && (
          <div className="pmsg mine">
            <span className="small muted pmeta">{t("You")}</span>
            <div className="pbubble">
              <span className="ptext">{pending}</span>
            </div>
          </div>
        )}
        {mine && (
          <div className="pmsg mine">
            <span className="small muted pmeta live-dot">{mine.status === "loading" ? t("Your model is reading the chat…") : t("Your model is answering…")}</span>
            {mine.content && (
              <div className="pbubble">
                <Markdown text={mine.content} />
              </div>
            )}
          </div>
        )}
        <div ref={end} />
      </div>
      {v && (
        <div className="shared-compose">
          <SmartInput
            className="input"
            rows={2}
            value={input}
            onValueChange={setInput}
            complete={helpers.complete}
            correct={helpers.correct}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
            placeholder={t("Write in {name}'s chat; your own model answers here", { name: c.name })}
            aria-label={t("Message")}
          />
          {mine ? (
            <button className="btn" onClick={() => api.peopleSharedStop(c.id, chat.id)}>
              {t("Stop")}
            </button>
          ) : (
            <button className="btn primary" onClick={send} disabled={!input.trim()}>
              {t("Send")}
            </button>
          )}
          <p className="muted small shared-compose-note">{t("{name} sees what you write and your model's answer in their chat. Your model runs on this PC.", { name: c.name })}</p>
        </div>
      )}
    </>
  );
}

type Item = { kind: "message"; at: number; m: PersonMessage } | { kind: "ask"; at: number; a: PersonAsk };

/** Messages and the assistants' questions and tasks, in time order. */
function timeline(messages: PersonMessage[], asks: PersonAsk[]): Item[] {
  const items: Item[] = [...messages.map((m) => ({ kind: "message" as const, at: m.at, m })), ...asks.map((a) => ({ kind: "ask" as const, at: a.at, a }))];
  return items.sort((x, y) => x.at - y.at);
}

/** A question between the two assistants, or a task they work out together. */
function AskCard({ a, contact: c, toast, along }: { a: PersonAsk; contact: PersonContact; toast: PushToast; along?: boolean }) {
  const act = async (f: () => Promise<void>) => {
    try {
      await f();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const task = a.kind === "task";
  const head = along
    ? a.from_me
      ? t("You also asked {name}'s assistant", { name: c.name })
      : t("{name} also asked your assistant", { name: c.name })
    : a.from_me
      ? task
        ? t("Your assistant is working something out with {name}'s", { name: c.name })
        : t("Your assistant asked {name}'s", { name: c.name })
      : task
        ? t("{name}'s assistant wants to work something out with yours", { name: c.name })
        : t("{name}'s assistant asked yours", { name: c.name });
  const status: Record<string, string> = {
    sent: t("Sent"),
    approval: a.from_me ? t("Waiting for {name} to allow it", { name: c.name }) : t("Waiting for you to allow it"),
    working: a.from_me && !task ? t("{name}'s assistant is working on it", { name: c.name }) : task ? t("The assistants are talking…") : t("Your assistant is working on it…"),
    declined: t("Declined"),
    failed: t("Didn't finish"),
  };
  const open = !["answered", "declined", "failed"].includes(a.status);
  return (
    <div className={`ask-card ${a.from_me ? "mine" : ""}`}>
      <div className="small muted">
        {head} · {time(a.at)}
      </div>
      {!along && <div className="ask-q">{a.question}</div>}
      {task && a.thread.length > 0 && (
        <ol className="ask-thread">
          {a.thread.map((turn, i) => (
            <li key={i} className={turn.from_me ? "mine" : ""}>
              <span className="small muted">{turn.from_me ? t("Your assistant") : t("{name}'s assistant", { name: c.name })}</span>
              <span className="ptext">{turn.text.replace(/^\s*(\*\*)?DONE:(\*\*)?\s*/i, "✓ ")}</span>
            </li>
          ))}
        </ol>
      )}
      {a.answer && (
        <div className={`ask-a ${a.status}`}>
          {a.status === "answered" ? (
            <>
              <span className="small muted">{task ? t("Outcome") : a.from_me ? t("Their answer") : t("Your assistant's answer")}</span>
              <Markdown text={a.answer} />
            </>
          ) : (
            <span className="small">{a.answer}</span>
          )}
        </div>
      )}
      {open && status[a.status] && <div className="small muted live-dot">{status[a.status]}</div>}
      {a.steps.length > 0 && (
        <details className="small muted">
          <summary>{t("What your assistant looked at")}</summary>
          <ul>
            {a.steps.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
          </ul>
        </details>
      )}
      {!a.from_me && a.status === "approval" && (
        <div className="row">
          <button className="btn primary small" onClick={() => act(() => api.peopleReply(a.id, "assistant", null, false))}>
            {task ? t("Let my assistant work on it") : t("Let my assistant answer")}
          </button>
          <button className="btn ghost small" onClick={() => act(() => api.peopleReply(a.id, "decline", null, false))}>
            {t("Decline")}
          </button>
        </div>
      )}
      {a.status === "working" && (!a.from_me || task) && (
        <button className="btn ghost small" onClick={() => act(() => api.peopleStop(a.id))}>
          {t("Stop")}
        </button>
      )}
    </div>
  );
}

/** What someone's assistant may do here. */
function AllowModal({ contact: c, onClose, toast }: { contact: PersonContact; onClose: () => void; toast: PushToast }) {
  // Settings saved by an older version have no free-time choice or model yet.
  const [a, setA] = useState<PersonAllow>({ ...c.allow, free: c.allow.free ?? "yes_no", calendar_add: c.allow.calendar_add ?? false, model: c.allow.model ?? null });
  const save = async () => {
    try {
      await api.peopleSetAllow(c.id, a);
      toast(t("Saved."), "success");
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const addFolder = async () => {
    const dir = await openFile({ directory: true, multiple: false });
    if (typeof dir === "string" && !a.folders.includes(dir)) setA({ ...a, folders: [...a.folders, dir] });
  };
  const modes: { id: PersonAllow["mode"]; label: string }[] = [
    { id: "off", label: t("Off: don't answer {name}'s questions", { name: c.name }) },
    { id: "ask", label: t("Ask me each time") },
    { id: "auto", label: t("Answer on its own") },
  ];
  const off = a.mode === "off";
  return (
    <Modal title={t("Your assistant for {name}", { name: c.name })} onClose={onClose}>
      <p className="muted small">
        {t("When {name} ticks “Also ask your assistant” on a message (or their assistant asks yours), your assistant answers on this PC. It only answers questions about you, like whether you're free or whether you did your part in a chat you share. It doesn't do work for them, and its model loads only for a question and goes again after. Every question shows in Activity.", { name: c.name })}
      </p>
      <div className="share-list" role="radiogroup" aria-label={t("Your assistant for {name}", { name: c.name })}>
        {modes.map((m) => (
          <label key={m.id} className="check">
            <input type="radio" name="allow-mode" checked={a.mode === m.id} onChange={() => setA({ ...a, mode: m.id })} />
            {m.label}
          </label>
        ))}
      </div>
      <fieldset className="allow-more" disabled={off}>
        <label className="adv-field">
          <span className="small">{t("Model it answers with")}</span>
          <ModelSelect value={a.model} onChange={(model) => setA({ ...a, model })} label={t("Model it answers with")} />
        </label>
        <strong className="small">{t("It may use")}</strong>
        <div className="share-list">
          <label className="check">
            <input type="checkbox" checked disabled />
            {t("Chats you share with {name}, and your messages together", { name: c.name })}
          </label>
          <label className="check">
            <input type="checkbox" checked={a.notes} onChange={(e) => setA({ ...a, notes: e.target.checked })} />
            {t("Your notes")}
          </label>
          <label className="check">
            <input type="checkbox" checked={a.calendar} onChange={(e) => setA({ ...a, calendar: e.target.checked })} />
            {t("Whether you're free (from your calendar)")}
          </label>
          {a.calendar && (
            <div className="allow-sub" role="radiogroup" aria-label={t("What it says about your time")}>
              <label className="check small">
                <input type="radio" name="allow-free" checked={a.free !== "times"} onChange={() => setA({ ...a, free: "yes_no" })} />
                {t("Only yes or no, for the time they ask about")}
              </label>
              <label className="check small">
                <input type="radio" name="allow-free" checked={a.free === "times"} onChange={() => setA({ ...a, free: "times" })} />
                {t("Your free times that day")}
              </label>
              <span className="muted small">{t("Never what you're doing or with whom.")}</span>
            </div>
          )}
          <label className="check">
            <input type="checkbox" checked={a.calendar_add} onChange={(e) => setA({ ...a, calendar_add: e.target.checked })} />
            {t("Add events to your calendar when {name} asks (no invitations; you're told each time)", { name: c.name })}
          </label>
          <label className="check">
            <input type="checkbox" checked={a.mail} onChange={(e) => setA({ ...a, mail: e.target.checked })} />
            {t("Your email")}
          </label>
          <div>
            <span className="small">{t("These folders (read-only):")}</span>
            {a.folders.length === 0 && <span className="small muted"> {t("none")}</span>}
            <ul className="people-addresses small">
              {a.folders.map((f) => (
                <li key={f} className="row">
                  <code className="ellipsis">{f}</code>
                  <button className="link small" onClick={() => setA({ ...a, folders: a.folders.filter((x) => x !== f) })}>
                    {t("Remove")}
                  </button>
                </li>
              ))}
            </ul>
            <button className="btn ghost small" onClick={addFolder}>
              {t("Add a folder…")}
            </button>
          </div>
        </div>
      </fieldset>
      <p className="muted small">
        {t("Calendar, email and folders also need their feature turned on in Features. Limits: up to 6 questions an hour and 20 a day from {name}, one at a time, each a short question with a short answer.", { name: c.name })}
      </p>
      <div className="adv-actions">
        <button className="btn primary" onClick={save}>
          {t("Save")}
        </button>
        <button className="btn ghost" onClick={onClose}>
          {t("Cancel")}
        </button>
      </div>
    </Modal>
  );
}
