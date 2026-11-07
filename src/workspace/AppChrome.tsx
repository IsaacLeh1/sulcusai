// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useState } from "react";
import type { Chat, InstallProgress } from "../api";
import { APP_NAME } from "../brand";
import { t } from "../i18n";
import type { View } from "../App";
import { CommandBar, type Command } from "../components/CommandBar";
import { Dialog, OfflineBanner } from "../components/UiKit";
import { LookAndFeel } from "../components/LookAndFeel";
import { TopTabs } from "../components/TopTabs";
import { matches, useUiPrefs } from "../ui/prefs";
import { desktop } from "../ui/desktop";

const PAGES: { view: View; label: string }[] = [
  { view: "home", label: "Home" },
  { view: "chat", label: "Chat" },
  { view: "studio", label: "Create: Studio" },
  { view: "writing", label: "Create: Writing" },
  { view: "translate", label: "Create: Translate" },
  { view: "people", label: "Social" },
  { view: "meetings", label: "Social: Meetings" },
  { view: "networks", label: "Networks" },
  { view: "library", label: "Library" },
  { view: "notes", label: "Library: Notes" },
  { view: "documents", label: "Library: Documents" },
  { view: "memory", label: "Library: Memory" },
  { view: "tasks", label: "Tasks" },
  { view: "mail", label: "Mail" },
  { view: "calendar", label: "Calendar" },
  { view: "scheduled", label: "Scheduled" },
  { view: "models", label: "Settings: Models" },
  { view: "features", label: "Settings: Add-ons" },
  { view: "connectors", label: "Settings: Connectors" },
  { view: "activity", label: "Settings: Activity" },
  { view: "pc", label: "How the PC is doing" },
  { view: "settings", label: "Settings" },
];

const TABS_KEY = "sulcus.open-tabs";

function readTabs(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(TABS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

/**
 * Everything around the page: the top tabs and open-chat tabs, the offline
 * banner, Ctrl+K and the other shortcuts, and the window title.
 */
export function AppChrome({ view, setView, chats, current, activeChat, openChat, newChat, peopleUnread, progress = {} }: { view: View; setView: (v: View) => void; chats: Chat[]; current: Chat | null; activeChat: string | null; openChat: (id: string) => void; newChat: () => void; peopleUnread: number; progress?: Record<string, InstallProgress> }) {
  const prefs = useUiPrefs();
  const [open, setOpen] = useState<string[]>(readTabs);
  const [cmd, setCmd] = useState(false);
  const [look, setLook] = useState(false);
  const save = useCallback((ids: string[]) => {
    setOpen(ids);
    try {
      localStorage.setItem(TABS_KEY, JSON.stringify(ids));
    } catch {
      // Tabs just won't come back after a restart.
    }
  }, []);

  // A chat opened anywhere gets a tab.
  useEffect(() => {
    if (view === "chat" && activeChat && !open.includes(activeChat)) save([...open, activeChat].slice(-12));
  }, [view, activeChat, open, save]);
  // Deleted chats lose their tab.
  useEffect(() => {
    const kept = open.filter((id) => chats.some((c) => c.id === id));
    if (chats.length > 0 && kept.length !== open.length) save(kept);
  }, [chats, open, save]);

  const tabs = open.map((id) => ({ id, title: chats.find((c) => c.id === id)?.title ?? "" }));
  const close = useCallback(
    (id: string) => {
      const i = open.indexOf(id);
      const rest = open.filter((x) => x !== id);
      save(rest);
      if (id === activeChat && view === "chat") {
        const next = rest[Math.min(i, rest.length - 1)];
        if (next) openChat(next);
        else setView("home");
      }
    },
    [open, save, activeChat, view, openChat, setView],
  );

  useEffect(() => {
    desktop.badge(prefs.calm ? 0 : peopleUnread);
  }, [peopleUnread, prefs.calm]);
  const recentKey = [...chats]
    .filter((c) => !c.incognito)
    .sort((a, b) => b.updated_at - a.updated_at)
    .slice(0, 6)
    .map((c) => JSON.stringify([c.id, c.title]))
    .join("\n");
  useEffect(() => {
    desktop.recentChats(recentKey ? recentKey.split("\n").map((l) => JSON.parse(l) as [string, string]) : []);
  }, [recentKey]);
  useEffect(() => {
    desktop.startAsk().then((a) => {
      if (a?.new_chat) newChat();
      else if (a?.chat) openChat(a.chat);
    });
    return desktop.onAsks(newChat, openChat);
    // Asked once per start; the handlers read the latest props through the events.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    desktop.keepInTray(prefs.keepInTray);
  }, [prefs.keepInTray]);
  // The taskbar shows the furthest-behind download, so it fills only when all are done.
  const jobs = Object.values(progress).filter((j) => j.total > 0);
  const least = jobs.length ? Math.min(...jobs.map((j) => j.received / j.total)) : null;
  useEffect(() => {
    desktop.progress(least);
  }, [least]);

  useEffect(() => {
    document.title = view === "chat" && current ? `${current.title || t("New chat")} - ${APP_NAME}` : APP_NAME;
  }, [view, current]);

  useEffect(() => {
    const s = prefs.shortcuts;
    const key = (e: KeyboardEvent) => {
      const cycle = (by: number) => {
        if (open.length === 0) return;
        const i = activeChat ? open.indexOf(activeChat) : -1;
        openChat(open[(i + by + open.length) % open.length]);
      };
      if (matches(e, s.commandBar)) (e.preventDefault(), setCmd((c) => !c));
      else if (matches(e, s.newChat)) (e.preventDefault(), newChat());
      else if (matches(e, s.settings)) (e.preventDefault(), setView("settings"));
      else if (matches(e, s.home)) (e.preventDefault(), setView("home"));
      else if (matches(e, s.closeTab) && view === "chat" && activeChat) (e.preventDefault(), close(activeChat));
      else if (matches(e, s.nextTab)) (e.preventDefault(), cycle(1));
      else if (matches(e, s.prevTab)) (e.preventDefault(), cycle(-1));
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [prefs.shortcuts, open, activeChat, view, openChat, newChat, setView, close]);

  const commands = useMemo<Command[]>(
    () => [
      { id: "new-chat", label: t("New chat"), group: t("Action"), hint: prefs.shortcuts.newChat, run: newChat },
      { id: "look", label: t("Look and feel: density, contrast, sounds, tabs, shortcuts"), group: t("Setting"), run: () => setLook(true) },
      ...PAGES.map((p) => ({ id: p.view, label: t(p.label), group: t("Page"), run: () => setView(p.view) })),
      ...chats.slice(0, 300).map((c) => ({ id: c.id, label: c.title || t("New chat"), group: t("Chat"), run: () => openChat(c.id) })),
    ],
    [chats, newChat, openChat, setView, prefs.shortcuts.newChat],
  );

  return (
    <>
      <OfflineBanner />
      <TopTabs
        view={view}
        setView={setView}
        open={tabs}
        active={activeChat}
        onOpen={openChat}
        onClose={close}
        onReorder={save}
        unread={{ social: peopleUnread }}
      />
      {cmd && <CommandBar commands={commands} onClose={() => setCmd(false)} />}
      {look && (
        <Dialog title={t("Look and feel")} onClose={() => setLook(false)}>
          <LookAndFeel />
        </Dialog>
      )}
    </>
  );
}

