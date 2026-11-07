// SPDX-License-Identifier: AGPL-3.0-only
// Calls straight to the commands in desktop.rs, kept apart from src/api so
// the shell pieces load without pulling in the API chunks.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const quiet = <T>(p: Promise<T>) => p.catch(() => undefined);

export const desktop = {
  badge: (count: number) => quiet(invoke("desktop_set_badge", { count })),
  progress: (fraction: number | null) => quiet(invoke("desktop_set_progress", { fraction })),
  keepInTray: (on: boolean) => quiet(invoke("desktop_keep_in_tray", { on })),
  startWithWindows: (on: boolean) => invoke<void>("desktop_start_with_windows", { on }),
  startsWithWindows: () => invoke<boolean>("desktop_starts_with_windows").catch(() => false),
  recentChats: (chats: [string, string][]) => quiet(invoke("desktop_set_recent_chats", { chats })),
  startAsk: () => invoke<{ new_chat?: boolean; chat?: string } | null>("desktop_start_ask").catch(() => null),
  /** The jump list's New chat and recent chats, picked while the app runs. */
  onAsks: (newChat: () => void, openChat: (id: string) => void) => {
    const subs = [listen("desktop:new-chat", () => newChat()), listen<{ id: string }>("desktop:open-chat", (e) => openChat(e.payload.id))];
    return () => subs.forEach((s) => s.then((un) => un()).catch(() => {}));
  },
};
