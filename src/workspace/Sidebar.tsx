// SPDX-License-Identifier: AGPL-3.0-only
import { api, errorText, type Chat, type FeatureId, type FolderView, type KeepLocks, type Project, type SecurityStatus, type SharedHere, type SidebarLayout } from "../api";
import { APP_NAME } from "../brand";
import { t, tx } from "../i18n";
import { SideMenu } from "../components/SideMenu";
import { AssistantsSection } from "../components/Assistants";
import { SETTINGS_VIEWS, SettingsNav } from "../components/SettingsNav";
import { IncognitoIcon } from "../components/IncognitoIcon";
import { LockIcon, PeopleIcon, SidebarIcon } from "../components/Icons";
import type { PushToast } from "../components/Toasts";
import { SidebarWidgets } from "./SidebarWidgets";
import type { View } from "../App";
import { SidebarNav } from "./SidebarNav";
import { SidebarProjects } from "./SidebarProjects";
import { SidebarSelectBar } from "./SidebarSelectBar";
import { SidebarChats } from "./SidebarChats";

export interface SidebarProps {
  security: SecurityStatus;
  phone: boolean;
  view: View;
  setView: (v: View) => void;
  setActiveChat: (id: string | null) => void;
  activeChat: string | null;
  activeProject: string | null;
  setActiveProject: (id: string | null) => void;
  openChat: (id: string) => void;
  refreshChats: () => Promise<void>;
  // The menu
  has: (f: FeatureId) => boolean;
  features: Set<FeatureId> | null;
  creates: boolean;
  computeOn: boolean;
  liveMeeting: string | null;
  peopleOn: boolean;
  peopleUnread: number;
  installing: boolean;
  layout: SidebarLayout;
  saveLayout: (next: SidebarLayout) => void;
  studioWindow: () => void;
  setNavMenu: (m: { id: string; x: number; y: number }) => void;
  // Settings' own sidebar
  settingsSection: string | null;
  onSettingsBack: () => void;
  onSettingsJump: (id: string) => void;
  // New chat
  installedCount: number;
  newChat: (projectId?: string | null, incognito?: boolean) => void;
  newChatWindow: () => void;
  // Projects
  projects: Project[];
  onNewProject: () => void;
  setProjectMenu: (m: { p: Project; x: number; y: number }) => void;
  // Chats
  chats: Chat[];
  listed: Chat[];
  visible: Chat[];
  hiddenOwn: Chat[];
  hiddenShared: SharedHere[];
  sharedVisible: SharedHere[];
  sharedHere: SharedHere[];
  sharedByMe: Map<string, string[]>;
  sharedKey: (s: SharedHere) => string;
  openShared: SharedHere | null;
  setOpenShared: (s: SharedHere) => void;
  folders: FolderView[];
  setFolders: (f: FolderView[]) => void;
  foldered: Set<string>;
  locks: KeepLocks;
  setLocks: (l: KeepLocks) => void;
  running: string[];
  selected: string[];
  setSelected: (f: (s: string[]) => string[]) => void;
  chatQuery: string;
  setChatQuery: (q: string) => void;
  showHidden: boolean;
  setShowHidden: (f: (v: boolean) => boolean) => void;
  setChatMenu: (m: { id: string; x: number; y: number }) => void;
  setSharedMenu: (m: { s: SharedHere; x: number; y: number }) => void;
  setFolderFor: (id: string) => void;
  setSharingMany: (on: boolean) => void;
  setDeletingMany: (on: boolean) => void;
  hideChat: (key: string, hide: boolean) => void;
  toast: PushToast;
}

/** The left column: brand and its buttons, the page menu, projects, assistants and chats (or Settings' own menu). */
export function Sidebar(p: SidebarProps) {
  const { phone, view, setView, layout, saveLayout, toast } = p;
  const inSettings = SETTINGS_VIEWS.includes(view);
  return (
    <aside className="sidebar">
      <div className="brand">
        <img src="logo.svg" alt="" width={26} height={26} />
        <span>{APP_NAME}</span>
        {p.security.lock_enabled && (
          <button className="icon-btn lock-btn" title={t("Lock now")} aria-label={t("Lock now")} onClick={() => api.lockNow()}>
            <LockIcon />
          </button>
        )}
        {p.peopleOn && (
          <button
            className={`icon-btn people-btn ${view === "people" ? "active" : ""}`}
            title={t("People (Shift+click opens them in their own window)")}
            aria-label={p.peopleUnread > 0 ? t("People, {n} unread", { n: p.peopleUnread }) : t("People")}
            onClick={(e) => {
              if (e.shiftKey && !phone) api.openPeopleWindow().catch((err) => toast(errorText(err), "error"));
              else setView("people");
            }}
          >
            <PeopleIcon />
            {p.peopleUnread > 0 && (
              <span className="people-unread-badge" aria-hidden="true">
                {p.peopleUnread > 99 ? "99+" : p.peopleUnread}
              </span>
            )}
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
          features={p.features ?? new Set()}
          section={p.settingsSection}
          onBack={p.onSettingsBack}
          onJump={p.onSettingsJump}
          onView={setView}
        />
      ) : (<>
        <div className="new-chat-row">
          <button
            className="btn primary block"
            onClick={(e) => (e.shiftKey && !phone ? p.newChatWindow() : p.newChat())}
            title={phone ? undefined : t("Shift+click: a new chat in a window of its own")}
          >
            {t("+ New chat")}
          </button>
          <button
            className="btn incognito-btn"
            onClick={() => p.newChat(null, true)}
            disabled={p.installedCount === 0}
            title={t("Incognito chat: not saved, no memory")}
            aria-label={t("New incognito chat")}
          >
            <IncognitoIcon size="1.3em" />
          </button>
        </div>
        <SidebarNav
          view={view}
          setView={setView}
          phone={phone}
          creates={p.creates}
          computeOn={p.computeOn}
          liveMeeting={p.liveMeeting}
          has={p.has}
          layout={layout}
          saveLayout={saveLayout}
          studioWindow={p.studioWindow}
          onMenu={p.setNavMenu}
        />
        {p.has("projects") && (
          <SidebarProjects
            projects={p.projects}
            view={view}
            activeProject={p.activeProject}
            onNew={p.onNewProject}
            onOpen={(id) => {
              p.setActiveProject(id);
              setView("project");
            }}
            onMenu={p.setProjectMenu}
          />
        )}
        <AssistantsSection
          onChat={async (id) => {
            await p.refreshChats();
            p.setActiveChat(id);
            setView("chat");
          }}
          toast={toast}
        />
        {p.selected.length > 0 && (
          <SidebarSelectBar
            selected={p.selected}
            setSelected={(ids) => p.setSelected(() => ids)}
            visible={p.visible}
            chats={p.chats}
            folders={p.folders}
            setFolders={p.setFolders}
            locks={p.locks}
            setLocks={p.setLocks}
            peopleOn={p.peopleOn}
            setFolderFor={p.setFolderFor}
            setSharingMany={p.setSharingMany}
            setDeletingMany={p.setDeletingMany}
            hideChat={p.hideChat}
            refreshChats={p.refreshChats}
            setActiveChat={p.setActiveChat}
            setView={setView}
            toast={toast}
          />
        )}
        <SidebarChats
          view={view}
          setView={setView}
          activeChat={p.activeChat}
          openChat={p.openChat}
          chatQuery={p.chatQuery}
          setChatQuery={p.setChatQuery}
          showHidden={p.showHidden}
          setShowHidden={p.setShowHidden}
          listed={p.listed}
          visible={p.visible}
          hiddenOwn={p.hiddenOwn}
          hiddenShared={p.hiddenShared}
          sharedVisible={p.sharedVisible}
          sharedHere={p.sharedHere}
          sharedByMe={p.sharedByMe}
          sharedKey={p.sharedKey}
          openShared={p.openShared}
          setOpenShared={p.setOpenShared}
          folders={p.folders}
          setFolders={p.setFolders}
          foldered={p.foldered}
          locks={p.locks}
          projects={p.projects}
          running={p.running}
          selected={p.selected}
          setSelected={p.setSelected}
          layout={layout}
          saveLayout={saveLayout}
          setChatMenu={p.setChatMenu}
          setSharedMenu={p.setSharedMenu}
          refreshChats={p.refreshChats}
          peopleOn={p.peopleOn}
          toast={toast}
        />
        <SidebarWidgets setView={setView} toast={toast} />
      </>)}
      {/* Connectors and People, when on, sit in the gear menu: the top is about the work. */}
      <SideMenu
        view={view}
        installing={p.installing}
        extra={[
          ...(p.has("connectors") ? [{ id: "connectors" as const, label: tx("Connectors") }] : []),
          ...(p.has("people") ? [{ id: "people" as const, label: tx("People"), unread: p.peopleUnread }] : []),
        ]}
        onOpen={setView}
      />
    </aside>
  );
}
