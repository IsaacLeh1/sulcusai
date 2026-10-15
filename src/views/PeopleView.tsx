// SPDX-License-Identifier: AGPL-3.0-only
// People: link with someone else's SulcusAI by a code and chat with them
// directly. Messages go app to app; they never go to a model.
import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type PeopleView as View, type PersonContact, type PersonMessage } from "../api";
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
    const subs = [on("people:changed", load), on("people:message", load)];
    // Keeps "online", the code's countdown and the check-ins current.
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
  const end = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    api.peopleMessages(c.id).then(setMessages).catch(() => {});
  }, [c.id]);
  useEffect(() => {
    load();
    const subs = [on("people:message", (p) => p.contact_id === c.id && load()), on("people:changed", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [load, c.id]);
  // Reading them here clears the unread count.
  useEffect(() => {
    if (messages) onChanged();
  }, [messages?.length]); // eslint-disable-line react-hooks/exhaustive-deps
  // New messages scroll into view (unless the user scrolled up to read).
  const count = messages?.length ?? 0;
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
        {(messages ?? []).map((m, i) => {
          const prev = messages![i - 1];
          const newDay = !prev || day(prev.at) !== day(m.at);
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
