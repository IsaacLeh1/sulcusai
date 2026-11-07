// SPDX-License-Identifier: AGPL-3.0-only
// Calls straight to the commands in desktop.rs, kept apart from src/api so
// the shell pieces load without pulling in the API chunks.
import { invoke } from "@tauri-apps/api/core";

const quiet = <T>(p: Promise<T>) => p.catch(() => undefined);

export const desktop = {
  badge: (count: number) => quiet(invoke("desktop_set_badge", { count })),
  progress: (fraction: number | null) => quiet(invoke("desktop_set_progress", { fraction })),
  keepInTray: (on: boolean) => quiet(invoke("desktop_keep_in_tray", { on })),
  startWithWindows: (on: boolean) => invoke<void>("desktop_start_with_windows", { on }),
  startsWithWindows: () => invoke<boolean>("desktop_starts_with_windows").catch(() => false),
};
