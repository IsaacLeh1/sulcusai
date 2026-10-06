// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  errorText,
  on,
  type CatalogView,
  type Chat,
  type Connectivity,
  type EngineStatus,
  type InstallProgress,
  type Project,
  type SecurityStatus,
  type Settings,
} from "./api";
import { APP_NAME } from "./brand";
import { ChatView } from "./views/ChatView";
import { ModelsView } from "./views/ModelsView";
import { SettingsView } from "./views/SettingsView";
import { ActivityView } from "./views/ActivityView";
import { MemoryView } from "./views/MemoryView";
import { ProjectView } from "./views/ProjectView";
import { ScheduledView } from "./views/ScheduledView";
import { ConnectorsView } from "./views/ConnectorsView";
import { Onboarding } from "./views/Onboarding";
import { ConnectivityMenu } from "./components/ConnectivityMenu";
import { Modal } from "./components/Modal";
import { LockScreen } from "./components/Security";
import { Toasts, useToasts, type PushToast } from "./components/Toasts";
import { useIdleLock } from "./idle";

export type View = "chat" | "models" | "memory" | "scheduled" | "connectors" | "project" | "activity" | "settings";

/** Shows the lock screen until unlocked; the workspace mounts only after. */
export default function App() {
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  // Kept out here so unlocking returns to the same page and chat.
  const [view, setView] = useState<View>("chat");
  const [activeChat, setActiveChat] = useState<string | null>(null);
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const toasts = useToasts();

  const refreshSecurity = useCallback(async () => setSecurity(await api.security()), []);
  useEffect(() => {
    refreshSecurity();
    const sub = on("security:locked", () => refreshSecurity());
    return () => {
      sub.then((un) => un());
    };
  }, [refreshSecurity]);

  useIdleLock(security?.lock_enabled && !security.locked ? security.auto_lock_minutes : 0, () => {
    api.lockNow().catch(() => {});
  });

  let body;
  if (!security) body = <div className="empty"><p className="muted">Starting…</p></div>;
  else if (security.locked) body = <LockScreen status={security} onUnlocked={refreshSecurity} />;
  else
    body = (
      <Workspace
        security={security}
        onSecurityChanged={refreshSecurity}
        toast={toasts.push}
        nav={{ view, setView, activeChat, setActiveChat, activeProject, setActiveProject }}
      />
    );

  return (
    <>
      {body}
      <Toasts toasts={toasts.list} onDismiss={toasts.dismiss} />
    </>
  );
}

interface Nav {
  view: View;
  setView: (v: View) => void;
  activeChat: string | null;
  setActiveChat: (id: string | null) => void;
  activeProject: string | null;
  setActiveProject: (id: string | null) => void;
}

function Workspace({ security, onSecurityChanged, toast, nav }: { security: SecurityStatus; onSecurityChanged: () => void; toast: PushToast; nav: Nav }) {
  const { view, setView, activeChat, setActiveChat, activeProject, setActiveProject } = nav;
  const [chats, setChats] = useState<Chat[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [newProject, setNewProject] = useState(false);
  const refreshProjects = useCallback(async () => setProjects(await api.projects()), []);
  useEffect(() => {
    refreshProjects();
  }, [refreshProjects]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [catalog, setCatalog] = useState<CatalogView | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [progress, setProgress] = useState<Record<string, InstallProgress>>({});
  const [running, setRunning] = useState<string[]>([]);
  const activeChatRef = useRef(activeChat);
  activeChatRef.current = activeChat;

  const refreshChats = useCallback(async () => setChats(await api.chats()), []);
  const refreshCatalog = useCallback(async () => setCatalog(await api.catalog()), []);
  const refreshEngine = useCallback(async () => setEngine(await api.engineStatus()), []);

  useEffect(() => {
    refreshChats();
    refreshCatalog();
    refreshEngine();
    api.settings().then(setSettings);
  }, [refreshChats, refreshCatalog, refreshEngine]);

  // Install progress outlives page switches, so it is tracked here.
  useEffect(() => {
    const subs = [
      on("install:progress", (p) => setProgress((all) => ({ ...all, [p.model_id]: p }))),
      on("install:finished", (f) => {
        setProgress(({ [f.model_id]: _, ...rest }) => rest);
        refreshCatalog();
        refreshEngine();
        api.settings().then(setSettings);
        const name = catalog?.models.find((m) => m.id === f.model_id)?.name ?? "The model";
        if (f.ok) toast(`${name} is installed and ready.`, "success");
        else if (!f.cancelled) toast(`${name} didn't install: ${f.error}`, "error");
      }),
      on("chat:done", (p) => {
        setRunning((r) => r.filter((id) => id !== p.chat_id));
        refreshChats();
        refreshEngine();
      }),
      on("chat:start", (p) => setRunning((r) => (r.includes(p.chat_id) ? r : [...r, p.chat_id]))),
      on("schedule:ran", (p) => {
        refreshChats();
        if (p.error) toast(`Scheduled task “${p.name}” didn't run: ${p.error}`, "error");
        else toast(`Scheduled task “${p.name}” finished. Its result is in the sidebar.`, "success");
      }),
      // A long chat continued in a new one: follow it.
      on("chat:handoff", async (p) => {
        await refreshChats();
        setRunning((r) => [...r.filter((id) => id !== p.from), p.to]);
        if (activeChatRef.current === p.from) setActiveChat(p.to);
      }),
      // The model is loaded by the time context is measured.
      on("chat:context", () => refreshEngine()),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [catalog, refreshCatalog, refreshChats, refreshEngine, toast]);

  const installed = useMemo(() => (catalog?.models ?? []).filter((m) => m.installed), [catalog]);
  const current = chats.find((c) => c.id === activeChat) ?? null;

  const newChat = async (projectId: string | null = null, incognito = false) => {
    try {
      const chat = await api.createChatIn(projectId, incognito);
      await refreshChats();
      setActiveChat(chat.id);
      setView("chat");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  // Leaving an incognito chat deletes it.
  const lastChat = useRef<Chat | null>(null);
  useEffect(() => {
    const prev = lastChat.current;
    if (prev?.incognito && (prev.id !== activeChat || view !== "chat")) {
      api.leaveIncognito(view === "chat" ? activeChat : null).then(refreshChats);
    }
    lastChat.current = view === "chat" ? chats.find((c) => c.id === activeChat) ?? null : null;
  }, [activeChat, view, chats, refreshChats]);

  const openChat = (id: string) => {
    setActiveChat(id);
    setView("chat");
  };

  const changeConnectivity = async (level: Connectivity) => {
    setSettings(await api.setConnectivity(level));
  };

  const toggleChatWeb = useCallback(async () => {
    if (!current) return;
    await api.setChatWeb(current.id, !current.web);
    await refreshChats();
  }, [current, refreshChats]);

  // Ctrl+Shift+W toggles web for the open chat.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "w") {
        e.preventDefault();
        toggleChatWeb();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleChatWeb]);

  if (!settings) return <div className="empty"><p className="muted">Loading…</p></div>;

  if (!settings.onboarded) {
    return (
      <Onboarding
        catalog={catalog}
        progress={progress}
        toast={toast}
        onFinish={async () => {
          setSettings(await api.finishOnboarding());
          onSecurityChanged();
          refreshCatalog();
        }}
      />
    );
  }

  const loadedName = catalog?.models.find((m) => m.id === engine?.model_id)?.name;

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <img src="/logo.svg" alt="" width={26} height={26} />
          <span>{APP_NAME}</span>
          {security.lock_enabled && (
            <button className="icon-btn lock-btn" title="Lock now" aria-label="Lock now" onClick={() => api.lockNow()}>
              🔒
            </button>
          )}
        </div>
        <div className="new-chat-row">
          <button className="btn primary block" onClick={() => newChat()} disabled={installed.length === 0}>
            + New chat
          </button>
          <button
            className="btn incognito-btn"
            onClick={() => newChat(null, true)}
            disabled={installed.length === 0}
            title="Incognito chat: not saved, no memory"
            aria-label="New incognito chat"
          >
            🕶
          </button>
        </div>
        <nav className="nav">
          <button className={view === "models" ? "active" : ""} onClick={() => setView("models")}>
            Models
            {Object.keys(progress).length > 0 && <span className="dot" aria-label="Installing" />}
          </button>
          <button className={view === "memory" ? "active" : ""} onClick={() => setView("memory")}>
            Memory
          </button>
          <button className={view === "scheduled" ? "active" : ""} onClick={() => setView("scheduled")}>
            Scheduled
          </button>
          <button className={view === "connectors" ? "active" : ""} onClick={() => setView("connectors")}>
            Connectors
          </button>
          <button className={view === "activity" ? "active" : ""} onClick={() => setView("activity")}>
            Activity
          </button>
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}>
            Settings
          </button>
        </nav>
        <div className="side-section">
          <div className="side-head">
            <span>Projects</span>
            <button className="icon-btn" title="New project" aria-label="New project" onClick={() => setNewProject(true)}>+</button>
          </div>
          {projects.map((p) => (
            <button
              key={p.id}
              className={`chat-item ${view === "project" && activeProject === p.id ? "active" : ""}`}
              onClick={() => {
                setActiveProject(p.id);
                setView("project");
              }}
            >
              <span aria-hidden>📚</span>
              <span className="ellipsis">{p.name}</span>
            </button>
          ))}
        </div>
        <div className="chat-list" role="list">
          {chats.length === 0 && <p className="muted small pad">Your chats will appear here.</p>}
          {chats.map((c) => (
            <button
              key={c.id}
              role="listitem"
              className={`chat-item ${view === "chat" && c.id === activeChat ? "active" : ""}`}
              onClick={() => openChat(c.id)}
              title={c.title}
            >
              {c.incognito && <span aria-label="Incognito">🕶</span>}
              <span className="ellipsis">{c.title}</span>
              {c.project_id && <span className="chat-project" title={projects.find((p) => p.id === c.project_id)?.name}>📚</span>}
              {c.web && <span className="globe-mini" title="Web on for this chat">🌐</span>}
              {running.includes(c.id) && <span className="dot" title="Working" aria-label="Working" />}
            </button>
          ))}
        </div>
      </aside>

      <main className="main">
        {view === "chat" && (
          <ChatView
            chat={current}
            projectName={projects.find((p) => p.id === current?.project_id)?.name ?? null}
            onOpenChat={openChat}
            installed={installed}
            defaultModel={settings.default_model}
            connectivity={settings.connectivity}
            onNewChat={() => newChat()}
            onGoModels={() => setView("models")}
            onChanged={refreshChats}
            onDeleted={async () => {
              setActiveChat(null);
              await refreshChats();
            }}
            onToggleWeb={toggleChatWeb}
            toast={toast}
          />
        )}
        {view === "models" && (
          <ModelsView
            catalog={catalog}
            progress={progress}
            defaultModel={settings.default_model}
            onRefresh={refreshCatalog}
            onSettings={setSettings}
            toast={toast}
          />
        )}
        {view === "activity" && <ActivityView toast={toast} />}
        {view === "connectors" && <ConnectorsView toast={toast} />}
        {view === "scheduled" && <ScheduledView installed={installed} onOpenChat={openChat} toast={toast} />}
        {view === "memory" && <MemoryView settings={settings} onSettings={setSettings} toast={toast} />}
        {view === "project" && activeProject && (
          <ProjectView
            key={activeProject}
            projectId={activeProject}
            chats={chats}
            onOpenChat={openChat}
            onNewChat={() => newChat(activeProject)}
            onChanged={refreshProjects}
            onDeleted={async () => {
              setActiveProject(null);
              setView("chat");
              await refreshProjects();
              await refreshChats();
            }}
            toast={toast}
          />
        )}
        {view === "settings" && (
          <SettingsView
            connectivity={settings.connectivity}
            onConnectivity={changeConnectivity}
            security={security}
            onSecurityChanged={onSecurityChanged}
            toast={toast}
          />
        )}
      </main>

      <footer className="statusbar">
        <ConnectivityMenu level={settings.connectivity} onChange={changeConnectivity} />
        <span className="status-item" title="The model currently loaded in memory">
          {loadedName ? (
            <>
              <span className="led on" /> {loadedName} loaded
              <button className="link" onClick={async () => { await api.unload(); refreshEngine(); }}>
                Unload
              </button>
            </>
          ) : (
            <>
              <span className="led" /> No model loaded
            </>
          )}
        </span>
        <span className="spacer" />
        <span className="status-item muted">
          🔐 Encrypted ·{" "}
          {settings.connectivity === "cloud" ? "AI runs on this PC unless you pick a cloud model" : "AI runs only on this PC"}
        </span>
      </footer>
      {newProject && (
        <NewProjectDialog
          onClose={() => setNewProject(false)}
          onCreate={async (name) => {
            try {
              const p = await api.createProject(name);
              setNewProject(false);
              await refreshProjects();
              setActiveProject(p.id);
              setView("project");
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        />
      )}
    </div>
  );
}

function NewProjectDialog({ onClose, onCreate }: { onClose: () => void; onCreate: (name: string) => void }) {
  const [name, setName] = useState("");
  return (
    <Modal title="New project" onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) onCreate(name.trim());
        }}
      >
        <p className="muted small">A project keeps its own instructions, folders and memories for a set of related chats.</p>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Project name, e.g. Thesis" autoFocus maxLength={80} />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn primary" disabled={!name.trim()}>Create</button>
        </div>
      </form>
    </Modal>
  );
}
