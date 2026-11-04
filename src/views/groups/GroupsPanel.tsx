// SPDX-License-Identifier: AGPL-3.0-only
// Groups: invite-only, run on their members' PCs. Channels, threads,
// reactions, pins, moderation, reports, catch-me-up and shared documents.
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { confirmed } from "../../confirm";
import { mentionsIn, newId, social, type Dm, type GroupMessage, type GroupView, type ModQueue, type SocialView } from "./socialApi";
import { GroupCall } from "./GroupCall";

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
  if (open) return <Group g={open} me={v.me} myName={myName} onBack={() => setOpenId(null)} run={run} toast={toast} />;
  return (
    <section className="card groups">
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
        <button onClick={() => run(() => social.setSwitches({ ...v.switches, groups: false }))}>{t("Turn off groups")}</button>
      </details>
    </section>
  );
}

function Group({ g, me, myName, onBack, run, toast }: { g: GroupView; me: string; myName: string; onBack: () => void; run: (f: () => Promise<unknown>) => Promise<void>; toast: PushToast }) {
  const [channel, setChannel] = useState(g.channels[0]?.id ?? "general");
  const [thread, setThread] = useState<string | null>(null);
  const [msgs, setMsgs] = useState<GroupMessage[]>([]);
  const [text, setText] = useState("");
  const [replyTo, setReplyTo] = useState<GroupMessage | null>(null);
  const [panel, setPanel] = useState<"chat" | "members" | "moderation" | "doc">("chat");
  const [summary, setSummary] = useState<string | null>(null);
  const [inCall, setInCall] = useState<string | null>(null);
  const endCall = useCallback(() => setInCall(null), []);
  const mod = g.role === "moderator" || g.role === "owner";
  const load = useCallback(() => {
    social.messages(g.id, channel, thread).then(setMsgs).catch((e) => toast(errorText(e)));
  }, [g.id, channel, thread, toast]);
  useEffect(() => {
    load();
    const un = listen("social:changed", load);
    return () => {
      un.then((f) => f());
    };
  }, [load]);
  const send = () => {
    const body = text.trim();
    if (!body) return;
    setText("");
    setReplyTo(null);
    run(() => social.send(g.id, channel, { ev: "post", id: newId(), channel, text: body, reply_to: replyTo?.id ?? null, thread, mentions: mentionsIn(body, g.members) })).then(load);
  };
  return (
    <section className="card groups">
      <header className="row">
        <button className="link" onClick={onBack}>
          {t("Back")}
        </button>
        <h3>{g.name}</h3>
        <div className="seg" role="tablist">
          {(
            [
              ["chat", t("Chat")],
              ["members", t("Members")],
              ["doc", t("Documents")],
              ...(mod ? [["moderation", t("Moderation")] as const] : []),
            ] as const
          ).map(([k, label]) => (
            <button key={k} role="tab" aria-selected={panel === k} className={panel === k ? "active" : ""} onClick={() => setPanel(k)}>
              {label}
              {k === "moderation" && g.held + g.reports > 0 && <span className="badge subtle">{g.held + g.reports}</span>}
            </button>
          ))}
        </div>
      </header>
      {panel === "members" && <Members g={g} me={me} run={run} />}
      {panel === "moderation" && <Moderation g={g} run={run} />}
      {panel === "doc" && <Doc g={g} run={run} />}
      {panel === "chat" && (
        <div className="group-chat">
          <nav className="group-channels">
            {g.channels.map((c) => (
              <button
                key={c.id}
                className={c.id === channel ? "active" : ""}
                onClick={() => {
                  if (c.kind === "voice") {
                    setInCall(c.id);
                    return;
                  }
                  setChannel(c.id);
                  setThread(null);
                }}
              >
                {c.kind === "voice" ? t("(voice)") + " " : "#"}
                {c.name}
                {c.unread > 0 && <span className="badge subtle">{c.unread}</span>}
              </button>
            ))}
            {mod && (
              <button
                onClick={() => {
                  const name = window.prompt(t("Channel name"));
                  if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "text", adults_only: false }));
                }}
              >
                {t("Add channel")}
              </button>
            )}
            {mod && (
              <button
                onClick={() => {
                  const name = window.prompt(t("Voice channel name"));
                  if (name) run(() => social.act(g.id, { op: "add_channel", id: newId(), name, kind: "voice", adults_only: false }));
                }}
              >
                {t("Add voice channel")}
              </button>
            )}
          </nav>
          <div className="group-messages">
            {inCall && <GroupCall key={inCall} group={g.id} channel={inCall} myName={myName} onClose={endCall} toast={toast} />}
            <div className="row">
              {thread && (
                <button className="link" onClick={() => setThread(null)}>
                  {t("Back to the channel")}
                </button>
              )}
              <button onClick={() => social.catchUp(g.id, channel).then(setSummary).catch((e) => toast(errorText(e)))}>{t("Catch me up")}</button>
            </div>
            {summary && (
              <div className="note small">
                {summary}
                <button className="link" onClick={() => setSummary(null)}>
                  {t("Close")}
                </button>
              </div>
            )}
            {msgs.map((m) => (
              <MessageRow key={m.id} m={m} g={g} me={me} mod={mod} run={run} toast={toast} onReply={() => setReplyTo(m)} onThread={() => setThread(m.id)} />
            ))}
            {replyTo && (
              <div className="small muted">
                {t("Replying to {name}", { name: replyTo.author_name })}{" "}
                <button className="link" onClick={() => setReplyTo(null)}>
                  {t("Cancel")}
                </button>
              </div>
            )}
            <div className="row">
              <textarea value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && (e.preventDefault(), send())} placeholder={t("Message")} />
              <button onClick={send}>{t("Send")}</button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

const QUICK = ["+1", "thanks", "ha", "?"];

function MessageRow({ m, g, me, mod, run, toast, onReply, onThread }: { m: GroupMessage; g: GroupView; me: string; mod: boolean; run: (f: () => Promise<unknown>) => Promise<void>; toast: PushToast; onReply: () => void; onThread: () => void }) {
  const [shown, setShown] = useState(!m.hidden);
  const [translated, setTranslated] = useState<string | null>(null);
  if (m.deleted) return <div className="small muted">{t("Message deleted")}</div>;
  if (!shown)
    return (
      <button className="link small muted" onClick={() => setShown(true)}>
        {t("Hidden message from someone you blocked or muted")}
      </button>
    );
  const ev = (e: Parameters<typeof social.send>[2]) => run(() => social.send(g.id, m.channel, e));
  return (
    <div className="group-msg">
      <strong>{m.author_name}</strong> <span className="small muted">{new Date(m.at).toLocaleTimeString([], { timeStyle: "short" })}</span>
      {m.pinned && <span className="small muted"> · {t("Pinned")}</span>}
      {m.edited && <span className="small muted"> · {t("edited")}</span>}
      <div>{translated ?? m.text}</div>
      <div className="row small">
        {Object.entries(m.reactions).map(([r, who]) => (
          <button key={r} className={who.includes(me) ? "active" : ""} onClick={() => ev({ ev: "react", id: m.id, reaction: r, on: !who.includes(me) })}>
            {r} {who.length}
          </button>
        ))}
        {QUICK.filter((r) => !m.reactions[r]).map((r) => (
          <button key={r} className="link" onClick={() => ev({ ev: "react", id: m.id, reaction: r, on: true })}>
            {r}
          </button>
        ))}
        <button className="link" onClick={onReply}>
          {t("Reply")}
        </button>
        {!m.thread && (
          <button className="link" onClick={onThread}>
            {m.replies > 0 ? t("Thread ({n})", { n: m.replies }) : t("Start a thread")}
          </button>
        )}
        <button className="link" onClick={() => social.translate(m.text, navigator.language).then(setTranslated).catch((e) => toast(errorText(e)))}>
          {t("Translate")}
        </button>
        {m.author === me && (
          <button
            className="link"
            onClick={() => {
              const text = window.prompt(t("Edit message"), m.text);
              if (text) ev({ ev: "edit", id: m.id, text });
            }}
          >
            {t("Edit")}
          </button>
        )}
        {(m.author === me || mod) && (
          <button className="link" onClick={async () => (await confirmed(t("Delete this message for everyone?"))) && ev({ ev: "delete", id: m.id })}>
            {t("Delete")}
          </button>
        )}
        {mod && (
          <button className="link" onClick={() => ev({ ev: "pin", id: m.id, on: !m.pinned })}>
            {m.pinned ? t("Unpin") : t("Pin")}
          </button>
        )}
        {m.author !== me && (
          <button
            className="link"
            onClick={() => {
              const reason = window.prompt(t("Why are you reporting this? Only this group's moderators see it."));
              if (reason) run(() => social.report(g.id, m.author, m.id, m.text, reason)).then(() => toast(t("Reported to the group's moderators.")));
            }}
          >
            {t("Report")}
          </button>
        )}
      </div>
    </div>
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
              <strong>{d.from_me ? t("You") : d.with_name}</strong>: {d.text}
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
