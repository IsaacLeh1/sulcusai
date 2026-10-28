// SPDX-License-Identifier: AGPL-3.0-only
// Quick ask: the small box Ctrl+Alt+Space opens from anywhere. Ask
// something, or act on what you copied; continue in the app if you like.
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type MediaItem, type QuickSelection } from "../api";
import { Markdown } from "../components/Markdown";
import { rememberSent, SmartInput, useTypingHelpers } from "../components/SmartInput";
import { useMediaUrl } from "../media";
import { t, tx } from "../i18n";
import { CameraIcon } from "../components/Icons";

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
  /** What was selected in the app in front when the box opened (read, not copied). */
  const [selected, setSelected] = useState<QuickSelection | null>(null);
  /** The answer is for the selection, so it can go back in its place. */
  const [forSelection, setForSelection] = useState(false);
  const [chatId, setChatId] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);
  /** "Ask about the screen": the screenshot to send with the question. */
  const [shot, setShot] = useState<MediaItem | null>(null);
  const [shooting, setShooting] = useState(false);
  const helpers = useTypingHelpers();
  const shotUrl = useMediaUrl(shot, true);
  const input = useRef<HTMLTextAreaElement>(null);
  const chatRef = useRef<string | null>(null);
  chatRef.current = chatId;

  const refresh = () => {
    api.security().then((s) => setLocked(s.locked)).catch(() => {});
    api.quickClipboard().then(setClip).catch(() => setClip(null));
    api.quickSelection().then(setSelected).catch(() => setSelected(null));
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
        if (p.error) setError(errorText(p.error));
        // What it wrote on the way stays; the finished answer joins it if it isn't there yet.
        const last = p.message?.content?.trim();
        if (last) setAnswer((a) => (!a.trim() ? last : a.includes(last) ? a : `${a.trimEnd()}\n\n${last}`));
      })),
    ];
    // Esc closes the box, unless it just hid a typing suggestion.
    const esc = (e: KeyboardEvent) => e.key === "Escape" && !e.defaultPrevented && api.quickHide();
    window.addEventListener("keydown", esc);
    return () => {
      subs.forEach((s) => s.then((u) => u()));
      window.removeEventListener("keydown", esc);
    };
  }, []);

  // Small while you type; room for the answer once one comes.
  const answering = phase !== "idle" || !!error;
  useEffect(() => {
    api.quickResize(answering ? 460 : ((clip || selected) && !chatId) || shot ? 150 : 76).catch(() => {});
  }, [answering, clip, selected, chatId, shot]);

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

  /** `about`: the selected text, sent after a question typed about it. */
  const ask = async (prompt: string, about?: string) => {
    if (!prompt.trim() && !shot) return;
    setError(null);
    setAnswer("");
    setPhase("thinking");
    try {
      // A follow-up continues the same chat.
      const id = chatRef.current ?? (await api.quickChat()).id;
      setChatId(id);
      chatRef.current = id;
      setText("");
      rememberSent(prompt);
      const images = shot ? [shot.id] : [];
      setShot(null);
      const question = prompt.trim() || "What's on my screen? Explain what I'm looking at.";
      await api.send(id, about ? `${question}\n\n${about}` : question, images);
    } catch (e) {
      setError(errorText(e));
      setPhase("done");
    }
  };

  const reset = () => {
    setForSelection(false);
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
        <img src="logo.svg" alt="" width={20} height={20} />
        <SmartInput
          ref={input}
          className="quick-input"
          rows={1}
          value={text}
          onValueChange={setText}
          complete={helpers.complete}
          correct={helpers.correct}
          placeholder={chatId ? t("Ask a follow-up…") : selected ? t("Ask about the selected text…") : t("Ask anything…")}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              // The first question with text selected is about that text.
              const about = selected && !chatRef.current && text.trim() ? selected : null;
              if (about) setForSelection(about.editable);
              ask(text, about?.text);
            }
          }}
          aria-label={t("Ask")}
        />
        <button className="icon-btn" onClick={takeShot} disabled={shooting} title={t("Ask about the screen: attaches a screenshot of the screen you're on")} aria-label={t("Ask about the screen")}>
          <CameraIcon />
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
      {selected && phase === "idle" && !chatId && (
        <div className="quick-clip">
          <span className="small muted ellipsis">{t("Selected: “{text}”", { text: selected.text.slice(0, 90) + (selected.text.length > 90 ? "…" : "") })}</span>
          <div className="chips">
            {ACTIONS.map((a) => (
              <button
                key={a.label}
                className="chip"
                onClick={() => {
                  setForSelection(selected.editable);
                  ask(a.prompt(selected.text));
                }}
              >
                {t(a.label)}
              </button>
            ))}
          </div>
        </div>
      )}
      {clip && !selected && phase === "idle" && !chatId && (
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
          {forSelection && answer && (
            <button
              className="btn primary small"
              title={t("Pastes the answer over the text you selected, in its app")}
              onClick={() => api.quickReplace(answer.trim()).catch((e) => setError(errorText(e)))}
            >
              {t("Put it in place of the selection")}
            </button>
          )}
          <button className="btn small" onClick={() => navigator.clipboard.writeText(answer).catch(() => {})} disabled={!answer}>{t("Copy")}</button>
          <button className="btn small" onClick={() => api.quickOpenInApp(chatId)}>{t("Continue in SulcusAI")}</button>
          <span className="spacer" />
          <button className="btn ghost small" onClick={reset}>{t("New question")}</button>
        </div>
      )}
    </div>
  );
}
