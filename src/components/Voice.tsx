// SPDX-License-Identifier: AGPL-3.0-only
// Dictation (the 🎤 button), voice mode's panel, and read-aloud.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, on } from "../api";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

type DictationState = "idle" | "loading" | "listening" | "hearing" | "finishing";

/** Streams dictated phrases to `onText` while active. */
export function useDictation(onText: (text: string) => void, toast: PushToast) {
  const [state, setState] = useState<DictationState>("idle");
  const [level, setLevel] = useState(0);
  const idRef = useRef<string | null>(null);
  const onTextRef = useRef(onText);
  onTextRef.current = onText;

  useEffect(() => {
    const sub = on("dictation", (e) => {
      if (e.id !== idRef.current) return;
      switch (e.state) {
        case "level":
          setLevel(e.level);
          break;
        case "text":
          onTextRef.current(e.text);
          break;
        case "listening":
        case "hearing": {
          const next = e.state === "hearing" ? "hearing" : "listening";
          setState((s) => (s === "finishing" ? s : next));
          break;
        }
        case "done":
          idRef.current = null;
          setState("idle");
          setLevel(0);
          break;
        case "error":
          idRef.current = null;
          setState("idle");
          setLevel(0);
          toast(e.error, "error");
          break;
      }
    });
    return () => {
      sub.then((un) => un());
      if (idRef.current) api.stopDictation(idRef.current);
    };
  }, [toast]);

  const start = useCallback(async () => {
    if (idRef.current) return;
    setState("loading");
    try {
      idRef.current = await api.startDictation();
    } catch (e) {
      setState("idle");
      toast(errorText(e), "error");
    }
  }, [toast]);

  const stop = useCallback(() => {
    if (!idRef.current) return;
    setState("finishing");
    api.stopDictation(idRef.current);
  }, []);

  const toggle = useCallback(() => (idRef.current ? stop() : start()), [start, stop]);
  return { state, level, start, stop, toggle, active: state !== "idle" };
}

const DICTATION_LABEL: Record<DictationState, string> = {
  idle: tx("Dictate (Ctrl+Shift+Space)"),
  loading: tx("Getting speech recognition ready…"),
  listening: tx("Listening… click to stop"),
  hearing: tx("Hearing you… click to stop"),
  finishing: tx("Finishing the last words…"),
};

/** A mic button that types what you say. Ctrl+Shift+Space toggles it too. */
export function MicButton({ onText, toast, disabled }: { onText: (text: string) => void; toast: PushToast; disabled?: boolean }) {
  const d = useDictation(onText, toast);
  const toggle = d.toggle;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.code === "Space") {
        e.preventDefault();
        if (!disabled) toggle();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggle, disabled]);
  const ring = Math.min(1, d.level * 8);
  return (
    <button
      type="button"
      className={`mic-btn ${d.active ? "on" : ""} ${d.state}`}
      onClick={d.toggle}
      disabled={disabled && !d.active}
      aria-pressed={d.active}
      aria-label={t(DICTATION_LABEL[d.state])}
      title={t(DICTATION_LABEL[d.state])}
      style={d.active ? { boxShadow: `0 0 0 ${2 + ring * 6}px color-mix(in srgb, var(--danger) ${20 + ring * 30}%, transparent)` } : undefined}
    >
      🎤
    </button>
  );
}

type VoiceState = "loading" | "listening" | "hearing" | "thinking" | "speaking" | "interrupted" | "ended";

const VOICE_LABEL: Record<VoiceState, string> = {
  loading: tx("Getting ready…"),
  listening: tx("Listening"),
  hearing: tx("Hearing you…"),
  thinking: tx("Thinking…"),
  speaking: tx("Speaking (talk to interrupt)"),
  interrupted: tx("Listening"),
  ended: tx("Voice chat ended"),
};

/** Voice mode for one chat: listens, answers aloud, stops when you talk. */
export function VoicePanel({ chatId, onHeard, onEnd, toast }: { chatId: string; onHeard: (text: string) => void; onEnd: () => void; toast: PushToast }) {
  const [state, setState] = useState<VoiceState>("loading");
  const [level, setLevel] = useState(0);
  const [talking, setTalking] = useState(false);
  const [heard, setHeard] = useState("");
  const onHeardRef = useRef(onHeard);
  onHeardRef.current = onHeard;
  const onEndRef = useRef(onEnd);
  onEndRef.current = onEnd;

  useEffect(() => {
    const sub = on("voice", (e) => {
      if (e.chat_id !== chatId) return;
      if (e.state === "level") {
        setLevel(e.level);
        setTalking(e.speaking);
      } else if (e.state === "heard") {
        setHeard(e.text);
        onHeardRef.current(e.text);
      } else if (e.state === "error") {
        toast(e.error, "error");
        onEndRef.current();
      } else if (e.state === "ended") {
        onEndRef.current();
      } else {
        setState(e.state);
      }
    });
    api.startVoice(chatId).catch((err) => {
      toast(errorText(err), "error");
      onEndRef.current();
    });
    return () => {
      sub.then((un) => un());
      api.stopVoice(chatId);
    };
  }, [chatId, toast]);

  const shown: VoiceState = talking && (state === "thinking" || state === "speaking") ? "speaking" : state;
  const scale = 1 + Math.min(0.35, level * 4);
  return (
    <div className="voice-panel" role="status" aria-live="polite">
      <div className={`voice-orb ${shown}`} style={{ transform: `scale(${shown === "listening" || shown === "hearing" ? scale : 1})` }} aria-hidden />
      <div className="voice-text">
        <strong>{t(VOICE_LABEL[shown])}</strong>
        {heard && <span className="muted small ellipsis" title={heard}>{t("You said: “{text}”", { text: heard })}</span>}
        <span className="muted small">{t("Headphones work best, so it doesn't hear itself.")}</span>
      </div>
      <span className="spacer" />
      {(shown === "speaking" || shown === "thinking") && (
        <button className="btn small" onClick={() => api.interruptVoice(chatId)}>{t("Interrupt")}</button>
      )}
      <button className="btn small danger" onClick={onEnd}>{t("End voice chat")}</button>
    </div>
  );
}

/** Reads a reply aloud with the chosen voice. */
export function SpeakButton({ text, toast }: { text: string; toast: PushToast }) {
  const [state, setState] = useState<"idle" | "preparing" | "speaking">("idle");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const click = async () => {
    if (state !== "idle") {
      window.clearTimeout(timer.current);
      api.stopSpeaking();
      setState("idle");
      return;
    }
    setState("preparing");
    try {
      await api.speak(text);
      setState("speaking");
      // About 15 characters a second at normal speed.
      timer.current = window.setTimeout(() => setState("idle"), Math.max(2000, (text.length / 15) * 1000));
    } catch (e) {
      setState("idle");
      toast(errorText(e), "error");
    }
  };
  return (
    <button className="icon-btn speak-btn" onClick={click} title={state === "idle" ? t("Read aloud") : t("Stop reading")} aria-label={state === "idle" ? t("Read aloud") : t("Stop reading")}>
      {state === "idle" ? "🔊" : state === "preparing" ? "…" : "⏹"}
    </button>
  );
}
