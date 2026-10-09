// SPDX-License-Identifier: AGPL-3.0-only
// Notes: Markdown notes with folders, tags and pins, kept encrypted on this PC.
import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorText, on, type Note, type NoteBody } from "../api";
import { Markdown } from "../components/Markdown";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

const EMPTY: NoteBody = { title: "", body: "", folder: "", tags: [] };

export function NotesView({ toast }: { toast: PushToast }) {
  const [notes, setNotes] = useState<Note[] | null>(null);
  const [query, setQuery] = useState("");
  const [folder, setFolder] = useState<string | null>(null);
  const [openId, setOpenId] = useState<string | "new" | null>(null);

  const load = () => api.notes().then(setNotes).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    // Notes written on another PC arrive by sync.
    const sub = on("sync:changed", () => load());
    return () => {
      sub.then((un) => un());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const folders = useMemo(() => [...new Set((notes ?? []).map((n) => n.folder).filter(Boolean))].sort(), [notes]);
  const shown = useMemo(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return (notes ?? []).filter((n) => {
      if (folder !== null && n.folder !== folder) return false;
      const hay = `${n.title} ${n.body} ${n.folder} ${n.tags.join(" ")}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
  }, [notes, query, folder]);

  const current = openId && openId !== "new" ? notes?.find((n) => n.id === openId) ?? null : null;

  return (
    <div className="notes-page">
      <aside className="notes-list">
        <div className="row">
          <input className="input" placeholder={t("Search notes")} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t("Search notes")} />
          <button className="btn primary small" onClick={() => setOpenId("new")} title={t("New note")}>{t("+ New")}</button>
        </div>
        {folders.length > 0 && (
          <div className="folder-chips">
            <button className={`chip ${folder === null ? "active" : ""}`} onClick={() => setFolder(null)}>{t("All")}</button>
            {folders.map((f) => (
              <button key={f} className={`chip ${folder === f ? "active" : ""}`} onClick={() => setFolder(f)}>📂 {f}</button>
            ))}
          </div>
        )}
        {notes && notes.length === 0 && <p className="muted small">{t("No notes yet. Click New, or ask the assistant to “note that down”.")}</p>}
        {notes && notes.length > 0 && shown.length === 0 && <p className="muted small">{t("Nothing matches.")}</p>}
        {shown.map((n) => (
          <button key={n.id} className={`note-item ${openId === n.id ? "active" : ""}`} onClick={() => setOpenId(n.id)}>
            <span className="row">
              {n.pinned && <span aria-label={t("Pinned")}>📌</span>}
              <strong className="ellipsis">{n.title}</strong>
            </span>
            <span className="small muted ellipsis">{n.body.replace(/[#*_`>]/g, "").slice(0, 120)}</span>
            {n.tags.length > 0 && <span className="small tags-line">{n.tags.map((tag) => `#${tag}`).join(" ")}</span>}
          </button>
        ))}
      </aside>
      <section className="note-editor">
        {openId ? (
          <Editor
            key={openId}
            note={current}
            onSaved={(n) => {
              load();
              setOpenId(n.id);
            }}
            onDeleted={() => {
              setOpenId(null);
              load();
            }}
            onPinned={load}
            toast={toast}
          />
        ) : (
          <div className="empty">
            <p className="muted">{t("Pick a note, or start a new one.")}</p>
          </div>
        )}
      </section>
    </div>
  );
}

function Editor({ note, onSaved, onDeleted, onPinned, toast }: { note: Note | null; onSaved: (n: Note) => void; onDeleted: () => void; onPinned: () => void; toast: PushToast }) {
  const [draft, setDraft] = useState<NoteBody>(note ? { title: note.title, body: note.body, folder: note.folder, tags: note.tags } : EMPTY);
  const [tags, setTags] = useState((note?.tags ?? []).join(", "));
  const [preview, setPreview] = useState(!!note);
  const [confirm, setConfirm] = useState(false);
  const saved = useRef(JSON.stringify(draft));
  const dirty = JSON.stringify({ ...draft, tags: tags.split(",").map((tag) => tag.trim()).filter(Boolean) }) !== saved.current;

  const save = async () => {
    try {
      const body = { ...draft, tags: tags.split(",").map((tag) => tag.trim()).filter(Boolean) };
      const n = await api.saveNote(note?.id ?? null, body);
      saved.current = JSON.stringify({ title: n.title, body: n.body, folder: n.folder, tags: n.tags });
      onSaved(n);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  // Ctrl+S saves.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="editor">
      <div className="row">
        <input className="input title-input" value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })} placeholder={t("Title (or the first line)")} aria-label={t("Title")} />
        <button className="btn small" onClick={() => setPreview((p) => !p)}>{preview ? t("Edit") : t("Preview")}</button>
        {note && (
          <button className="btn ghost small" onClick={() => api.pinNote(note.id, !note.pinned).then(onPinned)} title={note.pinned ? t("Unpin") : t("Pin to the top")}>
            {note.pinned ? t("Unpin") : t("📌 Pin")}
          </button>
        )}
        <button className="btn primary small" onClick={save} disabled={!dirty && !!note}>{t("Save")}</button>
      </div>
      <div className="row">
        <input className="input small-input" value={draft.folder} onChange={(e) => setDraft({ ...draft, folder: e.target.value })} placeholder={t("Folder")} aria-label={t("Folder")} />
        <input className="input small-input" value={tags} onChange={(e) => setTags(e.target.value)} placeholder={t("Tags, comma separated")} aria-label={t("Tags")} />
        {note && <button className="btn ghost small danger" onClick={() => setConfirm(true)}>{t("Delete")}</button>}
      </div>
      {preview ? (
        <div className="note-preview" onDoubleClick={() => setPreview(false)}>
          {draft.body ? <Markdown text={draft.body} /> : <p className="muted">{t("Empty note. Double-click to edit.")}</p>}
        </div>
      ) : (
        <textarea className="input note-body" value={draft.body} onChange={(e) => setDraft({ ...draft, body: e.target.value })} placeholder={t("Write in Markdown: # headings, - lists, **bold**…")} aria-label={t("Note text")} autoFocus />
      )}
      <p className="muted small">{dirty ? t("Unsaved changes (Ctrl+S saves).") : note ? t("Saved {time}", { time: new Date(note.updated_at).toLocaleString() }) : ""}</p>
      {confirm && note && (
        <Modal title={t("Delete this note?")} onClose={() => setConfirm(false)}>
          <p>{t("“{title}” will be permanently deleted from this PC.", { title: note.title })}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
            <button className="btn danger-fill" onClick={() => api.deleteNote(note.id).then(onDeleted)}>{t("Delete")}</button>
          </div>
        </Modal>
      )}
    </div>
  );
}
