// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Assistant } from "../api";
import { t } from "../i18n";
import { ModelSelect } from "./ModelChoices";
import { Modal } from "./Modal";
import { RouteEditor } from "./EnterpriseCloud";
import { ImportIcon } from "./Icons";
import type { PushToast } from "./Toasts";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

const EMPTY: Assistant = { id: "", name: "", description: "", instructions: "", model: null, starters: [] };

/** Making or changing an assistant of the person's own. */
/** Ready-made assistants come in English from the app; shown in the window's language. */
export function shownAssistant(a: Assistant): Assistant {
  if (!a.id.startsWith("builtin-")) return a;
  return { ...a, name: t(a.name), description: t(a.description), starters: a.starters.map((s) => t(s)) };
}

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
        {initial?.id && <RouteEditor scope="assistant" id={initial.id} toast={toast} />}
        <div className="modal-actions">
          {initial && !initial.id.startsWith("builtin-") && (
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
    api.assistants().then((list) => setAll(list.map(shownAssistant))).catch(() => {});
  }, []);
  return (
    <div className="side-section">
      <div className="side-head">
        <span>{t("Assistants")}</span>
        <button
          className="icon-btn"
          title={t("Import a character card (PNG or JSON)")}
          aria-label={t("Import a character card")}
          onClick={async () => {
            const path = await openFile({ multiple: false, filters: [{ name: t("Character card"), extensions: ["png", "json"] }] });
            if (typeof path !== "string") return;
            try {
              const list = await invoke<Assistant[]>("import_character", { path });
              setAll(list.map(shownAssistant));
              toast(t("Character added. Click it for a new chat."), "success");
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        >
          <ImportIcon />
        </button>
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
      {editing && <AssistantEditor initial={editing === "new" ? null : editing} onClose={() => setEditing(null)} onSaved={(list) => setAll(list.map(shownAssistant))} toast={toast} />}
    </div>
  );
}

/** A new chat with an assistant: the ways to start it offers. */
export function AssistantStarters({ chatId, onPick }: { chatId: string; onPick: (text: string) => void }) {
  const [a, setA] = useState<Assistant | null>(null);
  useEffect(() => {
    api.chatAssistant(chatId).then((x) => setA(x && shownAssistant(x))).catch(() => setA(null));
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
      <GroupCast chatId={chatId} ownId={a.id} />
    </div>
  );
}

/** Other assistants joining this chat: each answers after the main reply. */
function GroupCast({ chatId, ownId }: { chatId: string; ownId: string }) {
  const [all, setAll] = useState<Assistant[]>([]);
  const [cast, setCast] = useState<string[]>([]);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    api.assistants().then((list) => setAll(list.map(shownAssistant).filter((x) => x.id !== ownId))).catch(() => {});
    invoke<string[]>("chat_cast", { chatId }).then(setCast).catch(() => {});
  }, [chatId, ownId]);
  if (all.length === 0) return null;
  const toggle = (id: string, on: boolean) => {
    const next = on ? [...cast, id].slice(0, 4) : cast.filter((x) => x !== id);
    invoke<string[]>("set_chat_cast", { chatId, ids: next }).then(setCast).catch(() => {});
  };
  return (
    <div className="group-cast small">
      {cast.length > 0 && <span className="muted block">{t("Also in this chat: {names}", { names: all.filter((x) => cast.includes(x.id)).map((x) => x.name).join(", ") })}</span>}
      <button className="link" onClick={() => setOpen(!open)}>{open ? t("Done") : t("Add characters to this chat")}</button>
      {open && (
        <div className="group-cast-list">
          {all.map((x) => (
            <label key={x.id} className="check">
              <input type="checkbox" checked={cast.includes(x.id)} disabled={!cast.includes(x.id) && cast.length >= 4} onChange={(e) => toggle(x.id, e.target.checked)} />
              <span>{x.name}</span>
            </label>
          ))}
          <span className="muted block">{t("Each answers after the main reply, in their own voice. Up to 4.")}</span>
        </div>
      )}
    </div>
  );
}
