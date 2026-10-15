// SPDX-License-Identifier: AGPL-3.0-only
// People: link with someone else's SulcusAI by a code and chat with them
// directly. Messages go app to app; they never go to a model.
import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type PeopleView as View, type PersonAllow, type PersonAsk, type PersonContact, type PersonMessage, type SharedChat, type SharedView } from "../api";
import { Markdown } from "../components/Markdown";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { bytes } from "../format";
import { t } from "../i18n";

function seen(ms: number | null): string {
  if (!ms) return t("not seen yet");
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return t("seen just now");
  if (s < 3600) return t("seen {n} min ago", { n: Math.round(s / 60) });
  if (s < 86400) return t("seen {n} h ago", { n: Math.round(s / 3600) });
  return t("seen {date}", { date: new Date(ms).toLocaleDateString() });
}

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

  const load = useCallback(() => {
    api.peopleView().then(setV).catch((e) => toast(errorText(e), "error"));
  }, [toast]);
  useEffect(() => {
    load();
    const subs = [on("people:changed", load), on("people:message", load), on("people:shared", load)];
    // Keeps "online" and the code's countdown current.
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

  const saveName = async () => {
    const name = naming?.trim();
    setNaming(null);
    if (!name || name === v.name) return;
    try {
      setV(await api.setPeopleName(name));
    } catch (e) {
      toast(errorText(e), "error");
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
                {c.last ? (c.last.from_me ? t("You: {text}", { text: preview(c.last) }) : preview(c.last)) : c.online ? t("Online") : seen(c.last_seen)}
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
          <Conversation key={current.id} contact={current} onChanged={load} toast={toast} />
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
        <Connect
          v={v}
          onView={setV}
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
  if (m.text.trim()) return m.text.replace(/\s+/g, " ").slice(0, 80);
  return m.file ? "📎 " + m.file.name : "";
}

/** One conversation: messages, and the box to write in. */
function Conversation({ contact: c, onChanged, toast }: { contact: PersonContact; onChanged: () => void; toast: PushToast }) {
  const [messages, setMessages] = useState<PersonMessage[] | null>(null);
  const [text, setText] = useState("");
  const [file, setFile] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [menu, setMenu] = useState(false);
  const [address, setAddress] = useState<string | null>(null);
  const [tab, setTab] = useState<"messages" | "shared">("messages");
  const [asks, setAsks] = useState<PersonAsk[]>([]);
  const [allowing, setAllowing] = useState(false);
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

  const send = async () => {
    if (sending || (!text.trim() && !file)) return;
    setSending(true);
    try {
      const m = await api.peopleSend(c.id, text, file);
      // A refresh may already have brought it.
      setMessages((all) => (all?.some((x) => x.id === m.id) ? all : [...(all ?? []), m]));
      setText("");
      setFile(null);
      atBottom.current = true;
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSending(false);
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
            {c.online ? t("Online") : seen(c.last_seen)}
            {!c.online && c.error && <span className="warn-text"> · {t("Can't reach them right now.")}</span>}
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
            <div className="menu-pop" role="menu">
              <button
                role="menuitem"
                onClick={() => {
                  setMenu(false);
                  setAllowing(true);
                }}
              >
                {t("What {name}'s assistant can use", { name: c.name })}
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
              <button role="menuitem" className="danger" onClick={remove}>
                {t("Remove {name}", { name: c.name })}
              </button>
            </div>
          )}
        </div>
      </header>
      {tab === "shared" ? (
        watching ? (
          <SharedChatView key={watching.id} contact={c} chat={watching} onBack={() => setWatching(null)} toast={toast} />
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
            return (
              <div key={item.a.id}>
                {newDay && <div className="people-day small muted">{day(item.at)}</div>}
                <AskCard a={item.a} contact={c} toast={toast} />
              </div>
            );
          }
          const m = item.m;
          return (
            <div key={m.id}>
              {newDay && <div className="people-day small muted">{day(m.at)}</div>}
              <div className={`pmsg ${m.from_me ? "mine" : ""}`}>
                <div className="pbubble">
                  {m.text && <span className="ptext">{m.text}</span>}
                  {m.file && (
                    <span className="pfile">
                      <span aria-hidden>📎</span>
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
                  {m.from_me && (m.delivered ? " · " + t("Delivered") : " · " + t("Waiting to send"))}
                </span>
              </div>
            </div>
          );
        })}
        <div ref={end} />
      </div>
      {waiting > 0 && !c.online && (
        <p className="small muted people-note">
          {t("{name} isn't reachable right now. Your messages wait here and go as soon as either of your apps reaches the other.", { name: c.name })}
        </p>
      )}
      <div className="composer-wrap">
        <div className="composer">
          {fileName && (
            <div className="row small">
              <span className="chip">📎 {fileName}</span>
              <button className="link small" onClick={() => setFile(null)}>
                {t("Remove")}
              </button>
            </div>
          )}
          <textarea
            rows={2}
            value={text}
            placeholder={t("Message {name}", { name: c.name })}
            aria-label={t("Message {name}", { name: c.name })}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />
          <div className="composer-bar">
            <button className="icon-btn" onClick={pick} title={t("Send a file (up to 16 MB)")} aria-label={t("Send a file")}>
              📎
            </button>
            <span className="small muted">{t("To {name} only. Never sent to a model.", { name: c.name })}</span>
            <button className="btn primary small push-right" disabled={sending || (!text.trim() && !file)} onClick={send}>
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

/** Connect with someone: show a code here, or type the one they show. */
function Connect({
  v,
  onView,
  onClose,
  onLinked,
  toast,
}: {
  v: View;
  onView: (v: View) => void;
  onClose: () => void;
  onLinked: (id: string | null) => void;
  toast: PushToast;
}) {
  const [mode, setMode] = useState<"show" | "enter">("show");
  const [code, setCode] = useState("");
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  // Someone typed this code: they show up as a new contact.
  const before = useRef(new Set(v.contacts.map((c) => c.id)));
  useEffect(() => {
    const added = v.contacts.find((c) => !before.current.has(c.id));
    if (added) {
      toast(t("{name} connected with you.", { name: added.name }), "success");
      onLinked(added.id);
    }
  }, [v.contacts]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async (f: () => Promise<View>) => {
    try {
      onView(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  // Showing a code is the first choice, so show one right away.
  useEffect(() => {
    if (mode === "show" && !v.code) run(() => api.peopleShowCode());
  }, [mode]); // eslint-disable-line react-hooks/exhaustive-deps
  const close = () => {
    if (v.code) api.peopleCancelCode().then(onView).catch(() => {});
    onClose();
  };

  const connect = async () => {
    setBusy(true);
    try {
      const pick = target.trim() || (v.nearby.length === 1 ? v.nearby[0].id : "");
      const name = await api.peopleConnect(code, pick || null);
      toast(t("Connected with {name}.", { name }), "success");
      const fresh = await api.peopleView();
      onView(fresh);
      onLinked(fresh.contacts.find((c) => !before.current.has(c.id))?.id ?? null);
    } catch (e) {
      toast(errorText(e), "error");
      setBusy(false);
    }
  };
  const ready = code.replace(/[^0-9A-Za-z]/g, "").length === 8;
  const left = `${Math.floor(v.code_left / 60)}:${String(v.code_left % 60).padStart(2, "0")}`;

  return (
    <Modal title={t("Connect with someone")} onClose={close}>
      <div className="seg" role="tablist">
        <button role="tab" aria-selected={mode === "show"} className={mode === "show" ? "active" : ""} onClick={() => setMode("show")}>
          {t("Show my code")}
        </button>
        <button
          role="tab"
          aria-selected={mode === "enter"}
          className={mode === "enter" ? "active" : ""}
          onClick={() => {
            if (v.code) run(() => api.peopleCancelCode());
            setMode("enter");
          }}
        >
          {t("Enter their code")}
        </button>
      </div>

      {mode === "show" ? (
        <div className="form">
          {v.code ? (
            <div className="pair-code" role="status">
              <span className="small">{t("Ask them to open People → Connect → Enter their code, and type:")}</span>
              <strong className="code">{v.code}</strong>
              <span className="muted small">{t("Good for {time} more. Waiting for them…", { time: left })}</span>
            </div>
          ) : (
            <button className="btn" onClick={() => run(() => api.peopleShowCode())}>
              {t("Show a new code")}
            </button>
          )}
          {v.addresses.length > 0 && (
            <div>
              <strong className="small">{t("If they're not on your network, they also type your address:")}</strong>
              <ul className="people-addresses small">
                {v.addresses.map((a) => (
                  <li key={a.address}>
                    <code>{a.address}</code> <span className="muted">{a.network}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {!v.listening && <p className="warn-text small">{t("This PC isn't taking calls yet. Wait a moment, or restart SulcusAI if it stays like this.")}</p>}
          <Help />
        </div>
      ) : (
        <div className="form">
          <label>
            {t("The code they see")}
            <input
              className="input code-input"
              value={code}
              autoFocus
              placeholder="K7Q2-9XMB"
              maxLength={12}
              onChange={(e) => setCode(e.target.value.toUpperCase())}
              onKeyDown={(e) => e.key === "Enter" && ready && connect()}
            />
          </label>
          {v.nearby.length > 1 && (
            <label>
              {t("Who")}
              <select className="input" value={target} onChange={(e) => setTarget(e.target.value)}>
                <option value="">{t("Choose…")}</option>
                {v.nearby.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.name}
                  </option>
                ))}
              </select>
            </label>
          )}
          {v.nearby.length === 1 && !target && <p className="muted small">{t("Connecting with {name}, found on this network.", { name: v.nearby[0].name })}</p>}
          <label>
            {v.nearby.length > 0 ? t("Or their address") : t("Their address (shown under their code)")}
            <input className="input" value={target} placeholder="26.10.4.7" onChange={(e) => setTarget(e.target.value)} />
          </label>
          <div className="adv-actions">
            <button className="btn primary" disabled={busy || !ready} onClick={connect}>
              {busy ? t("Connecting…") : t("Connect")}
            </button>
          </div>
          <Help />
        </div>
      )}
    </Modal>
  );
}

function Help() {
  return (
    <details className="small muted">
      <summary>{t("Not on the same network?")}</summary>
      <p>
        {t("Join the same virtual network first, like Radmin VPN, Tailscale or ZeroTier (each of you installs it and joins the same network). Then use the address listed under that network's name. Your apps talk directly, encrypted; nothing goes through SulcusAI's servers, because there aren't any.")}
      </p>
      <p>
        {t("The first time, Windows may ask whether SulcusAI may use the network: allow it, or they can't reach you. It's enough that one of you can be reached. SulcusAI needs to be open (and unlocked) on both PCs for messages to arrive.")}
      </p>
    </details>
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
          <p className="muted small">{t("Nothing. To share a chat, open it and click 👥 Share at the top.")}</p>
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
  loading: "Loading the model…",
  thinking: "Thinking…",
  writing: "Writing…",
  working: "Using a tool…",
};

/** A chat someone shares, live and read-only. */
function SharedChatView({ contact: c, chat, onBack, toast }: { contact: PersonContact; chat: SharedChat; onBack: () => void; toast: PushToast }) {
  const [v, setV] = useState<SharedView | null>(null);
  const end = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  useEffect(() => {
    let alive = true;
    api
      .peopleWatch(c.id, chat.id)
      .then((x) => alive && setV(x))
      .catch((e) => {
        if (!alive) return;
        toast(errorText(e), "error");
        onBack();
      });
    const mine = (p: { contact_id: string; chat_id: string }) => p.contact_id === c.id && p.chat_id === chat.id;
    const subs = [
      on("people:snapshot", (p) => mine(p) && setV(p.view)),
      on("people:live", (p) => mine(p) && setV((x) => (x ? { ...x, live: p.live } : x))),
    ];
    return () => {
      alive = false;
      subs.forEach((s) => s.then((un) => un()));
      api.peopleUnwatch(c.id, chat.id).catch(() => {});
    };
  }, [c.id, chat.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const size = (v?.messages.length ?? 0) + (v?.live?.content.length ?? 0);
  useEffect(() => {
    if (atBottom.current) end.current?.scrollIntoView({ block: "end" });
  }, [size]);

  const info = v?.chat ?? chat;
  return (
    <>
      <div className="shared-head">
        <button className="link small" onClick={onBack}>
          {t("← Shared chats")}
        </button>
        <strong className="ellipsis">{info.title}</strong>
        <span className="small muted">
          {t("{name}'s chat, read-only", { name: c.name })}
          {info.model && " · " + t("replies by {model} on their PC", { model: info.model })}
        </span>
      </div>
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
              🔧 {m.text}
            </div>
          ) : m.role === "note" ? (
            <div key={m.id} className="people-day small muted">
              {t("The earlier part of this chat was summarized here.")}
            </div>
          ) : (
            <div key={m.id} className={`pmsg ${m.role === "user" ? "mine" : ""}`}>
              <span className="small muted pmeta">{m.role === "user" ? c.name : info.model || t("Assistant")}</span>
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
        <div ref={end} />
      </div>
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
function AskCard({ a, contact: c, toast }: { a: PersonAsk; contact: PersonContact; toast: PushToast }) {
  const act = async (f: () => Promise<void>) => {
    try {
      await f();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const task = a.kind === "task";
  const head = a.from_me
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
        🤖 {head} · {time(a.at)}
      </div>
      <div className="ask-q">{a.question}</div>
      {task && a.thread.length > 0 && (
        <ol className="ask-thread">
          {a.thread.map((turn, i) => (
            <li key={i} className={turn.from_me ? "mine" : ""}>
              <span className="small muted">{turn.from_me ? t("Your assistant") : t("{name}'s assistant", { name: c.name })}</span>
              <span className="ptext">{turn.text.replace(/^\s*(\*\*)?DONE:(\*\*)?\s*/i, "✅ ")}</span>
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
  const [a, setA] = useState<PersonAllow>(c.allow);
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
    { id: "ask", label: t("Ask me each time") },
    { id: "auto", label: t("Let my assistant answer on its own") },
    { id: "off", label: t("Don't take questions from {name}'s assistant", { name: c.name }) },
  ];
  return (
    <Modal title={t("What {name}'s assistant can use", { name: c.name })} onClose={onClose}>
      <p className="muted small">
        {t("When {name}'s assistant asks yours something, or wants to work something out with it, your assistant answers on this PC with your model. It can only look, never change anything, and every request shows in Activity.", { name: c.name })}
      </p>
      <div className="share-list" role="radiogroup">
        {modes.map((m) => (
          <label key={m.id} className="check">
            <input type="radio" name="allow-mode" checked={a.mode === m.id} onChange={() => setA({ ...a, mode: m.id })} />
            {m.label}
          </label>
        ))}
      </div>
      <strong className="small">{t("Your assistant may use")}</strong>
      <div className="share-list">
        <label className="check">
          <input type="checkbox" checked disabled />
          {t("Chats you share with {name}", { name: c.name })}
        </label>
        <label className="check">
          <input type="checkbox" checked={a.notes} onChange={(e) => setA({ ...a, notes: e.target.checked })} />
          {t("Your notes")}
        </label>
        <label className="check">
          <input type="checkbox" checked={a.calendar} onChange={(e) => setA({ ...a, calendar: e.target.checked })} />
          {t("Your calendar")}
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
      <p className="muted small">{t("Calendar, email and folders also need their feature turned on in Features.")}</p>
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
