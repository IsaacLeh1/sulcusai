// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { api, errorText, type SavedPrompt } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** The saved prompts, kept current when they change anywhere. */
export function useSavedPrompts(): [SavedPrompt[], (all: SavedPrompt[]) => void] {
  const [all, setAll] = useState<SavedPrompt[]>([]);
  useEffect(() => {
    const load = () => api.savedPrompts().then(setAll).catch(() => {});
    load();
    // Changed in another chat's box on this window.
    window.addEventListener("prompts-changed", load);
    return () => window.removeEventListener("prompts-changed", load);
  }, []);
  const set = useCallback((next: SavedPrompt[]) => {
    setAll(next);
    window.dispatchEvent(new Event("prompts-changed"));
  }, []);
  return [all, set];
}

/** "/name more words": the saved prompt's text, with the words in place of
 *  {input} or after it. Anything else comes back as it was. */
export function expandSlash(text: string, prompts: SavedPrompt[]): string {
  const m = /^\/([\p{L}\p{N}-]+)(?:\s+([\s\S]*))?$/u.exec(text.trim());
  const p = m && prompts.find((x) => x.name === m[1].toLowerCase());
  if (!p) return text;
  const rest = (m[2] ?? "").trim();
  if (p.text.includes("{input}")) return p.text.replaceAll("{input}", rest);
  return rest ? `${p.text}\n\n${rest}` : p.text;
}

/** Adding, changing and removing saved prompts. */
export function SavedPromptsManager({ prompts, onChange, onClose, toast }: { prompts: SavedPrompt[]; onChange: (all: SavedPrompt[]) => void; onClose: () => void; toast: PushToast }) {
  const [editing, setEditing] = useState<SavedPrompt | null>(null);
  const [name, setName] = useState("");
  const [text, setText] = useState("");
  const start = (p: SavedPrompt | null) => {
    setEditing(p ?? { id: "", name: "", text: "" });
    setName(p?.name ?? "");
    setText(p?.text ?? "");
  };
  const save = async () => {
    try {
      onChange(await api.savePrompt(editing?.id ?? "", name, text));
      setEditing(null);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <Modal title={t("Saved prompts")} onClose={onClose}>
      <p className="muted small">{t("Type / in a chat to use one. Add words after its name to go with it, or put {input} in the text where they should go.")}</p>
      {editing ? (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <label>
            {t("Name")}
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="tldr" maxLength={40} autoFocus aria-label={t("Name")} />
          </label>
          <label>
            {t("Text")}
            <textarea className="input" rows={6} value={text} onChange={(e) => setText(e.target.value)} placeholder={t("Sum this up in three short lines: {input}")} aria-label={t("Text")} />
          </label>
          <div className="modal-actions">
            <button type="button" className="btn" onClick={() => setEditing(null)}>{t("Cancel")}</button>
            <button type="submit" className="btn primary" disabled={!name.trim() || !text.trim()}>{t("Save")}</button>
          </div>
        </form>
      ) : (
        <>
          {prompts.length === 0 && <p className="muted small">{t("None yet.")}</p>}
          <ul className="saved-prompts">
            {prompts.map((p) => (
              <li key={p.id}>
                <div>
                  <strong>/{p.name}</strong>
                  <span className="muted small block ellipsis">{p.text}</span>
                </div>
                <button className="btn ghost small" onClick={() => start(p)}>{t("Edit")}</button>
                <button
                  className="btn ghost small danger"
                  onClick={async () => {
                    try {
                      onChange(await api.deletePrompt(p.id));
                    } catch (e) {
                      toast(errorText(e), "error");
                    }
                  }}
                >
                  {t("Delete")}
                </button>
              </li>
            ))}
          </ul>
          <div className="modal-actions">
            <button className="btn" onClick={() => start(null)}>{t("+ New prompt")}</button>
            <button className="btn primary" onClick={onClose}>{t("Done")}</button>
          </div>
        </>
      )}
    </Modal>
  );
}
