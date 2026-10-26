// SPDX-License-Identifier: AGPL-3.0-only
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
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
  type SharedHere,
  type SidebarLayout,
  type Settings,
  type BrowsersView,
  type FolderView,
  type KeepLocks,
} from "./api";
import { APP_NAME } from "./brand";
import { t, tx } from "./i18n";
import { ChatView } from "./views/ChatView";
import { ConnectivityMenu } from "./components/ConnectivityMenu";
import { GpuPill, PerfPill } from "./components/Performance";
import { GpuPicker } from "./components/GpuPicker";
import { SideMenu } from "./components/SideMenu";
import { ChatNotes, LinkedChats } from "./components/ChatPanels";
import { FolderSection, FloatingMenu, NameModal } from "./components/ChatFolders";
import { ShareChatDialog } from "./components/ShareChat";
import { ModelPicker } from "./components/ModelPicker";
import { Modal } from "./components/Modal";
import { LockScreen } from "./components/Security";
import { Toasts, useToasts, type PushToast } from "./components/Toasts";
import { useIdleLock } from "./idle";
import { UpdatePill } from "./components/Updates";
import { SETTINGS_VIEWS, SettingsNav } from "./components/SettingsNav";
import { PrivacyPill } from "./components/PrivacySettings";
import { WebPermission } from "./components/WebPermission";
import { FolderPermission } from "./components/FolderPermission";
import { KeyPermission } from "./components/KeyPermission";
import { MemoryWarningPopup } from "./components/MemoryWarning";
import { ElsewhereApprovals } from "./components/ElsewhereApprovals";
import { CrashReportPrompt, FeedbackButton } from "./components/Feedback";
import { PeopleRequests } from "./components/PeopleRequests";
import { WorkBar } from "./components/WorkBar";
import { IncognitoIcon } from "./components/IncognitoIcon";
import { BookIcon, GlobeIcon, LockIcon, MenuIcon, PcIcon, PeopleIcon, SidebarIcon, ViewerIcon } from "./components/Icons";
import { onPhone } from "./phone/env";
import { SharePermission } from "./components/SharePermission";

// Pages load the first time they are opened, so the app starts faster.
const ModelsView = lazy(() => import("./views/ModelsView").then((m) => ({ default: m.ModelsView })));
const SettingsView = lazy(() => import("./views/SettingsView").then((m) => ({ default: m.SettingsView })));
const ActivityView = lazy(() => import("./views/ActivityView").then((m) => ({ default: m.ActivityView })));
const WebViewerView = lazy(() => import("./views/WebViewerView").then((m) => ({ default: m.WebViewerView })));
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
const PeopleView = lazy(() => import("./views/PeopleView").then((m) => ({ default: m.PeopleView })));
const SharedChatView = lazy(() => import("./views/PeopleView").then((m) => ({ default: m.SharedChatView })));
const Onboarding = lazy(() => import("./views/Onboarding").then((m) => ({ default: m.Onboarding })));
const PcView = lazy(() => import("./views/PcView").then((m) => ({ default: m.PcView })));

export type View = "chat" | "models" | "features" | "studio" | "meetings" | "translate" | "notes" | "tasks" | "mail" | "calendar" | "memory" | "scheduled" | "connectors" | "people" | "shared" | "project" | "activity" | "settings" | "pc" | "web";

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
  people: "people",
  project: "projects",
};

/** Shows the lock screen until unlocked; the workspace mounts only after. */
/** Items in the order the user dragged them into; ones not placed yet
 * (new chats, a page just turned on) come first, in their usual order. */
function arrange<T extends { id: string }>(items: T[], order: string[]): T[] {
  if (order.length === 0) return items;
  const at = new Map(order.map((id, i) => [id, i]));
  const fresh = items.filter((x) => !at.has(x.id));
  const kept = items.filter((x) => at.has(x.id)).sort((a, b) => at.get(a.id)! - at.get(b.id)!);
  return [...fresh, ...kept];
}

/** The ids with `moved` put just before or after `target`. */
function placed(ids: string[], moved: string, target: string, at: "before" | "after"): string[] {
  const rest = ids.filter((id) => id !== moved);
  const i = rest.indexOf(target);
  if (i < 0) return ids;
  rest.splice(at === "before" ? i : i + 1, 0, moved);
  return rest;
}

/** Whether a drag is over the top or bottom half of an item. */
function half(e: { clientY: number; currentTarget: Element }): "before" | "after" {
  const r = e.currentTarget.getBoundingClientRect();
  return e.clientY < r.top + r.height / 2 ? "before" : "after";
}

export default function App() {
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  // Kept out here so unlocking returns to the same page and chat.
  const [view, setView] = useState<View>("chat");
  const [activeChat, setActiveChat] = useState<string | null>(null);
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const toasts = useToasts();
  // Actions that failed with nothing to catch them say so (see main.tsx).
  useEffect(() => {
    const show = (e: Event) => toasts.push(errorText((e as CustomEvent<string>).detail), "error");
    window.addEventListener("sulcus:error", show);
    return () => window.removeEventListener("sulcus:error", show);
  }, [toasts.push]);
  // A file dropped where nothing takes it would open in the window in the
  // app's place: dropping files is only for the places that take them.
  useEffect(() => {
    const keep = (e: DragEvent) => {
      if (e.dataTransfer?.types.includes("Files")) e.preventDefault();
    };
    window.addEventListener("dragover", keep);
    window.addEventListener("drop", keep);
    return () => {
      window.removeEventListener("dragover", keep);
      window.removeEventListener("drop", keep);
    };
  }, []);

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
  if (!security) body = <div className="empty"><p className="muted">{t("Starting…")}</p></div>;
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
  const inSettings = SETTINGS_VIEWS.includes(view);
  useEffect(() => {
    if (!SETTINGS_VIEWS.includes(view)) backTo.current = view;
    if (view !== "settings") setSettingsJump(null);
  }, [view]);
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
  const [sharedMenu, setSharedMenu] = useState<{ s: SharedHere; x: number; y: number } | null>(null);
  const [projectMenu, setProjectMenu] = useState<{ p: Project; x: number; y: number } | null>(null);
  const [renamingProject, setRenamingProject] = useState<Project | null>(null);
  const [deletingProject, setDeletingProject] = useState<Project | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Chat | null>(null);
  // The order the user dragged the menu and the chats into (empty: the usual order).
  const [layout, setLayout] = useState<SidebarLayout>({ nav: [], chats: [] });
  useEffect(() => {
    api.sidebarLayout().then((l) => l && setLayout(l)).catch(() => {});
  }, []);
  const saveLayout = (next: SidebarLayout) => {
    setLayout(next);
    api.setSidebarLayout(next).then(setLayout).catch((e) => toast(errorText(e), "error"));
  };
  // Where a dragged menu item or chat would land: before or after this one.
  const [navOver, setNavOver] = useState<{ id: string; at: "before" | "after" } | null>(null);
  const [chatOver, setChatOver] = useState<{ id: string; at: "before" | "after" } | null>(null);
  const [navMenu, setNavMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const tucked = layout.collapsed ?? [];
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
  // Chats hidden from the list (own ones by id, shared ones as "contact:chat"), shown under "Hidden chats".
  const [hidden, setHidden] = useState<string[]>([]);
  const [showHidden, setShowHidden] = useState(false);
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

  const [loadingNow, setLoadingNow] = useState(false);
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
  /** One chat in the sidebar (main list or a folder); it can be dragged onto a folder. */
  const chatItem = (c: Chat, inFolder: boolean) => (
    <button
      key={c.id}
      role="listitem"
      className={`chat-item ${inFolder ? "in-folder" : ""} ${view === "chat" && c.id === activeChat ? "active" : ""} ${chatOver?.id === c.id ? `drop-${chatOver.at}` : ""}`}
      onClick={() => openChat(c.id)}
      onContextMenu={(e) => {
        e.preventDefault();
        setChatMenu({ id: c.id, x: e.clientX, y: e.clientY });
      }}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("application/x-sulcus-chat", c.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("application/x-sulcus-chat")) return;
        e.preventDefault();
        e.stopPropagation();
        setChatOver({ id: c.id, at: half(e) });
      }}
      onDragLeave={() => setChatOver(null)}
      onDrop={(e) => {
        const moved = e.dataTransfer.getData("application/x-sulcus-chat");
        setChatOver(null);
        if (!moved || moved === c.id) return;
        e.preventDefault();
        e.stopPropagation();
        saveLayout({ ...layout, chats: placed(visible.map((x) => x.id), moved, c.id, half(e)) });
        // Dropped among a folder's chats: into that folder; among the others: out of its folder.
        const into = folders.find((f) => f.chats.includes(c.id)) ?? null;
        const from = folders.find((f) => f.chats.includes(moved)) ?? null;
        if (into?.id !== from?.id) api.moveToFolder(moved, into?.id ?? null).then(setFolders).catch((err) => toast(errorText(err), "error"));
      }}
      title={c.title}
    >
      {c.incognito && <IncognitoIcon title={t("Incognito")} />}
      <span className="ellipsis">{c.title}</span>
      {locks.chats.includes(c.id) && <LockIcon className="chat-kept" title={t("Locked: the automatic cleanup never removes it")} />}
      {c.project_id && <BookIcon className="chat-project" title={projects.find((p) => p.id === c.project_id)?.name} />}
      {c.web && <GlobeIcon className="globe-mini" title={t("Web on for this chat")} />}
      {sharedByMe.has(c.id) && <PeopleIcon className="chat-project" title={t("Shared with {names}", { names: sharedByMe.get(c.id)!.join(", ") })} />}
      {running.includes(c.id) && <span className="dot" title={t("Working")} aria-label={t("Working")} />}
    </button>
  );
  /** A chat someone shares with this user (People): right-click to hide it from the list, or show it again. */
  const sharedItem = (s: SharedHere) => (
    <button
      key={sharedKey(s)}
      role="listitem"
      className={`chat-item shared ${view === "shared" && openShared?.chat.id === s.chat.id && openShared.contact_id === s.contact_id ? "active" : ""} ${s.online ? "" : "offline"}`}
      onClick={() => {
        setOpenShared(s);
        setView("shared");
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        setSharedMenu({ s, x: e.clientX, y: e.clientY });
      }}
      title={s.online ? t("{name} shares this chat with you", { name: s.contact_name }) : t("{name} shares this chat with you (not connected right now)", { name: s.contact_name })}
    >
      <span className="ellipsis">{s.chat.title}</span>
      <span className="shared-from"><PeopleIcon /> {s.contact_name}</span>
      {s.chat.busy && <span className="dot" title={t("Working")} aria-label={t("Working")} />}
    </button>
  );
  const picker = (label: ReactNode) => (
    <ModelPicker
      label={label}
      installed={installed}
      current={settings?.default_model}
      onPick={pickDefault}
      onModels={() => setView("models")}
      auto={settings?.auto_model ?? true}
      onAuto={async (on) => {
        try {
          setSettings(await api.setAutoModel(on));
        } catch (e) {
          toast(errorText(e), "error");
        }
      }}
    />
  );

  return (
    <div className={phone ? `app phone${menuOpen ? " menu-open" : ""}` : `app${sideHidden ? " side-hidden" : ""}`}>
      {phone && menuOpen && <div className="phone-scrim" onClick={() => setMenuOpen(false)} aria-hidden="true" />}
      <aside className="sidebar">
        <div className="brand">
          <img src="logo.svg" alt="" width={26} height={26} />
          <span>{APP_NAME}</span>
          {security.lock_enabled && (
            <button className="icon-btn lock-btn" title={t("Lock now")} aria-label={t("Lock now")} onClick={() => api.lockNow()}>
              <LockIcon />
            </button>
          )}
          {peopleOn && (
            <button
              className={`icon-btn people-btn ${view === "people" ? "active" : ""}`}
              title={t("People (Shift+click opens them in their own window)")}
              aria-label={peopleUnread > 0 ? t("People, {n} unread", { n: peopleUnread }) : t("People")}
              onClick={(e) => {
                if (e.shiftKey && !phone) api.openPeopleWindow().catch((err) => toast(errorText(err), "error"));
                else setView("people");
              }}
            >
              <PeopleIcon />
              {peopleUnread > 0 && <span className="people-unread" aria-hidden="true" />}
            </button>
          )}
          {!phone && (
            <button className="icon-btn side-fold" title={t("Hide the menu (Ctrl+B)")} aria-label={t("Hide the menu")} onClick={() => saveLayout({ ...layout, hidden: true })}>
              <SidebarIcon />
            </button>
          )}
        </div>
        {inSettings ? (
          <SettingsNav
            view={view}
            features={features ?? new Set()}
            section={settingsSection}
            onBack={() => setView(backTo.current)}
            onJump={(id) => {
              setView("settings");
              setSettingsJump({ id, at: Date.now() });
            }}
            onView={setView}
          />
        ) : (<>
        <div className="new-chat-row">
          <button
            className="btn primary block"
            onClick={(e) => (e.shiftKey && !phone ? newChatWindow() : newChat())}
            title={phone ? undefined : t("Shift+click: a new chat in a window of its own")}
          >
            {t("+ New chat")}
          </button>
          <button
            className="btn incognito-btn"
            onClick={() => newChat(null, true)}
            disabled={installed.length === 0}
            title={t("Incognito chat: not saved, no memory")}
            aria-label={t("New incognito chat")}
          >
            <IncognitoIcon size="1.3em" />
          </button>
        </div>
        <nav className="nav" aria-label={t("Main")}>
          {(() => {
          const all = arrange(
            [
              { id: "pc", show: phone, label: <><PcIcon /> {t("PC")}</> },
              { id: "studio", show: creates, label: t("Studio") },
              { id: "meetings", show: has("meetings") || liveMeeting, label: <>{t("Meetings")}{liveMeeting && <span className="dot rec" aria-label={t("Recording")} />}</> },
              { id: "notes", show: has("notes"), label: t("Notes") },
              { id: "tasks", show: has("tasks"), label: t("Tasks") },
              { id: "mail", show: has("email"), label: t("Mail") },
              { id: "calendar", show: has("calendar"), label: t("Calendar") },
              { id: "translate", show: has("translate"), label: t("Translate") },
              { id: "memory", show: has("memory"), label: t("Memory") },
              { id: "scheduled", show: has("scheduled"), label: t("Scheduled") },
            ].filter((n) => n.show),
            layout.nav,
          );
          const navButton = (n: (typeof all)[number], i: number, shown: typeof all) => (
            <button
              key={n.id}
              className={`${view === n.id ? "active" : ""} ${navOver?.id === n.id ? `drop-${navOver.at}` : ""}`}
              onClick={(e) => (e.shiftKey && n.id === "studio" && !phone ? studioWindow() : setView(n.id as View))}
              draggable
              title={n.id === "studio" && !phone ? t("Shift+click: the Studio in a window of its own. Drag to put the menu in the order you like.") : t("Drag to put the menu in the order you like (or Alt+Up / Alt+Down)")}
              onDragStart={(e) => {
                e.dataTransfer.setData("application/x-sulcus-nav", n.id);
                e.dataTransfer.effectAllowed = "move";
              }}
              onDragOver={(e) => {
                if (!e.dataTransfer.types.includes("application/x-sulcus-nav")) return;
                e.preventDefault();
                setNavOver({ id: n.id, at: half(e) });
              }}
              onDragLeave={() => setNavOver(null)}
              onDrop={(e) => {
                const moved = e.dataTransfer.getData("application/x-sulcus-nav");
                setNavOver(null);
                if (moved) saveLayout({ ...layout, nav: placed(shown.map((x) => x.id), moved, n.id, half(e)) });
              }}
              onKeyDown={(e) => {
                if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
                e.preventDefault();
                const to = shown[i + (e.key === "ArrowUp" ? -1 : 1)];
                if (to) saveLayout({ ...layout, nav: placed(shown.map((x) => x.id), n.id, to.id, e.key === "ArrowUp" ? "before" : "after") });
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                setNavMenu({ id: n.id, x: e.clientX, y: e.clientY });
              }}
            >
              {n.label}
            </button>
          );
          const main = all.filter((n) => !tucked.includes(n.id));
          const folded = all.filter((n) => tucked.includes(n.id));
          // The page open now shows even when its group is folded.
          const open = !!layout.collapse_open || folded.some((n) => n.id === view);
          return (
            <>
              {main.map((n, i) => navButton(n, i, main))}
              {folded.length > 0 && (
                <>
                  <button className="nav-fold small muted" aria-expanded={open} onClick={() => saveLayout({ ...layout, collapse_open: !layout.collapse_open })}>
                    {open ? "▾" : "▸"} {t("Collapse ({n})", { n: folded.length })}
                  </button>
                  {open && <div className="nav-folded">{folded.map((n, i) => navButton(n, i, folded))}</div>}
                </>
              )}
            </>
          );
          })()}
        </nav>
        {has("projects") && <div className="side-section">
          <div className="side-head">
            <span>{t("Projects")}</span>
            <button className="icon-btn" title={t("New project")} aria-label={t("New project")} onClick={() => setNewProject(true)}>+</button>
          </div>
          {projects.map((p) => (
            <button
              key={p.id}
              className={`chat-item ${view === "project" && activeProject === p.id ? "active" : ""}`}
              onClick={() => {
                setActiveProject(p.id);
                setView("project");
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                setProjectMenu({ p, x: e.clientX, y: e.clientY });
              }}
            >
              <BookIcon />
              <span className="ellipsis">{p.name}</span>
            </button>
          ))}
        </div>}
        {(folders.length > 0 || visible.length > 0) && (
          <FolderSection folders={folders} chats={visible} renderChat={chatItem} onChanged={(f) => (setFolders(f), refreshChats())} peopleOn={peopleOn} toast={toast} />
        )}
        {/* Shared: chats people share with this user, and this user's shared chats. */}
        {(sharedVisible.length > 0 || visible.some((c) => sharedByMe.has(c.id) && !foldered.has(c.id))) && (
          <div className="side-section">
            <div className="side-head">
              <span>{t("Shared")}</span>
            </div>
            <div className="chat-list" role="list" aria-label={t("Shared")}>
              {sharedVisible.map((s) => sharedItem(s))}
              {visible.filter((c) => sharedByMe.has(c.id) && !foldered.has(c.id)).map((c) => chatItem(c, false))}
            </div>
          </div>
        )}
        <div className="chat-list" role="list">
          {listed.length === 0 && sharedHere.length === 0 && <p className="muted small pad">{t("Your chats will appear here.")}</p>}
          {visible.filter((c) => !foldered.has(c.id) && !sharedByMe.has(c.id)).map((c) => chatItem(c, false))}
          {hiddenOwn.length + hiddenShared.length > 0 && (
            <button className="hidden-toggle small muted" onClick={() => setShowHidden((v) => !v)} aria-expanded={showHidden}>
              {showHidden ? "▾" : "▸"} {t("Hidden chats ({n})", { n: hiddenOwn.length + hiddenShared.length })}
            </button>
          )}
          {showHidden && (
            <div className="hidden-chats" role="list" aria-label={t("Hidden chats")}>
              {hiddenShared.map((s) => sharedItem(s))}
              {hiddenOwn.map((c) => chatItem(c, false))}
              <p className="muted small pad">{t("Right-click a chat to show it in the list again.")}</p>
            </div>
          )}
        </div>
        </>)}
        {/* Connectors and People, when on, sit in the gear menu: the top is about the work. */}
        <SideMenu
          view={view}
          installing={Object.keys(progress).length > 0}
          extra={[
            ...(has("connectors") ? [{ id: "connectors" as const, label: tx("Connectors") }] : []),
            ...(has("people") ? [{ id: "people" as const, label: tx("People"), unread: peopleUnread }] : []),
          ]}
          onOpen={setView}
        />
      </aside>
      {chatMenu && (
        <FloatingMenu x={chatMenu.x} y={chatMenu.y} onClose={() => setChatMenu(null)}>
          {(() => {
            const id = chatMenu.id;
            const kept = locks.chats.includes(id);
            const current = folders.find((f) => f.chats.includes(id));
            const done = () => setChatMenu(null);
            const move = (folderId: string | null) => {
              done();
              api.moveToFolder(id, folderId).then(setFolders).catch((e) => toast(errorText(e), "error"));
            };
            return (
              <>
                <span className="menu-label">{t("Move to folder")}</span>
                {folders.filter((f) => f.id !== current?.id).map((f) => (
                  <button key={f.id} role="menuitem" onClick={() => move(f.id)}>
                    {f.name}
                  </button>
                ))}
                <button role="menuitem" onClick={() => (setFolderFor(id), done())}>
                  ＋ {t("New folder…")}
                </button>
                {current && (
                  <button role="menuitem" onClick={() => move(null)}>
                    ↩ {t("Out of “{name}”", { name: current.name })}
                  </button>
                )}
                <hr />
                {peopleOn && !chats.find((c) => c.id === id)?.incognito && (
                  <button role="menuitem" onClick={() => (done(), setSharing(id))}>
                    {sharedByMe.has(id) ? t("Shared with {names}…", { names: sharedByMe.get(id)!.join(", ") }) : t("Share…")}
                  </button>
                )}
                {!chats.find((c) => c.id === id)?.incognito && (
                  <>
                    {!phone && (
                      <button role="menuitem" onClick={() => (done(), chatWindow(id))}>
                        {t("Open in a new window")}
                      </button>
                    )}
                    <button role="menuitem" onClick={() => (done(), setNotesFor(id))}>
                      {t("Notes…")}
                    </button>
                    <button role="menuitem" onClick={() => (done(), setConnecting(id))}>
                      {t("Connect session…")}
                    </button>
                  </>
                )}
                {layout.chats.length > 0 && (
                  <button role="menuitem" onClick={() => (done(), saveLayout({ ...layout, chats: [] }))}>
                    {t("Sort chats by newest again")}
                  </button>
                )}
                <button
                  role="menuitem"
                  onClick={() => {
                    done();
                    hideChat(id, !hidden.includes(id));
                  }}
                >
                  {hidden.includes(id) ? t("Show in the list again") : t("Hide from the list")}
                </button>
                <button
                  role="menuitem"
                  onClick={() => {
                    done();
                    api.setKept("chat", id, !kept).then(setLocks).catch((e) => toast(errorText(e), "error"));
                  }}
                >
                  {kept ? t("Unlock (the cleanup may remove it)") : t("Lock (the cleanup never removes it)")}
                </button>
                <button
                  role="menuitem"
                  className="danger"
                  onClick={() => {
                    setConfirmDelete(chats.find((c) => c.id === id) ?? null);
                    done();
                  }}
                >
                  {t("Delete chat")}
                </button>
              </>
            );
          })()}
        </FloatingMenu>
      )}
      {navMenu && (
        <FloatingMenu x={navMenu.x} y={navMenu.y} onClose={() => setNavMenu(null)}>
          {navMenu.id === "studio" && !phone && (
            <button role="menuitem" onClick={() => (setNavMenu(null), studioWindow())}>
              {t("Open in a new window")}
            </button>
          )}
          {tucked.includes(navMenu.id) ? (
            <button role="menuitem" onClick={() => (setNavMenu(null), saveLayout({ ...layout, collapsed: tucked.filter((x) => x !== navMenu.id) }))}>
              {t("Take out of Collapse")}
            </button>
          ) : (
            <button role="menuitem" onClick={() => (setNavMenu(null), saveLayout({ ...layout, collapsed: [...tucked, navMenu.id] }))}>
              {t("Add to Collapse")}
            </button>
          )}
          {layout.nav.length > 0 && (
            <button role="menuitem" onClick={() => (setNavMenu(null), saveLayout({ ...layout, nav: [] }))}>
              {t("Put the menu back in its usual order")}
            </button>
          )}
        </FloatingMenu>
      )}
      {notesFor && chats.some((c) => c.id === notesFor) && (
        <ChatNotes chat={chats.find((c) => c.id === notesFor)!} onClose={() => setNotesFor(null)} toast={toast} />
      )}
      {connecting && chats.some((c) => c.id === connecting) && (
        <LinkedChats chat={chats.find((c) => c.id === connecting)!} onOpenChat={openChat} onClose={() => setConnecting(null)} toast={toast} />
      )}
      {projectMenu && (
        <FloatingMenu x={projectMenu.x} y={projectMenu.y} onClose={() => setProjectMenu(null)}>
          <button
            role="menuitem"
            onClick={() => {
              setActiveProject(projectMenu.p.id);
              setView("project");
              setProjectMenu(null);
            }}
          >
            {t("Open")}
          </button>
          <button
            role="menuitem"
            onClick={() => {
              newChat(projectMenu.p.id);
              setProjectMenu(null);
            }}
          >
            ＋ {t("New chat in this project")}
          </button>
          <button role="menuitem" onClick={() => (setRenamingProject(projectMenu.p), setProjectMenu(null))}>
            {t("Rename…")}
          </button>
          <hr />
          <button role="menuitem" className="danger" onClick={() => (setDeletingProject(projectMenu.p), setProjectMenu(null))}>
            {t("Delete project…")}
          </button>
        </FloatingMenu>
      )}
      {renamingProject && (
        <NameModal
          title={t("Rename project")}
          initial={renamingProject.name}
          label={t("Project name")}
          maxLength={80}
          onClose={() => setRenamingProject(null)}
          onSave={async (name) => {
            const p = renamingProject;
            setRenamingProject(null);
            try {
              await api.updateProject(p.id, name, p.instructions);
              await refreshProjects();
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        />
      )}
      {deletingProject && (
        <Modal title={t("Delete {name}?", { name: deletingProject.name })} onClose={() => setDeletingProject(null)}>
          <p>{t("This deletes the project's instructions and memories. Its chats are kept and move out of the project. Files in its folders aren't touched.")}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setDeletingProject(null)}>
              {t("Cancel")}
            </button>
            <button
              className="btn danger-fill"
              autoFocus
              onClick={async () => {
                const p = deletingProject;
                setDeletingProject(null);
                try {
                  await api.deleteProject(p.id);
                  if (activeProject === p.id) {
                    setActiveProject(null);
                    setView("chat");
                  }
                  await refreshProjects();
                  await refreshChats();
                  toast(t("Deleted {name}.", { name: p.name }), "success");
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Delete project")}
            </button>
          </div>
        </Modal>
      )}
      {sharedMenu && (
        <FloatingMenu x={sharedMenu.x} y={sharedMenu.y} onClose={() => setSharedMenu(null)}>
          {(() => {
            const s = sharedMenu.s;
            const key = sharedKey(s);
            const done = () => setSharedMenu(null);
            return (
              <>
                <span className="menu-label">{t("Shared by {name}", { name: s.contact_name })}</span>
                <button
                  role="menuitem"
                  onClick={() => {
                    done();
                    setOpenShared(s);
                    setView("shared");
                  }}
                >
                  {t("Open")}
                </button>
                <button
                  role="menuitem"
                  onClick={() => {
                    done();
                    hideChat(key, !hidden.includes(key));
                  }}
                >
                  {hidden.includes(key) ? t("Show in the list again") : t("Hide from the list")}
                </button>
              </>
            );
          })()}
        </FloatingMenu>
      )}
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
      {confirmDelete && (
        <Modal title={t("Delete this chat?")} onClose={() => setConfirmDelete(null)}>
          <p>
            {t("“{title}” and its messages will be deleted from this PC. This can't be undone.", { title: confirmDelete.title })}
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(null)}>{t("Cancel")}</button>
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
      )}

      <main className="main">
        {phone && (
          <div className="phone-bar">
            <button className="icon-btn" aria-label={t("Menu")} title={t("Menu")} onClick={() => setMenuOpen(true)}>
              <MenuIcon size="1.4em" />
            </button>
            <span className="phone-title ellipsis">{view === "chat" && current ? current.title : APP_NAME}</span>
            <button className={`icon-btn${view === "pc" ? " active" : ""}`} aria-label={t("How the PC is doing")} title={t("How the PC is doing")} onClick={() => setView("pc")}>
              <PcIcon size="1.4em" />
            </button>
          </div>
        )}
        {!phone && (
          <TopBar
            sideHidden={sideHidden}
            onShowSide={() => saveLayout({ ...layout, hidden: false })}
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
            toast={toast}
          />
        )}
        <Suspense fallback={<PageLoading />}>
        {view === "pc" && <PcView toast={toast} />}
        {view === "web" && !phone && <WebViewerView start={viewerStart} toast={toast} onClose={() => setView("chat")} />}
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
              // It leaves its folder (and the cleanup locks) too.
              refreshFolders();
            }}
            onToggleWeb={toggleChatWeb}
            features={features ?? new Set()}
            onGoFeatures={() => setView("features")}
            browser={!phone && current ? browserChats[current.id] ?? null : null}
            onBrowser={(open) => current && (open ? openChatBrowser(current.id, null) : shutChatBrowser(current.id))}
            onPopOut={!phone && current ? () => chatWindow(current.id) : undefined}
            toast={toast}
          />
        )}
        {view === "features" && <FeaturesView progress={progress} toast={toast} />}
        {view === "studio" && <StudioView features={features ?? new Set()} progress={progress} toast={toast} onGoFeatures={() => setView("features")} onPopOut={phone ? undefined : studioWindow} />}
        {view === "notes" && <NotesView toast={toast} />}
        {view === "tasks" && <TasksView onOpenChat={openChat} toast={toast} />}
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
        {view === "people" && <PeopleView toast={toast} />}
        {view === "shared" && openShared && (
          <div className="page shared-page">
            <SharedChatView key={`${openShared.contact_id}:${openShared.chat.id}`} contactId={openShared.contact_id} contactName={openShared.contact_name} chat={openShared.chat} toast={toast} />
          </div>
        )}
        {view === "scheduled" && <ScheduledView installed={installed} onOpenChat={openChat} toast={toast} />}
        {view === "memory" && <MemoryView settings={settings} onSettings={setSettings} toast={toast} />}
        {view === "project" && activeProject && (
          <ProjectView
            key={activeProject}
            projectId={activeProject}
            listed={projects.find((p) => p.id === activeProject)}
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
            jump={settingsJump}
            onSection={setSettingsSection}
          />
        )}
        </Suspense>
      </main>

      <footer className="statusbar">
        <ConnectivityMenu level={settings.connectivity} onChange={changeConnectivity} onRefresh={() => api.settings().then(setSettings).catch(() => {})} />
        <span className="status-item" title={loadedName ? t("The model in memory") : t("Models load when you send a message and unload after a while idle, to free memory")}>
          {engineDl ? (
            <>
              <span className="led busy" /> {t("Getting the AI engine (first time only)")}{engineDl.total ? ` · ${Math.round((engineDl.received / engineDl.total) * 100)}%` : "…"}
            </>
          ) : loadedName ? (
            <>
              <span className="led on" /> {picker(t("{name} loaded", { name: loadedName }))}
              <button className="link" onClick={async () => { await api.unload(); refreshEngine(); }}>
                {t("Unload")}
              </button>
            </>
          ) : defaultName ? (
            <>
              <span className="led" /> {loadingNow ? t("Loading {name}…", { name: defaultName }) : picker(t("{name} · loads when you chat", { name: defaultName }))}
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
                  {t("Load now")}
                </button>
              )}
            </>
          ) : (
            <>
              <span className="led" /> {t("No model installed")}
            </>
          )}
        </span>
        <GpuPicker engine={engine} onEngineChanged={refreshEngine} toast={toast} />
        {liveMeeting && (
          <button className="status-item rec-pill" onClick={() => setView("meetings")} title={t("A meeting is being recorded")}>
            <span className="rec-dot on" /> {t("Recording meeting")}
          </button>
        )}
        <PerfPill onOpen={() => setView("settings")} />
        <GpuPill onOpen={() => setView("settings")} />
        <WorkBar onOpen={(page) => setView(page)} />
        <UpdatePill toast={toast} />
        <PrivacyPill onOpen={() => setView("settings")} />
        <span className="spacer" />
        <span className="status-item muted">
          {t("Encrypted ·")}{" "}
          {settings.connectivity === "cloud" ? t("AI runs on this PC unless you pick a cloud model") : t("AI runs only on this PC")}
        </span>
      </footer>
      <WebPermission chats={chats} onOpenChat={openChat} />
      <FolderPermission chats={chats} onOpenChat={openChat} />
      <KeyPermission chats={chats} onOpenChat={openChat} />
      <SharePermission chats={chats} onOpenChat={openChat} />
      <MemoryWarningPopup />
      <ElsewhereApprovals chats={chats} openChatId={view === "chat" ? activeChat : null} onOpenChat={openChat} />
      <CrashReportPrompt toast={toast} />
      <PeopleRequests enabled={has("people")} onOpenPeople={() => setView("people")} toast={toast} />
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
    <Modal title={t("New project")} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) onCreate(name.trim());
        }}
      >
        <p className="muted small">{t("A project keeps its own instructions, folders and memories for a set of related chats.")}</p>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Project name, e.g. Thesis")} aria-label={t("Project name, e.g. Thesis")} autoFocus maxLength={80} />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!name.trim()}>{t("Create")}</button>
        </div>
      </form>
    </Modal>
  );
}


/** Across the top of the page: the web viewer's button, and the globe,
 * which opens the main browser (Chrome unless another was picked). */
function TopBar({ sideHidden, onShowSide, viewerOpen, onViewer, toast }: { sideHidden: boolean; onShowSide: () => void; viewerOpen: boolean; onViewer: () => void; toast: PushToast }) {
  const [view, setView] = useState<BrowsersView | null>(null);
  useEffect(() => {
    const load = () => api.browsersView().then(setView).catch(() => {});
    load();
    window.addEventListener("browser-changed", load);
    return () => window.removeEventListener("browser-changed", load);
  }, []);
  const name = view?.current.kind === "app" ? view.current.name : view?.current.kind === "system" ? t("your default browser") : t("the web viewer");
  return (
    <div className="topbar">
      {sideHidden && (
        <button className="icon-btn side-show" title={t("Show the menu (Ctrl+B)")} aria-label={t("Show the menu")} onClick={onShowSide}>
          <SidebarIcon />
        </button>
      )}
      <FeedbackButton toast={toast} />
      <button className={`icon-btn web-btn${viewerOpen ? " active" : ""}`} title={t("Open the web viewer")} aria-label={t("Open the web viewer")} onClick={onViewer}>
        <ViewerIcon />
      </button>
      {view && (
        <button className="icon-btn web-btn" title={t("Open {name}", { name })} aria-label={t("Open {name}", { name })} onClick={() => api.openWeb(null).catch((e) => toast(errorText(e), "error"))}>
          <GlobeIcon />
        </button>
      )}
    </div>
  );
}

function listNames(names: string[]): string {
  return names.length <= 1 ? names.join("") : t("{list} and {last}", { list: names.slice(0, -1).join(", "), last: names[names.length - 1] });
}

function PageLoading() {
  return <div className="page"><p className="muted">{t("Loading…")}</p></div>;
}
