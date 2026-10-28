// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useEffect, useState, type ReactNode } from "react";
import { api, type GoBack, type MediaItem, type Message, type Versions } from "../api";
import { type Status } from "../chatState";
import { t } from "../i18n";
import { ToolCard } from "../components/AgentCards";
import { MediaInline } from "../components/MediaBits";
import { type PreviewTarget } from "../components/PreviewPanel";
import { useMediaUrl } from "../media";
import { Markdown } from "../components/Markdown";
import { CopyButton } from "../components/CopyButton";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { SpeakButton } from "../components/Voice";
import { BackIcon, EditIcon, ForwardIcon, GlobeIcon, ReloadIcon } from "../components/Icons";

/** Messages as the reader sees them: tool results tucked into their calls,
 *  and an Undo button at the end of each turn that changed files. */
export function renderItems(
  messages: Message[],
  undoable: Set<string>,
  busy: boolean,
  runningCalls: Set<string>,
  undo: (turnId: string) => void,
  helperSteps: Record<string, string[]>,
  openChat: (id: string) => void,
  toast?: PushToast,
  onEnableWeb?: (() => void) | null,
  openPicture?: (i: MediaItem) => void,
  readUpTo = 0,
  openPreview?: (p: PreviewTarget) => void,
  /** For saving pictures made in the chat (always on; `toast` above is read-aloud's). */
  saveToast?: PushToast,
  turns?: TurnActions,
): ReactNode[] {
  const results = new Map<string, Message>();
  // Where versions start (their first message), or, for one with nothing
  // in it yet, the message before.
  const startsAt = new Map<string, Versions>();
  const after = new Map<string, Versions>();
  for (const v of turns?.versions ?? []) (v.first ? startsAt : after).set(v.first ?? v.anchor, v);
  const switcher = (v: Versions) => (
    <VersionSwitch key={`v-${v.anchor}-${v.first}`} v={v} disabled={busy} onSwitch={(to) => turns?.switchTo(v.anchor, to)} />
  );
  // The reply that ends each turn gets "another reply".
  const lastReply = new Map<string, string>();
  {
    let from: string | null = null;
    for (const m of messages) {
      if (m.role === "user" && !m.meta?.added) from = !m.meta?.from_person && !m.meta?.from_chat && !m.id.startsWith("pending-") ? m.id : null;
      else if (m.role === "assistant" && from && m.content && !m.meta?.compacted) lastReply.set(from, m.id);
    }
  }
  const againFor = new Map([...lastReply].map(([user, reply]) => [reply, user]));
  const userTools = (m: Message) => {
    if (!turns || m.meta?.added || m.meta?.from_person || m.meta?.from_chat || m.id.startsWith("pending-")) return {};
    return {
      editing: turns.editing === m.id,
      onEdit: busy ? undefined : () => turns.setEditing(m.id),
      onEditDone: (text: string | null) => (text === null ? turns.setEditing(null) : turns.edit(m.id, text)),
      onGoBack: busy ? undefined : () => turns.goBack(m.id),
    };
  };
  const out: ReactNode[] = [];
  if (after.has("")) out.push(switcher(after.get("")!));
  // "Sent" or "Read" under the newest message (and ones added mid-reply):
  // read once the model started answering after it.
  const lastUser = messages.map((m) => m.role === "user" && !m.meta?.from_person).lastIndexOf(true);
  const receipt = (m: Message, i: number): "sent" | "read" | undefined => {
    if (m.meta?.from_person || (i !== lastUser && !m.meta?.added)) return undefined;
    const answered = messages.slice(i + 1).some((x) => x.role === "assistant" || x.role === "tool");
    return answered || (readUpTo > 0 && m.created_at <= readUpTo) ? "read" : "sent";
  };
  for (const m of messages) if (m.role === "tool" && m.tool_call_id) results.set(m.tool_call_id, m);

  let turnId: string | null = null;
  const closeTurn = (key: string, isLastTurn: boolean) => {
    if (turnId && undoable.has(turnId) && !(busy && isLastTurn)) {
      const id = turnId;
      out.push(
        <div key={`undo-${key}`} className="undo-row">
          <button className="btn ghost small" onClick={() => undo(id)}>{t("↶ Undo file changes from this reply")}</button>
        </div>,
      );
    }
  };
  messages.forEach((m, i) => {
    if (startsAt.has(m.id)) out.push(switcher(startsAt.get(m.id)!));
    if (m.role === "user" && m.meta?.added) {
      // Added mid-reply: still the same turn (and the same Undo).
      out.push(<MessageView key={m.id} m={m} onPicture={openPicture} onOpenChat={openChat} receipt={receipt(m, i)} />);
    } else if (m.role === "user") {
      closeTurn(m.id, false);
      turnId = m.id;
      out.push(<MessageView key={m.id} m={m} onPicture={openPicture} onOpenChat={openChat} receipt={receipt(m, i)} {...userTools(m)} />);
    } else if (m.role === "assistant" && m.meta?.compacted) {
      out.push(<CompactedNote key={m.id} m={m} />);
    } else if (m.role === "assistant") {
      const again = againFor.get(m.id);
      out.push(
        <Fragment key={m.id}>
          {(m.content || m.thinking) && <MessageView m={m} toast={toast} onAgain={again && turns && !busy ? () => turns.again(again) : undefined} />}
          {m.meta?.handoff_to && (
            <div className="undo-row">
              <button className="btn small" onClick={() => openChat(m.meta!.handoff_to!)}>{t("Open the continued chat →")}</button>
            </div>
          )}
          {m.tool_calls?.map((c) => {
            const r = results.get(c.id);
            if (c.name === "request_web" && r?.meta?.web_request) {
              return <WebRequestCard key={c.id} title={r.meta.title} onEnable={onEnableWeb ?? null} />;
            }
            return (
              <Fragment key={c.id}>
                <ToolCard call={c} meta={r?.meta} output={r?.content} running={runningCalls.has(c.id)} liveSteps={helperSteps[c.id]} onOpenChat={openChat} onPreview={openPreview} toast={saveToast} />
                {r?.meta?.media && r.meta.media.length > 0 && <MediaInline items={r.meta.media} onOpen={openPicture} toast={saveToast} />}
              </Fragment>
            );
          })}
        </Fragment>,
      );
    }
    if (after.has(m.id)) out.push(switcher(after.get(m.id)!));
    if (i === messages.length - 1) closeTurn("end", true);
  });
  return out;
}

/** What a chat's messages can do about asking again (`renderItems`). */
export interface TurnActions {
  versions: Versions[];
  editing: string | null;
  setEditing: (id: string | null) => void;
  again: (userMessageId: string) => void;
  edit: (userMessageId: string, text: string) => void;
  switchTo: (anchor: string, to: number) => void;
  /** Back to before this message: what follows is set aside, its file changes put back. */
  goBack: (userMessageId: string) => void;
}

/** "‹ 2 of 3 ›": which version of the chat from here on is shown. */
function VersionSwitch({ v, disabled, onSwitch }: { v: Versions; disabled: boolean; onSwitch: (to: number) => void }) {
  return (
    <div className="version-switch" role="group" aria-label={t("Versions from here on")}>
      <button className="icon-btn" disabled={disabled || v.index === 0} onClick={() => onSwitch(v.index - 1)} title={t("Earlier version")} aria-label={t("Earlier version")}>
        <BackIcon />
      </button>
      <span className="small muted">{t("Version {n} of {count}", { n: v.index + 1, count: v.count })}</span>
      <button className="icon-btn" disabled={disabled || v.index >= v.count - 1} onClick={() => onSwitch(v.index + 1)} title={t("Later version")} aria-label={t("Later version")}>
        <ForwardIcon />
      </button>
    </div>
  );
}

/** A message of the person's being changed, in place. */
function EditBox({ text, onDone }: { text: string; onDone: (text: string | null) => void }) {
  const [value, setValue] = useState(text);
  const send = () => value.trim() && onDone(value.trim());
  return (
    <div className="edit-box">
      <textarea
        className="input"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        autoFocus
        rows={Math.min(12, Math.max(2, value.split("\n").length))}
        aria-label={t("Your message")}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
          } else if (e.key === "Escape") onDone(null);
        }}
      />
      <div className="edit-box-actions">
        <span className="small muted">{t("The old message and its reply are kept: switch back with the arrows.")}</span>
        <button className="btn small" onClick={() => onDone(null)}>{t("Cancel")}</button>
        <button className="btn primary small" disabled={!value.trim() || value.trim() === text.trim()} onClick={send}>{t("Send")}</button>
      </div>
    </div>
  );
}

/** Where a chat was compacted: the model works from this summary instead
 * of the messages above it (they're still here to read). */
function CompactedNote({ m }: { m: Message }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="compacted-note">
      <div className="compacted-line">
        <span>
          {m.meta?.auto ? t("This chat was getting full, so it compacted itself.") : t("Compacted.")}{" "}
          {t("The model now works from a summary of the messages above.")}
        </span>
        <button className="link small" onClick={() => setOpen((o) => !o)}>
          {open ? t("Hide the summary") : t("Show the summary")}
        </button>
      </div>
      {open && (
        <div className="compacted-summary">
          <Markdown text={m.content} />
        </div>
      )}
    </div>
  );
}

// Thought processes the person folded. The rest stay open: the live reply
// is drawn again as the saved one, and folding by itself (or cutting the
// text to a scrolling box) looked like the model deleting what it wrote.
const thoughtFolded = new Set<string>();

export function MessageView({
  m,
  live,
  toast,
  onPicture,
  onOpenChat,
  receipt,
  waiting,
  editing,
  onEdit,
  onEditDone,
  onAgain,
  onGoBack,
}: {
  m: Message;
  live?: Status;
  toast?: PushToast;
  onPicture?: (i: MediaItem) => void;
  onOpenChat?: (id: string) => void;
  receipt?: "sent" | "read";
  waiting?: boolean;
  editing?: boolean;
  onEdit?: () => void;
  onEditDone?: (text: string | null) => void;
  onAgain?: () => void;
  onGoBack?: () => void;
}) {
  const thinkingNow = live === "thinking" && !m.content;
  const summary = m.role === "assistant" ? m.meta?.thought_summary : undefined;
  return (
    <div className={`msg ${m.role}${waiting ? " waiting" : ""}`}>
      {m.role === "user" && m.meta?.images && m.meta.images.length > 0 && (
        <div className="msg-images">
          {m.meta.images.map((id) => (
            <UserImage key={id} id={id} onOpen={onPicture} />
          ))}
        </div>
      )}
      {m.thinking && (
        <details className="thinking" open={!thoughtFolded.has(m.id)} onToggle={(e) => (e.currentTarget.open ? thoughtFolded.delete(m.id) : thoughtFolded.add(m.id))}>
          <summary>{thinkingNow ? t("Thinking…") : t("Thought process")}</summary>
          <div className="thinking-text">{m.thinking}</div>
        </details>
      )}
      {summary && (
        <div className="thought-summary">
          <div className="thought-summary-head">{t("In short")}</div>
          <Markdown text={summary} />
        </div>
      )}
      {m.role === "assistant" && m.content && (m.thinking || summary) && <div className="answer-head">{t("Answer")}</div>}
      {m.role === "user" ? (
        editing && onEditDone ? <EditBox text={m.content} onDone={onEditDone} /> : <div className="bubble">{m.content}</div>
      ) : m.content ? (
        <Markdown text={m.content} />
      ) : null}
      {waiting && (
        <span className="added-note" role="status" title={t("It reads this at its next step, or right after the current reply")}>
          {t("Sent while it worked · waiting for the model to read it")}
        </span>
      )}
      {m.role === "user" && (m.meta?.added || receipt) && (
        <span className={`added-note ${receipt === "read" ? "read" : ""}`} title={receipt === "read" ? t("The model has read this") : t("The model hasn't read this yet")}>
          {[m.meta?.added ? t("Added while it worked") : null, receipt === "read" ? "✓✓ " + t("Read") : receipt === "sent" ? "✓ " + t("Sent") : null].filter(Boolean).join(" · ")}
        </span>
      )}
      {m.meta?.from_person && (
        <span className="added-note">
          {m.role === "user" ? t("From {name}", { name: m.meta.from_name ?? "" }) : t("{name}'s assistant", { name: m.meta.from_name ?? "" }) + (m.meta.model ? " · " + m.meta.model : "")}
        </span>
      )}
      {m.role === "user" && m.meta?.from_chat && (
        <span className="added-note">
          {t("From the assistant in")}{" "}
          <button className="link" onClick={() => onOpenChat?.(m.meta!.from_chat!)}>
            “{m.meta.from_title ?? t("another chat")}”
          </button>
        </span>
      )}
      {live && !m.content && !m.thinking && <span className="typing" aria-label={t("Writing")} />}
      {m.content && !live && !editing && (m.role === "assistant" || m.role === "user") && (
        <div className="msg-tools">
          <CopyButton text={m.content} />
          {onEdit && (
            <button className="icon-btn speak-btn" onClick={onEdit} title={t("Edit and send again")} aria-label={t("Edit and send again")}>
              <EditIcon />
            </button>
          )}
          {onAgain && (
            <button className="icon-btn speak-btn" onClick={onAgain} title={t("Write another reply")} aria-label={t("Write another reply")}>
              <ReloadIcon />
            </button>
          )}
          {onGoBack && (
            <button className="icon-btn speak-btn" onClick={onGoBack} title={t("Go back to here")} aria-label={t("Go back to here")}>
              <BackIcon />
            </button>
          )}
          {toast && m.role === "assistant" && <SpeakButton text={m.content} toast={toast} />}
        </div>
      )}
    </div>
  );
}

/** A picture the user attached, shown above their message. */
function UserImage({ id, onOpen }: { id: string; onOpen?: (i: MediaItem) => void }) {
  // A song attached to work on: heard here, not shown as a picture.
  const [song, setSong] = useState<MediaItem | null>(null);
  useEffect(() => {
    let live = true;
    api.mediaGet(id).then((i) => live && i.kind === "audio" && setSong(i)).catch(() => {});
    return () => {
      live = false;
    };
  }, [id]);
  const src = useMediaUrl(song ? null : { id, mime: "image/jpeg", kind: "image" }, true);
  if (song) return <UserSong item={song} />;
  if (!src) return null;
  // A button, so the keyboard reaches it too.
  return (
    <button className="msg-image-btn" onClick={() => api.mediaGet(id).then((i) => onOpen?.(i))} title={t("Open")}>
      <img src={src} alt={t("Attached picture")} />
    </button>
  );
}

/** A song the user attached, with a player. */
function UserSong({ item }: { item: MediaItem }) {
  const src = useMediaUrl(item);
  return (
    <div className="user-song">
      <span className="small">{item.prompt || t("A song")}</span>
      {src && <audio controls controlsList="nodownload" src={src} />}
    </div>
  );
}

export function AttachChip({ item, onRemove }: { item: MediaItem; onRemove: () => void }) {
  const src = useMediaUrl(item.kind === "image" ? item : null, true);
  return (
    <span className={`attach-chip ${item.kind === "audio" ? "song" : ""}`}>
      {item.kind === "audio" ? <span className="small">{item.prompt || t("A song")}</span> : src ? <img src={src} alt={t("Attached picture")} /> : <span className="media-inline-ph">…</span>}
      <button onClick={onRemove} aria-label={item.kind === "audio" ? t("Remove song") : t("Remove picture")}>✕</button>
    </span>
  );
}

function WebRequestCard({ title, onEnable }: { title: string; onEnable: (() => void) | null }) {
  const reason = title.replace(/^Needs web access: /, "");
  return (
    <div className="web-request" role="note">
      <GlobeIcon className="web-request-icon" />
      <div>
        <strong>{t("This needs web access")}</strong>
        <span className="small muted block">{t("To look up: {reason}. Only the search and the pages it opens go online; the AI stays on this PC.", { reason })}</span>
      </div>
      {onEnable ? (
        <button className="btn primary small" onClick={onEnable}>{t("Turn on web for this chat")}</button>
      ) : (
        <span className="small muted">{t("Web is on")}</span>
      )}
    </div>
  );
}

export function RenameDialog({ title, onClose, onSave }: { title: string; onClose: () => void; onSave: (t: string) => void }) {
  const [value, setValue] = useState(title);
  return (
    <Modal title={t("Rename chat")} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSave(value);
        }}
      >
        <input className="input" value={value} onChange={(e) => setValue(e.target.value)} autoFocus maxLength={80} aria-label={t("Chat name")} />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!value.trim()}>{t("Save")}</button>
        </div>
      </form>
    </Modal>
  );
}

/** A busy cloud provider's next try, counted down. */
export function RetryLine({ provider, at, attempt, of }: { provider: string; at: number; attempt: number; of: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, []);
  const seconds = Math.max(0, Math.ceil((at - now) / 1000));
  return (
    <p className="muted small pad">
      <span className="spinner small" aria-hidden />
      {t("{provider} is busy. Trying again in {seconds} s ({attempt} of {of})", { provider, seconds, attempt, of })}
    </p>
  );
}

/** Going back to a message you wrote: what it takes away, said first. It
 * always asks, since it removes messages and puts files back. */
export function GoBackDialog({ info, onCancel, onConfirm }: { info: GoBack; onCancel: () => void; onConfirm: (files: boolean) => void }) {
  const [files, setFiles] = useState(true);
  return (
    <Modal title={t("Go back to this message?")} onClose={onCancel}>
      <p>
        {info.messages === 1 ? t("This removes the message after this one.") : t("This removes the {n} messages after this one.", { n: info.messages })}{" "}
        {t("They're kept as a version you can switch back to, and this message goes back into the box to change and send again.")}
      </p>
      {info.changes > 0 && (
        <label className="check">
          <input type="checkbox" checked={files} onChange={(e) => setFiles(e.target.checked)} />
          <span>{info.changes === 1 ? t("Also undo the 1 file change since then") : t("Also undo the {n} file changes since then", { n: info.changes })}</span>
        </label>
      )}
      <div className="modal-actions">
        <button className="btn" onClick={onCancel}>{t("Cancel")}</button>
        <button className="btn primary" onClick={() => onConfirm(files && info.changes > 0)}>{t("Go back")}</button>
      </div>
    </Modal>
  );
}
