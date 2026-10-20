// SPDX-License-Identifier: AGPL-3.0-only
// The Cancel button on a model being installed: stops it and deletes what
// was downloaded so far. Hidden while the model is being tried out on this
// PC (it's downloaded by then).
import { api, type InstallProgress } from "../api";
import { t } from "../i18n";
import { TESTING } from "./Testing";

const DOWNLOADING = new Set<InstallProgress["phase"]>(["engine", "download", "verify", "vision"]);

export function CancelInstall({ p, name }: { p: InstallProgress; name?: string }) {
  if (TESTING.has(p.phase)) return null;
  const label = DOWNLOADING.has(p.phase) ? t("Cancel download") : t("Cancel");
  return (
    <button
      type="button"
      className="btn small cancel-install"
      onClick={() => api.cancelInstall(p.model_id)}
      title={t("Stops it and deletes what was downloaded so far")}
      aria-label={name ? `${label} · ${name}` : undefined}
    >
      {label}
    </button>
  );
}
