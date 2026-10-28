// SPDX-License-Identifier: AGPL-3.0-only
import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type KeyboardEvent, type TextareaHTMLAttributes } from "react";
import { api, on, type Vocabulary } from "../api";
import { language, t } from "../i18n";
import { matching } from "../shortcuts";

/** Whether Autocomplete and Autocorrect are on, kept current as Features change
 * (for boxes outside the chat, which don't get the feature list). */
export function useTypingHelpers(): { complete: boolean; correct: boolean } {
  const [helpers, setHelpers] = useState({ complete: false, correct: false });
  useEffect(() => {
    const load = () =>
      api
        .features()
        .then((list) => {
          const enabled = new Set(list.filter((f) => f.enabled).map((f) => f.id));
          setHelpers({ complete: enabled.has("autocomplete"), correct: enabled.has("autocorrect") });
        })
        .catch(() => {});
    load();
    const sub = on("features:changed", load);
    return () => {
      sub.then((un) => un());
    };
  }, []);
  return helpers;
}

/** Autocomplete's words and requests, loaded once and refreshed after a few minutes. */
let vocab: { at: number; data: Promise<Vocabulary> } | null = null;
function vocabulary(): Promise<Vocabulary> {
  if (!vocab || Date.now() - vocab.at > 5 * 60_000) {
    vocab = { at: Date.now(), data: api.typingVocabulary().catch(() => ({ words: [], prompts: [] })) };
  }
  return vocab.data;
}

/** A message the person just sent, so autocomplete offers it again right away. */
export function rememberSent(text: string) {
  const line = text.trim();
  if (!vocab || line.includes("\n") || line.length < 8 || line.length >= 200) return;
  vocab.data = vocab.data.then((v) => ({ ...v, prompts: [line, ...v.prompts.filter((p) => p !== line)] }));
}

const WORD_END = /[\p{L}']+$/u;
const BOUNDARY = /[\s.,!?;:)]/;

/** The rest of what's being typed, from the person's own requests (a whole
 * line they've asked before) or words (the most used one starting with
 * the last word's letters). Empty when nothing fits. */
export function completion(value: string, v: Vocabulary): { text: string; word: boolean } {
  const typed = value.trimStart();
  if (typed.length >= 6) {
    const lower = value.toLowerCase();
    const p = v.prompts.find((x) => x.length > value.length && x.toLowerCase().startsWith(lower));
    if (p) return { text: p.slice(value.length), word: false };
  }
  const m = value.match(WORD_END);
  if (!m || m[0].length < 2) return { text: "", word: true };
  const part = m[0].toLowerCase();
  const w = v.words.find(([x]) => x.length > part.length && x.startsWith(part));
  return { text: w ? w[0].slice(part.length) : "", word: true };
}

/** Where `piece` (a word and what ended it) now is in `text`, the copy closest
 * to where it was typed (earlier fixes may have moved it a little); -1 when
 * it's gone. */
export function findTyped(text: string, piece: string, near: number): number {
  let best = -1;
  for (let i = text.indexOf(piece); i >= 0; i = text.indexOf(piece, i + 1)) {
    const whole = i === 0 || !/[\p{L}']/u.test(text[i - 1]);
    if (whole && Math.abs(i - near) <= 40 && (best < 0 || Math.abs(i - near) < Math.abs(best - near))) best = i;
  }
  return best;
}

/** Whether the word before the caret sits in code or a web address, where nothing is corrected. */
function inCodeOrLink(before: string): boolean {
  if ((before.match(/`/g) ?? []).length % 2 === 1) return true;
  const last = before.split(/\s/).pop() ?? "";
  return /:\/\/|www\.|@|[\\/_#=<>{}]|\d/.test(last);
}

type Props = Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "value" | "onChange"> & {
  value: string;
  onValueChange: (value: string) => void;
  /** Which helpers are on (the Autocomplete and Autocorrect features). */
  complete?: boolean;
  correct?: boolean;
  /** A whole message to offer while the box is empty (the suggested next message). */
  suggestion?: string | null;
  /** The suggestion was typed over, dismissed or used. */
  onSuggestionGone?: () => void;
  /** What typing "/" at the start offers (saved prompts, commands). */
  commands?: SlashCommand[];
};

/** One choice in the "/" menu. */
export interface SlashCommand {
  name: string;
  hint: string;
  pick: () => void;
}

/** A text box that helps while typing: the rest of a word or of an earlier
 * request in light grey (Tab takes it, Esc hides it), a suggested next
 * message while it's empty, and misspelled words fixed as they're finished
 * (Backspace right after puts the word back). */
export const SmartInput = forwardRef<HTMLTextAreaElement, Props>(function SmartInput(
  { value, onValueChange, complete = false, correct = false, suggestion = null, onSuggestionGone, commands, onKeyDown, placeholder, className, ...rest },
  ref,
) {
  // The "/" menu: while the box holds only "/" and the start of a name.
  const [slashAt, setSlashAt] = useState(0);
  const [slashOff, setSlashOff] = useState<string | null>(null);
  const typedName = /^\/[\p{L}\p{N}_-]*$/u.test(value) ? value.slice(1).toLowerCase() : null;
  const slash = typedName !== null && slashOff !== value ? matching(commands ?? [], typedName) : [];
  const slashPick = Math.min(slashAt, Math.max(0, slash.length - 1));
  const box = useRef<HTMLTextAreaElement>(null);
  const ghostEl = useRef<HTMLDivElement>(null);
  useImperativeHandle(ref, () => box.current as HTMLTextAreaElement);
  const [words, setWords] = useState<Vocabulary | null>(null);
  const [hidden, setHidden] = useState<string | null>(null);
  const [atEnd, setAtEnd] = useState(true);
  // The last fix, to put back with Backspace or the Undo link.
  const [fix, setFix] = useState<{ original: string; fixed: string; text: string; at: number } | null>(null);
  const current = useRef(value);
  current.current = value;

  useEffect(() => {
    if (complete) vocabulary().then(setWords);
  }, [complete]);
  useEffect(() => {
    if (!fix) return;
    const timer = setTimeout(() => setFix(null), 6000);
    return () => clearTimeout(timer);
  }, [fix]);

  const offer = value === "" ? (suggestion ?? "") : "";
  const auto = complete && words && atEnd && value !== "" && hidden !== value && !slash.length ? completion(value, words) : { text: "", word: false };
  const ghost = offer || auto.text;

  // The grey text sits behind the box, laid out exactly like it.
  useLayoutEffect(() => {
    const ta = box.current;
    const g = ghostEl.current;
    if (!ta || !g) return;
    const cs = getComputedStyle(ta);
    for (const p of ["font", "lineHeight", "letterSpacing", "padding", "borderWidth", "borderStyle", "textIndent", "wordSpacing", "tabSize"] as const) {
      g.style[p] = cs[p];
    }
    g.style.borderColor = "transparent";
    g.style.height = `${ta.offsetHeight}px`;
    g.style.width = `${ta.offsetWidth}px`;
    g.scrollTop = ta.scrollTop;
  });

  const take = () => {
    if (offer) {
      onValueChange(offer);
      onSuggestionGone?.();
    } else {
      onValueChange(value + auto.text + (auto.word ? " " : ""));
    }
  };

  const change = (next: string) => {
    if (suggestion && next !== "") onSuggestionGone?.();
    setHidden(null);
    onValueChange(next);
    const ta = box.current;
    const caretAtEnd = !ta || ta.selectionStart >= next.length - 1;
    // A word just finished (one character typed, a space or punctuation):
    // Windows' spell checker fixes it when it's sure.
    if (!correct || next.length !== value.length + 1 || !next.startsWith(value) || !caretAtEnd) return;
    const end = next.slice(-1);
    if (!BOUNDARY.test(end)) return;
    const before = next.slice(0, -1);
    const m = before.match(WORD_END);
    if (!m || inCodeOrLink(before)) return;
    const word = m[0];
    const known = !!words?.words.some(([w, n]) => w === word.toLowerCase() && n >= 3);
    api
      .typingFix(word, language(), known)
      .then((fixed) => {
        if (!fixed || fixed === word) return;
        // Only if the word is still there as it was typed.
        const now = current.current;
        const at = findTyped(now, word + end, before.length - word.length);
        if (at < 0) return;
        const text = now.slice(0, at) + fixed + now.slice(at + word.length);
        // A second fix arriving before the next render builds on this one.
        current.current = text;
        setFix({ original: word, fixed, text, at });
        onValueChange(text);
      })
      .catch(() => {});
  };

  const undoFix = () => {
    if (!fix) return;
    const i = findTyped(value, fix.fixed, fix.at);
    if (i >= 0) onValueChange(value.slice(0, i) + fix.original + value.slice(i + fix.fixed.length));
    setFix(null);
  };

  const key = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (slash.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSlashAt((slashPick + (e.key === "ArrowDown" ? 1 : slash.length - 1)) % slash.length);
        return;
      }
      if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
        e.preventDefault();
        setSlashAt(0);
        slash[slashPick].pick();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setSlashOff(value);
        return;
      }
    }
    if (ghost && e.key === "Tab" && !e.shiftKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      take();
      return;
    }
    if (ghost && e.key === "Escape") {
      e.preventDefault();
      if (offer) onSuggestionGone?.();
      else setHidden(value);
      return;
    }
    // Backspace right after a fix puts the typed word back.
    if (fix && e.key === "Backspace" && value === fix.text) {
      e.preventDefault();
      undoFix();
      return;
    }
    onKeyDown?.(e);
  };

  const sync = () => {
    const ta = box.current;
    if (!ta) return;
    setAtEnd(ta.selectionStart === ta.value.length && ta.selectionEnd === ta.value.length);
    if (ghostEl.current) ghostEl.current.scrollTop = ta.scrollTop;
  };

  return (
    <div className="smart-input">
      {slash.length > 0 && (
        <div className="slash-menu" role="listbox" aria-label={t("Saved prompts and commands")}>
          {slash.map((c, i) => (
            <button
              key={c.name}
              type="button"
              role="option"
              aria-selected={i === slashPick}
              className={i === slashPick ? "active" : ""}
              onMouseDown={(e) => e.preventDefault()}
              onMouseEnter={() => setSlashAt(i)}
              onClick={() => {
                setSlashAt(0);
                c.pick();
              }}
            >
              <strong>/{c.name}</strong>
              <span className="muted small ellipsis">{c.hint}</span>
            </button>
          ))}
        </div>
      )}
      <div className="smart-ghost" ref={ghostEl} aria-hidden>
        <span className="smart-typed">{value}</span>
        <span className="smart-hint">{ghost}</span>
        {ghost && <span className="smart-tab">{t("Tab")}</span>}
      </div>
      <textarea
        {...rest}
        className={className}
        ref={box}
        value={value}
        placeholder={offer ? "" : placeholder}
        // Named for screen readers even while a suggestion hides the placeholder.
        aria-label={rest["aria-label"] ?? placeholder}
        onChange={(e) => change(e.target.value)}
        onKeyDown={key}
        onKeyUp={sync}
        onClick={sync}
        onSelect={sync}
        onScroll={sync}
      />
      {fix && (
        <div className="smart-fix" role="status">
          {t("Changed “{from}” to “{to}”.", { from: fix.original, to: fix.fixed })}{" "}
          <button type="button" className="link" onMouseDown={(e) => e.preventDefault()} onClick={undoFix}>
            {t("Undo")}
          </button>
        </div>
      )}
    </div>
  );
});
