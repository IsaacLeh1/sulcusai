// SPDX-License-Identifier: AGPL-3.0-only
import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, errorText, on, type CatalogView, type Chat, type Connectivity, type FeatureId, type EngineStatus, type InstallProgress, type Project, type SecurityStatus, type SharedHere, type SidebarLayout, type Settings, type FolderView, type KeepLocks } from "./api";
import { t } from "./i18n";
import { everyWhileVisible } from "./visible";
import { ChatNotes, LinkedChats } from "./components/ChatPanels";
import { SkillFromChat } from "./components/SkillFromChat";
import { NameModal } from "./components/ChatFolders";
import { ShareChatDialog, ShareChatsDialog } from "./components/ShareChat";
import { type PushToast } from "./components/Toasts";
import { SETTINGS_VIEWS } from "./components/SettingsNav";
import { ChatMenu } from "./components/ChatMenu";
import { UnprotectDialog } from "./components/ProtectedChat";
import { onPhone } from "./phone/env";
import type { View } from "./App";
import { Onboarding, VIEW_FEATURE } from "./pages";
import { NewProjectDialog, listNames, PageLoading } from "./components/WorkspaceBits";
import { arrange } from "./workspace/order";
import { Sidebar } from "./workspace/Sidebar";
import { NavMenu, ProjectMenu, SharedMenu } from "./workspace/SidebarMenus";
import { RenameProjectDialog, DeleteProjectDialog, DeleteChatsDialog, DeleteChatDialog } from "./workspace/Dialogs";
import { MainPane } from "./workspace/MainPane";
import { StatusBar } from "./workspace/StatusBar";
import { Popups } from "./workspace/Popups";

interface Nav {
  view: View;
  setView: (v: View) => void;
  activeChat: string | null;
  setActiveChat: (id: string | null) => void;
  activeProject: string | null;
  setActiveProject: (id: string | null) => void;
}

export function Workspace({ security, onSecurityChanged, toast, nav }: { security: SecurityStatus; onSecurityChanged: () => void; toast: PushToast; nav: Nav }) {
  const { view, setView, activeChat, setActiveChat, activeProject, setActiveProject } = nav;
  // Where the web viewer opens: a link sent to it, or as it was (url null).
  const [viewerStart, setViewerStart] = useState<{ url: string | null; at: number }>({ url: null, at: 0 });
  // Chats whose own browser is open beside them, and where each starts.
  const [browserChats, setBrowserChats] = useState<Record<string, { url: string | null; at: number }>>({});
  const openChatBrowser = (chatId: string, url: string | null) => setBrowserChats((all) => ({ ...all, [chatId]: { url, at: Date.now() } }));
  const shutChatBrowser = (chatId: string) =>
    setBrowserChats((all) => {
      const { [chatId]: _, ...rest } = all;
      return rest;
    });
  const viewRef = useRef<string>("chat");
  // On a paired phone: the sidebar slides in as a menu, closed again on the way somewhere.
  const phone = onPhone();
  const [menuOpen, setMenuOpen] = useState(false);
  useEffect(() => setMenuOpen(false), [view, activeChat, activeProject]);
  // Settings' sidebar: where Back returns to, a quick link to scroll to, and
  // the section being read.
  const backTo = useRef<View>("chat");
  const [settingsJump, setSettingsJump] = useState<{ id: string; at: number } | null>(null);
  const [settingsSection, setSettingsSection] = useState<string | null>(null);
  useEffect(() => {
    if (!SETTINGS_VIEWS.includes(view)) backTo.current = view;
    if (view !== "settings") setSettingsJump(null);
  }, [view]);
  // A chat opened from elsewhere in the page (What left this PC).
  useEffect(() => {
    const go = (e: Event) => {
      setActiveChat((e as CustomEvent<string>).detail);
      setView("chat");
    };
    window.addEventListener("open-chat", go);
    return () => window.removeEventListener("open-chat", go);
  }, [setActiveChat, setView]);
  // A link elsewhere opening a page ("compute").
  useEffect(() => {
    const go = (e: Event) => setView((e as CustomEvent<View>).detail);
    window.addEventListener("open-view", go);
    return () => window.removeEventListener("open-view", go);
  }, [setView]);
  // Whether a compute PC is linked (its page shows in the menu).
  const [computeOn, setComputeOn] = useState(false);
  useEffect(() => {
    const check = () => api.computeSettings().then((c) => setComputeOn(!!c.settings.peer)).catch(() => {});
    check();
    window.addEventListener("compute-changed", check);
    return () => window.removeEventListener("compute-changed", check);
  }, []);
  // A link elsewhere (the connectivity menu) opening a Settings section.
  useEffect(() => {
    const go = (e: Event) => {
      setView("settings");
      setSettingsJump({ id: (e as CustomEvent<string>).detail, at: Date.now() });
    };
    window.addEventListener("open-settings", go);
    return () => window.removeEventListener("open-settings", go);
  }, [setView]);
  const [chats, setChats] = useState<Chat[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [newProject, setNewProject] = useState(false);
  const refreshProjects = useCallback(async () => setProjects(await api.projects()), []);
  useEffect(() => {
    refreshProjects();
  }, [refreshProjects]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [catalog, setCatalog] = useState<CatalogView | null>(null);
  // Read by the app-wide event handlers, so they stay subscribed when the catalog changes.
  const catalogRef = useRef(catalog);
  catalogRef.current = catalog;
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  // While a model is loaded: working or idle, and when it unloads by itself,
  // so nothing sits in memory unnoticed.
  const [modelNow, setModelNow] = useState<{ busy: boolean; idle_secs: number; unloads_in: number | null } | null>(null);
  const modelLoaded = !!engine?.model_id;
  useEffect(() => {
    if (!modelLoaded) {
      setModelNow(null);
      return;
    }
    const load = () => !document.hidden && api.modelIdle().then(setModelNow).catch(() => {});
    load();
    return everyWhileVisible(load, 15000);
  }, [modelLoaded]);
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
  // Unread messages from people, for the badge on People.
  const [peopleUnread, setPeopleUnread] = useState(0);
  const peopleOn = has("people");
  // Chats others share with this user: in the sidebar with this user's own.
  const [sharedHere, setSharedHere] = useState<SharedHere[]>([]);
  const [openShared, setOpenShared] = useState<SharedHere | null>(null);
  useEffect(() => {
    if (!peopleOn) return setSharedHere([]);
    const load = () => api.peopleSharedChats().then(setSharedHere).catch(() => {});
    load();
    const subs = [on("people:shared", load), on("people:changed", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [peopleOn]);
  // This user's chats shared with people, and with whom: they move to the Shared section.
  const [sharedByMe, setSharedByMe] = useState<Map<string, string[]>>(new Map());
  const [sharing, setSharing] = useState<string | null>(null);
  useEffect(() => {
    if (!peopleOn) return setSharedByMe(new Map());
    const load = () =>
      api
        .peopleView()
        .then((v) => {
          const m = new Map<string, string[]>();
          for (const c of v.contacts) for (const s of c.sharing) m.set(s.chat_id, [...(m.get(s.chat_id) ?? []), c.name]);
          setSharedByMe(m);
        })
        .catch(() => {});
    load();
    const subs = [on("people:changed", load), on("people:shared", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [peopleOn]);
  useEffect(() => {
    if (!peopleOn) return setPeopleUnread(0);
    const count = () => api.peopleUnread().then(setPeopleUnread).catch(() => {});
    count();
    const subs = [on("people:message", count), on("people:changed", count)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [peopleOn]);
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
  viewRef.current = view;

  const refreshChats = useCallback(async () => setChats(await api.chats()), []);
  // Chat folders and the chats and models locked against the automatic cleanup.
  const [folders, setFolders] = useState<FolderView[]>([]);
  const [locks, setLocks] = useState<KeepLocks>({ chats: [], models: [] });
  const refreshFolders = useCallback(() => {
    api.foldersView().then(setFolders).catch(() => {});
    api.keepLocks().then(setLocks).catch(() => {});
  }, []);
  const refreshCatalog = useCallback(async () => setCatalog(await api.catalog()), []);
  const refreshEngine = useCallback(async () => setEngine(await api.engineStatus()), []);

  useEffect(() => {
    refreshChats();
    refreshFolders();
    refreshCatalog();
    refreshEngine();
    api.settings().then(setSettings);
  }, [refreshChats, refreshFolders, refreshCatalog, refreshEngine]);

  // Install progress outlives page switches, so it is tracked here.
  useEffect(() => {
    const subs = [
      on("install:progress", (p) => setProgress((all) => ({ ...all, [p.model_id]: p }))),
      on("install:finished", (f) => {
        setProgress(({ [f.model_id]: _, ...rest }) => rest);
        refreshCatalog();
        refreshEngine();
        api.settings().then(setSettings);
        const name = f.name ?? catalogRef.current?.models.find((m) => m.id === f.model_id)?.name ?? t("The model");
        if (f.ok) toast(t("{name} is installed and ready.", { name }), "success");
        else if (f.paused) toast(t("Paused {name}. Resume it under Downloads on the Models page.", { name }), "info");
        else if (!f.cancelled) toast(t("{name} didn't install: {error}", { name, error: errorText(f.error) }), "error");
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
      // The assistant deleted a chat (the user approved it).
      on("chat:deleted", (p) => {
        refreshChats();
        if (activeChatRef.current === p.chat_id) setActiveChat(null);
      }),
      // The assistant asked for the web and got a yes: the Web button shows it.
      on("chat:web", () => refreshChats()),
      // The assistant showing the user a page (open_page).
      on("app:navigate", (p) => setView(p.page as View)),
      // A link opened while the main browser is the web viewer.
      // In a chat: its own browser, beside it; elsewhere the Web page.
      on("viewer:open", (p) => {
        const chat = activeChatRef.current;
        if (viewRef.current === "chat" && chat) return openChatBrowser(chat, p.url);
        setViewerStart({ url: p.url, at: Date.now() });
        setView("web");
      }),
      // A chat's assistant opened a page: its browser shows beside the chat.
      on("viewer:dock", (p) => {
        if (p.key === "main") return;
        setBrowserChats((all) => (all[p.key] ? all : { ...all, [p.key]: { url: null, at: Date.now() } }));
      }),
      on("sync:changed", () => {
        refreshChats();
        refreshProjects();
      }),
      on("models:found", (p) => {
        refreshCatalog();
        api.settings().then(setSettings);
        toast(
          p.names.length === 1
            ? t("Found {names} on this PC. It's ready to use, no download needed.", { names: listNames(p.names) })
            : t("Found {names} on this PC. They're ready to use, no download needed.", { names: listNames(p.names) }),
          "success",
        );
      }),
      on("task:reminder", (p) => toast(t("Reminder: {title}", { title: p.title }), "success")),
      // Picture, video and song jobs end here whatever page is open: a
      // failure on another page used to vanish without a word.
      on("media:done", (d) => {
        if (d.error && !d.cancelled) toast(t("A picture, video or song didn't finish: {error}", { error: errorText(d.error) }), "error");
        const missing = (d.items ?? []).flatMap((i) => i.missing ?? []);
        if (d.ok && missing.length) toast(t("Done, but the check didn't find everything you asked for: {missing}. Open it in the Studio and change it, saying what to add.", { missing: missing.join("; ") }), "info");
      }),
      on("schedule:ran", (p) => {
        refreshChats();
        if (p.error) toast(t("Scheduled task “{name}” didn't run: {error}", { name: p.name, error: errorText(p.error) }), "error");
        else toast(t("Scheduled task “{name}” finished. Its result is in the sidebar.", { name: p.name }), "success");
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
      on("quick:hotkey_taken", (p) => toast(t("Another app already uses {hotkey}, so Quick ask can't use it.", { hotkey: p.hotkey }), "error")),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [refreshCatalog, refreshChats, refreshEngine, refreshProjects, setActiveChat, setView, toast]);

  const [started, setStarted] = useState<Set<string>>(new Set());
  const [chatMenu, setChatMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const [unprotecting, setUnprotecting] = useState<string | null>(null);
  const protectChat = (id: string) =>
    api
      .protectChat(id)
      .then(() => {
        toast(t("Protected. It opens with your PIN from now on."), "success");
        refreshChats();
      })
      .catch((e) => toast(errorText(e), "error"));
  const [sharedMenu, setSharedMenu] = useState<{ s: SharedHere; x: number; y: number } | null>(null);
  const [projectMenu, setProjectMenu] = useState<{ p: Project; x: number; y: number } | null>(null);
  const [renamingProject, setRenamingProject] = useState<Project | null>(null);
  const [deletingProject, setDeletingProject] = useState<Project | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Chat | null>(null);
  // Chats picked together (Ctrl+click, or Select in a chat's menu), and what's done with them.
  const [selected, setSelected] = useState<string[]>([]);
  const [deletingMany, setDeletingMany] = useState(false);
  const [sharingMany, setSharingMany] = useState(false);
  useEffect(() => {
    if (selected.length === 0) return;
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setSelected([]);
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [selected.length]);
  // The order the user dragged the menu and the chats into (empty: the usual order).
  const [layout, setLayout] = useState<SidebarLayout>({ nav: [], chats: [] });
  useEffect(() => {
    api.sidebarLayout().then((l) => l && setLayout(l)).catch(() => {});
  }, []);
  const saveLayout = (next: SidebarLayout) => {
    setLayout(next);
    api.setSidebarLayout(next).then(setLayout).catch((e) => toast(errorText(e), "error"));
  };
  const [navMenu, setNavMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const sideHidden = !!layout.hidden;
  // Ctrl+B folds the sidebar away or brings it back (not while typing).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.shiftKey || e.altKey || e.key.toLowerCase() !== "b") return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName))) return;
      e.preventDefault();
      setLayout((l) => {
        const next = { ...l, hidden: !l.hidden };
        api.setSidebarLayout(next).catch(() => {});
        return next;
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  // Connecting this chat with another, so they can ask and tell each other things.
  const [connecting, setConnecting] = useState<string | null>(null);
  // A chat's notes, opened from its menu.
  const [notesFor, setNotesFor] = useState<string | null>(null);
  const [skillFor, setSkillFor] = useState<string | null>(null);
  // Chats hidden from the list (own ones by id, shared ones as "contact:chat"), shown under "Hidden chats".
  const [hidden, setHidden] = useState<string[]>([]);
  const [showHidden, setShowHidden] = useState(false);
  const [chatQuery, setChatQuery] = useState("");
  useEffect(() => {
    api.hiddenChats().then(setHidden).catch(() => {});
  }, []);
  // New chats stay out of the list until something is sent in them.
  const listed = chats.filter((c) => !c.empty || started.has(c.id) || running.includes(c.id));
  const sharedKey = (s: SharedHere) => `${s.contact_id}:${s.chat.id}`;
  const visible = arrange(listed.filter((c) => !hidden.includes(c.id)), layout.chats);
  const hiddenOwn = listed.filter((c) => hidden.includes(c.id));
  const sharedVisible = sharedHere.filter((s) => !hidden.includes(sharedKey(s)));
  const hiddenShared = sharedHere.filter((s) => hidden.includes(sharedKey(s)));
  const hideChat = (key: string, hide: boolean) =>
    api
      .setChatHidden(key, hide)
      .then((h) => {
        setHidden(h);
        toast(hide ? t("Hidden from the list. It's under Hidden chats at the bottom.") : t("Back in the list."), "success");
      })
      .catch((e) => toast(errorText(e), "error"));
  const foldered = new Set(folders.flatMap((f) => f.chats));
  const [folderFor, setFolderFor] = useState<string | null>(null);
  const installed = useMemo(() => (catalog?.models ?? []).filter((m) => m.installed), [catalog]);
  const current = chats.find((c) => c.id === activeChat) ?? null;
  // A protected chat shuts again once the window leaves it.
  const openProtected = useRef<string | null>(null);
  useEffect(() => {
    const here = view === "chat" && current?.protected && !current.shut ? current.id : null;
    const was = openProtected.current;
    openProtected.current = here;
    if (was && was !== here) api.shutProtectedChat(was).then(refreshChats).catch(() => {});
  }, [view, current?.id, current?.protected, current?.shut, refreshChats]);

  // The wake word: a new chat, talking.
  const [voiceFor, setVoiceFor] = useState<string | null>(null);
  useEffect(() => {
    const sub = on("wake:heard", async () => {
      try {
        const chat = await api.createChatIn(null, false);
        await refreshChats();
        setActiveChat(chat.id);
        setView("chat");
        setVoiceFor(chat.id);
      } catch (e) {
        toast(errorText(e), "error");
      }
    });
    return () => {
      sub.then((un) => un());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
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
  // Chats and the Studio in windows of their own (not on a phone, where there's one screen).
  const chatWindow = (id: string) => api.openChatWindow(id).catch((e) => toast(errorText(e), "error"));
  const studioWindow = () => api.openStudioWindow().catch((e) => toast(errorText(e), "error"));
  /** A new chat, in a window of its own: a model session beside this one. */
  const newChatWindow = async () => {
    if (installed.length === 0) return newChat();
    try {
      const chat = await api.createChatIn(null, false);
      await refreshChats();
      chatWindow(chat.id);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const changeConnectivity = async (level: Connectivity) => {
    setSettings(await api.setConnectivity(level));
  };

  const toggleChatWeb = useCallback(async () => {
    if (!current) return;
    try {
      await api.setChatWeb(current.id, !current.web);
      await refreshChats();
    } catch (e) {
      toast(errorText(e), "error");
    }
  }, [current, refreshChats, toast]);

  // Ctrl+Shift+W toggles web for the open chat, as its Web button does: only
  // on the chat page, and only Offline (other levels have web on for every chat).
  const webKey = view === "chat" && !!current && settings?.connectivity === "offline";
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "w") {
        e.preventDefault();
        if (webKey) toggleChatWeb();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleChatWeb, webKey]);

  // The engine itself, downloaded with the first chat when models came from another app.
  const [engineDl, setEngineDl] = useState<{ received: number; total: number } | null>(null);
  useEffect(() => {
    const sub = on("engine:download", (p) => setEngineDl(p.done ? null : { received: p.received ?? 0, total: p.total ?? 0 }));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!settings) return <div className="empty"><p className="muted">{t("Loading…")}</p></div>;

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
  // The model used before, still loaded beside it (Settings › Performance).
  const keptName = engine?.kept ? (catalog?.models.find((m) => m.id === engine.kept)?.name ?? engine.kept) : null;
  const defaultName = catalog?.models.find((m) => m.installed && m.id === settings?.default_model)?.name ?? installed[0]?.name;
  // Picking a model in the status bar makes it the default for new chats.
  const pickDefault = async (id: string) => {
    try {
      setSettings(await api.setDefaultModel(id));
      const name = installed.find((m) => m.id === id)?.name ?? id;
      toast(t("{name} is now the default for new chats.", { name }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className={phone ? `app phone${menuOpen ? " menu-open" : ""}` : `app${sideHidden ? " side-hidden" : ""}`}>
      {phone && menuOpen && <div className="phone-scrim" onClick={() => setMenuOpen(false)} aria-hidden="true" />}
      <Sidebar
        security={security}
        phone={phone}
        view={view}
        setView={setView}
        setActiveChat={setActiveChat}
        activeChat={activeChat}
        activeProject={activeProject}
        setActiveProject={setActiveProject}
        openChat={openChat}
        refreshChats={refreshChats}
        has={has}
        features={features}
        creates={creates}
        computeOn={computeOn}
        liveMeeting={liveMeeting}
        peopleOn={peopleOn}
        peopleUnread={peopleUnread}
        installing={Object.keys(progress).length > 0}
        layout={layout}
        saveLayout={saveLayout}
        studioWindow={studioWindow}
        setNavMenu={setNavMenu}
        settingsSection={settingsSection}
        onSettingsBack={() => setView(backTo.current)}
        onSettingsJump={(id) => {
          setView("settings");
          setSettingsJump({ id, at: Date.now() });
        }}
        installedCount={installed.length}
        newChat={newChat}
        newChatWindow={newChatWindow}
        projects={projects}
        onNewProject={() => setNewProject(true)}
        setProjectMenu={setProjectMenu}
        chats={chats}
        listed={listed}
        visible={visible}
        hiddenOwn={hiddenOwn}
        hiddenShared={hiddenShared}
        sharedVisible={sharedVisible}
        sharedHere={sharedHere}
        sharedByMe={sharedByMe}
        sharedKey={sharedKey}
        openShared={openShared}
        setOpenShared={setOpenShared}
        folders={folders}
        setFolders={setFolders}
        foldered={foldered}
        locks={locks}
        setLocks={setLocks}
        running={running}
        selected={selected}
        setSelected={setSelected}
        chatQuery={chatQuery}
        setChatQuery={setChatQuery}
        showHidden={showHidden}
        setShowHidden={setShowHidden}
        setChatMenu={setChatMenu}
        setSharedMenu={setSharedMenu}
        setFolderFor={setFolderFor}
        setSharingMany={setSharingMany}
        setDeletingMany={setDeletingMany}
        hideChat={hideChat}
        toast={toast}
      />
      {chatMenu && (
        <ChatMenu
          menu={chatMenu}
          onClose={() => setChatMenu(null)}
          chats={chats}
          folders={folders}
          setFolders={setFolders}
          locks={locks}
          setLocks={setLocks}
          layout={layout}
          saveLayout={saveLayout}
          hidden={hidden}
          hideChat={hideChat}
          sharedByMe={sharedByMe}
          peopleOn={peopleOn}
          phone={phone}
          setSelected={setSelected}
          setFolderFor={setFolderFor}
          setSharing={setSharing}
          chatWindow={chatWindow}
          setNotesFor={setNotesFor}
          setSkillFor={setSkillFor}
          setConnecting={setConnecting}
          setConfirmDelete={setConfirmDelete}
          onProtect={protectChat}
          onUnprotect={setUnprotecting}
          toast={toast}
        />
      )}
      {navMenu && <NavMenu menu={navMenu} onClose={() => setNavMenu(null)} phone={phone} layout={layout} saveLayout={saveLayout} studioWindow={studioWindow} />}
      {skillFor && <SkillFromChat chatId={skillFor} onClose={() => setSkillFor(null)} toast={toast} />}
      {notesFor && chats.some((c) => c.id === notesFor) && (
        <ChatNotes chat={chats.find((c) => c.id === notesFor)!} onClose={() => setNotesFor(null)} toast={toast} />
      )}
      {connecting && chats.some((c) => c.id === connecting) && (
        <LinkedChats chat={chats.find((c) => c.id === connecting)!} onOpenChat={openChat} onClose={() => setConnecting(null)} toast={toast} />
      )}
      {projectMenu && (
        <ProjectMenu
          menu={projectMenu}
          onClose={() => setProjectMenu(null)}
          setActiveProject={setActiveProject}
          setView={setView}
          newChat={(id) => newChat(id)}
          onRename={setRenamingProject}
          onDelete={setDeletingProject}
        />
      )}
      {renamingProject && <RenameProjectDialog project={renamingProject} onClose={() => setRenamingProject(null)} refreshProjects={refreshProjects} toast={toast} />}
      {deletingProject && (
        <DeleteProjectDialog
          project={deletingProject}
          onClose={() => setDeletingProject(null)}
          activeProject={activeProject}
          setActiveProject={setActiveProject}
          setView={setView}
          refreshProjects={refreshProjects}
          refreshChats={refreshChats}
          toast={toast}
        />
      )}
      {sharedMenu && <SharedMenu menu={sharedMenu} onClose={() => setSharedMenu(null)} sharedKey={sharedKey} hidden={hidden} hideChat={hideChat} openShared={setOpenShared} setView={setView} />}
      {sharing && <ShareChatDialog chatId={sharing} onClose={() => setSharing(null)} toast={toast} />}
      {folderFor && (
        <NameModal
          title={t("New folder")}
          initial=""
          onClose={() => setFolderFor(null)}
          onSave={(name) => {
            const id = folderFor;
            setFolderFor(null);
            api.createFolder(name, id).then(setFolders).catch((e) => toast(errorText(e), "error"));
          }}
        />
      )}
      {deletingMany && (
        <DeleteChatsDialog
          ids={selected}
          chats={chats}
          onClose={() => setDeletingMany(false)}
          onDone={() => setSelected([])}
          activeChat={activeChat}
          setActiveChat={setActiveChat}
          refreshChats={refreshChats}
          refreshFolders={refreshFolders}
          toast={toast}
        />
      )}
      {sharingMany && <ShareChatsDialog chatIds={selected} onClose={() => setSharingMany(false)} toast={toast} />}
      {unprotecting && (
        <UnprotectDialog
          chatId={unprotecting}
          security={security}
          onClose={() => setUnprotecting(null)}
          onDone={() => {
            setUnprotecting(null);
            toast(t("The PIN protection is off."), "success");
            refreshChats();
          }}
        />
      )}
      {confirmDelete && (
        <DeleteChatDialog
          chat={confirmDelete}
          onClose={() => setConfirmDelete(null)}
          activeChat={activeChat}
          setActiveChat={setActiveChat}
          refreshChats={refreshChats}
          refreshFolders={refreshFolders}
          toast={toast}
        />
      )}

      <MainPane
        view={view}
        setView={setView}
        phone={phone}
        toast={toast}
        security={security}
        onSecurityChanged={onSecurityChanged}
        settings={settings}
        setSettings={setSettings}
        features={features}
        catalog={catalog}
        refreshCatalog={refreshCatalog}
        progress={progress}
        installed={installed}
        setMenuOpen={setMenuOpen}
        sideHidden={sideHidden}
        showSide={() => saveLayout({ ...layout, hidden: false })}
        viewerOpen={view === "web" || (view === "chat" && !!activeChat && !!browserChats[activeChat])}
        onViewer={() => {
          // In a chat: its own browser beside it (shown or hidden).
          if (view === "chat" && activeChat) {
            if (browserChats[activeChat]) shutChatBrowser(activeChat);
            else openChatBrowser(activeChat, null);
            return;
          }
          setViewerStart({ url: null, at: Date.now() });
          setView("web");
        }}
        viewerStart={viewerStart}
        chats={chats}
        current={current}
        activeChat={activeChat}
        openChat={openChat}
        newChat={newChat}
        refreshChats={refreshChats}
        onChatDeleted={async () => {
          setActiveChat(null);
          await refreshChats();
          // It leaves its folder (and the cleanup locks) too.
          refreshFolders();
        }}
        toggleChatWeb={toggleChatWeb}
        browser={!phone && current ? browserChats[current.id] ?? null : null}
        onChatBrowser={(open) => current && (open ? openChatBrowser(current.id, null) : shutChatBrowser(current.id))}
        onPopOut={!phone && current ? () => chatWindow(current.id) : undefined}
        startVoice={!!current && voiceFor === current.id}
        studioWindow={studioWindow}
        projects={projects}
        activeProject={activeProject}
        refreshProjects={refreshProjects}
        onProjectDeleted={async () => {
          setActiveProject(null);
          setView("chat");
          await refreshProjects();
          await refreshChats();
        }}
        liveMeeting={liveMeeting}
        setLiveMeeting={setLiveMeeting}
        openShared={openShared}
        settingsJump={settingsJump}
        onSettingsSection={setSettingsSection}
        changeConnectivity={changeConnectivity}
      />

      <StatusBar
        settings={settings}
        setSettings={setSettings}
        engine={engine}
        installed={installed}
        loadedName={loadedName}
        keptName={keptName}
        defaultName={defaultName}
        modelNow={modelNow}
        engineDl={engineDl}
        liveMeeting={liveMeeting}
        onEngineChanged={refreshEngine}
        onChangeConnectivity={changeConnectivity}
        onPickDefault={pickDefault}
        setView={setView}
        toast={toast}
      />
      <Popups chats={chats} openChatId={view === "chat" ? activeChat : null} onOpenChat={openChat} peopleOn={has("people")} onOpenPeople={() => setView("people")} toast={toast} />
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
