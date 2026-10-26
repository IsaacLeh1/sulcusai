// SPDX-License-Identifier: AGPL-3.0-only
// A chat, the Studio, or a song studio project in a window of its own
// (main.tsx picks this by the window's label: "chat-<id>", "studio-<n>",
// "songstudio-<id>"). A chat window keeps its own model, so different model
// sessions sit side by side; what happens in one (a reply, a new picture,
// a project saved) shows in every window. While SulcusAI is locked it shows
// only that; unlocking happens in the main window.
import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorText, on, type CatalogView, type Chat, type FeatureId, type InstallProgress, type MediaItem, type Project, type SecurityStatus, type Settings } from "../api";
import { APP_NAME } from "../brand";
import { FolderPermission } from "../components/FolderPermission";
import { KeyPermission } from "../components/KeyPermission";
import { SharePermission } from "../components/SharePermission";
import { SongStudio } from "../components/SongStudio";
import { Toasts, useToasts, type PushToast } from "../components/Toasts";
import { WebPermission } from "../components/WebPermission";
import { WorkBar } from "../components/WorkBar";
import { t } from "../i18n";
import type { Project as StudioProject } from "../studio/model";
import { ChatView } from "./ChatView";
import { StudioView } from "./StudioView";

const close = () => getCurrentWindow().close().catch(() => {});
const title = (what: string) => getCurrentWindow().setTitle(`${what} - ${APP_NAME}`).catch(() => {});

/** The features turned on, kept current. */
function useFeatures() {
  const [features, setFeatures] = useState<Set<FeatureId>>(new Set());
  useEffect(() => {
    const load = () => api.features().then((l) => setFeatures(new Set(l.filter((f) => f.enabled).map((f) => f.id)))).catch(() => {});
    load();
    const subs = [on("features:changed", load), on("install:finished", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, []);
  return features;
}

/** Models being downloaded (started here or in another window). */
function useInstallProgress() {
  const [progress, setProgress] = useState<Record<string, InstallProgress>>({});
  useEffect(() => {
    const subs = [
      on("install:progress", (p) => setProgress((all) => ({ ...all, [p.model_id]: p }))),
      on("install:finished", (f) => setProgress(({ [f.model_id]: _, ...rest }) => rest)),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, []);
  return progress;
}

function ChatWindow({ first, toast }: { first: string; toast: PushToast }) {
  // The chat shown (it can move on: a chat continued in a new one, or another opened from this one).
  const [chatId, setChatId] = useState(first);
  const [chats, setChats] = useState<Chat[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [catalog, setCatalog] = useState<CatalogView | null>(null);
  const [browser, setBrowser] = useState<{ url: string | null; at: number } | null>(null);
  const features = useFeatures();
  const refreshChats = useCallback(() => api.chats().then(setChats).catch(() => {}), []);
  const refreshModels = useCallback(() => {
    api.catalog().then(setCatalog).catch(() => {});
    api.settings().then(setSettings).catch(() => {});
  }, []);
  useEffect(() => {
    refreshChats();
    refreshModels();
    api.projects().then(setProjects).catch(() => {});
  }, [refreshChats, refreshModels]);
  useEffect(() => {
    const subs = [
      on("chat:done", refreshChats),
      on("chat:start", refreshChats),
      on("chat:titled", refreshChats),
      on("chat:web", refreshChats),
      on("sync:changed", refreshChats),
      on("install:finished", refreshModels),
      on("models:found", refreshModels),
      // Deleted (here, in another window, or by the assistant): this window goes too.
      on("chat:deleted", (p) => p.chat_id === chatId && close()),
      // A long chat continued in a new one: followed here.
      on("chat:handoff", (p) => {
        if (p.from !== chatId) return;
        refreshChats();
        setChatId(p.to);
      }),
      // Its assistant opened a page: its browser shows beside it, here.
      on("viewer:dock", (p) => p.key === chatId && setBrowser((b) => b ?? { url: null, at: Date.now() })),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [chatId, refreshChats, refreshModels]);
  const installed = useMemo(() => (catalog?.models ?? []).filter((m) => m.installed), [catalog]);
  const chat = chats.find((c) => c.id === chatId) ?? null;
  useEffect(() => {
    title(chat?.title || t("New chat"));
  }, [chat?.title]);
  // The connectivity level changes in the main window's status bar: looked at again when this one is used.
  useEffect(() => {
    const again = () => api.settings().then(setSettings).catch(() => {});
    window.addEventListener("focus", again);
    return () => window.removeEventListener("focus", again);
  }, []);
  if (!settings || !catalog) return null;
  const goMain = (page: string) => api.showMainPage(page).catch((e) => toast(errorText(e), "error"));
  return (
    <div className="side-window chat-window">
      <ChatView
        chat={chat}
        projectName={projects.find((p) => p.id === chat?.project_id)?.name ?? null}
        onOpenChat={setChatId}
        installed={installed}
        defaultModel={settings.default_model}
        connectivity={settings.connectivity}
        onNewChat={async () => {
          try {
            const c = await api.createChatIn(null, false);
            await refreshChats();
            setChatId(c.id);
          } catch (e) {
            toast(errorText(e), "error");
          }
        }}
        onGoModels={() => goMain("models")}
        onChanged={refreshChats}
        onDeleted={close}
        onToggleWeb={async () => {
          if (!chat) return;
          try {
            await api.setChatWeb(chat.id, !chat.web);
            await refreshChats();
          } catch (e) {
            toast(errorText(e), "error");
          }
        }}
        features={features}
        onGoFeatures={() => goMain("features")}
        browser={browser}
        onBrowser={(open) => setBrowser(open ? { url: null, at: Date.now() } : null)}
        toast={toast}
      />
      <WebPermission chats={chats} onOpenChat={setChatId} only={chatId} />
      <FolderPermission chats={chats} onOpenChat={setChatId} only={chatId} />
      <KeyPermission chats={chats} onOpenChat={setChatId} only={chatId} />
      <SharePermission chats={chats} onOpenChat={setChatId} only={chatId} />
    </div>
  );
}

function StudioWindow({ toast }: { toast: PushToast }) {
  const features = useFeatures();
  const progress = useInstallProgress();
  useEffect(() => {
    title(t("Studio"));
  }, []);
  // Picture, video and song jobs that failed say so here too (the main window does the same).
  useEffect(() => {
    const sub = on("media:done", (d) => {
      if (d.error && !d.cancelled) toast(t("A picture, video or song didn't finish: {error}", { error: errorText(d.error) }), "error");
    });
    return () => {
      sub.then((un) => un());
    };
  }, [toast]);
  return (
    <div className="side-window studio-window">
      <StudioView features={features} progress={progress} toast={toast} onGoFeatures={() => api.showMainPage("features").catch(() => {})} onPopOut={() => api.openStudioWindow().catch((e) => toast(errorText(e), "error"))} />
    </div>
  );
}

function SongStudioWindow({ projectId, toast }: { projectId: string; toast: PushToast }) {
  const [project, setProject] = useState<StudioProject | null | undefined>(undefined);
  const [songs, setSongs] = useState<MediaItem[]>([]);
  useEffect(() => {
    api
      .studioProjects()
      .then((all) => setProject(all.find((p) => p.id === projectId) ?? null))
      .catch(() => setProject(null));
    api.mediaList("audio", null, 200).then(setSongs).catch(() => {});
  }, [projectId]);
  useEffect(() => {
    if (project) title(`${project.name} (${t("Song studio")})`);
  }, [project]);
  if (project === undefined) return null;
  if (!project) {
    return (
      <div className="empty">
        <p className="muted">{t("That project is no longer here.")}</p>
        <button className="btn" onClick={close}>{t("Close")}</button>
      </div>
    );
  }
  return <SongStudio initial={project} songs={songs} onClose={close} toast={toast} inWindow />;
}

export default function SideWindow({ label }: { label: string }) {
  const toasts = useToasts();
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  // Actions that failed with nothing to catch them say so (see main.tsx).
  useEffect(() => {
    const show = (e: Event) => toasts.push(errorText((e as CustomEvent<string>).detail), "error");
    window.addEventListener("sulcus:error", show);
    return () => window.removeEventListener("sulcus:error", show);
  }, [toasts.push]);
  const refresh = useCallback(() => api.security().then(setSecurity).catch(() => {}), []);
  useEffect(() => {
    refresh();
    const sub = on("security:locked", refresh);
    return () => {
      sub.then((un) => un());
    };
  }, [refresh]);
  // Unlocked in the main window: noticed within a few seconds.
  useEffect(() => {
    if (!security?.locked) return;
    const timer = window.setInterval(refresh, 3000);
    return () => window.clearInterval(timer);
  }, [security?.locked, refresh]);

  let body = null;
  if (security?.locked) {
    body = (
      <div className="empty">
        <p className="muted">{t("SulcusAI is locked. Unlock it in the main window.")}</p>
      </div>
    );
  } else if (security) {
    if (label.startsWith("chat-")) body = <ChatWindow first={label.slice(5)} toast={toasts.push} />;
    else if (label.startsWith("songstudio-")) body = <SongStudioWindow projectId={label.slice(11)} toast={toasts.push} />;
    else body = <StudioWindow toast={toasts.push} />;
  }
  return (
    <>
      <div className="side-body">{body}</div>
      {security && !security.locked && (
        <footer className="statusbar side-status">
          <WorkBar onOpen={(page) => api.showMainPage(page).catch(() => {})} />
        </footer>
      )}
      <Toasts toasts={toasts.list} onDismiss={toasts.dismiss} />
    </>
  );
}
