// SPDX-License-Identifier: AGPL-3.0-only
// Translate: text and text documents, with your local model.
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Language } from "../api";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

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
      title: t("Choose a document to translate"),
      filters: [{ name: t("Text documents"), extensions: ["txt", "md", "markdown", "srt", "vtt", "csv", "html", "htm", "json"] }],
    });
    if (typeof path !== "string") return;
    setBusy("file");
    try {
      const out = await api.translateFile(path, to);
      toast(t("Saved the translation as {path}", { path: out }), "success");
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
          <h1>{t("Translate")}</h1>
          <p className="muted">{t("Translated on this PC by your local model. Nothing is sent anywhere.")}</p>
        </div>
        <button className="btn" onClick={translateFile} disabled={!hasModel || busy !== null}>
          {busy === "file" ? t("Translating the document…") : t("Translate a document…")}
        </button>
      </header>
      {!hasModel && (
        <div className="callout">
          {t("Translation uses your chat model.")} <button className="link" onClick={onGoModels}>{t("Install one from Models")}</button>
        </div>
      )}
      <div className="translate-grid">
        <section className="card translate-pane">
          <div className="row">
            <strong>{detected ? t("{language} (detected)", { language: detected }) : t("Any language")}</strong>
            <span className="spacer" />
            <button className="icon-btn" title={t("Read aloud")} aria-label={t("Read the original aloud")} disabled={!text.trim()} onClick={() => api.speak(text).catch((e) => toast(errorText(e), "error"))}>
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
            placeholder={t("Type or paste text to translate")}
            aria-label={t("Text to translate")}
          />
        </section>
        <section className="card translate-pane">
          <div className="row">
            <select value={to} onChange={(e) => pick(e.target.value)} aria-label={t("Translate into")}>
              {languages.map((l) => (
                <option key={l.code} value={l.code}>{l.name}</option>
              ))}
            </select>
            <span className="spacer" />
            <button className="icon-btn" title={t("Read aloud")} aria-label={t("Read the translation aloud")} disabled={!result} onClick={() => api.speak(result, to).catch((e) => toast(errorText(e), "error"))}>
              🔊
            </button>
            <button className="btn ghost small" disabled={!result} onClick={() => navigator.clipboard.writeText(result).then(() => toast(t("Copied."), "success"))}>
              {t("Copy")}
            </button>
          </div>
          <div className="translate-out" aria-live="polite">
            {busy === "text" ? <span className="muted">{t("Translating into {language}…", { language: toName })}</span> : result || <span className="muted">{t("The translation appears here.")}</span>}
          </div>
        </section>
      </div>
      <div className="row">
        <button className="btn primary" onClick={run} disabled={!hasModel || !text.trim() || busy !== null}>
          {t("Translate into {language}", { language: toName })}
        </button>
        <span className="muted small">{t("Ctrl+Enter also translates. Documents: text, Markdown, subtitles (.srt, .vtt), CSV, HTML and JSON keep their formatting.")}</span>
      </div>
      <p className="muted small">
        {t("For live speech, turn on translated captions when you begin a meeting. Translating Word and PDF files, and text in pictures, comes with the documents and vision features.")}
      </p>
    </div>
  );
}
