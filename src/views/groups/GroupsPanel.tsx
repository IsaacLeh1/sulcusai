// SPDX-License-Identifier: AGPL-3.0-only
// Groups: invite-only, run on their members' PCs. Channels, threads,
// reactions, pins, moderation, reports, catch-me-up and shared documents.
import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useDismiss } from "../../useDismiss";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { confirmed } from "../../confirm";
import { mentionsIn, newId, social, type Dm, type Event, type FileRef, type GroupMessage, type GroupView, type MemberView, type ModQueue, type Profile, type SocialView, type Switches } from "./socialApi";
import { Avatar, nameIn, ProfileCard, ProfileStrip } from "./Profiles";
import { OpenFullApp, PartsSettings } from "./Parts";
import { Networks } from "./Networks";
import { EmojiPicker, GifPicker, readFile, RichText, saveDropped, saveSticker, StickerPicker, VoiceRecorder } from "./ChatParts";
import { shareMake } from "../../api/sharing";
import { slash, SLASH_COMMANDS } from "./chatText";
import { shortcodes } from "./emoji";
import { clock } from "./GroupCall";
import { ChevronDownIcon, DocumentIcon, GifIcon, MembersIcon, PinIcon, PlusCircleIcon, PlusIcon, PowerIcon, SearchIcon, SmileIcon, SparkIcon, StickerIcon, UnreadIcon } from "./SocialIcons";
import { useCall } from "./CallProvider";
import { playGroupMessage } from "./sounds";
import { MessageRow, ForwardDialog, AskPanel } from "./MessageRow";
import { ChannelGear, ChannelSettingsDialog } from "./ChannelSettingsDialog";
import { GroupMembers } from "./GroupMembers";
import "./social.css";

type Contact = { id: string; name: string };

/** Groups are opened from a rail of round icons; Home is the direct chats (`home`) and "+" makes or joins groups. */
export function GroupsPanel({ contacts, myName, toast, home }: { contacts: Contact[]; myName: string; toast: PushToast; home: ReactNode }) {
  const [v, setV] = useState<SocialView | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [joining, setJoining] = useState(false);
  const [hub, setHub] = useState("");
  const [code, setCode] = useState("");
  const call = useCall();
  // The voice bar asks to go back to the call: open its group on the voice channel.
  const [voiceAsk, setVoiceAsk] = useState(0);
  useEffect(() => {
    call.onOpenChannel((p) => {
      setOpenId(p.group);
      setVoiceAsk(Date.now());
    });
    return () => call.onOpenChannel(null);
  }, [call]);
  useEffect(() => {
    const onOpenGroup = (e: any) => {
      const d = e.detail as { groupId?: string };
      if (d?.groupId) setOpenId(d.groupId);
    };
    window.addEventListener("social:open-group", onOpenGroup);
    return () => window.removeEventListener("social:open-group", onOpenGroup);
  }, []);
  const [profiles, setProfiles] = useState<Record<string, Profile>>({});
  const loadProfiles = useCallback(() => {
    Promise.all([social.profiles(), social.profile(), social.view()])
      .then(([theirs, mine, view]) => {
        const map: Record<string, Profile> = { ...theirs };
        if (view?.me && mine) {
          map[view.me] = mine;
        }
        setProfiles(map);
      })
      .catch(() => {});
  }, []);
  useEffect(() => {
    loadProfiles();
    const un = [listen("social:profile", loadProfiles), listen("social:changed", loadProfiles)];
    return () => {
      un.forEach((p) => p.then((f) => f()));
    };
  }, [loadProfiles]);
  const stripFor = (group?: { id: string; name: string }) => (
    <ProfileStrip myName={myName} muted={call.muted} deafened={call.deafened} setMuted={call.setMuted} setDeafened={call.setDeafened} toast={toast} extras={!!v?.switches.extras} group={group} />
  );
  const strip = stripFor();
  const prevUnreadRef = useRef<number | null>(null);
  const load = useCallback(() => {
    social
      .view()
      .then((view) => {
        setV(view);
        const totalUnread = (view?.groups ?? []).reduce(
          (sum, g) => sum + g.channels.reduce((cs, c) => cs + c.unread, 0),
          0,
        );
        if (prevUnreadRef.current !== null && totalUnread > prevUnreadRef.current) {
          playGroupMessage();
        }
        prevUnreadRef.current = totalUnread;
      })
      .catch(() => setV(null));
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
          return (
            <button key={g.id} className={g.id === openId ? "social-pill active" : unread > 0 ? "social-pill unread" : "social-pill"} title={g.name} onClick={() => setOpenId(g.id)}>
              {g.name
                .split(/\s+/)
                .map((w) => w.slice(0, 1))
                .join("")
                .slice(0, 3)
                .toUpperCase()}
              {unread > 0 && <span className="social-badge">{unread > 99 ? "99+" : unread}</span>}
            </button>
          );
        })}
        <button className={openId === "+" ? "social-pill add active" : "social-pill add"} onClick={() => setOpenId("+")} title={t("Make or join a group")} aria-label={t("Make or join a group")}>
          +
        </button>
        <OpenFullApp />
      </nav>
      {!openId || !v ? (
        <div className="people-page">
          {home}
          {strip}
        </div>
      ) : open ? (
        <Group key={open.id} g={open} me={v.me} myName={myName} switches={v.switches} run={run} toast={toast} profiles={profiles} voiceAsk={voiceAsk} strip={stripFor({ id: open.id, name: open.name })} contacts={contacts} />
      ) : !groupsOn ? (
        <div className="social-home" style={{ maxWidth: 640, margin: "40px auto" }}>
          <h3>{t("Groups")}</h3>
          <p className="small muted">{t("Invite-only groups that run on their members' PCs, encrypted end to end. Nothing is listed anywhere.")}</p>
          <button type="button" className="btn-discord" onClick={() => run(() => social.setSwitches({ ...v.switches, groups: true }))}>{t("Turn on groups")}</button>
        </div>
      ) : (
        <div className="social-overview">
          <nav className="social-overview-side" aria-label={t("Groups")}>
            <div className="social-overview-head">
              <span>{t("Groups")}</span>
              <button
                type="button"
                className="icon-ghost"
                title={t("New group")}
                onClick={() => {
                  const name = window.prompt(t("Group name"));
                  if (name) run(() => social.create(name, myName));
                }}
              >
                <PlusIcon size={16} />
              </button>
            </div>
            <ul className="social-overview-list">
              {v.groups.map((g) => {
                const unread = g.channels.reduce((a, c) => a + c.unread, 0);
                return (
                  <li key={g.id}>
                    <button type="button" className="social-overview-item" onClick={() => setOpenId(g.id)}>
                      <span className="ellipsis">{g.name}</span>
                      <span className="small muted">
                        {unread > 0 ? (
                          <span className="social-badge">{unread}</span>
                        ) : g.members.length === 1 ? (
                          t("1 member")
                        ) : (
                          t("{n} members", { n: g.members.length })
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
            <DirectMessages groups={v.groups} run={run} toast={toast} />
            <div style={{ marginTop: "auto" }}>
              {strip}
            </div>
          </nav>
          <main className="social-overview-main">
            <div className="row" style={{ marginBottom: 16 }}>
              <button
                type="button"
                className="btn-discord"
                onClick={() => {
                  const name = window.prompt(t("Group name"));
                  if (name) run(() => social.create(name, myName));
                }}
              >
                + {t("New group")}
              </button>
              <button type="button" className="btn-discord secondary" onClick={() => setJoining((j) => !j)}>
                {t("Join with an invite")}
              </button>
            </div>
            {joining && (
              <div className="row" style={{ marginBottom: 16 }}>
                <select value={hub} onChange={(e) => setHub(e.target.value)} aria-label={t("Who invited you")}>
                  <option value="">{t("Who invited you")}</option>
                  {contacts.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
                <input value={code} onChange={(e) => setCode(e.target.value)} placeholder={t("Invite code")} />
                <button
                  type="button"
                  className="btn-discord"
                  disabled={!hub || !code.trim()}
                  onClick={() => run(() => social.join(hub, code, myName)).then(() => toast(t("Asked to join. The group appears once they let you in.")))}
                >
                  {t("Join")}
                </button>
              </div>
            )}
            <Networks toast={toast} />
            <details style={{ marginTop: 24 }}>
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
                  ["extras", t("Extras, free for everyone: profile colours and frames, animated pictures, your name per group, your own emoji, 12 MB files and HD screen share")],
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
              <button type="button" className="btn-discord danger" onClick={() => run(() => social.setSwitches({ ...v.switches, groups: false }))}>{t("Turn off groups")}</button>
            </details>
          </main>
        </div>
      )}
    </div>
  );
}

type PostEvent = Extract<Event, { ev: "post" }>;
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
  voiceAsk,
  strip,
  contacts,
}: {
  g: GroupView;
  me: string;
  myName: string;
  switches: Switches;
  run: (f: () => Promise<unknown>) => Promise<void>;
  toast: PushToast;
  profiles: Record<string, Profile>;
  voiceAsk: number;
  strip: ReactNode;
  contacts: Contact[];
}) {
  const [channel, setChannel] = useState(g.channels.find((c) => c.kind !== "voice")?.id ?? "general");
  const [thread, setThread] = useState<string | null>(null);
  const [msgs, setMsgs] = useState<GroupMessage[]>([]);
  const [panel, setPanel] = useState<"chat" | "voice" | "members" | "moderation" | "doc" | "networks">("chat");
  const [summary, setSummary] = useState<string | null>(null);
  const call = useCall();
  const inCall = call.place?.group === g.id ? call.place.channel : null;
  const callPeople = inCall ? call.people : [];
  const joinCall = (id: string) => {
    const c = g.channels.find((x) => x.id === id);
    call.join({ group: g.id, groupName: g.name, channel: id, channelName: c?.name ?? "", me, myName: nameIn(profiles[me], g.id, myName), voice: c, hd: !!switches.extras, moderator: mod });
    setPanel("voice");
  };
  useEffect(() => {
    if (voiceAsk && inCall) setPanel("voice");
    // Only when the voice bar asks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [voiceAsk]);
  // The call's tiles show here while its channel is open; the call itself lives above every page.
  const callHost = useRef<HTMLDivElement>(null);
  const showCall = call.show;
  useEffect(() => {
    if (panel !== "voice" || !inCall) return;
    showCall(callHost.current);
    return () => showCall(null);
  }, [panel, inCall, showCall]);
  const [tick, setTick] = useState(Date.now());
  useEffect(() => {
    if (!inCall) return;
    const id = window.setInterval(() => setTick(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [inCall]);
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
  const [pinsOpen, setPinsOpen] = useState(false);
  const pinsBox = useRef<HTMLDivElement>(null);
  useDismiss(pinsOpen, () => setPinsOpen(false), pinsBox, ".pins-toggle");
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState("");
  const [replyTo, setReplyTo] = useState<GroupMessage | null>(null);
  const [typing, setTyping] = useState<Record<string, number>>({});
  const [seen, setSeen] = useState<[string, number][]>([]);
  const [stickerNote, setStickerNote] = useState(false);
  useEffect(() => {
    if (!inCall) setPanel((p) => (p === "voice" ? "chat" : p));
  }, [inCall]);
  // Where the user stopped reading, taken when the channel opens (opening it marks it read).
  const [readAt, setReadAt] = useState(0);
  const [selected, setSelected] = useState<string[] | null>(null);
  const [asking, setAsking] = useState<GroupMessage | null>(null);
  const [forwarding, setForwarding] = useState<GroupMessage[] | null>(null);
  const [insert, setInsert] = useState<{ text: string; n: number } | null>(null);
  const [card, setCard] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [serverMenuOpen, setServerMenuOpen] = useState(false);
  const serverMenuRef = useRef<HTMLDivElement>(null);
  useDismiss(serverMenuOpen, () => setServerMenuOpen(false), serverMenuRef, ".social-server-header");
  const [collapseText, setCollapseText] = useState(false);
  const [collapseVoice, setCollapseVoice] = useState(false);
  const [collapseShared, setCollapseShared] = useState(false);
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
  const lastMsgIdRef = useRef<string | null>(null);
  const load = useCallback(() => {
    social
      .messages(g.id, channel, thread)
      .then((ms) => {
        setMsgs(ms);
        const last = ms[ms.length - 1];
        if (last && last.author !== me) {
          if (lastMsgIdRef.current !== null && lastMsgIdRef.current !== last.id) {
            playGroupMessage();
          }
        }
        if (last) lastMsgIdRef.current = last.id;
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
  // Sent messages show at once, greyed until the send is done.
  const [pending, setPending] = useState<{ ev: PostEvent; at: number; failed: string | null }[]>([]);
  const submit = useCallback(
    (ev: PostEvent) => {
      setPending((p) => [...p.filter((x) => x.ev.id !== ev.id), { ev, at: Date.now(), failed: null }]);
      social
        .send(g.id, ev.channel, ev)
        .then(load)
        .catch((e) => setPending((p) => p.map((x) => (x.ev.id === ev.id ? { ...x, failed: errorText(e) } : x))));
    },
    [g.id, load],
  );
  useEffect(() => {
    setPending((p) => (p.some((x) => msgs.some((m) => m.id === x.ev.id)) ? p.filter((x) => !msgs.some((m) => m.id === x.ev.id)) : p));
  }, [msgs]);
  const waiting = pending.filter((x) => x.ev.channel === channel && x.ev.thread === thread && !msgs.some((m) => m.id === x.ev.id));
  const typers = Object.keys(typing).map((id) => g.members.find((m) => m.id === id)?.name ?? t("Someone"));
  const shown = msgs.filter((m) => (!query.trim() || m.text.toLowerCase().includes(query.toLowerCase()) || m.author_name.toLowerCase().includes(query.toLowerCase())));
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
  // Like Discord: who's online, by role, then everyone offline.
  const isOn = (m: MemberView) => m.online !== false && profiles[m.id]?.presence !== "invisible";
  const byRole = [
    ...(["owner", "moderator", "member"] as const).map((r) => [r, g.members.filter((m) => m.role === r && isOn(m))] as const),
    ["offline", g.members.filter((m) => !isOn(m))] as const,
  ].filter(([, ms]) => ms.length > 0);
  return (
    <div className={showMembers ? "social-group with-members" : "social-group"}>
      <nav className="social-channels" aria-label={t("Channels")}>
        <div
          className="social-server-header"
          onClick={() => setServerMenuOpen((o) => !o)}
          role="button"
          tabIndex={0}
          aria-haspopup="true"
          aria-expanded={serverMenuOpen}
        >
          <span className="ellipsis">{g.name}</span>
          <ChevronDownIcon open={serverMenuOpen} size={16} />
        </div>
        {serverMenuOpen && (
          <div className="social-server-popover" ref={serverMenuRef} role="menu">
            <button
              type="button"
              onClick={() => {
                setServerMenuOpen(false);
                setPanel("members");
              }}
            >
              <span>{t("Members and invites")}</span>
              <MembersIcon size={16} />
            </button>
            <button
              type="button"
              onClick={() => {
                setServerMenuOpen(false);
                setPanel("doc");
              }}
            >
              <span>{t("Shared documents")}</span>
              <DocumentIcon size={16} />
            </button>
            <button
              type="button"
              onClick={() => {
                setServerMenuOpen(false);
                setPanel("networks");
              }}
            >
              <span>{t("Networks (Virtual LAN)")}</span>
              <PowerIcon size={16} />
            </button>
            {mod && (
              <>
                <hr />
                <button
                  type="button"
                  onClick={() => {
                    setServerMenuOpen(false);
                    const name = window.prompt(t("Channel name"));
                    if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "text", adults_only: false }));
                  }}
                >
                  <span>{t("Create channel")}</span>
                  <PlusIcon size={16} />
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setServerMenuOpen(false);
                    setPanel("moderation");
                  }}
                >
                  <span>{t("Moderation")}</span>
                  {g.held + g.reports > 0 && <span className="social-badge">{g.held + g.reports}</span>}
                </button>
              </>
            )}
          </div>
        )}

        {/* Text channels category */}
        <button
          type="button"
          className="social-cat-header"
          onClick={() => setCollapseText((c) => !c)}
          aria-expanded={!collapseText}
        >
          <ChevronDownIcon open={!collapseText} size={12} />
          <span className="social-cat-title">{t("Text channels")}</span>
          {mod && (
            <span
              className="social-cat-add"
              title={t("Add channel")}
              onClick={(e) => {
                e.stopPropagation();
                const name = window.prompt(t("Channel name"));
                if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "text", adults_only: false }));
              }}
            >
              <PlusIcon size={14} />
            </span>
          )}
        </button>
        {!collapseText &&
          g.channels
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
                <span className="ellipsis">{c.name}</span>
                {c.private && <LockIcon />}
                {c.kind === "announcements" && <span className="small muted"> {t("(announcements)")}</span>}
                {c.mentions > 0 ? (
                  <span className="social-badge" title={t("Mentions of you")}>
                    {c.mentions > 99 ? "99+" : c.mentions}
                  </span>
                ) : c.unread > 0 ? (
                  <span className="social-badge" title={t("Unread messages")}>
                    {c.unread > 99 ? "99+" : c.unread}
                  </span>
                ) : null}
                {mod && <ChannelGear onOpen={() => setEditing(c.id)} />}
              </button>
            ))}

        {/* Voice channels category */}
        <button
          type="button"
          className="social-cat-header"
          onClick={() => setCollapseVoice((c) => !c)}
          aria-expanded={!collapseVoice}
        >
          <ChevronDownIcon open={!collapseVoice} size={12} />
          <span className="social-cat-title">{t("Voice channels")}</span>
          {mod && (
            <span
              className="social-cat-add"
              title={t("Add voice channel")}
              onClick={(e) => {
                e.stopPropagation();
                const name = window.prompt(t("Voice channel name"));
                if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "voice", adults_only: false }));
              }}
            >
              <PlusIcon size={14} />
            </span>
          )}
        </button>
        {!collapseVoice &&
          g.channels
            .filter((c) => c.kind === "voice")
            .map((c) => (
              <div key={c.id}>
                <button
                  className={inCall === c.id ? "social-ch active" : "social-ch"}
                  onClick={() => joinCall(c.id)}
                  title={inCall === c.id ? t("Open the call") : t("Join the call")}
                >
                  <SpeakerIcon />
                  <span className="ellipsis">{c.name}</span>
                  {c.private && <LockIcon />}
                  {c.user_limit > 0 && <span className="small muted">{t("{n} max", { n: c.user_limit })}</span>}
                  {mod && <ChannelGear onOpen={() => setEditing(c.id)} />}
                </button>
                {inCall === c.id && (
                  <ul className="social-voice-people">
                    {callPeople.map((p) => (
                      <li key={p.id} className={p.speaking ? "speaking" : ""}>
                        <span className="social-voice-avatar"><Avatar name={p.name} p={profiles[p.id]} size={22} id={p.id} /></span>
                        <span className="ellipsis">{p.name}</span>
                        <span className="small muted">{clock(tick - p.since)}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ))}

        {/* Shared category */}
        <button
          type="button"
          className="social-cat-header"
          onClick={() => setCollapseShared((c) => !c)}
          aria-expanded={!collapseShared}
        >
          <ChevronDownIcon open={!collapseShared} size={12} />
          <span className="social-cat-title">{t("Shared")}</span>
        </button>
        {!collapseShared && (
          <>
            <button
              type="button"
              className={panel === "doc" ? "social-ch active" : "social-ch"}
              onClick={() => setPanel("doc")}
            >
              <DocumentIcon size={16} />
              <span className="ellipsis">{t("Documents")}</span>
            </button>
            <button
              type="button"
              className={panel === "networks" ? "social-ch active" : "social-ch"}
              onClick={() => setPanel("networks")}
            >
              <PowerIcon size={16} />
              <span className="ellipsis">{t("Networks")}</span>
            </button>
          </>
        )}
        <div style={{ marginTop: "auto" }}>
          {strip}
        </div>
      </nav>
      <main className="social-main">
        {panel === "voice" && inCall && <div className="call-host" ref={callHost} />}
        {panel === "members" && <GroupMembers g={g} me={me} run={run} contacts={contacts} toast={toast} />}
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
                if (voice) joinCall(voice.id);
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
              {!thread && ch?.topic && <span className="small muted social-topic ellipsis" title={ch.topic}>{ch.topic}</span>}
              {thread && <span className="small muted">{t("Thread")}</span>}
              <span className="social-head-tools">
                {firstNew && (
                  <button className="icon-ghost" onClick={() => jump(firstNew.id)} title={t("Jump to the first unread message")} aria-label={t("Jump to the first unread message")}>
                    <UnreadIcon />
                  </button>
                )}
                <button className="icon-ghost" onClick={() => social.catchUp(g.id, channel).then(setSummary).catch((e) => toast(errorText(e)))} title={t("Catch me up")} aria-label={t("Catch me up")}>
                  <SparkIcon />
                </button>
                <span className="pins-wrap">
                  <button className={pinsOpen ? "icon-ghost pins-toggle active" : "icon-ghost pins-toggle"} onClick={() => setPinsOpen((p) => !p)} title={t("Pinned messages")} aria-label={t("Pinned messages")} aria-expanded={pinsOpen}>
                    <PinIcon />
                  </button>
                  {pinsOpen && (
                    <div className="pins-pop" ref={pinsBox} role="dialog" aria-label={t("Pinned messages")}>
                      <strong>{t("Pinned messages")}</strong>
                      {msgs.filter((m) => m.pinned && !m.deleted).length === 0 && <p className="small muted">{t("No pinned messages here.")}</p>}
                      {msgs
                        .filter((m) => m.pinned && !m.deleted)
                        .map((m) => (
                          <button
                            key={m.id}
                            className="pins-item"
                            onClick={() => {
                              setPinsOpen(false);
                              jump(m.id);
                            }}
                          >
                            <span className="small">
                              <strong>{nameIn(profiles[m.author], g.id, m.author_name)}</strong> <span className="muted">{new Date(m.at).toLocaleDateString()}</span>
                            </span>
                            <span className="pins-text">{m.text || t("(a file)")}</span>
                          </button>
                        ))}
                    </div>
                  )}
                </span>
                <button className={showMembers ? "icon-ghost active" : "icon-ghost"} aria-pressed={showMembers} title={showMembers ? t("Hide the member list") : t("Show the member list")} aria-label={t("Member list")} onClick={() => setShowMembers((s) => !s)}>
                  <MembersIcon />
                </button>
                <label className={searching || query ? "social-search open" : "social-search"}>
                  <input type="search" value={query} onFocus={() => setSearching(true)} onBlur={() => setSearching(false)} onChange={(e) => setQuery(e.target.value)} placeholder={t("Search")} aria-label={t("Search messages")} />
                  <SearchIcon size={16} />
                </label>
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
              {shown.length === 0 && (
                query ? (
                  <p className="small muted" style={{ padding: "24px 20px" }}>{t("No messages match.")}</p>
                ) : (
                  <div className="social-welcome-hero">
                    <div className="social-welcome-icon">#</div>
                    <h3>{t("Welcome to #{name}!", { name: ch?.name ?? g.name })}</h3>
                    <p>{t("This is the start of the #{name} channel.", { name: ch?.name ?? g.name })}</p>
                  </div>
                )
              )}
              {shown.map((m, i) => {
                const prev = shown[i - 1];
                const isNew = firstNew?.id === m.id && !query;
                const newDay = !prev || new Date(prev.at).toDateString() !== new Date(m.at).toDateString();
                const compact = !isNew && !newDay && !!prev && prev.author === m.author && m.at - prev.at < GROUP_GAP && !m.reply_to && !prev.deleted;
                return (
                  <Fragment key={m.id}>
                  {newDay && (
                    <div className="social-day" role="separator">
                      <span>{new Date(m.at).toLocaleDateString([], { year: "numeric", month: "long", day: "numeric" })}</span>
                    </div>
                  )}
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
              {waiting.map((x) => (
                <div key={x.ev.id} className={x.failed ? "social-msg social-pending-msg failed" : "social-msg social-pending-msg"} aria-busy={!x.failed}>
                  <div className="social-msg-head">
                    <span className="social-msg-name">{myName}</span> <span className="small muted">{x.failed ? t("Not sent") : t("Sending...")}</span>
                  </div>
                  <div className="chat-text">{x.ev.text}</div>
                  {x.failed && (
                    <div className="row small">
                      <span className="muted">{x.failed}</span>
                      <button className="link" onClick={() => submit(x.ev)}>
                        {t("Retry")}
                      </button>
                      <button className="link" onClick={() => setPending((p) => p.filter((y) => y.ev.id !== x.ev.id))}>
                        {t("Delete")}
                      </button>
                    </div>
                  )}
                </div>
              ))}
              {seenBy.length > 0 && <div className="small muted social-seen">{t("Seen by {names}", { names: seenBy.join(", ") })}</div>}
            </div>
            <div className="small muted social-typing" aria-live="polite">
              {typers.length === 1 ? t("{name} is typing...", { name: typers[0] }) : typers.length > 1 ? t("Several people are typing...") : ""}
            </div>
            {(ch?.kind === "announcements" && !mod) || ch?.can_send === false ? (
              <p className="small muted">{ch?.kind === "announcements" ? t("Only moderators post in this channel.") : t("You can't post in this channel.")}</p>
            ) : (
              <Composer
                g={g}
                channel={channel}
                thread={thread}
                replyTo={replyTo}
                clearReply={() => setReplyTo(null)}
                switches={switches}
                placeholder={t("Message #{name}", { name: ch?.name ?? "" })}
                toast={toast}
                insert={insert}
                submit={submit}
              />
            )}
          </>
        )}
      </main>
      {asking && <AskPanel key={asking.id} m={asking} onClose={() => setAsking(null)} toast={toast} />}
      {editing && g.channels.some((c) => c.id === editing) && (
        <ChannelSettingsDialog
          g={g}
          c={g.channels.find((c) => c.id === editing)!}
          onClose={() => setEditing(null)}
          onSave={(settings) => run(() => social.act(g.id, { op: "edit_channel", id: editing, settings })).then(() => setEditing(null))}
          onDelete={async () => {
            if (!(await confirmed(t("Delete this channel and its messages for everyone?")))) return;
            await run(() => social.act(g.id, { op: "remove_channel", id: editing }));
            setEditing(null);
          }}
        />
      )}
      {forwarding && <ForwardDialog g={g} me={me} onPick={forward} onClose={() => setForwarding(null)} />}
      {card && (
        <ProfileCard
          p={profiles[card]}
          id={card}
          self={card === me}
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
                {r === "owner" ? t("Owner") : r === "moderator" ? t("Moderators") : r === "member" ? t("Online") : t("Offline")} - {ms.length}
              </div>
              {ms.map((m) => (
                <button
                  key={m.id}
                  type="button"
                  className={`social-member ${roleClass(m)}${r === "offline" ? " offline" : ""}`}
                  style={{ color: profiles[m.id]?.accent || undefined }}
                  onClick={() => setCard(m.id)}
                >
                  <Avatar name={m.name} p={profiles[m.id]} size={32} id={m.id} />
                  <span className="social-member-text">
                    <strong>{nameIn(profiles[m.id], g.id, m.name)}</strong>
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
  toast,
  insert,
  submit,
}: {
  g: GroupView;
  channel: string;
  thread: string | null;
  replyTo: GroupMessage | null;
  clearReply: () => void;
  switches: Switches;
  placeholder: string;
  toast: PushToast;
  insert: { text: string; n: number } | null;
  submit: (ev: PostEvent) => void;
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
    submit({ ev: "post", id: newId(), channel, text: shortcodes(body), reply_to: replyTo?.id ?? null, thread, mentions: mentionsIn(body, g.members), files: withFiles });
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
  // Files over the message limit go peer to peer instead: a share link anyone in the group can download from.
  const [bigBusy, setBigBusy] = useState("");
  const sendBig = async (file: File) => {
    try {
      setBigBusy(t("Getting {name} ready...", { name: file.name }));
      const path = await saveDropped(file, (done) => setBigBusy(t("Getting {name} ready... {pct}%", { name: file.name, pct: Math.floor((done / Math.max(1, file.size)) * 100) })));
      const made = await shareMake(path);
      post(made.link, []);
    } catch (e) {
      toast(t("Big files go peer to peer: turn on Network and File sharing in Settings. ({why})", { why: errorText(e) }));
    } finally {
      setBigBusy("");
    }
  };
  const addFiles = async (list: FileList | File[]) => {
    try {
      // 3 MB a message, or 12 MB with Extras on (counted as base64).
      const limit = switches.extras ? 16_000_000 : 4_000_000;
      const all = [...list];
      const big = all.filter((f) => (f.size * 4) / 3 > limit);
      for (const f of big) await sendBig(f);
      const read = await Promise.all(all.filter((f) => !big.includes(f)).map(readFile));
      const size = [...files, ...read].reduce((n, f) => n + f.data.length, 0);
      if (size > limit) return toast(t("Those files together are too big for one message: send them one at a time."));
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
      {bigBusy && <div className="small muted">{bigBusy}</div>}
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
      {picker === "emoji" && (
        <EmojiPicker
          onPick={(e) => setText((x) => x + e)}
          onClose={() => setPicker(null)}
          onCustom={
            switches.extras
              ? (f) => {
                  setPicker(null);
                  post("", [f]);
                }
              : undefined
          }
        />
      )}
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
        <button className="icon-ghost compose-add" onClick={() => fileIn.current?.click()} title={t("Send a file, picture, video or sound")} aria-label={t("Attach")}>
          <PlusCircleIcon size={22} />
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
        {switches.gif_search && (
          <button
            aria-label={t("GIF")}
            title={t("GIF")}
            className={picker === "gif" ? "icon-ghost picker-toggle active" : "icon-ghost picker-toggle"}
            onClick={() => {
              setGifStart(undefined);
              setPicker((p) => (p === "gif" ? null : "gif"));
            }}
          >
            <GifIcon />
          </button>
        )}
        <button className={picker === "sticker" ? "icon-ghost picker-toggle active" : "icon-ghost picker-toggle"} onClick={() => setPicker((p) => (p === "sticker" ? null : "sticker"))} title={t("Stickers")} aria-label={t("Stickers")}>
          <StickerIcon />
        </button>
        <button className={picker === "emoji" ? "icon-ghost picker-toggle active" : "icon-ghost picker-toggle"} onClick={() => setPicker((p) => (p === "emoji" ? null : "emoji"))} title={t("Emoji")} aria-label={t("Emoji")}>
          <SmileIcon />
        </button>
        <VoiceRecorder onDone={(f) => post("", [f])} />
      </div>
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
    <div className="group-doc-pane">
      <div className="group-doc-bar">
        <select value={name} onChange={(e) => setName(e.target.value)} aria-label={t("Document")}>
          {[...new Set([...g.docs, name])].map((d) => (
            <option key={d}>{d}</option>
          ))}
        </select>
        <button
          type="button"
          className="btn-discord secondary"
          onClick={() => {
            const n = window.prompt(t("Document name (a song sheet, plans, notes)"));
            if (n) setName(n);
          }}
        >
          + {t("New document")}
        </button>
        <span className="grow" />
        <button type="button" className="btn-discord" onClick={() => run(() => social.docEdit(g.id, name, text))}>
          {t("Save for everyone")}
        </button>
      </div>
      <textarea className="group-doc" value={text} onChange={(e) => setText(e.target.value)} aria-label={t("Document text")} />
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

function LockIcon() {
  return (
    <svg className="social-icon" width="12" height="12" viewBox="0 0 16 16" role="img" aria-label={t("Private")}>
      <path fill="currentColor" d="M4 7V5a4 4 0 118 0v2h1v8H3V7zm2 0h4V5a2 2 0 10-4 0z" />
    </svg>
  );
}
