// SPDX-License-Identifier: AGPL-3.0-only
// Groups: invite-only, run on their members' PCs. Channels, threads,
// reactions, pins, moderation, reports, catch-me-up and shared documents.
import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { confirmed } from "../../confirm";
import { mentionsIn, newId, social, type Dm, type FileRef, type GroupMessage, type GroupView, type MemberView, type ModQueue, type Profile, type SocialView, type Switches } from "./socialApi";
import { Avatar, ProfileCard, ProfileStrip } from "./Profiles";
import { OpenFullApp, PartsSettings } from "./Parts";
import { Networks } from "./Networks";
import { Attachments, EmojiPicker, GifPicker, LinkEmbeds, readFile, RichText, saveSticker, StickerPicker, VoiceRecorder } from "./ChatParts";
import { linksIn, slash, SLASH_COMMANDS } from "./chatText";
import { shortcodes } from "./emoji";
import { GroupCall } from "./GroupCall";
import "./social.css";

type Contact = { id: string; name: string };

/** Groups are opened from a rail of round icons; Home is the direct chats (`home`) and "+" makes or joins groups. */
export function GroupsPanel({ contacts, myName, toast, home }: { contacts: Contact[]; myName: string; toast: PushToast; home: ReactNode }) {
  const [v, setV] = useState<SocialView | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [joining, setJoining] = useState(false);
  const [hub, setHub] = useState("");
  const [code, setCode] = useState("");
  const [muted, setMuted] = useState(false);
  const [deafened, setDeafened] = useState(false);
  const [profiles, setProfiles] = useState<Record<string, Profile>>({});
  useEffect(() => {
    const get = () => social.profiles().then(setProfiles).catch(() => {});
    get();
    const un = listen("social:profile", get);
    return () => {
      un.then((f) => f());
    };
  }, []);
  const strip = <ProfileStrip myName={myName} muted={muted} deafened={deafened} setMuted={setMuted} setDeafened={setDeafened} toast={toast} />;
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
  const groupsOn = v?.switches.groups ?? false;
  const open = groupsOn ? v?.groups.find((g) => g.id === openId) : undefined;
  return (
    <div className="social-shell">
      <nav className="social-rail" aria-label={t("Groups")}>
        <button className={!openId ? "social-pill active" : "social-pill"} onClick={() => setOpenId(null)} title={t("Direct messages")}>
          <HomeIcon />
        </button>
        <span className="social-rail-line" aria-hidden />
        {(groupsOn ? v?.groups ?? [] : []).map((g) => {
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
        <button className={openId === "+" ? "social-pill add active" : "social-pill add"} onClick={() => setOpenId("+")} title={t("Make or join a group")} aria-label={t("Make or join a group")}>
          +
        </button>
        <OpenFullApp />
      </nav>
      {!openId || !v ? (
        <div className="people-page">{home}</div>
      ) : open ? (
        <Group key={open.id} g={open} me={v.me} myName={myName} switches={v.switches} run={run} toast={toast} profiles={profiles} muted={muted} deafened={deafened} strip={strip} />
      ) : !groupsOn ? (
        <div className="social-home">
          <h3>{t("Groups")}</h3>
          <p className="small muted">{t("Invite-only groups that run on their members' PCs, encrypted end to end. Nothing is listed anywhere.")}</p>
          <button onClick={() => run(() => social.setSwitches({ ...v.switches, groups: true }))}>{t("Turn on groups")}</button>
        </div>
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
      <Networks toast={toast} />
      {strip}
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
            ["since_summary", t("A model on this PC sums up what's new when you open a channel")],
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
        <PartsSettings toast={toast} />
        <button onClick={() => run(() => social.setSwitches({ ...v.switches, groups: false }))}>{t("Turn off groups")}</button>
      </details>
      </div>
      )}
    </div>
  );
}

const roleClass = (m?: MemberView) => (m?.assistant ? "role-assistant" : m?.role === "owner" ? "role-owner" : m?.role === "moderator" ? "role-mod" : "");
/** Messages from one sender this close together share one header. */
const GROUP_GAP = 5 * 60_000;

function Group({
  g,
  me,
  myName,
  switches,
  run,
  toast,
  profiles,
  muted,
  deafened,
  strip,
}: {
  g: GroupView;
  me: string;
  myName: string;
  switches: Switches;
  run: (f: () => Promise<unknown>) => Promise<void>;
  toast: PushToast;
  profiles: Record<string, Profile>;
  muted: boolean;
  deafened: boolean;
  strip: ReactNode;
}) {
  const [channel, setChannel] = useState(g.channels.find((c) => c.kind !== "voice")?.id ?? "general");
  const [thread, setThread] = useState<string | null>(null);
  const [msgs, setMsgs] = useState<GroupMessage[]>([]);
  const [panel, setPanel] = useState<"chat" | "members" | "moderation" | "doc" | "networks">("chat");
  const [summary, setSummary] = useState<string | null>(null);
  const [inCall, setInCall] = useState<string | null>(null);
  const [callPeople, setCallPeople] = useState<string[]>([]);
  // Shown or hidden, the member list stays as the user last left it.
  const [showMembers, setShowMembersRaw] = useState(() => {
    try {
      return localStorage.getItem("sulcus.showMembers") !== "0";
    } catch {
      return true;
    }
  });
  const setShowMembers = (f: (s: boolean) => boolean) =>
    setShowMembersRaw((s) => {
      const next = f(s);
      try {
        localStorage.setItem("sulcus.showMembers", next ? "1" : "0");
      } catch {
        // Only a convenience.
      }
      return next;
    });
  const [pinsOnly, setPinsOnly] = useState(false);
  const [query, setQuery] = useState("");
  const [replyTo, setReplyTo] = useState<GroupMessage | null>(null);
  const [typing, setTyping] = useState<Record<string, number>>({});
  const [seen, setSeen] = useState<[string, number][]>([]);
  const [stickerNote, setStickerNote] = useState(false);
  const endCall = useCallback(() => setInCall(null), []);
  // Where the user stopped reading, taken when the channel opens (opening it marks it read).
  const [readAt, setReadAt] = useState(0);
  const [selected, setSelected] = useState<string[] | null>(null);
  const [asking, setAsking] = useState<GroupMessage | null>(null);
  const [forwarding, setForwarding] = useState<GroupMessage[] | null>(null);
  const [insert, setInsert] = useState<{ text: string; n: number } | null>(null);
  const [card, setCard] = useState<string | null>(null);
  const sinceAsked = useRef("");
  useEffect(() => {
    setReadAt(g.channels.find((c) => c.id === channel)?.read_at ?? 0);
    setSelected(null);
    setSummary(null);
    // Only a channel switch takes a new mark.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channel]);
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
    if (!inCall) setCallPeople([]);
  }, [inCall]);
  const typers = Object.keys(typing).map((id) => g.members.find((m) => m.id === id)?.name ?? t("Someone"));
  const shown = msgs.filter((m) => (!pinsOnly || m.pinned) && (!query.trim() || m.text.toLowerCase().includes(query.toLowerCase()) || m.author_name.toLowerCase().includes(query.toLowerCase())));
  const lastMine = [...msgs].reverse().find((m) => m.author === me);
  const seenBy = lastMine ? seen.filter(([, at]) => at >= lastMine.at).map(([n]) => n) : [];
  const firstNew = thread ? undefined : msgs.find((m) => m.at > readAt && m.author !== me && !m.deleted);
  const newOnes = firstNew ? msgs.filter((m) => m.at >= firstNew.at && m.author !== me && !m.deleted) : [];
  const posters = [...new Set(newOnes.map((m) => m.author_name))];
  useEffect(() => {
    if (!switches.since_summary || newOnes.length < 3 || sinceAsked.current === channel + readAt) return;
    sinceAsked.current = channel + readAt;
    social.catchUp(g.id, channel).then(setSummary).catch(() => {});
  }, [switches.since_summary, newOnes.length, channel, readAt, g.id]);
  const jump = (id: string) => document.getElementById(`msg-${id}`)?.scrollIntoView({ block: "center" });
  const forward = (to: { channel: string } | { member: string }) => {
    const list = forwarding ?? [];
    setForwarding(null);
    setSelected(null);
    const body = list.map((m) => `> ${m.text.split("\n").join("\n> ")}\n> - ${m.author_name}`).join("\n\n");
    const files = list.flatMap((m) => m.files ?? []).slice(0, 10);
    if ("channel" in to) run(() => social.send(g.id, to.channel, { ev: "post", id: newId(), channel: to.channel, text: body || t("(forwarded)"), reply_to: null, thread: null, mentions: [], files }));
    else run(() => social.dmSend(g.id, to.member, body)).then(() => toast(t("Forwarded.")));
  };
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
              ["networks", t("Networks")],
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
        {strip}
      </nav>
      <main className="social-main">
        {inCall && <GroupCall key={inCall} group={g.id} channel={inCall} myName={myName} onClose={endCall} toast={toast} muted={muted} deafened={deafened} onPeople={setCallPeople} />}
        {panel === "members" && <Members g={g} me={me} run={run} />}
        {panel === "moderation" && <Moderation g={g} run={run} />}
        {panel === "doc" && <Doc g={g} run={run} />}
        {panel === "networks" && (
          <Networks
            group={g.id}
            toast={toast}
            actions={{
              chat: (device, name) => {
                const text = window.prompt(t("Message to {name}", { name }));
                if (text) run(() => social.dmSend(g.id, device, text));
              },
              call: () => {
                const voice = g.channels.find((c) => c.kind === "voice");
                if (voice) setInCall(voice.id);
                else toast(t("This group has no voice channel yet."));
              },
              block: (device) => run(() => social.block(device, true, false)),
            }}
          />
        )}
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
                {firstNew && (
                  <button onClick={() => jump(firstNew.id)} title={t("Jump to the first unread message")}>
                    {t("First unread")}
                  </button>
                )}
                <button className={pinsOnly ? "active" : ""} onClick={() => setPinsOnly((p) => !p)} title={t("Pinned messages")}>
                  {t("Pins")}
                </button>
                <button onClick={() => social.catchUp(g.id, channel).then(setSummary).catch((e) => toast(errorText(e)))}>{t("Catch me up")}</button>
                <button className={showMembers ? "active" : ""} aria-pressed={showMembers} title={showMembers ? t("Hide the member list") : t("Show the member list")} onClick={() => setShowMembers((s) => !s)}>
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
            {newOnes.length > 0 && !summary && (
              <div className="social-since small">
                {t("Since you were last here: {n} new from {names}.", { n: newOnes.length, names: posters.slice(0, 5).join(", ") + (posters.length > 5 ? t(" and others") : "") })}
              </div>
            )}
            {selected && (
              <div className="social-bulk row small">
                <strong>{t("{n} selected", { n: selected.length })}</strong>
                <button disabled={!selected.length} onClick={() => navigator.clipboard.writeText(msgs.filter((m) => selected.includes(m.id)).map((m) => `${m.author_name}: ${m.text}`).join("\n")).catch(() => {})}>
                  {t("Copy")}
                </button>
                <button disabled={!selected.length} onClick={() => setForwarding(msgs.filter((m) => selected.includes(m.id)))}>
                  {t("Forward")}
                </button>
                <button
                  className="danger"
                  disabled={!selected.length || msgs.some((m) => selected.includes(m.id) && m.author !== me && !mod)}
                  title={t("Only your own messages, or any if you moderate")}
                  onClick={async () => {
                    if (!(await confirmed(t("Delete {n} messages for everyone?", { n: selected.length })))) return;
                    const ids = selected;
                    setSelected(null);
                    for (const id of ids) await social.send(g.id, channel, { ev: "delete", id }).catch((e) => toast(errorText(e)));
                    load();
                  }}
                >
                  {t("Delete")}
                </button>
                <button onClick={() => setSelected(null)}>{t("Done")}</button>
              </div>
            )}
            {stickerNote && <div className="small muted">{t("Saved to your stickers.")}</div>}
            <div className="social-messages">
              {shown.length === 0 && <p className="small muted">{pinsOnly ? t("No pinned messages here.") : query ? t("No messages match.") : t("No messages yet. Say hello.")}</p>}
              {shown.map((m, i) => {
                const prev = shown[i - 1];
                const isNew = firstNew?.id === m.id && !pinsOnly && !query;
                const compact = !isNew && !!prev && prev.author === m.author && m.at - prev.at < GROUP_GAP && !m.reply_to && !prev.deleted;
                return (
                  <Fragment key={m.id}>
                  {isNew && (
                    <div className="social-new-line" role="separator">
                      <span>{t("New messages")}</span>
                    </div>
                  )}
                  <MessageRow
                    m={m}
                    all={msgs}
                    compact={compact}
                    g={g}
                    me={me}
                    mod={mod}
                    names={names}
                    previews={switches.link_previews}
                    profile={profiles[m.author]}
                    selecting={selected !== null}
                    selected={!!selected?.includes(m.id)}
                    run={run}
                    toast={toast}
                    h={{
                      onReply: () => setReplyTo(m),
                      onThread: () => setThread(m.id),
                      onSticker: (f) => {
                        setStickerNote(saveSticker(f));
                        window.setTimeout(() => setStickerNote(false), 3000);
                      },
                      onQuote: () => setInsert({ text: `> ${m.text.split("\n").join("\n> ")}\n`, n: Date.now() }),
                      onForward: () => setForwarding([m]),
                      onAsk: () => setAsking(m),
                      onMarkUnread: () =>
                        run(() => social.markUnread(g.id, m.channel, m.at)).then(() => {
                          setReadAt(m.at - 1);
                          toast(t("Marked unread from here."));
                        }),
                      onSelect: () => setSelected((s) => (s === null ? [m.id] : s.includes(m.id) ? s.filter((x) => x !== m.id) : [...s, m.id])),
                      onProfile: () => setCard(m.author),
                    }}
                  />
                  </Fragment>
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
                insert={insert}
              />
            )}
          </>
        )}
      </main>
      {asking && <AskPanel key={asking.id} m={asking} onClose={() => setAsking(null)} toast={toast} />}
      {forwarding && <ForwardDialog g={g} me={me} onPick={forward} onClose={() => setForwarding(null)} />}
      {card && (
        <ProfileCard
          p={profiles[card]}
          name={g.members.find((x) => x.id === card)?.name ?? ""}
          onClose={() => setCard(null)}
          onMessage={
            card === me
              ? undefined
              : () => {
                  const name = g.members.find((x) => x.id === card)?.name ?? "";
                  const text = window.prompt(t("Message to {name}", { name }));
                  if (text) run(() => social.dmSend(g.id, card, text));
                  setCard(null);
                }
          }
        />
      )}
      {showMembers && (
        <aside className="social-members" aria-label={t("Members")}>
          {byRole.map(([r, ms]) => (
            <div key={r}>
              <div className="social-section">
                {r === "owner" ? t("Owner") : r === "moderator" ? t("Moderators") : t("Members")} ({ms.length})
              </div>
              {ms.map((m) => (
                <button key={m.id} className={`social-member ${roleClass(m)}`} onClick={() => setCard(m.id)}>
                  <Avatar name={m.name} p={profiles[m.id]} size={28} />
                  <span className="social-member-text">
                    {profiles[m.id]?.name || m.name}
                    {profiles[m.id]?.status && <span className="small muted">{profiles[m.id].status}</span>}
                  </span>
                  {m.assistant && <span className="small muted"> {t("assistant")}</span>}
                </button>
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
  insert,
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
  insert: { text: string; n: number } | null;
}) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<FileRef[]>([]);
  const [picker, setPicker] = useState<null | "emoji" | "gif" | "sticker">(null);
  const [gifStart, setGifStart] = useState<string | undefined>();
  const [dragging, setDragging] = useState(false);
  const lastTyping = useRef(0);
  useEffect(() => {
    if (insert) setText((x) => (x ? x + "\n" : "") + insert.text);
  }, [insert]);
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

const QUICK = ["👍", "❤️", "😂", "🎉", "😮", "😢"];

export type RowHandlers = {
  onReply: () => void;
  onThread: () => void;
  onSticker: (f: FileRef) => void;
  onQuote: () => void;
  onForward: () => void;
  onAsk: () => void;
  onMarkUnread: () => void;
  onSelect: () => void;
  onProfile: () => void;
};

/** Clicks on these do their own thing, not open the message menu. */
const OWN_CLICK = "a, button, input, textarea, select, video, audio, img, iframe, .chat-spoiler, .social-msg-menu";

function MessageRow({
  m,
  all,
  compact,
  g,
  me,
  mod,
  names,
  previews,
  profile,
  selecting,
  selected,
  run,
  toast,
  h,
}: {
  m: GroupMessage;
  all: GroupMessage[];
  compact: boolean;
  g: GroupView;
  me: string;
  mod: boolean;
  names: string[];
  previews: boolean;
  profile?: Profile;
  selecting: boolean;
  selected: boolean;
  run: (f: () => Promise<unknown>) => Promise<void>;
  toast: PushToast;
  h: RowHandlers;
}) {
  const [shown, setShown] = useState(!m.hidden);
  const [translated, setTranslated] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [remind, setRemind] = useState(false);
  useEffect(() => {
    if (!menu) return;
    const close = (e: Event) => {
      if (e instanceof KeyboardEvent && e.key !== "Escape") return;
      if (e instanceof MouseEvent && (e.target as HTMLElement).closest(".social-msg-menu")) return;
      setMenu(null);
      setRemind(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [menu]);
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
  const copy = (s: string, what: string) =>
    navigator.clipboard
      .writeText(s)
      .then(() => toast(t("{what} copied.", { what })))
      .catch(() => {});
  const act = (f: () => void) => () => {
    setMenu(null);
    f();
  };
  const open = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest(OWN_CLICK) || window.getSelection()?.toString()) return;
    e.preventDefault();
    if (selecting) return h.onSelect();
    setMenu({ x: Math.min(e.clientX, window.innerWidth - 260), y: Math.min(e.clientY, window.innerHeight - 420) });
  };
  const speak = () => {
    speechSynthesis.cancel();
    speechSynthesis.speak(new SpeechSynthesisUtterance(translated ?? m.text));
  };
  return (
    <div
      id={`msg-${m.id}`}
      className={`social-msg${compact ? " compact" : ""}${mentionsMe ? " mentioned" : ""}${menu ? " menu-open" : ""}${selected ? " selected" : ""}`}
      onClick={open}
      onContextMenu={open}
      tabIndex={0}
      onKeyDown={(e) => (e.key === "Enter" || e.key === "ContextMenu") && e.target === e.currentTarget && setMenu({ x: 80, y: (e.currentTarget.getBoundingClientRect().top || 80) + 20 })}
      aria-label={t("Message from {name}. Press Enter for actions.", { name: m.author_name })}
    >
      {selecting && <input type="checkbox" className="social-select" checked={selected} onChange={h.onSelect} aria-label={t("Select this message")} />}
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
          <button className="social-msg-avatar" onClick={h.onProfile} aria-label={t("{name}'s profile", { name: m.author_name })}>
            <Avatar name={m.author_name} p={profile} />
          </button>
          <button className={`link social-msg-name ${roleClass(author)}`} onClick={h.onProfile}>
            {profile?.name || m.author_name}
          </button>{" "}
          <span className="small muted" title={time.toLocaleString()}>
            {time.toLocaleString([], { dateStyle: "short", timeStyle: "short" })}
          </span>
          {m.pinned && <span className="small muted"> · {t("Pinned")}</span>}
        </div>
      )}
      <div className="social-msg-body">
        {editing !== null ? (
          <div className="row">
            <textarea autoFocus value={editing} onChange={(e) => setEditing(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setEditing(null)} />
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
        {m.files && m.files.length > 0 && <Attachments files={m.files} onSticker={h.onSticker} />}
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
          <button className="link small" onClick={h.onThread}>
            {t("{n} replies in a thread", { n: m.replies })}
          </button>
        )}
      </div>
      {menu && (
        <div className="social-msg-menu" role="menu" style={{ left: menu.x, top: menu.y }}>
          <div className="social-quick" role="group" aria-label={t("React")}>
            {QUICK.map((r) => (
              <button key={r} role="menuitem" title={t("React with {r}", { r })} onClick={act(() => ev({ ev: "react", id: m.id, reaction: r, on: !(m.reactions[r] ?? []).includes(me) }))}>
                {r}
              </button>
            ))}
            <button role="menuitem" onClick={act(() => setPicking(true))} title={t("Any emoji")}>
              +
            </button>
          </div>
          <button role="menuitem" onClick={act(h.onReply)}>
            {t("Reply")}
          </button>
          <button role="menuitem" onClick={act(h.onQuote)}>
            {t("Quote")}
          </button>
          {!m.thread && (
            <button role="menuitem" onClick={act(h.onThread)}>
              {t("Start a thread")}
            </button>
          )}
          <button role="menuitem" onClick={act(h.onForward)}>
            {t("Forward")}
          </button>
          {m.author === me && (
            <button role="menuitem" onClick={act(() => setEditing(m.text))}>
              {t("Edit")}
            </button>
          )}
          {mod && (
            <button role="menuitem" onClick={act(() => ev({ ev: "pin", id: m.id, on: !m.pinned }))}>
              {m.pinned ? t("Unpin") : t("Pin")}
            </button>
          )}
          <hr />
          <button role="menuitem" onClick={act(() => copy(m.text, t("Text")))}>
            {t("Copy text")}
          </button>
          <button role="menuitem" onClick={act(() => copy(`sulcusai://group/${g.id}/${m.channel}/${m.id}`, t("Link")))}>
            {t("Copy link")}
          </button>
          <button role="menuitem" onClick={act(() => copy(m.id, t("Message ID")))}>
            {t("Copy message ID")}
          </button>
          <button role="menuitem" onClick={act(h.onMarkUnread)}>
            {t("Mark unread from here")}
          </button>
          {remind ? (
            <div className="social-quick">
              {(
                [
                  [20, t("20 minutes")],
                  [60, t("1 hour")],
                  [180, t("3 hours")],
                  [60 * 24, t("Tomorrow")],
                ] as const
              ).map(([min, label]) => (
                <button key={min} role="menuitem" onClick={act(() => run(() => social.remind(m.author_name, m.text || t("(a file)"), min)).then(() => toast(t("Reminder added to your tasks."))))}>
                  {label}
                </button>
              ))}
            </div>
          ) : (
            <button role="menuitem" onClick={() => setRemind(true)}>
              {t("Remind me")}
            </button>
          )}
          <button role="menuitem" onClick={act(h.onAsk)}>
            {t("Ask a model about it")}
          </button>
          <button role="menuitem" onClick={act(() => social.translate(m.text, navigator.language).then(setTranslated).catch((e) => toast(errorText(e))))}>
            {t("Translate")}
          </button>
          <button role="menuitem" onClick={act(speak)}>
            {t("Speak aloud")}
          </button>
          <button role="menuitem" onClick={act(h.onSelect)}>
            {t("Select messages")}
          </button>
          <hr />
          {m.author !== me && (
            <button
              role="menuitem"
              onClick={act(() => {
                const reason = window.prompt(t("Why are you reporting this? Only this group's moderators see it."));
                if (reason) run(() => social.report(g.id, m.author, m.id, m.text, reason)).then(() => toast(t("Reported to the group's moderators.")));
              })}
            >
              {t("Report")}
            </button>
          )}
          {(m.author === me || mod) && (
            <button role="menuitem" className="danger" onClick={act(async () => (await confirmed(t("Delete this message for everyone?"))) && ev({ ev: "delete", id: m.id }))}>
              {t("Delete")}
            </button>
          )}
        </div>
      )}
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

/** Choose where to forward a message: a channel in this group or a member. */
function ForwardDialog({ g, me, onPick, onClose }: { g: GroupView; me: string; onPick: (to: { channel: string } | { member: string }) => void; onClose: () => void }) {
  return (
    <div className="social-modal" role="dialog" aria-label={t("Forward")} onClick={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="social-modal-box" onClick={(e) => e.stopPropagation()}>
        <h4>{t("Forward to")}</h4>
        {g.channels
          .filter((c) => c.kind !== "voice")
          .map((c) => (
            <button key={c.id} className="social-ch" onClick={() => onPick({ channel: c.id })}>
              # {c.name}
            </button>
          ))}
        {g.members
          .filter((m) => m.id !== me && !m.assistant && !m.blocked)
          .map((m) => (
            <button key={m.id} className="social-ch" onClick={() => onPick({ member: m.id })}>
              {t("Message {name}", { name: m.name })}
            </button>
          ))}
        <button onClick={onClose}>{t("Cancel")}</button>
      </div>
    </div>
  );
}

/** A side chat with a model on this PC about one message. */
function AskPanel({ m, onClose, toast }: { m: GroupMessage; onClose: () => void; toast: PushToast }) {
  const [talk, setTalk] = useState<[string, string][]>([]);
  const [q, setQ] = useState(t("What does this mean?"));
  const [busy, setBusy] = useState(false);
  const ask = async () => {
    if (!q.trim() || busy) return;
    setBusy(true);
    try {
      const a = await social.ask(m.author_name, m.text, talk, q);
      setTalk((x) => [...x, [q, a]]);
      setQ("");
    } catch (e) {
      toast(errorText(e));
    }
    setBusy(false);
  };
  return (
    <aside className="social-ask" aria-label={t("Ask a model")}>
      <header className="row">
        <strong>{t("Ask about this message")}</strong>
        <button className="link" onClick={onClose}>
          {t("Close")}
        </button>
      </header>
      <blockquote className="chat-quote small">
        <strong>{m.author_name}:</strong> {m.text.slice(0, 400)}
      </blockquote>
      <p className="small muted">{t("Answered by a model on this PC; the message doesn't leave it.")}</p>
      {talk.map(([qq, a], i) => (
        <div key={i}>
          <div className="small">
            <strong>{t("You")}:</strong> {qq}
          </div>
          <RichText text={a} />
        </div>
      ))}
      <div className="row">
        <textarea value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && (e.preventDefault(), ask())} rows={2} />
        <button disabled={busy} onClick={ask}>
          {busy ? t("Thinking...") : t("Ask")}
        </button>
      </div>
    </aside>
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
