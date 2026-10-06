// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errorText, on, type Chat, type Connectivity, type ContextInfo, type Message, type ModelCard } from "../api";
import { APP_NAME } from "../brand";
import { ContextMeter } from "../components/ContextMeter";
import { Markdown } from "../components/Markdown";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

interface Props {
  chat: Chat | null;
  installed: ModelCard[];
  defaultModel: string | null;
  connectivity: Connectivity;
  onNewChat: () => void;
  onGoModels: () => void;
  onChanged: () => void;
  onDeleted: () => void;
  onToggleWeb: () => void;
  toast: PushToast;
}

interface Streaming {
  id: string;
  content: string;
  thinking: string;
}

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

function Conversation({ chat, installed, defaultModel, connectivity, onChanged, onDeleted, onToggleWeb, toast }: Props & { chat: Chat }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [streaming, setStreaming] = useState<Streaming | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "thinking" | "writing">("idle");
  const [context, setContext] = useState<ContextInfo | null>(null);
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [tps, setTps] = useState<number | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const modelId = chat.model_id ?? defaultModel;
  const model = installed.find((m) => m.id === modelId) ?? null;
  const webOn = connectivity !== "offline" || chat.web;
  const busy = status !== "idle";

  useEffect(() => {
    api.messages(chat.id).then(setMessages);
    api.context(chat.id).then(setContext);
    inputRef.current?.focus();
  }, [chat.id]);

  useEffect(() => {
    const mine = <T extends { chat_id: string }>(fn: (p: T) => void) => (p: T) => p.chat_id === chat.id && fn(p);
    const subs = [
      on("chat:status", mine(() => setStatus("loading"))),
      on("chat:context", mine((p) => setContext(p.context))),
      on("chat:start", mine((p) => {
        setStatus("thinking");
        setStreaming({ id: p.message_id, content: "", thinking: "" });
      })),
      on("chat:delta", mine((p) => {
        if (p.content) setStatus("writing");
        setStreaming((s) =>
          s && s.id === p.message_id
            ? { ...s, content: s.content + (p.content ?? ""), thinking: s.thinking + (p.thinking ?? "") }
            : s,
        );
      })),
      on("chat:done", mine((p) => {
        setContext(p.context);
        setTps(p.tps);
        setStreaming(null);
        setStatus("idle");
        api.messages(chat.id).then(setMessages);
      })),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [chat.id]);

  // Keep the newest text in view while it streams.
  useEffect(() => {
    const el = scroller.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 160) el.scrollTop = el.scrollHeight;
  }, [messages, streaming]);

  const send = async () => {
    const text = input.trim();
    if (!text || busy) return;
    setError(null);
    setInput("");
    setStatus("loading");
    setMessages((m) => [
      ...m,
      { id: `pending-${Date.now()}`, chat_id: chat.id, role: "user", content: text, thinking: null, created_at: Date.now() },
    ]);
    try {
      await api.send(chat.id, text);
    } catch (e) {
      setError(errorText(e));
      setStreaming(null);
      setStatus("idle");
      api.messages(chat.id).then(setMessages);
    } finally {
      onChanged();
    }
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

  return (
    <div className="conversation">
      <header className="chat-header">
        <h1 className="ellipsis" title={chat.title}>{chat.title}</h1>
        <ContextMeter info={context} fallbackCtx={model?.fit.ctx ?? null} />
        <div className="chat-actions">
          <button className="btn ghost small" onClick={() => setRenaming(true)}>Rename</button>
          <button className="btn ghost small danger" onClick={() => setConfirmDelete(true)}>Delete</button>
        </div>
      </header>

      <div className="messages" ref={scroller}>
        {messages.length === 0 && !streaming && (
          <div className="hello">
            <p className="muted">
              You're chatting with <strong>{model?.name ?? "a local model"}</strong>, running on this PC.
            </p>
          </div>
        )}
        {messages.map((m) => (
          <MessageView key={m.id} m={m} />
        ))}
        {streaming && (
          <MessageView
            m={{ id: streaming.id, chat_id: chat.id, role: "assistant", content: streaming.content, thinking: streaming.thinking || null, created_at: 0 }}
            live={status}
          />
        )}
        {status === "loading" && !streaming && <p className="muted small pad">Loading {model?.name ?? "the model"} into memory…</p>}
        {error && <div className="error-box">{error}</div>}
      </div>

      <div className="composer-wrap">
        <div className="composer">
          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={onKey}
            placeholder={`Message ${model?.name ?? APP_NAME}…`}
            rows={Math.min(8, Math.max(1, input.split("\n").length))}
            aria-label="Message"
          />
          <div className="composer-bar">
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
        {webOn && (
          <p className="muted small center">
            Web access is allowed for this chat. Web search and browsing tools arrive in a later update.
          </p>
        )}
      </div>

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

function MessageView({ m, live }: { m: Message; live?: "idle" | "loading" | "thinking" | "writing" }) {
  const thinkingNow = live === "thinking" && !m.content;
  return (
    <div className={`msg ${m.role}`}>
      {m.thinking && (
        <details className="thinking" open={thinkingNow}>
          <summary>{thinkingNow ? "Thinking…" : "Thought process"}</summary>
          <div className="thinking-text">{m.thinking}</div>
        </details>
      )}
      {m.role === "user" ? <div className="bubble">{m.content}</div> : <Markdown text={m.content} />}
      {live && !m.content && !m.thinking && <span className="typing" aria-label="Writing" />}
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
