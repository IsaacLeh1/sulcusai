// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import {
  api,
  errorText,
  on,
  type Chat,
  type Connectivity,
  type FeatureId,
  type ContextInfo,
  type Message,
  type ModelCard,
  type PendingApproval,
  type RunMode,
} from "../api";
import { APP_NAME } from "../brand";
import { ApprovalCard, FolderMenu, ToolCard } from "../components/AgentCards";
import { ContextMeter } from "../components/ContextMeter";
import { Markdown } from "../components/Markdown";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { MicButton, SpeakButton, VoicePanel } from "../components/Voice";
import { insertDictation } from "../format";

interface Props {
  chat: Chat | null;
  projectName: string | null;
  onOpenChat: (id: string) => void;
  installed: ModelCard[];
  defaultModel: string | null;
  connectivity: Connectivity;
  onNewChat: () => void;
  onGoModels: () => void;
  onChanged: () => void;
  onDeleted: () => void;
  onToggleWeb: () => void;
  features: Set<FeatureId>;
  onGoFeatures: () => void;
  toast: PushToast;
}

interface Streaming {
  id: string;
  content: string;
  thinking: string;
}

type Status = "idle" | "loading" | "thinking" | "writing" | "working";

const MODES: { id: RunMode; label: string; help: string }[] = [
  { id: "plan", label: "Plan", help: "Looks around and proposes a plan. Changes nothing until you approve." },
  { id: "auto", label: "Auto", help: "Reads freely; asks before changing files or running commands." },
  { id: "bypass", label: "Bypass", help: "Changes files and runs commands without asking. Undo still works for file changes." },
];

export function ChatView(props: Props) {
  const { chat, installed } = props;

  if (installed.length === 0) {
    return (
      <div className="empty">
        <img src="/logo.svg" alt="" width={64} height={64} />
        <h1>Welcome to {APP_NAME}</h1>
        <p className="muted">Install a model to start chatting. Everything runs privately on this PC.</p>
        <button className="btn primary" onClick={props.onGoModels}>
          Choose a model
        </button>
      </div>
    );
  }
  if (!chat) {
    return (
      <div className="empty">
        <img src="/logo.svg" alt="" width={64} height={64} />
        <h1>What can I help with?</h1>
        <p className="muted">Start a new chat, or pick one from the sidebar.</p>
        <button className="btn primary" onClick={props.onNewChat}>
          New chat
        </button>
      </div>
    );
  }
  return <Conversation key={chat.id} {...props} chat={chat} />;
}

function Conversation({ chat, projectName, onOpenChat, installed, defaultModel, connectivity, onChanged, onDeleted, onToggleWeb, features, onGoFeatures, toast }: Props & { chat: Chat }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [streaming, setStreaming] = useState<Streaming | null>(null);
  const [status, setStatus] = useState<Status>("idle");
  const [helperSteps, setHelperSteps] = useState<Record<string, string[]>>({});
  const [handingOff, setHandingOff] = useState(false);
  const [context, setContext] = useState<ContextInfo | null>(null);
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [tps, setTps] = useState<number | null>(null);
  const [pending, setPending] = useState<PendingApproval[]>([]);
  const [runningCall, setRunningCall] = useState<string | null>(null);
  const [undoable, setUndoable] = useState<Set<string>>(new Set());
  const [renaming, setRenaming] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmBypass, setConfirmBypass] = useState(false);
  const [voice, setVoice] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const modelId = chat.model_id ?? defaultModel;
  const model = installed.find((m) => m.id === modelId) ?? null;
  const webOn = connectivity !== "offline" || chat.web;
  const busy = status !== "idle";

  const reload = useCallback(() => {
    api.messages(chat.id).then(setMessages);
    api.undoableTurns(chat.id).then((t) => setUndoable(new Set(t))).catch(() => {});
  }, [chat.id]);

  useEffect(() => {
    reload();
    api.context(chat.id).then(setContext);
    api.pendingApprovals(chat.id).then(setPending).catch(() => {});
    inputRef.current?.focus();
  }, [chat.id, reload]);

  useEffect(() => {
    const mine = <T extends { chat_id: string }>(fn: (p: T) => void) => (p: T) => p.chat_id === chat.id && fn(p);
    const subs = [
      on("chat:status", mine((p) => {
        if (p.status === "handoff") setHandingOff(true);
        else setStatus("loading");
      })),
      on("agent:helper", mine((p) => setHelperSteps((all) => ({ ...all, [p.call_id]: [...(all[p.call_id] ?? []), p.step] })))),
      on("chat:context", mine((p) => setContext(p.context))),
      on("chat:start", mine((p) => {
        setStatus("thinking");
        setStreaming({ id: p.message_id, content: "", thinking: "" });
      })),
      on("chat:delta", mine((p) => {
        if (p.content) setStatus("writing");
        setStreaming((s) =>
          s && s.id === p.message_id ? { ...s, content: s.content + (p.content ?? ""), thinking: s.thinking + (p.thinking ?? "") } : s,
        );
      })),
      on("agent:step", mine(() => {
        setStreaming(null);
        setStatus("working");
        setRunningCall(null);
        api.messages(chat.id).then(setMessages);
      })),
      on("agent:tool_start", mine((p) => setRunningCall(p.call_id))),
      on("agent:approval", mine((p) => setPending((all) => [...all.filter((a) => a.call_id !== p.call_id), p]))),
      on("agent:approval_done", mine((p) => setPending((all) => all.filter((a) => a.call_id !== p.call_id)))),
      on("chat:done", mine((p) => {
        if (p.context) setContext(p.context);
        if (p.tps !== undefined) setTps(p.tps ?? null);
        if (p.error) setError(p.error);
        setStreaming(null);
        setStatus("idle");
        setRunningCall(null);
        setPending([]);
        reload();
      })),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [chat.id, reload]);

  // Keep the newest text in view while it streams.
  useEffect(() => {
    const el = scroller.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 200) el.scrollTop = el.scrollHeight;
  }, [messages, streaming, pending]);

  const sendText = async (text: string) => {
    if (!text || busy) return;
    setError(null);
    setStatus("loading");
    setMessages((m) => [...m, { id: `pending-${Date.now()}`, chat_id: chat.id, role: "user", content: text, thinking: null, created_at: Date.now() }]);
    try {
      await api.send(chat.id, text);
    } catch (e) {
      setError(errorText(e));
      setStreaming(null);
      setStatus("idle");
      reload();
    } finally {
      onChanged();
    }
  };

  // Dictated phrases land at the cursor.
  const dictate = useCallback((text: string) => {
    const el = inputRef.current;
    setInput((value) => {
      const start = el?.selectionStart ?? value.length;
      const end = el?.selectionEnd ?? value.length;
      const r = insertDictation(value, start, end, text);
      requestAnimationFrame(() => {
        el?.focus();
        el?.setSelectionRange(r.cursor, r.cursor);
      });
      return r.value;
    });
  }, []);

  // The assistant asked for web access: turn it on for this chat and carry on.
  const enableWeb = useCallback(async () => {
    await api.setChatWeb(chat.id, true);
    onChanged();
    sendText("Web access is on now. Please go ahead.");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat.id]);

  // Voice mode sends what it heard itself; show it like a typed message.
  const heard = useCallback((text: string) => {
    setError(null);
    setStatus("loading");
    setMessages((m) => [...m, { id: `pending-${Date.now()}`, chat_id: chat.id, role: "user", content: text, thinking: null, created_at: Date.now() }]);
  }, [chat.id]);

  const send = () => {
    const text = input.trim();
    if (!text || busy) return;
    setInput("");
    sendText(text);
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send();
    }
  };

  const switchModel = async (id: string) => {
    try {
      await api.setChatModel(chat.id, id);
      setContext(null);
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const setMode = async (mode: RunMode) => {
    if (mode === "bypass" && chat.mode !== "bypass") {
      setConfirmBypass(true);
      return;
    }
    await api.setChatMode(chat.id, mode);
    onChanged();
  };

  const runPlan = async () => {
    await api.setChatMode(chat.id, "auto");
    onChanged();
    sendText("Go ahead and carry out the plan.");
  };

  const undo = async (turnId: string) => {
    try {
      const notes = await api.undoTurn(chat.id, turnId);
      toast(notes.length ? `Undone, with notes: ${notes.join(" ")}` : "Changes from that reply were undone.", notes.length ? "error" : "success");
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const items = useMemo(
    () => renderItems(messages, undoable, busy, runningCall, undo, helperSteps, onOpenChat, features.has("read_aloud") ? toast : undefined, webOn ? null : enableWeb),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [messages, undoable, busy, runningCall, helperSteps, features],
  );
  const last = messages[messages.length - 1];
  const showRunPlan = chat.mode === "plan" && !busy && last?.role === "assistant" && !!last.content && !last.tool_calls?.length;

  return (
    <div className="conversation">
      <header className="chat-header">
        <h1 className="ellipsis" title={chat.title}>{chat.title}</h1>
        <div className="mode-switch" role="radiogroup" aria-label="Run mode">
          {MODES.map((m) => (
            <button
              key={m.id}
              role="radio"
              aria-checked={chat.mode === m.id}
              className={`mode ${m.id} ${chat.mode === m.id ? "active" : ""}`}
              title={m.help}
              onClick={() => setMode(m.id)}
              disabled={busy}
            >
              {m.label}
            </button>
          ))}
        </div>
        <ContextMeter info={context} fallbackCtx={model?.fit.ctx ?? null} />
        <div className="chat-actions">
          <button className="btn ghost small" onClick={() => setRenaming(true)}>Rename</button>
          <button className="btn ghost small danger" onClick={() => setConfirmDelete(true)}>Delete</button>
        </div>
      </header>

      {(chat.incognito || projectName || chat.parent_id) && (
        <div className={`chat-banner ${chat.incognito ? "incognito" : ""}`}>
          {chat.incognito ? "🕶 Incognito: this chat isn't saved and doesn't use or create memories. It's deleted when you leave it." : projectName ? `📚 In project ${projectName}` : null}
          {chat.parent_id && (
            <button className="link" onClick={() => onOpenChat(chat.parent_id!)}>
              ↩ Continues an earlier chat
            </button>
          )}
        </div>
      )}
      <div className="messages" ref={scroller}>
        {messages.length === 0 && !streaming && (
          <div className="hello">
            <p className="muted">
              You're chatting with <strong>{model?.name ?? "a local model"}</strong>, running on this PC.
            </p>
            {model?.tools && features.has("files") && <p className="muted small">Share a folder with 📁 and it can read and change files there, or run commands to build and test code.</p>}
            {!features.has("files") && features.size === 0 && (
              <p className="muted small">
                Want dictation, voice chat, files or meetings? <button className="link" onClick={onGoFeatures}>Add features</button>
              </p>
            )}
          </div>
        )}
        {items}
        {streaming && (
          <MessageView
            m={{ id: streaming.id, chat_id: chat.id, role: "assistant", content: streaming.content, thinking: streaming.thinking || null, created_at: 0 }}
            live={status}
          />
        )}
        {pending.map((p) => (
          <ApprovalCard key={p.call_id} p={p} onAnswered={() => setPending((all) => all.filter((a) => a.call_id !== p.call_id))} />
        ))}
        {handingOff && <p className="muted small pad">This chat is nearly full. Summarizing it to continue in a new chat…</p>}
        {status === "loading" && !streaming && <p className="muted small pad">Loading {model?.name ?? "the model"} into memory…</p>}
        {status === "working" && !streaming && pending.length === 0 && <p className="muted small pad">Working…</p>}
        {showRunPlan && (
          <div className="plan-actions">
            <button className="btn primary" onClick={runPlan}>Run this plan</button>
            <span className="muted small">Switches to Auto, so you'll still approve each change.</span>
          </div>
        )}
        {error && <div className="error-box">{error}</div>}
      </div>

      <div className="composer-wrap">
        {voice && features.has("voice_chat") && <VoicePanel chatId={chat.id} onHeard={heard} onEnd={() => setVoice(false)} toast={toast} />}
        <div className="composer">
          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={onKey}
            placeholder={chat.mode === "plan" ? "Describe what you want; it will propose a plan first…" : `Message ${model?.name ?? APP_NAME}…`}
            rows={Math.min(8, Math.max(1, input.split("\n").length))}
            aria-label="Message"
          />
          <div className="composer-bar">
            {model?.tools && features.has("files") && <FolderMenu toast={toast} />}
            <button
              className={`globe ${webOn ? "on" : ""}`}
              onClick={onToggleWeb}
              disabled={connectivity !== "offline"}
              aria-pressed={webOn}
              title={
                connectivity !== "offline"
                  ? "Web is on for every chat. Change it in the status bar."
                  : webOn
                    ? "Web is on for this chat (Ctrl+Shift+W)"
                    : "Turn on web for this chat (Ctrl+Shift+W)"
              }
            >
              🌐 {webOn ? "Web on" : "Web off"}
            </button>
            <select value={modelId ?? ""} onChange={(e) => switchModel(e.target.value)} aria-label="Model for this chat" disabled={busy}>
              {installed.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
            <span className="spacer" />
            {tps !== null && !busy && <span className="muted small">{tps} tokens/sec</span>}
            {features.has("dictation") && <MicButton onText={dictate} toast={toast} disabled={voice} />}
            {features.has("voice_chat") && (
              <button
                type="button"
                className={`voice-btn ${voice ? "on" : ""}`}
                onClick={() => setVoice((v) => !v)}
                aria-pressed={voice}
                title={voice ? "End voice chat" : "Voice chat: talk, and hear the answers"}
                aria-label={voice ? "End voice chat" : "Start voice chat"}
              >
                🗣
              </button>
            )}
            {busy ? (
              <button className="btn" onClick={() => api.stop(chat.id)}>
                Stop
              </button>
            ) : (
              <button className="btn primary" onClick={send} disabled={!input.trim()}>
                Send
              </button>
            )}
          </div>
        </div>
        {model && !model.tools && (
          <p className="muted small center">{model.name} can chat but can't use files or commands. Qwen3 models can.</p>
        )}
        {webOn && <p className="muted small center">Web access is on for this chat: it can search and read pages. Your messages and the AI stay on this PC.</p>}
      </div>

      {confirmBypass && (
        <Modal title="Switch to Bypass mode?" onClose={() => setConfirmBypass(false)}>
          <p>In Bypass mode the assistant changes files and runs commands in your shared folders <strong>without asking</strong>.</p>
          <ul>
            <li>File changes can still be undone after each reply.</li>
            <li>Commands can't be undone, so their effects stay.</li>
          </ul>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmBypass(false)}>Stay in Auto</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                setConfirmBypass(false);
                await api.setChatMode(chat.id, "bypass");
                onChanged();
              }}
            >
              Use Bypass
            </button>
          </div>
        </Modal>
      )}
      {renaming && (
        <RenameDialog
          title={chat.title}
          onClose={() => setRenaming(false)}
          onSave={async (t) => {
            try {
              await api.renameChat(chat.id, t);
              setRenaming(false);
              onChanged();
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        />
      )}
      {confirmDelete && (
        <Modal title="Delete this chat?" onClose={() => setConfirmDelete(false)}>
          <p>“{chat.title}” and all its messages will be permanently deleted from this PC.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(false)}>Cancel</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                await api.deleteChat(chat.id);
                setConfirmDelete(false);
                onDeleted();
              }}
            >
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/** Messages as the reader sees them: tool results tucked into their calls,
 *  and an Undo button at the end of each turn that changed files. */
function renderItems(
  messages: Message[],
  undoable: Set<string>,
  busy: boolean,
  runningCall: string | null,
  undo: (turnId: string) => void,
  helperSteps: Record<string, string[]>,
  openChat: (id: string) => void,
  toast?: PushToast,
  onEnableWeb?: (() => void) | null,
): ReactNode[] {
  const results = new Map<string, Message>();
  for (const m of messages) if (m.role === "tool" && m.tool_call_id) results.set(m.tool_call_id, m);

  const out: ReactNode[] = [];
  let turnId: string | null = null;
  const closeTurn = (key: string, isLastTurn: boolean) => {
    if (turnId && undoable.has(turnId) && !(busy && isLastTurn)) {
      const id = turnId;
      out.push(
        <div key={`undo-${key}`} className="undo-row">
          <button className="btn ghost small" onClick={() => undo(id)}>↶ Undo file changes from this reply</button>
        </div>,
      );
    }
  };
  messages.forEach((m, i) => {
    if (m.role === "user") {
      closeTurn(m.id, false);
      turnId = m.id;
      out.push(<MessageView key={m.id} m={m} />);
    } else if (m.role === "assistant") {
      out.push(
        <Fragment key={m.id}>
          {(m.content || m.thinking) && <MessageView m={m} toast={toast} />}
          {m.meta?.handoff_to && (
            <div className="undo-row">
              <button className="btn small" onClick={() => openChat(m.meta!.handoff_to!)}>Open the continued chat →</button>
            </div>
          )}
          {m.tool_calls?.map((c) => {
            const r = results.get(c.id);
            if (c.name === "request_web" && r?.meta?.web_request) {
              return <WebRequestCard key={c.id} title={r.meta.title} onEnable={onEnableWeb ?? null} />;
            }
            return <ToolCard key={c.id} call={c} meta={r?.meta} output={r?.content} running={runningCall === c.id} liveSteps={helperSteps[c.id]} />;
          })}
        </Fragment>,
      );
    }
    if (i === messages.length - 1) closeTurn("end", true);
  });
  return out;
}

function MessageView({ m, live, toast }: { m: Message; live?: Status; toast?: PushToast }) {
  const thinkingNow = live === "thinking" && !m.content;
  return (
    <div className={`msg ${m.role}`}>
      {m.thinking && (
        <details className="thinking" open={thinkingNow}>
          <summary>{thinkingNow ? "Thinking…" : "Thought process"}</summary>
          <div className="thinking-text">{m.thinking}</div>
        </details>
      )}
      {m.role === "user" ? <div className="bubble">{m.content}</div> : m.content ? <Markdown text={m.content} /> : null}
      {live && !m.content && !m.thinking && <span className="typing" aria-label="Writing" />}
      {toast && m.role === "assistant" && m.content && (
        <div className="msg-tools">
          <SpeakButton text={m.content} toast={toast} />
        </div>
      )}
    </div>
  );
}

function WebRequestCard({ title, onEnable }: { title: string; onEnable: (() => void) | null }) {
  const reason = title.replace(/^Needs web access: /, "");
  return (
    <div className="web-request" role="note">
      <span aria-hidden>🌐</span>
      <div>
        <strong>This needs web access</strong>
        <span className="small muted block">To look up: {reason}. Only the search and the pages it opens go online; the AI stays on this PC.</span>
      </div>
      {onEnable ? (
        <button className="btn primary small" onClick={onEnable}>Turn on web for this chat</button>
      ) : (
        <span className="small muted">Web is on</span>
      )}
    </div>
  );
}

function RenameDialog({ title, onClose, onSave }: { title: string; onClose: () => void; onSave: (t: string) => void }) {
  const [value, setValue] = useState(title);
  return (
    <Modal title="Rename chat" onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSave(value);
        }}
      >
        <input className="input" value={value} onChange={(e) => setValue(e.target.value)} autoFocus maxLength={80} aria-label="Chat name" />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn primary" disabled={!value.trim()}>Save</button>
        </div>
      </form>
    </Modal>
  );
}
