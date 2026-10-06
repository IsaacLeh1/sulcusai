// SPDX-License-Identifier: AGPL-3.0-only
// Translate: text and text documents, with your local model.
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Language } from "../api";
import type { PushToast } from "../components/Toasts";

const LAST_LANGUAGE = "sulcusai.translate.to";

function lastLanguage(): string {
  try {
    return localStorage.getItem(LAST_LANGUAGE) || "es";
  } catch {
    return "es";
  }
}

export function TranslateView({ hasModel, onGoModels, toast }: { hasModel: boolean; onGoModels: () => void; toast: PushToast }) {
  const [languages, setLanguages] = useState<Language[]>([]);
  const [to, setTo] = useState(lastLanguage);
  const [text, setText] = useState("");
  const [result, setResult] = useState("");
  const [detected, setDetected] = useState<string | null>(null);
  const [busy, setBusy] = useState<"text" | "file" | null>(null);

  useEffect(() => {
    api.languages().then(setLanguages).catch(() => {});
  }, []);

  const pick = (code: string) => {
    setTo(code);
    try {
      localStorage.setItem(LAST_LANGUAGE, code);
    } catch {
      // Remembering the language is only a convenience.
    }
  };

  const run = async () => {
    if (!text.trim()) return;
    setBusy("text");
    try {
      const r = await api.translate(text, to, true);
      setResult(r.text);
      setDetected(r.detected);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const translateFile = async () => {
    const path = await open({
      multiple: false,
      title: "Choose a document to translate",
      filters: [{ name: "Text documents", extensions: ["txt", "md", "markdown", "srt", "vtt", "csv", "html", "htm", "json"] }],
    });
    if (typeof path !== "string") return;
    setBusy("file");
    try {
      const out = await api.translateFile(path, to);
      toast(`Saved the translation as ${out}`, "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const toName = languages.find((l) => l.code === to)?.name ?? to;
  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>Translate</h1>
          <p className="muted">Translated on this PC by your local model. Nothing is sent anywhere.</p>
        </div>
        <button className="btn" onClick={translateFile} disabled={!hasModel || busy !== null}>
          {busy === "file" ? "Translating the document…" : "Translate a document…"}
        </button>
      </header>
      {!hasModel && (
        <div className="callout">
          Translation uses your chat model. <button className="link" onClick={onGoModels}>Install one from Models</button>
        </div>
      )}
      <div className="translate-grid">
        <section className="card translate-pane">
          <div className="row">
            <strong>{detected ? `${detected} (detected)` : "Any language"}</strong>
            <span className="spacer" />
            <button className="icon-btn" title="Read aloud" aria-label="Read the original aloud" disabled={!text.trim()} onClick={() => api.speak(text).catch((e) => toast(errorText(e), "error"))}>
              🔊
            </button>
          </div>
          <textarea
            className="input"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setDetected(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) run();
            }}
            placeholder="Type or paste text to translate"
            aria-label="Text to translate"
          />
        </section>
        <section className="card translate-pane">
          <div className="row">
            <select value={to} onChange={(e) => pick(e.target.value)} aria-label="Translate into">
              {languages.map((l) => (
                <option key={l.code} value={l.code}>{l.name}</option>
              ))}
            </select>
            <span className="spacer" />
            <button className="icon-btn" title="Read aloud" aria-label="Read the translation aloud" disabled={!result} onClick={() => api.speak(result, to).catch((e) => toast(errorText(e), "error"))}>
              🔊
            </button>
            <button className="btn ghost small" disabled={!result} onClick={() => navigator.clipboard.writeText(result).then(() => toast("Copied.", "success"))}>
              Copy
            </button>
          </div>
          <div className="translate-out" aria-live="polite">
            {busy === "text" ? <span className="muted">Translating into {toName}…</span> : result || <span className="muted">The translation appears here.</span>}
          </div>
        </section>
      </div>
      <div className="row">
        <button className="btn primary" onClick={run} disabled={!hasModel || !text.trim() || busy !== null}>
          Translate into {toName}
        </button>
        <span className="muted small">Ctrl+Enter also translates. Documents: text, Markdown, subtitles (.srt, .vtt), CSV, HTML and JSON keep their formatting.</span>
      </div>
      <p className="muted small">
        For live speech, turn on translated captions when you begin a meeting. Translating Word and PDF files, and text in
        pictures, comes with the documents and vision features.
      </p>
    </div>
  );
}
