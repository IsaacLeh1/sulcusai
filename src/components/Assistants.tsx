// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Assistant } from "../api";
import { t } from "../i18n";
import { ModelSelect } from "./ModelChoices";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const EMPTY: Assistant = { id: "", name: "", description: "", instructions: "", model: null, starters: [] };

/** Making or changing an assistant of the person's own. */
function AssistantEditor({ initial, onClose, onSaved, toast }: { initial: Assistant | null; onClose: () => void; onSaved: (all: Assistant[]) => void; toast: PushToast }) {
  const [a, setA] = useState<Assistant>(initial ?? EMPTY);
  const [starters, setStarters] = useState((initial?.starters ?? []).join("\n"));
  const save = async () => {
    try {
      onSaved(await api.saveAssistant({ ...a, starters: starters.split("\n") }));
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <Modal title={initial ? t("Edit {name}", { name: initial.name }) : t("New assistant")} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <label>
          {t("Name")}
          <input className="input" value={a.name} onChange={(e) => setA({ ...a, name: e.target.value })} placeholder={t("Email writer, Spanish tutor…")} maxLength={60} autoFocus aria-label={t("Name")} />
        </label>
        <label>
          {t("What it's for")} <span className="muted small">{t("(one line, shown in the list)")}</span>
          <input className="input" value={a.description} onChange={(e) => setA({ ...a, description: e.target.value })} maxLength={140} aria-label={t("What it's for")} />
        </label>
        <label>
          {t("Instructions")}
          <textarea
            className="input"
            rows={6}
            value={a.instructions}
            onChange={(e) => setA({ ...a, instructions: e.target.value })}
            placeholder={t("How it should answer: its role, tone, what to always or never do, what it should know about you.")}
            aria-label={t("Instructions")}
          />
        </label>
        <label className="row wrap">
          <span>{t("Model")}</span>
          <ModelSelect value={a.model} label={t("Model")} onChange={(model) => setA({ ...a, model })} />
        </label>
        <label>
          {t("Ways to start")} <span className="muted small">{t("(one per line, offered in a new chat)")}</span>
          <textarea className="input" rows={3} value={starters} onChange={(e) => setStarters(e.target.value)} placeholder={t("Write a polite follow-up to this email:")} aria-label={t("Ways to start")} />
        </label>
        <div className="modal-actions">
          {initial && (
            <button
              type="button"
              className="btn ghost danger"
              onClick={async () => {
                try {
                  onSaved(await api.deleteAssistant(initial.id));
                  onClose();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Delete")}
            </button>
          )}
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!a.name.trim()}>{t("Save")}</button>
        </div>
      </form>
    </Modal>
  );
}

/** The sidebar's assistants: click one for a new chat with it; right-click to change it. */
export function AssistantsSection({ onChat, toast }: { onChat: (chatId: string) => void; toast: PushToast }) {
  const [all, setAll] = useState<Assistant[]>([]);
  const [editing, setEditing] = useState<Assistant | null | "new">(null);
  useEffect(() => {
    api.assistants().then(setAll).catch(() => {});
  }, []);
  return (
    <div className="side-section">
      <div className="side-head">
        <span>{t("Assistants")}</span>
        <button className="icon-btn" title={t("New assistant")} aria-label={t("New assistant")} onClick={() => setEditing("new")}>+</button>
      </div>
      {all.map((a) => (
        <button
          key={a.id}
          className="chat-item"
          title={a.description || t("A new chat with {name}", { name: a.name })}
          onClick={async () => {
            try {
              onChat((await api.chatWithAssistant(a.id)).id);
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
          onContextMenu={(e) => {
            e.preventDefault();
            setEditing(a);
          }}
        >
          <span className="assistant-mark" aria-hidden>{a.name.trim().charAt(0).toUpperCase()}</span>
          <span className="ellipsis">{a.name}</span>
        </button>
      ))}
      {all.length > 0 && <p className="muted small pad">{t("Right-click one to change it.")}</p>}
      {editing && <AssistantEditor initial={editing === "new" ? null : editing} onClose={() => setEditing(null)} onSaved={setAll} toast={toast} />}
    </div>
  );
}

/** A new chat with an assistant: the ways to start it offers. */
export function AssistantStarters({ chatId, onPick }: { chatId: string; onPick: (text: string) => void }) {
  const [a, setA] = useState<Assistant | null>(null);
  useEffect(() => {
    api.chatAssistant(chatId).then(setA).catch(() => setA(null));
  }, [chatId]);
  if (!a) return null;
  return (
    <div className="assistant-start">
      <strong>{a.name}</strong>
      {a.description && <span className="muted small block">{a.description}</span>}
      {a.starters.length > 0 && (
        <div className="chips">
          {a.starters.map((s) => (
            <button key={s} className="chip" onClick={() => onPick(s)}>
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
