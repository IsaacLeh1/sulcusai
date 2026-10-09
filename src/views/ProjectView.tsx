// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Chat, type Memory, type Project } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

interface Props {
  projectId: string;
  chats: Chat[];
  onOpenChat: (id: string) => void;
  onNewChat: () => void;
  onChanged: () => void;
  onDeleted: () => void;
  toast: PushToast;
}

export function ProjectView({ projectId, chats, onOpenChat, onNewChat, onChanged, onDeleted, toast }: Props) {
  const [project, setProject] = useState<Project | null>(null);
  const [name, setName] = useState("");
  const [instructions, setInstructions] = useState("");
  const [memories, setMemories] = useState<Memory[]>([]);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const load = async () => {
    const p = (await api.projects()).find((x) => x.id === projectId) ?? null;
    setProject(p);
    if (p) {
      setName(p.name);
      setInstructions(p.instructions);
    }
    setMemories((await api.memories()).filter((m) => m.project_id === projectId));
  };
  useEffect(() => {
    load();
  }, [projectId]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!project) return <div className="page"><p className="muted">{t("Loading…")}</p></div>;
  const dirty = name !== project.name || instructions !== project.instructions;
  const projectChats = chats.filter((c) => c.project_id === projectId);

  const save = async () => {
    try {
      await api.updateProject(projectId, name, instructions);
      await load();
      onChanged();
      toast(t("Project saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const addFolder = async () => {
    const picked = await open({ directory: true, multiple: false, title: t("Add a folder to {name}", { name: project.name }) });
    if (typeof picked !== "string") return;
    try {
      await api.addProjectFolder(projectId, picked);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>📚 {project.name}</h1>
            <p className="muted">{t("A project keeps its own instructions, folders and memories. Every chat in it uses them.")}</p>
          </div>
          <button className="btn primary" onClick={onNewChat}>{t("+ New chat in project")}</button>
        </header>

        <section className="card form">
          <label>
            {t("Name")}
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} />
          </label>
          <label>
            {t("Instructions")}
            <textarea
              className="input"
              rows={5}
              value={instructions}
              onChange={(e) => setInstructions(e.target.value)}
              maxLength={8000}
              placeholder={t("What should the assistant know or do in this project? For example: This is my thesis on coral reefs. Use APA citations and a formal tone.")}
            />
          </label>
          <div>
            <button className="btn primary" onClick={save} disabled={!dirty || !name.trim()}>{t("Save")}</button>
          </div>
        </section>

        <section className="card">
          <h2>{t("Folders")}</h2>
          <p className="muted small">{t("Chats in this project can use these folders, in addition to any you shared everywhere.")}</p>
          {project.folders.length === 0 && <p className="small">{t("No folders yet.")}</p>}
          {project.folders.map((f) => (
            <div key={f} className="setting-row">
              <span className="ellipsis small" title={f}>📁 {f}</span>
              <button
                className="btn ghost danger small"
                onClick={async () => {
                  await api.removeProjectFolder(projectId, f);
                  load();
                }}
              >
                {t("Remove")}
              </button>
            </div>
          ))}
          <button className="btn" onClick={addFolder}>{t("+ Add a folder")}</button>
        </section>

        <section className="card">
          <h2>{t("Memories in this project")}</h2>
          {memories.length === 0 ? (
            <p className="muted small">{t("None yet. The assistant saves them as you work, and you can manage them on the Memory page.")}</p>
          ) : (
            <ul className="memory-list compact">
              {memories.map((m) => (
                <li key={m.id}><span className="memory-text">{m.content}</span></li>
              ))}
            </ul>
          )}
        </section>

        <section className="card">
          <h2>{t("Chats")}</h2>
          {projectChats.length === 0 && <p className="muted small">{t("No chats in this project yet.")}</p>}
          {projectChats.map((c) => (
            <button key={c.id} className="chat-item" onClick={() => onOpenChat(c.id)}>
              <span className="ellipsis">{c.title}</span>
            </button>
          ))}
        </section>

        <button className="btn ghost danger" onClick={() => setConfirmDelete(true)}>{t("Delete project")}</button>
      </div>
      {confirmDelete && (
        <Modal title={t("Delete {name}?", { name: project.name })} onClose={() => setConfirmDelete(false)}>
          <p>
            {memories.length === 1
              ? t("This deletes the project's instructions and its 1 memory. Its chats are kept and move out of the project. Files in its folders aren't touched.")
              : t("This deletes the project's instructions and its {n} memories. Its chats are kept and move out of the project. Files in its folders aren't touched.", { n: memories.length })}
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                await api.deleteProject(projectId);
                setConfirmDelete(false);
                onDeleted();
              }}
            >
              {t("Delete project")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
