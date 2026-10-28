// SPDX-License-Identifier: AGPL-3.0-only
import { ask } from "@tauri-apps/plugin-dialog";
import { APP_NAME } from "./brand";

/** A yes-or-no question before going ahead. Not `window.confirm`: Tauri's
 * dialog plugin replaces it with an async one, so `if (!window.confirm(…))`
 * got a Promise, always truthy, and went ahead without waiting. */
export function confirmed(question: string): Promise<boolean> {
  return ask(question, { title: APP_NAME, kind: "warning" });
}
