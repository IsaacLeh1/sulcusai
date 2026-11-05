// SPDX-License-Identifier: AGPL-3.0-only
// Groups: invite-only, run on their members' PCs. Channels, threads,
// reactions, pins, moderation, reports, catch-me-up and shared documents.
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { confirmed } from "../../confirm";
import { mentionsIn, newId, social, type Dm, type FileRef, type GroupMessage, type GroupView, type MemberView, type ModQueue, type SocialView, type Switches } from "./socialApi";
import { Attachments, EmojiPicker, GifPicker, LinkEmbeds, readFile, RichText, saveSticker, StickerPicker, VoiceRecorder } from "./ChatParts";
import { linksIn, slash, SLASH_COMMANDS } from "./chatText";
import { shortcodes } from "./emoji";
import { GroupCall } from "./GroupCall";
import "./social.css";

type Contact = { id: string; name: string };

export function GroupsPanel({ contacts, myName, toast }: { contacts: Contact[]; myName: string; toast: PushToast }) {
  const [v, setV] = useState<SocialView | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [joining, setJoining] = useState(false);
  const [hub, setHub] = useState("");
  const [code, setCode] = useState("");
  const load = useCallback(() => {
    social.view().then(setV).catch(() => setV(null));
  }, []);
  useEffect(() => {
    load();
    const un = [listen("social:changed", load), listen<{ why: string }>("social:refused", (e) => toast(e.payload.why))];
    return () => un.forEach((p) => p.then((f) => f()));
  }, [load, toast]);
  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
      load();
    } catch (e) {
      toast(errorText(e));
    }
  };
  if (!v) return null;
  if (!v.switches.groups) {
    return (
      <section className="card groups">
        <h3>{t("Groups")}</h3>
        <p className="small muted">{t("Invite-only groups that run on their members' PCs, encrypted end to end. Nothing is listed anywhere.")}</p>
        <button onClick={() => run(() => social.setSwitches({ ...v.switches, groups: true }))}>{t("Turn on groups")}</button>
      </section>
    );
  }
  const open = v.groups.find((g) => g.id === openId);
  return (
    <section className="card social">
      <nav className="social-rail" aria-label={t("Groups")}>
        <button className={!open ? "social-pill active" : "social-pill"} onClick={() => setOpenId(null)} title={t("Home: direct messages and settings")}>
          <HomeIcon />
        </button>
        {v.groups.map((g) => {
          const unread = g.channels.reduce((a, c) => a + c.unread, 0);
          const mentions = g.channels.reduce((a, c) => a + c.mentions, 0);
          return (
            <button key={g.id} className={g.id === openId ? "social-pill active" : unread > 0 ? "social-pill unread" : "social-pill"} title={g.name} onClick={() => setOpenId(g.id)}>
              {g.name
                .split(/\s+/)
                .map((w) => w.slice(0, 1))
                .join("")
                .slice(0, 3)
                .toUpperCase()}
              {mentions > 0 && <span className="social-badge">{mentions > 99 ? "99+" : mentions}</span>}
            </button>
          );
        })}
      </nav>
      {open ? (
        <Group key={open.id} g={open} me={v.me} myName={myName} switches={v.switches} run={run} toast={toast} />
      ) : (
      <div className="social-home">
      <h3>{t("Groups")}</h3>
      <ul className="plain">
        {v.groups.map((g) => (
          <li key={g.id}>
            <button className="link" onClick={() => setOpenId(g.id)}>
              {g.name}
            </button>
            <span className="small muted"> {t("{n} members", { n: g.members.length })}</span>
            {g.channels.some((c) => c.unread > 0) && <span className="badge subtle">{g.channels.reduce((a, c) => a + c.unread, 0)}</span>}
          </li>
        ))}
      </ul>
      <div className="row">
        <button
          onClick={() => {
            const name = window.prompt(t("Group name"));
            if (name) run(() => social.create(name, myName));
          }}
        >
          {t("New group")}
        </button>
        <button onClick={() => setJoining((j) => !j)}>{t("Join with an invite")}</button>
      </div>
      <DirectMessages groups={v.groups} run={run} toast={toast} />
      {joining && (
        <div className="row">
          <select value={hub} onChange={(e) => setHub(e.target.value)} aria-label={t("Who invited you")}>
            <option value="">{t("Who invited you")}</option>
            {contacts.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <input value={code} onChange={(e) => setCode(e.target.value)} placeholder={t("Invite code")} />
          <button disabled={!hub || !code.trim()} onClick={() => run(() => social.join(hub, code, myName)).then(() => toast(t("Asked to join. The group appears once they let you in.")))}>
            {t("Join")}
          </button>
        </div>
      )}
      <details>
        <summary>{t("Group settings on this PC")}</summary>
        {(
          [
            ["calls", t("Calls and screen share")],
            ["model_moderation", t("A model on this PC checks messages in groups you own")],
            ["assistants_in_groups", t("Assistants as group members")],
          ] as const
        ).map(([k, label]) => (
          <label key={k} className="row">
            <input type="checkbox" checked={v.switches[k]} onChange={(e) => run(() => social.setSwitches({ ...v.switches, [k]: e.target.checked }))} />
            {label}
          </label>
        ))}
        <label className="row">
          <input
            type="checkbox"
            checked={v.teen.on}
            onChange={(e) => {
              const pin = e.target.checked ? undefined : window.prompt(t("App lock PIN")) ?? "";
              run(() => social.setTeen({ ...v.teen, on: e.target.checked }, pin));
            }}
          />
          {t("Teen settings (direct messages only from contacts, strict filter, no adults-only channels)")}
        </label>
        {(
          [
            ["link_previews", t("Previews and players for links in chats (fetches each link through your web settings)")],
            ["gif_search", t("GIF search (your search words and key go to GIPHY; the GIF you pick is sent inside the message)")],
          ] as const
        ).map(([k, label]) => (
          <label key={k} className="row">
            <input type="checkbox" checked={v.switches[k]} onChange={(e) => run(() => social.setSwitches({ ...v.switches, [k]: e.target.checked }))} />
            {label}
          </label>
        ))}
        {v.switches.gif_search && (
          <label className="row">
            {t("GIPHY key")}
            <input type="password" defaultValue={v.switches.gif_key} onBlur={(e) => e.target.value !== v.switches.gif_key && run(() => social.setSwitches({ ...v.switches, gif_key: e.target.value }))} />
          </label>
        )}
        <button onClick={() => run(() => social.setSwitches({ ...v.switches, groups: false }))}>{t("Turn off groups")}</button>
      </details>
      </div>
      )}
    </section>
  );
}

const roleClass = (m?: MemberView) => (m?.assistant ? "role-assistant" : m?.role === "owner" ? "role-owner" : m?.role === "moderator" ? "role-mod" : "");
/** Messages from one sender this close together share one header. */
const GROUP_GAP = 5 * 60_000;

function Group({ g, me, myName, switches, run, toast }: { g: GroupView; me: string; myName: string; switches: Switches; run: (f: () => Promise<unknown>) => Promise<void>; toast: PushToast }) {
  const [channel, setChannel] = useState(g.channels.find((c) => c.kind !== "voice")?.id ?? "general");
  const [thread, setThread] = useState<string | null>(null);
  const [msgs, setMsgs] = useState<GroupMessage[]>([]);
  const [panel, setPanel] = useState<"chat" | "members" | "moderation" | "doc">("chat");
  const [summary, setSummary] = useState<string | null>(null);
  const [inCall, setInCall] = useState<string | null>(null);
  const [callPeople, setCallPeople] = useState<string[]>([]);
  const [showMembers, setShowMembers] = useState(true);
  const [pinsOnly, setPinsOnly] = useState(false);
  const [query, setQuery] = useState("");
  const [replyTo, setReplyTo] = useState<GroupMessage | null>(null);
  const [typing, setTyping] = useState<Record<string, number>>({});
  const [seen, setSeen] = useState<[string, number][]>([]);
  const [stickerNote, setStickerNote] = useState(false);
  const endCall = useCallback(() => setInCall(null), []);
  const mod = g.role === "moderator" || g.role === "owner";
  const ch = g.channels.find((c) => c.id === channel);
  const names = g.members.map((m) => m.name);
  const lastRead = useRef("");
  const load = useCallback(() => {
    social
      .messages(g.id, channel, thread)
      .then((ms) => {
        setMsgs(ms);
        const last = ms[ms.length - 1];
        // A read mark goes out once per newest message from someone else.
        if (last && last.author !== me && lastRead.current !== last.id) {
          lastRead.current = last.id;
          social.send(g.id, channel, { ev: "read", channel, at: last.at }).catch(() => {});
        }
      })
      .catch((e) => toast(errorText(e)));
    social.seen(g.id, channel).then(setSeen).catch(() => setSeen([]));
  }, [g.id, channel, thread, me, toast]);
  useEffect(() => {
    load();
    const un = [
      listen("social:changed", load),
      listen<{ group: string; channel: string; from: string }>("social:typing", (e) => {
        if (e.payload.group === g.id && e.payload.channel === channel && e.payload.from !== me) setTyping((x) => ({ ...x, [e.payload.from]: Date.now() + 6000 }));
      }),
    ];
    return () => un.forEach((p) => p.then((f) => f()));
  }, [load, g.id, channel, me]);
  useEffect(() => {
    const id = window.setInterval(() => setTyping((x) => Object.fromEntries(Object.entries(x).filter(([, until]) => until > Date.now()))), 2000);
    return () => window.clearInterval(id);
  }, []);
  useEffect(() => {
    if (!inCall) return setCallPeople([]);
    const tick = () =>
      social
        .callView()
        .then((c) => setCallPeople(c ? c.people.map(([, n]) => n) : []))
        .catch(() => {});
    tick();
    const id = window.setInterval(tick, 3000);
    return () => window.clearInterval(id);
  }, [inCall]);
  const typers = Object.keys(typing).map((id) => g.members.find((m) => m.id === id)?.name ?? t("Someone"));
  const shown = msgs.filter((m) => (!pinsOnly || m.pinned) && (!query.trim() || m.text.toLowerCase().includes(query.toLowerCase()) || m.author_name.toLowerCase().includes(query.toLowerCase())));
  const lastMine = [...msgs].reverse().find((m) => m.author === me);
  const seenBy = lastMine ? seen.filter(([, at]) => at >= lastMine.at).map(([n]) => n) : [];
  const byRole = (["owner", "moderator", "member"] as const).map((r) => [r, g.members.filter((m) => m.role === r)] as const).filter(([, ms]) => ms.length > 0);
  return (
    <div className={showMembers ? "social-group with-members" : "social-group"}>
      <nav className="social-channels" aria-label={t("Channels")}>
        <h3 className="social-group-name">{g.name}</h3>
        <div className="social-menu">
          {(
            [
              ["chat", t("Chat")],
              ["members", t("Members and invites")],
              ["doc", t("Documents")],
              ...(mod ? [["moderation", t("Moderation")] as const] : []),
            ] as const
          ).map(([k, label]) => (
            <button key={k} className={panel === k ? "active" : ""} onClick={() => setPanel(k)}>
              {label}
              {k === "moderation" && g.held + g.reports > 0 && <span className="social-badge">{g.held + g.reports}</span>}
            </button>
          ))}
        </div>
        <div className="social-section">{t("Text channels")}</div>
        {g.channels
          .filter((c) => c.kind !== "voice")
          .map((c) => (
            <button
              key={c.id}
              className={c.id === channel && panel === "chat" ? "social-ch active" : c.unread > 0 ? "social-ch unread" : "social-ch"}
              onClick={() => {
                setChannel(c.id);
                setThread(null);
                setPanel("chat");
              }}
            >
              <span className="social-hash" aria-hidden>
                #
              </span>
              {c.name}
              {c.kind === "announcements" && <span className="small muted"> {t("(announcements)")}</span>}
              {c.mentions > 0 && <span className="social-badge" title={t("Mentions of you")}>{c.mentions}</span>}
            </button>
          ))}
        {mod && (
          <button
            className="social-ch add"
            onClick={() => {
              const name = window.prompt(t("Channel name"));
              if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "text", adults_only: false }));
            }}
          >
            + {t("Add channel")}
          </button>
        )}
        <div className="social-section">{t("Voice channels")}</div>
        {g.channels
          .filter((c) => c.kind === "voice")
          .map((c) => (
            <div key={c.id}>
              <button className={inCall === c.id ? "social-ch active" : "social-ch"} onClick={() => setInCall(c.id)} title={t("Join the call")}>
                <SpeakerIcon />
                {c.name}
              </button>
              {inCall === c.id && (
                <ul className="social-voice-people">
                  {callPeople.map((n) => (
                    <li key={n}>{n}</li>
                  ))}
                </ul>
              )}
            </div>
          ))}
        {mod && (
          <button
            className="social-ch add"
            onClick={() => {
              const name = window.prompt(t("Voice channel name"));
              if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "voice", adults_only: false }));
            }}
          >
            + {t("Add voice channel")}
          </button>
        )}
      </nav>
      <main className="social-main">
        {inCall && <GroupCall key={inCall} group={g.id} channel={inCall} myName={myName} onClose={endCall} toast={toast} />}
        {panel === "members" && <Members g={g} me={me} run={run} />}
        {panel === "moderation" && <Moderation g={g} run={run} />}
        {panel === "doc" && <Doc g={g} run={run} />}
        {panel === "chat" && (
          <>
            <header className="social-head">
              {thread ? (
                <button className="link" onClick={() => setThread(null)}>
                  {t("Back to #{name}", { name: ch?.name ?? "" })}
                </button>
              ) : (
                <strong>
                  <span className="social-hash" aria-hidden>
                    #
                  </span>
                  {ch?.name}
                </strong>
              )}
              {thread && <span className="small muted">{t("Thread")}</span>}
              <span className="social-head-tools">
                <button className={pinsOnly ? "active" : ""} onClick={() => setPinsOnly((p) => !p)} title={t("Pinned messages")}>
                  {t("Pins")}
                </button>
                <button onClick={() => social.catchUp(g.id, channel).then(setSummary).catch((e) => toast(errorText(e)))}>{t("Catch me up")}</button>
                <button className={showMembers ? "active" : ""} onClick={() => setShowMembers((s) => !s)}>
                  {t("Members")}
                </button>
                <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("Search")} aria-label={t("Search messages")} />
              </span>
            </header>
            {summary && (
              <div className="note small">
                {summary}{" "}
                <button className="link" onClick={() => setSummary(null)}>
                  {t("Close")}
                </button>
              </div>
            )}
            {stickerNote && <div className="small muted">{t("Saved to your stickers.")}</div>}
            <div className="social-messages">
              {shown.length === 0 && <p className="small muted">{pinsOnly ? t("No pinned messages here.") : query ? t("No messages match.") : t("No messages yet. Say hello.")}</p>}
              {shown.map((m, i) => {
                const prev = shown[i - 1];
                const compact = !!prev && prev.author === m.author && m.at - prev.at < GROUP_GAP && !m.reply_to && !prev.deleted;
                return (
                  <MessageRow
                    key={m.id}
                    m={m}
                    all={msgs}
                    compact={compact}
                    g={g}
                    me={me}
                    mod={mod}
                    names={names}
                    previews={switches.link_previews}
                    run={run}
                    toast={toast}
                    onReply={() => setReplyTo(m)}
                    onThread={() => setThread(m.id)}
                    onSticker={(f) => {
                      setStickerNote(saveSticker(f));
                      window.setTimeout(() => setStickerNote(false), 3000);
                    }}
                  />
                );
              })}
              {seenBy.length > 0 && <div className="small muted social-seen">{t("Seen by {names}", { names: seenBy.join(", ") })}</div>}
            </div>
            <div className="small muted social-typing" aria-live="polite">
              {typers.length === 1 ? t("{name} is typing...", { name: typers[0] }) : typers.length > 1 ? t("Several people are typing...") : ""}
            </div>
            {ch?.kind === "announcements" && !mod ? (
              <p className="small muted">{t("Only moderators post in this channel.")}</p>
            ) : (
              <Composer
                g={g}
                channel={channel}
                thread={thread}
                replyTo={replyTo}
                clearReply={() => setReplyTo(null)}
                switches={switches}
                placeholder={t("Message #{name}", { name: ch?.name ?? "" })}
                run={run}
                toast={toast}
                after={load}
              />
            )}
          </>
        )}
      </main>
      {showMembers && (
        <aside className="social-members" aria-label={t("Members")}>
          {byRole.map(([r, ms]) => (
            <div key={r}>
              <div className="social-section">
                {r === "owner" ? t("Owner") : r === "moderator" ? t("Moderators") : t("Members")} ({ms.length})
              </div>
              {ms.map((m) => (
                <div key={m.id} className={`social-member ${roleClass(m)}`}>
                  <span className="social-avatar" aria-hidden>
                    {m.name.slice(0, 1).toUpperCase()}
                  </span>
                  {m.name}
                  {m.assistant && <span className="small muted"> {t("assistant")}</span>}
                </div>
              ))}
            </div>
          ))}
        </aside>
      )}
    </div>
  );
}

function SpeakerIcon() {
  return (
    <svg className="social-icon" width="14" height="14" viewBox="0 0 16 16" aria-hidden>
      <path d="M2 6h3l4-3v10l-4-3H2z" fill="currentColor" />
      <path d="M11 5.5a3.5 3.5 0 0 1 0 5" stroke="currentColor" fill="none" strokeWidth="1.4" />
    </svg>
  );
}

function Composer({
  g,
  channel,
  thread,
  replyTo,
  clearReply,
  switches,
  placeholder,
  run,
  toast,
  after,
}: {
  g: GroupView;
  channel: string;
  thread: string | null;
  replyTo: GroupMessage | null;
  clearReply: () => void;
  switches: Switches;
  placeholder: string;
  run: (f: () => Promise<unknown>) => Promise<void>;
  toast: PushToast;
  after: () => void;
}) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<FileRef[]>([]);
  const [picker, setPicker] = useState<null | "emoji" | "gif" | "sticker">(null);
  const [gifStart, setGifStart] = useState<string | undefined>();
  const [dragging, setDragging] = useState(false);
  const lastTyping = useRef(0);
  const fileIn = useRef<HTMLInputElement>(null);
  const post = (body: string, withFiles: FileRef[]) => {
    if (!body.trim() && withFiles.length === 0) return;
    clearReply();
    run(() => social.send(g.id, channel, { ev: "post", id: newId(), channel, text: shortcodes(body), reply_to: replyTo?.id ?? null, thread, mentions: mentionsIn(body, g.members), files: withFiles })).then(after);
  };
  const send = () => {
    const s = slash(text.trim());
    if (s === null) return;
    if ("error" in s) return toast(s.error);
    if ("gif" in s) {
      if (!switches.gif_search) return toast(t("GIF search is off. Turn it on in group settings."));
      setText("");
      setGifStart(s.gif);
      setPicker("gif");
      return;
    }
    setText("");
    const fs = files;
    setFiles([]);
    post(s.text, fs);
  };
  const addFiles = async (list: FileList | File[]) => {
    try {
      const read = await Promise.all([...list].map(readFile));
      setFiles((f) => [...f, ...read].slice(0, 10));
    } catch (e) {
      toast(errorText(e));
    }
  };
  const suggestions = text.startsWith("/") && !text.includes(" ") ? SLASH_COMMANDS.filter(([c]) => c.startsWith(text.toLowerCase())) : [];
  return (
    <div
      className={dragging ? "social-composer dragging" : "social-composer"}
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        addFiles(e.dataTransfer.files);
      }}
    >
      {replyTo && (
        <div className="small muted social-replying">
          {t("Replying to {name}", { name: replyTo.author_name })}{" "}
          <button className="link" onClick={clearReply}>
            {t("Cancel")}
          </button>
        </div>
      )}
      {files.length > 0 && (
        <div className="social-pending">
          {files.map((f, i) => (
            <span key={i} className="chip">
              {f.name}
              <button className="link" aria-label={t("Remove {name}", { name: f.name })} onClick={() => setFiles((x) => x.filter((_, j) => j !== i))}>
                {t("Remove")}
              </button>
            </span>
          ))}
        </div>
      )}
      {suggestions.length > 0 && (
        <ul className="plain social-slash">
          {suggestions.map(([c, d]) => (
            <li key={c}>
              <button className="link" onClick={() => setText(c + " ")}>
                <strong>{c}</strong> <span className="small muted">{t(d)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {picker === "emoji" && <EmojiPicker onPick={(e) => setText((x) => x + e)} onClose={() => setPicker(null)} />}
      {picker === "gif" && (
        <GifPicker
          start={gifStart}
          onPick={(f) => {
            setPicker(null);
            post("", [f]);
          }}
          onClose={() => setPicker(null)}
        />
      )}
      {picker === "sticker" && (
        <StickerPicker
          onPick={(f) => {
            setPicker(null);
            post("", [f]);
          }}
          onClose={() => setPicker(null)}
        />
      )}
      <div className="social-compose-row">
        <button onClick={() => fileIn.current?.click()} title={t("Send a file, picture, video or sound")}>
          {t("Attach")}
        </button>
        <input ref={fileIn} type="file" multiple hidden onChange={(e) => e.target.files && addFiles(e.target.files)} />
        <textarea
          value={text}
          rows={1}
          onChange={(e) => {
            setText(e.target.value);
            if (Date.now() - lastTyping.current > 4000) {
              lastTyping.current = Date.now();
              social.send(g.id, channel, { ev: "typing", channel }).catch(() => {});
            }
          }}
          onPaste={(e) => e.clipboardData.files.length > 0 && (e.preventDefault(), addFiles(e.clipboardData.files))}
          onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && (e.preventDefault(), send())}
          placeholder={placeholder}
        />
        <button className={picker === "emoji" ? "active" : ""} onClick={() => setPicker((p) => (p === "emoji" ? null : "emoji"))}>
          {t("Emoji")}
        </button>
        {switches.gif_search && (
          <button
            className={picker === "gif" ? "active" : ""}
            onClick={() => {
              setGifStart(undefined);
              setPicker((p) => (p === "gif" ? null : "gif"));
            }}
          >
            {t("GIF")}
          </button>
        )}
        <button className={picker === "sticker" ? "active" : ""} onClick={() => setPicker((p) => (p === "sticker" ? null : "sticker"))}>
          {t("Stickers")}
        </button>
        <VoiceRecorder onDone={(f) => post("", [f])} />
        <button className="primary" onClick={send}>
          {t("Send")}
        </button>
      </div>
    </div>
  );
}

const QUICK = ["👍", "❤️", "😂", "🎉"];

function MessageRow({
  m,
  all,
  compact,
  g,
  me,
  mod,
  names,
  previews,
  run,
  toast,
  onReply,
  onThread,
  onSticker,
}: {
  m: GroupMessage;
  all: GroupMessage[];
  compact: boolean;
  g: GroupView;
  me: string;
  mod: boolean;
  names: string[];
  previews: boolean;
  run: (f: () => Promise<unknown>) => Promise<void>;
  toast: PushToast;
  onReply: () => void;
  onThread: () => void;
  onSticker: (f: FileRef) => void;
}) {
  const [shown, setShown] = useState(!m.hidden);
  const [translated, setTranslated] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  if (m.deleted) return <div className="small muted social-msg">{t("Message deleted")}</div>;
  if (!shown)
    return (
      <button className="link small muted" onClick={() => setShown(true)}>
        {t("Hidden message from someone you blocked or muted")}
      </button>
    );
  const ev = (e: Parameters<typeof social.send>[2]) => run(() => social.send(g.id, m.channel, e));
  const author = g.members.find((x) => x.id === m.author);
  const parent = m.reply_to ? all.find((x) => x.id === m.reply_to) : undefined;
  const time = new Date(m.at);
  const mentionsMe = m.mentions.includes(me);
  return (
    <div className={`social-msg${compact ? " compact" : ""}${mentionsMe ? " mentioned" : ""}`}>
      {parent && (
        <div className="small muted social-reply-ref">
          {t("Reply to {name}:", { name: parent.author_name })} {parent.deleted ? t("Message deleted") : parent.text.slice(0, 80) || t("(a file)")}
        </div>
      )}
      {compact ? (
        <span className="social-time-side small muted" title={time.toLocaleString()}>
          {time.toLocaleTimeString([], { timeStyle: "short" })}
        </span>
      ) : (
        <div className="social-msg-head">
          <span className="social-avatar" aria-hidden>
            {m.author_name.slice(0, 1).toUpperCase()}
          </span>
          <strong className={roleClass(author)}>{m.author_name}</strong>{" "}
          <span className="small muted" title={time.toLocaleString()}>
            {time.toLocaleString([], { dateStyle: "short", timeStyle: "short" })}
          </span>
          {m.pinned && <span className="small muted"> · {t("Pinned")}</span>}
        </div>
      )}
      <div className="social-msg-body">
        {editing !== null ? (
          <div className="row">
            <textarea value={editing} onChange={(e) => setEditing(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setEditing(null)} />
            <button
              onClick={() => {
                if (editing.trim()) ev({ ev: "edit", id: m.id, text: editing });
                setEditing(null);
              }}
            >
              {t("Save")}
            </button>
          </div>
        ) : (
          m.text && <RichText text={translated ?? m.text} mentionable={names} />
        )}
        {m.edited && <span className="small muted"> {t("(edited)")}</span>}
        {m.files && m.files.length > 0 && <Attachments files={m.files} onSticker={onSticker} />}
        <LinkEmbeds links={linksIn(m.text)} on={previews} />
        {Object.keys(m.reactions).length > 0 && (
          <div className="social-reactions">
            {Object.entries(m.reactions).map(([r, who]) => (
              <button key={r} className={who.includes(me) ? "active" : ""} title={who.map((id) => g.members.find((x) => x.id === id)?.name ?? "").join(", ")} onClick={() => ev({ ev: "react", id: m.id, reaction: r, on: !who.includes(me) })}>
                {r} {who.length}
              </button>
            ))}
          </div>
        )}
        {!m.thread && m.replies > 0 && (
          <button className="link small" onClick={onThread}>
            {t("{n} replies in a thread", { n: m.replies })}
          </button>
        )}
      </div>
      <div className="social-actions" role="toolbar" aria-label={t("Message actions")}>
        {QUICK.map((r) => (
          <button key={r} title={t("React with {r}", { r })} onClick={() => ev({ ev: "react", id: m.id, reaction: r, on: !(m.reactions[r] ?? []).includes(me) })}>
            {r}
          </button>
        ))}
        <button onClick={() => setPicking((p) => !p)}>{t("React")}</button>
        <button onClick={onReply}>{t("Reply")}</button>
        {!m.thread && <button onClick={onThread}>{t("Thread")}</button>}
        <button onClick={() => social.translate(m.text, navigator.language).then(setTranslated).catch((e) => toast(errorText(e)))}>{t("Translate")}</button>
        <button onClick={() => navigator.clipboard.writeText(m.text).catch(() => {})}>{t("Copy")}</button>
        {m.author === me && <button onClick={() => setEditing(m.text)}>{t("Edit")}</button>}
        {mod && <button onClick={() => ev({ ev: "pin", id: m.id, on: !m.pinned })}>{m.pinned ? t("Unpin") : t("Pin")}</button>}
        {(m.author === me || mod) && (
          <button className="danger" onClick={async () => (await confirmed(t("Delete this message for everyone?"))) && ev({ ev: "delete", id: m.id })}>
            {t("Delete")}
          </button>
        )}
        {m.author !== me && (
          <button
            onClick={() => {
              const reason = window.prompt(t("Why are you reporting this? Only this group's moderators see it."));
              if (reason) run(() => social.report(g.id, m.author, m.id, m.text, reason)).then(() => toast(t("Reported to the group's moderators.")));
            }}
          >
            {t("Report")}
          </button>
        )}
      </div>
      {picking && (
        <EmojiPicker
          onPick={(r) => {
            setPicking(false);
            ev({ ev: "react", id: m.id, reaction: r, on: true });
          }}
          onClose={() => setPicking(false)}
        />
      )}
    </div>
  );
}

function HomeIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden>
      <path d="M3 9.5 10 3l7 6.5V17h-5v-4.5H8V17H3z" fill="currentColor" />
    </svg>
  );
}

function Members({ g, me, run }: { g: GroupView; me: string; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const [invite, setInvite] = useState<string | null>(null);
  const owner = g.role === "owner";
  const mod = owner || g.role === "moderator";
  return (
    <div>
      {mod && (
        <div className="row">
          <button onClick={() => run(async () => setInvite(await social.invite(g.id, 1, 24)))}>{t("Make an invite (one person, 24 hours)")}</button>
          {invite && <code className="selectable">{invite}</code>}
        </div>
      )}
      {invite && <p className="small muted">{t("Give this to someone you've linked with in People. It works once and isn't shown again.")}</p>}
      <ul className="plain">
        {g.members.map((m) => (
          <li key={m.id} className="row">
            <span>
              {m.name}
              {m.assistant && <span className="small muted"> ({t("assistant")})</span>}
              <span className="small muted"> · {m.role === "owner" ? t("Owner") : m.role === "moderator" ? t("Moderator") : t("Member")}</span>
              {m.muted && <span className="small muted"> · {t("Muted")}</span>}
            </span>
            {m.id !== me && (
              <>
                {!m.assistant && !m.blocked && (
                  <button
                    className="link"
                    onClick={() => {
                      const text = window.prompt(t("Message to {name}", { name: m.name }));
                      if (text) run(() => social.dmSend(g.id, m.id, text));
                    }}
                  >
                    {t("Message")}
                  </button>
                )}
                <button className="link" onClick={() => run(() => social.block(m.id, !m.blocked, false))}>
                  {m.blocked ? t("Unblock") : t("Block")}
                </button>
                {owner && m.role !== "owner" && (
                  <button className="link" onClick={() => run(() => social.act(g.id, { op: "set_role", member: m.id, role: m.role === "moderator" ? "member" : "moderator" }))}>
                    {m.role === "moderator" ? t("Make member") : t("Make moderator")}
                  </button>
                )}
                {mod && m.role === "member" && (
                  <>
                    <button className="link" onClick={() => run(() => social.act(g.id, { op: "mute", member: m.id, until: m.muted ? 0 : Date.now() + 3600_000 }))}>
                      {m.muted ? t("Unmute") : t("Mute for an hour")}
                    </button>
                    <button className="link" onClick={async () => (await confirmed(t("Remove {name} from the group?", { name: m.name }))) && run(() => social.act(g.id, { op: "kick", member: m.id }))}>
                      {t("Remove")}
                    </button>
                    <button className="link" onClick={async () => (await confirmed(t("Ban {name}? They can't join again.", { name: m.name }))) && run(() => social.act(g.id, { op: "ban", member: m.id }))}>
                      {t("Ban")}
                    </button>
                  </>
                )}
              </>
            )}
          </li>
        ))}
      </ul>
      {mod && (
        <button
          onClick={() => {
            const name = window.prompt(t("Assistant name"));
            if (name) run(() => social.addAssistant(g.id, name, g.channels[0]?.id ?? "general"));
          }}
        >
          {t("Add an assistant (answers when named)")}
        </button>
      )}
      <button className="danger" onClick={async () => (await confirmed(t("Leave {name}?", { name: g.name }))) && run(() => social.leave(g.id))}>
        {t("Leave group")}
      </button>
    </div>
  );
}

function Moderation({ g, run }: { g: GroupView; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const [q, setQ] = useState<ModQueue | null>(null);
  const [words, setWords] = useState(g.rules.blocked_words.join(", "));
  const load = useCallback(() => {
    social.modQueue(g.id).then(setQ).catch(() => setQ(null));
  }, [g.id]);
  useEffect(load, [load, g.held, g.reports]);
  return (
    <div>
      <h4>{t("Held by automatic moderation")}</h4>
      {q?.held.length === 0 && <p className="small muted">{t("Nothing held.")}</p>}
      {q?.held.map((h) => (
        <div key={h.index} className="row">
          <span>
            <strong>{h.from}</strong>: {h.text} <span className="small muted">({h.why})</span>
          </span>
          <button onClick={() => run(() => social.release(g.id, h.index, true)).then(load)}>{t("Allow")}</button>
          <button onClick={() => run(() => social.release(g.id, h.index, false)).then(load)}>{t("Remove")}</button>
        </div>
      ))}
      <h4>{t("Reports")}</h4>
      {q?.reports.length === 0 && <p className="small muted">{t("No open reports.")}</p>}
      {q?.reports.map((r) => (
        <div key={r.id} className="row">
          <span>
            “{r.quote}” <span className="small muted">{r.reason}</span>
          </span>
          <button onClick={() => run(() => social.resolveReport(r.id)).then(load)}>{t("Done")}</button>
        </div>
      ))}
      {g.mine && (
        <>
          <h4>{t("Rules")}</h4>
          <label className="row">
            {t("Words not allowed")}
            <input value={words} onChange={(e) => setWords(e.target.value)} />
          </label>
          <label className="row">
            <input type="checkbox" checked={g.rules.block_links} onChange={(e) => run(() => social.setRules(g.id, { ...g.rules, block_links: e.target.checked }))} />
            {t("No links")}
          </label>
          <label className="row">
            <input type="checkbox" checked={g.teens_allowed} onChange={(e) => run(() => social.act(g.id, { op: "teens_allowed", allowed: e.target.checked }))} />
            {t("Teens may join")}
          </label>
          <button onClick={() => run(() => social.setRules(g.id, { ...g.rules, blocked_words: words.split(",").map((w) => w.trim()).filter(Boolean) }))}>{t("Save rules")}</button>
        </>
      )}
    </div>
  );
}

function Doc({ g, run }: { g: GroupView; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const [name, setName] = useState(g.docs[0] ?? "notes");
  const [text, setText] = useState("");
  useEffect(() => {
    social.doc(g.id, name).then(setText).catch(() => setText(""));
  }, [g.id, name, g.docs]);
  return (
    <div>
      <div className="row">
        <select value={name} onChange={(e) => setName(e.target.value)} aria-label={t("Document")}>
          {[...new Set([...g.docs, name])].map((d) => (
            <option key={d}>{d}</option>
          ))}
        </select>
        <button
          onClick={() => {
            const n = window.prompt(t("Document name (a song sheet, plans, notes)"));
            if (n) setName(n);
          }}
        >
          {t("New document")}
        </button>
      </div>
      <textarea className="group-doc" value={text} onChange={(e) => setText(e.target.value)} rows={14} />
      <button onClick={() => run(() => social.docEdit(g.id, name, text))}>{t("Save for everyone")}</button>
    </div>
  );
}

/** Direct messages with group members, and requests from people who aren't contacts. */
function DirectMessages({ groups, run, toast }: { groups: GroupView[]; run: (f: () => Promise<unknown>) => Promise<void>; toast: PushToast }) {
  const [dms, setDms] = useState<Dm[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  const [text, setText] = useState("");
  const load = useCallback(() => {
    social.dms().then(setDms).catch(() => setDms([]));
  }, []);
  useEffect(() => {
    load();
    const un = listen("social:dm", load);
    return () => {
      un.then((f) => f());
    };
  }, [load]);
  const people = [...new Map(dms.map((d) => [d.with, d])).values()];
  const requests = people.filter((d) => dms.some((x) => x.with === d.with && x.request));
  const talks = people.filter((d) => !requests.includes(d));
  const thread = dms.filter((d) => d.with === open);
  const last = thread[thread.length - 1];
  if (people.length === 0) return null;
  return (
    <div className="group-dms">
      <h4>{t("Direct messages")}</h4>
      {requests.length > 0 && (
        <div>
          <span className="small muted">{t("Message requests")}</span>
          {requests.map((d) => (
            <div key={d.with} className="row">
              <span>
                <strong>{d.with_name}</strong>: {d.text}
              </span>
              <button onClick={() => run(() => social.dmAnswer(d.with, true)).then(load)}>{t("Accept")}</button>
              <button onClick={() => run(() => social.dmAnswer(d.with, false)).then(load)}>{t("Decline and block")}</button>
            </div>
          ))}
        </div>
      )}
      <div className="row">
        {talks.map((d) => (
          <button key={d.with} className={open === d.with ? "active" : ""} onClick={() => setOpen(d.with)}>
            {d.with_name}
          </button>
        ))}
      </div>
      {open && last && (
        <div>
          {thread.map((d, i) => (
            <div key={i} className={d.from_me ? "small dm-mine" : "small"}>
              <strong>{d.from_me ? t("You") : d.with_name}</strong>
              <RichText text={d.text} />
            </div>
          ))}
          <div className="row">
            <input value={text} onChange={(e) => setText(e.target.value)} placeholder={t("Message")} />
            <button
              disabled={!text.trim() || !groups.some((g) => g.id === last.group)}
              onClick={() => {
                const body = text;
                setText("");
                social.dmSend(last.group, open, body).then(load).catch((e) => toast(errorText(e)));
              }}
            >
              {t("Send")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
