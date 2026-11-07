// SPDX-License-Identifier: AGPL-3.0-only
import { Suspense } from "react";
import type { CatalogView, Chat, Connectivity, FeatureId, InstallProgress, ModelCard, Project, SecurityStatus, SharedHere, Settings } from "../api";
import { APP_NAME } from "../brand";
import { t } from "../i18n";
import { ChatView } from "../views/ChatView";
import { ProtectedGate } from "../components/ProtectedChat";
import { MenuIcon, PcIcon } from "../components/Icons";
import { TopBar, PageLoading } from "../components/WorkspaceBits";
import type { PushToast } from "../components/Toasts";
import type { View } from "../App";
import { ModelsView, SettingsView, ActivityView, WebViewerView, MemoryView, ProjectView, ScheduledView, ConnectorsView, MeetingsView, TranslateView, WritingView, FeaturesView, NotesView, DocumentsView, StudyView, ReviewView, ComputeView, TasksView, MailView, CalendarView, StudioView, PeopleView, SharedChatView, PcView, PersonalView, LibraryView, HomeView, NetworksPage } from "../pages";
import { AppChrome } from "./AppChrome";

export interface MainPaneProps {
  view: View;
  setView: (v: View) => void;
  phone: boolean;
  toast: PushToast;
  security: SecurityStatus;
  onSecurityChanged: () => void;
  settings: Settings;
  setSettings: (s: Settings) => void;
  features: Set<FeatureId> | null;
  catalog: CatalogView | null;
  refreshCatalog: () => Promise<void>;
  progress: Record<string, InstallProgress>;
  installed: ModelCard[];
  // The top bar, the phone bar and the web viewer
  setMenuOpen: (open: boolean) => void;
  sideHidden: boolean;
  showSide: () => void;
  onViewer: () => void;
  viewerOpen: boolean;
  viewerStart: { url: string | null; at: number };
  // Chats
  chats: Chat[];
  current: Chat | null;
  activeChat: string | null;
  openChat: (id: string) => void;
  newChat: (projectId?: string | null) => void;
  refreshChats: () => Promise<void>;
  onChatDeleted: () => Promise<void>;
  toggleChatWeb: () => void;
  browser: { url: string | null; at: number } | null;
  onChatBrowser: (open: boolean) => void;
  onPopOut: (() => void) | undefined;
  startVoice: boolean;
  studioWindow: () => void;
  // Projects
  projects: Project[];
  activeProject: string | null;
  refreshProjects: () => Promise<void>;
  onProjectDeleted: () => Promise<void>;
  // Meetings, shared chats, Settings
  liveMeeting: string | null;
  setLiveMeeting: (id: string | null) => void;
  openShared: SharedHere | null;
  settingsJump: { id: string; at: number } | null;
  onSettingsSection: (id: string | null) => void;
  changeConnectivity: (level: Connectivity) => void;
  peopleUnread?: number;
}

/** The right-hand side: the top bar (or the phone's), and whichever page the view names. */
export function MainPane(p: MainPaneProps) {
  const { view, setView, phone, toast, settings, current } = p;
  const features = p.features ?? new Set<FeatureId>();
  return (
    <main className="main">
      {phone && (
        <div className="phone-bar">
          <button className="icon-btn" aria-label={t("Menu")} title={t("Menu")} onClick={() => p.setMenuOpen(true)}>
            <MenuIcon size="1.4em" />
          </button>
          <span className="phone-title ellipsis">{view === "chat" && current ? current.title : APP_NAME}</span>
          <button className={`icon-btn${view === "pc" ? " active" : ""}`} aria-label={t("How the PC is doing")} title={t("How the PC is doing")} onClick={() => setView("pc")}>
            <PcIcon size="1.4em" />
          </button>
        </div>
      )}
      {!phone && (
        <AppChrome view={view} setView={setView} chats={p.chats} current={current} activeChat={p.activeChat} openChat={p.openChat} newChat={() => p.newChat()} peopleUnread={p.peopleUnread ?? 0} progress={p.progress} models={p.installed} />
      )}
      {!phone && <TopBar sideHidden={p.sideHidden} onShowSide={p.showSide} viewerOpen={p.viewerOpen} onViewer={p.onViewer} toast={toast} />}
      <Suspense fallback={<PageLoading />}>
        {view === "home" && <HomeView chats={p.chats} progress={p.progress} peopleUnread={p.peopleUnread ?? 0} openChat={p.openChat} newChat={() => p.newChat()} setView={setView} />}
        {view === "networks" && <NetworksPage toast={toast} />}
        {view === "pc" && <PcView toast={toast} />}
        {view === "web" && !phone && <WebViewerView start={p.viewerStart} toast={toast} onClose={() => setView("chat")} />}
        {view === "chat" && current?.shut && <ProtectedGate key={current.id} chatId={current.id} security={p.security} onOpened={p.refreshChats} />}
        {view === "chat" && !current?.shut && (
          <ChatView
            chat={current}
            projectName={p.projects.find((x) => x.id === current?.project_id)?.name ?? null}
            onOpenChat={p.openChat}
            installed={p.installed}
            defaultModel={settings.default_model}
            connectivity={settings.connectivity}
            onNewChat={() => p.newChat()}
            onGoModels={() => setView("models")}
            onChanged={p.refreshChats}
            onDeleted={p.onChatDeleted}
            onToggleWeb={p.toggleChatWeb}
            features={features}
            onGoFeatures={() => setView("features")}
            browser={p.browser}
            onBrowser={p.onChatBrowser}
            onPopOut={p.onPopOut}
            startVoice={p.startVoice}
            toast={toast}
          />
        )}
        {view === "features" && <FeaturesView progress={p.progress} toast={toast} />}
        {view === "studio" && <StudioView features={features} progress={p.progress} toast={toast} onGoFeatures={() => setView("features")} onPopOut={phone ? undefined : p.studioWindow} />}
        {view === "notes" && <NotesView toast={toast} />}
        {view === "library" && <LibraryView toast={toast} />}
        {view === "documents" && <DocumentsView toast={toast} />}
        {view === "study" && <StudyView toast={toast} onOpenStudio={() => setView("studio")} />}
        {view === "review" && <ReviewView toast={toast} />}
        {view === "compute" && <ComputeView toast={toast} />}
        {view === "tasks" && <TasksView onOpenChat={p.openChat} toast={toast} />}
        {view === "mail" && <MailView toast={toast} />}
        {view === "calendar" && <CalendarView toast={toast} />}
        {view === "models" && (
          <ModelsView
            catalog={p.catalog}
            progress={p.progress}
            defaultModel={settings.default_model}
            onRefresh={p.refreshCatalog}
            onSettings={p.setSettings}
            toast={toast}
          />
        )}
        {view === "meetings" && (
          <MeetingsView
            live={p.liveMeeting}
            onLiveChanged={p.setLiveMeeting}
            onOpenChat={async (id) => {
              await p.refreshChats();
              p.openChat(id);
            }}
            onGoModels={() => setView("models")}
            toast={toast}
          />
        )}
        {view === "translate" && <TranslateView hasModel={p.installed.length > 0} onGoModels={() => setView("models")} toast={toast} />}
        {view === "writing" && <WritingView toast={toast} />}
        {view === "personal" && <PersonalView toast={toast} />}
        {view === "activity" && <ActivityView toast={toast} />}
        {view === "connectors" && <ConnectorsView toast={toast} />}
        {view === "people" && <PeopleView toast={toast} />}
        {view === "shared" && p.openShared && (
          <div className="page shared-page">
            <SharedChatView key={`${p.openShared.contact_id}:${p.openShared.chat.id}`} contactId={p.openShared.contact_id} contactName={p.openShared.contact_name} chat={p.openShared.chat} toast={toast} />
          </div>
        )}
        {view === "scheduled" && <ScheduledView installed={p.installed} onOpenChat={p.openChat} toast={toast} />}
        {view === "memory" && <MemoryView settings={settings} onSettings={p.setSettings} toast={toast} />}
        {view === "project" && p.activeProject && (
          <ProjectView
            key={p.activeProject}
            projectId={p.activeProject}
            listed={p.projects.find((x) => x.id === p.activeProject)}
            chats={p.chats}
            onOpenChat={p.openChat}
            onNewChat={() => p.newChat(p.activeProject)}
            onChanged={p.refreshProjects}
            onDeleted={p.onProjectDeleted}
            toast={toast}
          />
        )}
        {view === "settings" && (
          <SettingsView
            connectivity={settings.connectivity}
            onConnectivity={p.changeConnectivity}
            security={p.security}
            onSecurityChanged={p.onSecurityChanged}
            features={features}
            toast={toast}
            jump={p.settingsJump}
            onSection={p.onSettingsSection}
          />
        )}
      </Suspense>
    </main>
  );
}
