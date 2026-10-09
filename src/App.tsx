// SPDX-License-Identifier: AGPL-3.0-only
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  errorText,
  on,
  type CatalogView,
  type Chat,
  type Connectivity,
  type FeatureId,
  type EngineStatus,
  type InstallProgress,
  type Project,
  type SecurityStatus,
  type Settings,
  type BrowsersView,
} from "./api";
import { APP_NAME } from "./brand";
import { ChatView } from "./views/ChatView";
import { ConnectivityMenu } from "./components/ConnectivityMenu";
import { PerfPill } from "./components/Performance";
import { SideMenu } from "./components/SideMenu";
import { Modal } from "./components/Modal";
import { LockScreen } from "./components/Security";
import { Toasts, useToasts, type PushToast } from "./components/Toasts";
import { useIdleLock } from "./idle";
import { UpdatePill } from "./components/Updates";

// Pages load the first time they are opened, so the app starts faster.
const ModelsView = lazy(() => import("./views/ModelsView").then((m) => ({ default: m.ModelsView })));
const SettingsView = lazy(() => import("./views/SettingsView").then((m) => ({ default: m.SettingsView })));
const ActivityView = lazy(() => import("./views/ActivityView").then((m) => ({ default: m.ActivityView })));
const MemoryView = lazy(() => import("./views/MemoryView").then((m) => ({ default: m.MemoryView })));
const ProjectView = lazy(() => import("./views/ProjectView").then((m) => ({ default: m.ProjectView })));
const ScheduledView = lazy(() => import("./views/ScheduledView").then((m) => ({ default: m.ScheduledView })));
const ConnectorsView = lazy(() => import("./views/ConnectorsView").then((m) => ({ default: m.ConnectorsView })));
const MeetingsView = lazy(() => import("./views/MeetingsView").then((m) => ({ default: m.MeetingsView })));
const TranslateView = lazy(() => import("./views/TranslateView").then((m) => ({ default: m.TranslateView })));
const FeaturesView = lazy(() => import("./views/FeaturesView").then((m) => ({ default: m.FeaturesView })));
const NotesView = lazy(() => import("./views/NotesView").then((m) => ({ default: m.NotesView })));
const TasksView = lazy(() => import("./views/TasksView").then((m) => ({ default: m.TasksView })));
const MailView = lazy(() => import("./views/MailView").then((m) => ({ default: m.MailView })));
const CalendarView = lazy(() => import("./views/CalendarView").then((m) => ({ default: m.CalendarView })));
const StudioView = lazy(() => import("./views/StudioView").then((m) => ({ default: m.StudioView })));
const Onboarding = lazy(() => import("./views/Onboarding").then((m) => ({ default: m.Onboarding })));

export type View = "chat" | "models" | "features" | "studio" | "meetings" | "translate" | "notes" | "tasks" | "mail" | "calendar" | "memory" | "scheduled" | "connectors" | "project" | "activity" | "settings";

/** Pages that belong to a feature, hidden while it's off. */
const VIEW_FEATURE: Partial<Record<View, FeatureId>> = {
  meetings: "meetings",
  translate: "translate",
  notes: "notes",
  tasks: "tasks",
  mail: "email",
  calendar: "calendar",
  memory: "memory",
  scheduled: "scheduled",
  connectors: "connectors",
  project: "projects",
};

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
  const [features, setFeatures] = useState<Set<FeatureId> | null>(null);
  const refreshFeatures = useCallback(async () => {
    const list = await api.features();
    setFeatures(new Set(list.filter((f) => f.enabled).map((f) => f.id)));
  }, []);
  useEffect(() => {
    refreshFeatures();
    const subs = [on("features:changed", () => refreshFeatures()), on("install:finished", () => refreshFeatures())];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [refreshFeatures]);
  const has = (f: FeatureId) => features?.has(f) ?? false;
  const creates = has("images") || has("video") || has("music");
  // Leave a page whose feature was just turned off.
  useEffect(() => {
    const f = VIEW_FEATURE[view];
    if (features && f && !features.has(f)) setView("chat");
    if (features && view === "studio" && !creates) setView("chat");
  }, [features, view, setView, creates]);
  // The meeting being recorded, tracked here so it survives page changes.
  const [liveMeeting, setLiveMeeting] = useState<string | null>(null);
  useEffect(() => {
    api.liveMeeting().then(setLiveMeeting).catch(() => {});
    const sub = on("meeting", (e) => {
      if (e.kind === "done" || e.kind === "error") setLiveMeeting((id) => (id === e.meeting_id ? null : id));
    });
    return () => {
      sub.then((un) => un());
    };
  }, []);
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
        const name = f.name ?? catalog?.models.find((m) => m.id === f.model_id)?.name ?? "The model";
        if (f.ok) toast(`${name} is installed and ready.`, "success");
        else if (!f.cancelled) toast(`${name} didn't install: ${f.error}`, "error");
      }),
      on("chat:done", (p) => {
        setRunning((r) => r.filter((id) => id !== p.chat_id));
        refreshChats();
        refreshEngine();
      }),
      on("chat:start", (p) => {
        setRunning((r) => (r.includes(p.chat_id) ? r : [...r, p.chat_id]));
        refreshChats();
      }),
      // A new chat joins the sidebar as soon as its first message is sent.
      on("chat:status", (p) => setStarted((s) => (s.has(p.chat_id) ? s : new Set(s).add(p.chat_id)))),
      on("chat:titled", () => refreshChats()),
      on("models:found", (p) => {
        refreshCatalog();
        api.settings().then(setSettings);
        toast(`Found ${listNames(p.names)} on this PC. ${p.names.length === 1 ? "It's" : "They're"} ready to use, no download needed.`, "success");
      }),
      on("task:reminder", (p) => toast(`⏰ ${p.title}`, "success")),
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
      on("engine:unloaded", () => refreshEngine()),
      on("quick:open-chat", async (p) => {
        await refreshChats();
        setActiveChat(p.chat_id);
        setView("chat");
      }),
      on("quick:hotkey_taken", (p) => toast(`Another app already uses ${p.hotkey}, so Quick ask can't use it.`, "error")),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [catalog, refreshCatalog, refreshChats, refreshEngine, toast]);

  const [started, setStarted] = useState<Set<string>>(new Set());
  const [chatMenu, setChatMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Chat | null>(null);
  // New chats stay out of the list until something is sent in them.
  const listed = chats.filter((c) => !c.empty || started.has(c.id) || running.includes(c.id));
  const installed = useMemo(() => (catalog?.models ?? []).filter((m) => m.installed), [catalog]);
  const current = chats.find((c) => c.id === activeChat) ?? null;

  const newChat = async (projectId: string | null = null, incognito = false) => {
    // No model yet: the home screen, which says how to get one.
    if (installed.length === 0) {
      setActiveChat(null);
      setView("chat");
      return;
    }
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

  const [loadingNow, setLoadingNow] = useState(false);
  // The engine itself, downloaded with the first chat when models came from another app.
  const [engineDl, setEngineDl] = useState<{ received: number; total: number } | null>(null);
  useEffect(() => {
    const sub = on("engine:download", (p) => setEngineDl(p.done ? null : { received: p.received ?? 0, total: p.total ?? 0 }));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!settings) return <div className="empty"><p className="muted">Loading…</p></div>;

  if (!settings.onboarded) {
    return (
      <Suspense fallback={<PageLoading />}>
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
      </Suspense>
    );
  }

  const loadedName = catalog?.models.find((m) => m.id === engine?.model_id)?.name;
  const defaultName = catalog?.models.find((m) => m.installed && m.id === settings?.default_model)?.name ?? installed[0]?.name;

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
          <button className="btn primary block" onClick={() => newChat()}>
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
        <nav className="nav" aria-label="Main">
          {creates && (
            <button className={view === "studio" ? "active" : ""} onClick={() => setView("studio")}>
              Studio
            </button>
          )}
          {(has("meetings") || liveMeeting) && (
            <button className={view === "meetings" ? "active" : ""} onClick={() => setView("meetings")}>
              Meetings
              {liveMeeting && <span className="dot rec" aria-label="Recording" />}
            </button>
          )}
          {has("notes") && (
            <button className={view === "notes" ? "active" : ""} onClick={() => setView("notes")}>
              Notes
            </button>
          )}
          {has("tasks") && (
            <button className={view === "tasks" ? "active" : ""} onClick={() => setView("tasks")}>
              Tasks
            </button>
          )}
          {has("email") && (
            <button className={view === "mail" ? "active" : ""} onClick={() => setView("mail")}>
              Mail
            </button>
          )}
          {has("calendar") && (
            <button className={view === "calendar" ? "active" : ""} onClick={() => setView("calendar")}>
              Calendar
            </button>
          )}
          {has("translate") && (
            <button className={view === "translate" ? "active" : ""} onClick={() => setView("translate")}>
              Translate
            </button>
          )}
          {has("memory") && (
            <button className={view === "memory" ? "active" : ""} onClick={() => setView("memory")}>
              Memory
            </button>
          )}
          {has("scheduled") && (
            <button className={view === "scheduled" ? "active" : ""} onClick={() => setView("scheduled")}>
              Scheduled
            </button>
          )}
          {has("connectors") && (
            <button className={view === "connectors" ? "active" : ""} onClick={() => setView("connectors")}>
              Connectors
            </button>
          )}
        </nav>
        {has("projects") && <div className="side-section">
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
        </div>}
        <div className="chat-list" role="list">
          {listed.length === 0 && <p className="muted small pad">Your chats will appear here.</p>}
          {listed.map((c) => (
            <button
              key={c.id}
              role="listitem"
              className={`chat-item ${view === "chat" && c.id === activeChat ? "active" : ""}`}
              onClick={() => openChat(c.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                setChatMenu({ id: c.id, x: e.clientX, y: e.clientY });
              }}
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
        <SideMenu view={view} installing={Object.keys(progress).length > 0} onOpen={setView} />
      </aside>
      {chatMenu && (
        <ChatMenu
          x={chatMenu.x}
          y={chatMenu.y}
          onClose={() => setChatMenu(null)}
          onDelete={() => {
            setConfirmDelete(chats.find((c) => c.id === chatMenu.id) ?? null);
            setChatMenu(null);
          }}
        />
      )}
      {confirmDelete && (
        <Modal title="Delete this chat?" onClose={() => setConfirmDelete(null)}>
          <p>
            “{confirmDelete.title}” and its messages will be deleted from this PC. This can't be undone.
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(null)}>Cancel</button>
            <button
              className="btn danger-fill"
              autoFocus
              onClick={async () => {
                const id = confirmDelete.id;
                setConfirmDelete(null);
                try {
                  await api.deleteChat(id);
                  if (activeChat === id) setActiveChat(null);
                  await refreshChats();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Delete
            </button>
          </div>
        </Modal>
      )}

      <main className="main">
        <TopBar browserOn={has("browser")} toast={toast} />
        <Suspense fallback={<PageLoading />}>
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
            features={features ?? new Set()}
            onGoFeatures={() => setView("features")}
            toast={toast}
          />
        )}
        {view === "features" && <FeaturesView progress={progress} toast={toast} />}
        {view === "studio" && <StudioView features={features ?? new Set()} progress={progress} toast={toast} onGoFeatures={() => setView("features")} />}
        {view === "notes" && <NotesView toast={toast} />}
        {view === "tasks" && <TasksView toast={toast} />}
        {view === "mail" && <MailView toast={toast} />}
        {view === "calendar" && <CalendarView toast={toast} />}
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
        {view === "meetings" && (
          <MeetingsView
            live={liveMeeting}
            onLiveChanged={setLiveMeeting}
            onOpenChat={async (id) => {
              await refreshChats();
              openChat(id);
            }}
            onGoModels={() => setView("models")}
            toast={toast}
          />
        )}
        {view === "translate" && <TranslateView hasModel={installed.length > 0} onGoModels={() => setView("models")} toast={toast} />}
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
            features={features ?? new Set()}
            toast={toast}
          />
        )}
        </Suspense>
      </main>

      <footer className="statusbar">
        <ConnectivityMenu level={settings.connectivity} onChange={changeConnectivity} />
        <span className="status-item" title={loadedName ? "The model in memory" : "Models load when you send a message and unload after a while idle, to free memory"}>
          {engineDl ? (
            <>
              <span className="led busy" /> Getting the AI engine (first time only){engineDl.total ? ` · ${Math.round((engineDl.received / engineDl.total) * 100)}%` : "…"}
            </>
          ) : loadedName ? (
            <>
              <span className="led on" /> {loadedName} loaded
              <button className="link" onClick={async () => { await api.unload(); refreshEngine(); }}>
                Unload
              </button>
            </>
          ) : defaultName ? (
            <>
              <span className="led" /> {loadingNow ? `Loading ${defaultName}…` : `${defaultName} · loads when you chat`}
              {!loadingNow && (
                <button
                  className="link"
                  onClick={async () => {
                    setLoadingNow(true);
                    try {
                      await api.loadDefault();
                    } catch (e) {
                      toast(errorText(e), "error");
                    } finally {
                      setLoadingNow(false);
                      refreshEngine();
                    }
                  }}
                >
                  Load now
                </button>
              )}
            </>
          ) : (
            <>
              <span className="led" /> No model installed
            </>
          )}
        </span>
        {liveMeeting && (
          <button className="status-item rec-pill" onClick={() => setView("meetings")} title="A meeting is being recorded">
            <span className="rec-dot on" /> Recording meeting
          </button>
        )}
        <PerfPill onOpen={() => setView("settings")} />
        <UpdatePill toast={toast} />
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

/** Right-click menu on a chat in the sidebar. */
function ChatMenu({ x, y, onClose, onDelete }: { x: number; y: number; onClose: () => void; onDelete: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);
  return (
    <div ref={ref} className="context-menu" role="menu" style={{ left: Math.min(x, window.innerWidth - 180), top: Math.min(y, window.innerHeight - 60) }}>
      <button role="menuitem" className="danger" onClick={onDelete} autoFocus>
        🗑 Delete chat
      </button>
    </div>
  );
}

/** Across the top of the page: the web button, which opens the main browser. */
function TopBar({ browserOn, toast }: { browserOn: boolean; toast: PushToast }) {
  const [view, setView] = useState<BrowsersView | null>(null);
  useEffect(() => {
    const load = () => api.browsersView().then(setView).catch(() => {});
    load();
    window.addEventListener("browser-changed", load);
    return () => window.removeEventListener("browser-changed", load);
  }, []);
  if (!view) return null;
  const builtin = view.current.kind === "builtin";
  // The built-in browser is a feature; another browser can always be opened.
  if (builtin && !browserOn) return null;
  const name = view.current.kind === "app" ? view.current.name : view.current.kind === "system" ? "your default browser" : "the SulcusAI browser";
  return (
    <div className="topbar">
      <button className="icon-btn web-btn" title={`Open ${name}`} aria-label={`Open ${name}`} onClick={() => api.openWeb(null).catch((e) => toast(errorText(e), "error"))}>
        🌐
      </button>
    </div>
  );
}

function listNames(names: string[]): string {
  return names.length <= 1 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

function PageLoading() {
  return <div className="page"><p className="muted">Loading…</p></div>;
}
