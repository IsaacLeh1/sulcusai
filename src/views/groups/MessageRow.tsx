// SPDX-License-Identifier: AGPL-3.0-only
// Message rows, message actions menu, forwarding dialog and message explanation panel.
import { useRef, useState } from "react";
import { useDismiss } from "../../useDismiss";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { confirmed } from "../../confirm";
import { social, type FileRef, type GroupMessage, type GroupView, type MemberView, type Profile } from "./socialApi";
import { Avatar, nameIn } from "./Profiles";
import { Attachments, EmojiPicker, isShareLink, LinkEmbeds, RichText, ShareCard } from "./ChatParts";
import { linksIn } from "./chatText";
import { CogIcon, MoreIcon, ReplyIcon, SmileIcon } from "./SocialIcons";

export const QUICK = ["👍", "❤️", "😂", "🎉", "😮", "😢"];

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

const roleClass = (m?: MemberView) => (m?.assistant ? "role-assistant" : m?.role === "owner" ? "role-owner" : m?.role === "moderator" ? "role-mod" : "");

export function MessageRow({
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
  const menuRef = useRef<HTMLDivElement>(null);
  useDismiss(!!menu, () => (setMenu(null), setRemind(false)), menuRef);
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
  const openContextMenu = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest(OWN_CLICK) || window.getSelection()?.toString()) return;
    e.preventDefault();
    if (selecting) return h.onSelect();
    setMenu({ x: Math.min(e.clientX, window.innerWidth - 260), y: Math.min(e.clientY, window.innerHeight - 420) });
  };
  const handleClick = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest(OWN_CLICK)) return;
    if (selecting) {
      e.preventDefault();
      h.onSelect();
    }
  };
  const speak = () => {
    speechSynthesis.cancel();
    speechSynthesis.speak(new SpeechSynthesisUtterance(translated ?? m.text));
  };
  return (
    <div
      id={`msg-${m.id}`}
      className={`social-msg${compact ? " compact" : ""}${mentionsMe ? " mentioned" : ""}${menu ? " menu-open" : ""}${selected ? " selected" : ""}`}
      onClick={handleClick}
      onContextMenu={openContextMenu}
      tabIndex={0}
      onKeyDown={(e) => (e.key === "Enter" || e.key === "ContextMenu") && e.target === e.currentTarget && setMenu({ x: 80, y: (e.currentTarget.getBoundingClientRect().top || 80) + 20 })}
      aria-label={t("Message from {name}. Press Enter for actions.", { name: m.author_name })}
    >
      <div className="social-msg-hoverbar" role="toolbar" aria-label={t("Message actions")}>
        <button
          type="button"
          className="quick-emoji"
          title={t("React with 👍")}
          onClick={(e) => {
            e.stopPropagation();
            ev({ ev: "react", id: m.id, reaction: "👍", on: !(m.reactions?.["👍"] ?? []).includes(me) });
          }}
        >
          👍
        </button>
        <button
          type="button"
          className="quick-emoji"
          title={t("React with ❤️")}
          onClick={(e) => {
            e.stopPropagation();
            ev({ ev: "react", id: m.id, reaction: "❤️", on: !(m.reactions?.["❤️"] ?? []).includes(me) });
          }}
        >
          ❤️
        </button>
        <button
          type="button"
          title={t("Add reaction")}
          onClick={(e) => {
            e.stopPropagation();
            setPicking((p) => !p);
          }}
        >
          <SmileIcon size={16} />
        </button>
        <button
          type="button"
          title={t("Reply")}
          onClick={(e) => {
            e.stopPropagation();
            h.onReply();
          }}
        >
          <ReplyIcon size={16} />
        </button>
        {m.author === me && (
          <button
            type="button"
            title={t("Edit")}
            onClick={(e) => {
              e.stopPropagation();
              setEditing(m.text);
            }}
          >
            <CogIcon size={15} />
          </button>
        )}
        <button
          type="button"
          title={t("More")}
          onClick={(e) => {
            e.stopPropagation();
            const rect = e.currentTarget.getBoundingClientRect();
            setMenu({ x: Math.min(rect.right - 240, window.innerWidth - 260), y: Math.min(rect.bottom + 4, window.innerHeight - 420) });
          }}
        >
          <MoreIcon size={16} />
        </button>
      </div>
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
          <button type="button" className="social-msg-avatar" onClick={h.onProfile} aria-label={t("{name}'s profile", { name: m.author_name })}>
            <Avatar name={m.author_name} p={profile} id={m.author} />
          </button>
          <button
            type="button"
            className={`link social-msg-name ${roleClass(author)}`}
            style={{ color: profile?.accent || undefined }}
            onClick={h.onProfile}
          >
            {nameIn(profile, g.id, m.author_name)}
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
          m.text && (isShareLink(m.text) ? <ShareCard link={m.text} /> : <RichText text={translated ?? m.text} mentionable={names} />)
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
        <div ref={menuRef} className="social-msg-menu" role="menu" style={{ left: menu.x, top: menu.y }}>
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
export function ForwardDialog({ g, me, onPick, onClose }: { g: GroupView; me: string; onPick: (to: { channel: string } | { member: string }) => void; onClose: () => void }) {
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
export function AskPanel({ m, onClose, toast }: { m: GroupMessage; onClose: () => void; toast: PushToast }) {
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
