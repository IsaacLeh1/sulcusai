// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  api,
  errorText,
  on,
  type CatalogView,
  type Chat,
  type Connectivity,
  type EngineStatus,
  type InstallProgress,
  type SecurityStatus,
  type Settings,
} from "./api";
import { APP_NAME } from "./brand";
import { ChatView } from "./views/ChatView";
import { ModelsView } from "./views/ModelsView";
import { SettingsView } from "./views/SettingsView";
import { ActivityView } from "./views/ActivityView";
import { Onboarding } from "./views/Onboarding";
import { ConnectivityMenu } from "./components/ConnectivityMenu";
import { LockScreen } from "./components/Security";
import { Toasts, useToasts, type PushToast } from "./components/Toasts";
import { useIdleLock } from "./idle";

export type View = "chat" | "models" | "activity" | "settings";

/** Shows the lock screen until unlocked; the workspace mounts only after. */
export default function App() {
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  // Kept out here so unlocking returns to the same page and chat.
  const [view, setView] = useState<View>("chat");
  const [activeChat, setActiveChat] = useState<string | null>(null);
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
        nav={{ view, setView, activeChat, setActiveChat }}
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
}

function Workspace({ security, onSecurityChanged, toast, nav }: { security: SecurityStatus; onSecurityChanged: () => void; toast: PushToast; nav: Nav }) {
  const { view, setView, activeChat, setActiveChat } = nav;
  const [chats, setChats] = useState<Chat[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [catalog, setCatalog] = useState<CatalogView | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [progress, setProgress] = useState<Record<string, InstallProgress>>({});

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
      on("chat:done", () => {
        refreshChats();
        refreshEngine();
      }),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [catalog, refreshCatalog, refreshChats, refreshEngine, toast]);

  const installed = useMemo(() => (catalog?.models ?? []).filter((m) => m.installed), [catalog]);
  const current = chats.find((c) => c.id === activeChat) ?? null;

  const newChat = async () => {
    try {
      const chat = await api.createChat();
      await refreshChats();
      setActiveChat(chat.id);
      setView("chat");
    } catch (e) {
      toast(errorText(e), "error");
    }
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
        <button className="btn primary block" onClick={newChat} disabled={installed.length === 0}>
          + New chat
        </button>
        <nav className="nav">
          <button className={view === "models" ? "active" : ""} onClick={() => setView("models")}>
            Models
            {Object.keys(progress).length > 0 && <span className="dot" aria-label="Installing" />}
          </button>
          <button className={view === "activity" ? "active" : ""} onClick={() => setView("activity")}>
            Activity
          </button>
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}>
            Settings
          </button>
        </nav>
        <div className="chat-list" role="list">
          {chats.length === 0 && <p className="muted small pad">Your chats will appear here.</p>}
          {chats.map((c) => (
            <button
              key={c.id}
              role="listitem"
              className={`chat-item ${view === "chat" && c.id === activeChat ? "active" : ""}`}
              onClick={() => {
                setActiveChat(c.id);
                setView("chat");
              }}
              title={c.title}
            >
              <span className="ellipsis">{c.title}</span>
              {c.web && <span className="globe-mini" title="Web on for this chat">🌐</span>}
            </button>
          ))}
        </div>
      </aside>

      <main className="main">
        {view === "chat" && (
          <ChatView
            chat={current}
            installed={installed}
            defaultModel={settings.default_model}
            connectivity={settings.connectivity}
            onNewChat={newChat}
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
    </div>
  );
}
