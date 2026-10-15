// SPDX-License-Identifier: AGPL-3.0-only
import { ask } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { t } from "./i18n";

/** Before sharing a folder: one inside Windows, Program Files or ProgramData
 * is shared only after a yes to the warning. `go` says whether to share it,
 * `system` whether the person confirmed a system folder. */
export async function confirmFolder(path: string): Promise<{ go: boolean; system: boolean }> {
  const warning = await api.folderWarning(path);
  if (!warning) return { go: true, system: false };
  const go = await ask(t(warning), { title: t("Share a system folder?"), kind: "warning", okLabel: t("Share anyway"), cancelLabel: t("Cancel") });
  return { go, system: go };
}
