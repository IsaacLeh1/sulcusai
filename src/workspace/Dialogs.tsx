// SPDX-License-Identifier: AGPL-3.0-only
import { api, errorText, type Chat, type Project } from "../api";
import { t } from "../i18n";
import { Modal } from "../components/Modal";
import { NameModal } from "../components/ChatFolders";
import type { PushToast } from "../components/Toasts";
import type { View } from "../App";

/** Renaming a project keeps its instructions. */
export function RenameProjectDialog({ project, onClose, refreshProjects, toast }: {
  project: Project;
  onClose: () => void;
  refreshProjects: () => Promise<void>;
  toast: PushToast;
}) {
  return (
    <NameModal
      title={t("Rename project")}
      initial={project.name}
      label={t("Project name")}
      maxLength={80}
      onClose={onClose}
      onSave={async (name) => {
        onClose();
        try {
          await api.updateProject(project.id, name, project.instructions);
          await refreshProjects();
        } catch (e) {
          toast(errorText(e), "error");
        }
      }}
    />
  );
}

/** Deleting a project keeps its chats, which move out of it. */
export function DeleteProjectDialog({ project, onClose, activeProject, setActiveProject, setView, refreshProjects, refreshChats, toast }: {
  project: Project;
  onClose: () => void;
  activeProject: string | null;
  setActiveProject: (id: string | null) => void;
  setView: (v: View) => void;
  refreshProjects: () => Promise<void>;
  refreshChats: () => Promise<void>;
  toast: PushToast;
}) {
  return (
    <Modal title={t("Delete {name}?", { name: project.name })} onClose={onClose}>
      <p>{t("This deletes the project's instructions and memories. Its chats are kept and move out of the project. Files in its folders aren't touched.")}</p>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>
          {t("Cancel")}
        </button>
        <button
          className="btn danger-fill"
          autoFocus
          onClick={async () => {
            onClose();
            try {
              await api.deleteProject(project.id);
              if (activeProject === project.id) {
                setActiveProject(null);
                setView("chat");
              }
              await refreshProjects();
              await refreshChats();
              toast(t("Deleted {name}.", { name: project.name }), "success");
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        >
          {t("Delete project")}
        </button>
      </div>
    </Modal>
  );
}

/** Deleting the chats picked together. */
export function DeleteChatsDialog({ ids, chats, onClose, onDone, activeChat, setActiveChat, refreshChats, refreshFolders, toast }: {
  ids: string[];
  chats: Chat[];
  onClose: () => void;
  /** Called as the delete starts, so the selection clears. */
  onDone: () => void;
  activeChat: string | null;
  setActiveChat: (id: string | null) => void;
  refreshChats: () => Promise<void>;
  refreshFolders: () => void;
  toast: PushToast;
}) {
  return (
    <Modal title={t("Delete {n} chats?", { n: ids.length })} onClose={onClose}>
      <p>{t("These chats and their messages will be deleted from this PC. This can't be undone.")}</p>
      <ul className="small plain-list">
        {ids.map((id) => (
          <li key={id} className="ellipsis">{chats.find((c) => c.id === id)?.title ?? id}</li>
        ))}
      </ul>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button
          className="btn danger-fill"
          onClick={async () => {
            onClose();
            onDone();
            try {
              for (const id of ids) await api.deleteChat(id);
              if (activeChat && ids.includes(activeChat)) setActiveChat(null);
              await refreshChats();
              refreshFolders();
              toast(t("Deleted {n} chats.", { n: ids.length }), "success");
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        >
          {t("Delete them")}
        </button>
      </div>
    </Modal>
  );
}

/** Deleting one chat from its menu. */
export function DeleteChatDialog({ chat, onClose, activeChat, setActiveChat, refreshChats, refreshFolders, toast }: {
  chat: Chat;
  onClose: () => void;
  activeChat: string | null;
  setActiveChat: (id: string | null) => void;
  refreshChats: () => Promise<void>;
  refreshFolders: () => void;
  toast: PushToast;
}) {
  return (
    <Modal title={t("Delete this chat?")} onClose={onClose}>
      <p>
        {t("“{title}” and its messages will be deleted from this PC. This can't be undone.", { title: chat.title })}
      </p>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button
          className="btn danger-fill"
          autoFocus
          onClick={async () => {
            onClose();
            try {
              await api.deleteChat(chat.id);
              if (activeChat === chat.id) setActiveChat(null);
              await refreshChats();
              refreshFolders();
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        >
          {t("Delete")}
        </button>
      </div>
    </Modal>
  );
}
