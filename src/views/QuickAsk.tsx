// SPDX-License-Identifier: AGPL-3.0-only
// Quick ask: the small box Ctrl+Alt+Space opens from anywhere. Ask
// something, or act on what you copied; continue in the app if you like.
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type MediaItem } from "../api";
import { Markdown } from "../components/Markdown";
import { useMediaUrl } from "../media";
import { t, tx } from "../i18n";

const ACTIONS: { label: string; prompt: (t: string) => string }[] = [
  { label: tx("Summarize"), prompt: (x) => `Summarize this in a few bullet points:\n\n${x}` },
  { label: tx("Explain"), prompt: (x) => `Explain this simply:\n\n${x}` },
  { label: tx("Fix grammar"), prompt: (x) => `Fix the spelling and grammar. Reply with only the corrected text:\n\n${x}` },
  { label: tx("Translate to English"), prompt: (x) => `Translate this to English. Reply with only the translation:\n\n${x}` },
  { label: tx("Draft a reply"), prompt: (x) => `Draft a short, friendly reply to this message:\n\n${x}` },
];

type Phase = "idle" | "thinking" | "writing" | "done";

export function QuickAsk() {
  const [text, setText] = useState("");
  const [clip, setClip] = useState<string | null>(null);
  const [chatId, setChatId] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);
  /** "Ask about the screen": the screenshot to send with the question. */
  const [shot, setShot] = useState<MediaItem | null>(null);
  const [shooting, setShooting] = useState(false);
  const shotUrl = useMediaUrl(shot, true);
  const input = useRef<HTMLTextAreaElement>(null);
  const chatRef = useRef<string | null>(null);
  chatRef.current = chatId;

  const refresh = () => {
    api.security().then((s) => setLocked(s.locked)).catch(() => {});
    api.quickClipboard().then(setClip).catch(() => setClip(null));
    setTimeout(() => input.current?.focus(), 30);
  };

  useEffect(() => {
    refresh();
    const mine = <T extends { chat_id: string }>(fn: (p: T) => void) => (p: T) => p.chat_id === chatRef.current && fn(p);
    const subs = [
      on("quick:shown", () => refresh()),
      on("chat:start", mine(() => setPhase("thinking"))),
      on("chat:delta", mine((p) => {
        if (p.content) {
          setPhase("writing");
          setAnswer((a) => a + p.content);
        }
      })),
      on("chat:done", mine((p) => {
        setPhase("done");
        if (p.error) setError(p.error);
        if (p.message?.content) setAnswer(p.message.content);
      })),
    ];
    const esc = (e: KeyboardEvent) => e.key === "Escape" && api.quickHide();
    window.addEventListener("keydown", esc);
    return () => {
      subs.forEach((s) => s.then((u) => u()));
      window.removeEventListener("keydown", esc);
    };
  }, []);

  // Small while you type; room for the answer once one comes.
  const answering = phase !== "idle" || !!error;
  useEffect(() => {
    api.quickResize(answering ? 460 : (clip && !chatId) || shot ? 150 : 76).catch(() => {});
  }, [answering, clip, chatId, shot]);

  const takeShot = async () => {
    setShooting(true);
    setError(null);
    try {
      setShot(await api.quickScreenshot());
      setTimeout(() => input.current?.focus(), 30);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setShooting(false);
    }
  };

  const ask = async (prompt: string) => {
    if (!prompt.trim() && !shot) return;
    setError(null);
    setAnswer("");
    setPhase("thinking");
    try {
      // A follow-up continues the same chat.
      const id = chatRef.current ?? (await api.createChatIn(null, false)).id;
      setChatId(id);
      chatRef.current = id;
      setText("");
      const images = shot ? [shot.id] : [];
      setShot(null);
      await api.send(id, prompt.trim() || "What's on my screen? Explain what I'm looking at.", images);
    } catch (e) {
      setError(errorText(e));
      setPhase("done");
    }
  };

  const reset = () => {
    setChatId(null);
    setAnswer("");
    setPhase("idle");
    setError(null);
    input.current?.focus();
  };

  if (locked) {
    return (
      <div className="quick">
        <p className="muted">{t("SulcusAI is locked.")}</p>
        <button className="btn primary" onClick={() => api.quickOpenInApp(null)}>{t("Open SulcusAI to unlock")}</button>
      </div>
    );
  }

  return (
    <div className="quick" data-tauri-drag-region>
      <div className="quick-head" data-tauri-drag-region>
        <img src="/logo.svg" alt="" width={20} height={20} />
        <textarea
          ref={input}
          className="quick-input"
          rows={1}
          value={text}
          placeholder={chatId ? t("Ask a follow-up…") : t("Ask anything…")}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              ask(text);
            }
          }}
          aria-label={t("Ask")}
        />
        <button className="icon-btn" onClick={takeShot} disabled={shooting} title={t("Ask about the screen: attaches a screenshot of the screen you're on")} aria-label={t("Ask about the screen")}>
          📷
        </button>
        <button className="icon-btn" onClick={() => api.quickHide()} title={t("Close (Esc)")} aria-label={t("Close")}>×</button>
      </div>
      {shot && (
        <div className="quick-clip">
          <span className="row small">
            {shotUrl && <img src={shotUrl} alt={t("Screenshot")} className="chip-thumb" />} {t("Screenshot attached. Ask about it, or press Enter.")}
            <button className="link" onClick={() => setShot(null)}>{t("Remove")}</button>
          </span>
        </div>
      )}
      {clip && phase === "idle" && !chatId && (
        <div className="quick-clip">
          <span className="small muted ellipsis">{t("Copied: “{text}”", { text: clip.slice(0, 90) + (clip.length > 90 ? "…" : "") })}</span>
          <div className="chips">
            {ACTIONS.map((a) => (
              <button key={a.label} className="chip" onClick={() => ask(a.prompt(clip))}>{t(a.label)}</button>
            ))}
          </div>
        </div>
      )}
      {(phase !== "idle" || error) && (
        <div className="quick-answer">
          {phase === "thinking" && !answer && <p className="muted small">{t("Thinking…")}</p>}
          {answer && <Markdown text={answer} />}
          {error && <p className="error-text">{error}</p>}
        </div>
      )}
      {phase === "done" && (
        <div className="quick-foot">
          <button className="btn small" onClick={() => navigator.clipboard.writeText(answer).catch(() => {})} disabled={!answer}>{t("Copy")}</button>
          <button className="btn small" onClick={() => api.quickOpenInApp(chatId)}>{t("Continue in SulcusAI")}</button>
          <span className="spacer" />
          <button className="btn ghost small" onClick={reset}>{t("New question")}</button>
        </div>
      )}
    </div>
  );
}
