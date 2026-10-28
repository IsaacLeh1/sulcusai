// SPDX-License-Identifier: AGPL-3.0-only
import type { FeatureId } from "../api";
import type { View } from "../App";
import { APP_NAME } from "../brand";
import { t, tx } from "../i18n";

export const VOICE_FEATURES: FeatureId[] = ["dictation", "voice_chat", "read_aloud", "meetings"];

/** The Settings page's sections, in the page's order and with its conditions.
 * Each `id` matches the page's anchor `settings-<id>`. */
export function settingsSections(features: Set<FeatureId>): { id: string; label: string }[] {
  const all: { id: string; label: string; show?: boolean }[] = [
    { id: "appearance", label: tx("Appearance") },
    { id: "language", label: tx("Language") },
    { id: "about-you", label: tx("About you") },
    { id: "security", label: tx("Privacy & security") },
    { id: "folders", label: tx("Shared folders"), show: features.has("files") },
    { id: "handoffs", label: tx("Handoffs between chats"), show: features.has("files") },
    { id: "permissions", label: tx("Permissions") },
    { id: "voice", label: tx("Voice and meetings"), show: VOICE_FEATURES.some((f) => features.has(f)) },
    { id: "performance", label: tx("Performance") },
    { id: "check", label: tx("Check this PC") },
    { id: "servers", label: tx("Servers and ports") },
    { id: "storage", label: tx("Where models are saved") },
    { id: "cleanup", label: tx("Cleanup and space") },
    { id: "sign-in", label: tx("Microsoft and Google sign-in"), show: features.has("email") || features.has("calendar") },
    { id: "connectivity", label: tx("Connectivity") },
    { id: "private", label: tx("Private connection") },
    { id: "privacy-mode", label: tx("Privacy mode") },
    { id: "sent", label: tx("What left this PC") },
    { id: "devices", label: tx("Your devices") },
    { id: "phone", label: tx("Phone") },
    { id: "backup", label: tx("Backup and restore") },
    { id: "cloud", label: tx("Cloud models") },
    { id: "api", label: tx("Local API server") },
    { id: "advanced", label: tx("Advanced") },
    { id: "logs", label: tx("Logs") },
    { id: "about", label: tx("About {app}") },
    { id: "update-notes", label: tx("Update notes") },
  ];
  return all.filter((s) => s.show !== false);
}

/** The other pages under the gear menu that share this sidebar. */
const PAGES: { view: View; label: string }[] = [
  { view: "models", label: tx("Models") },
  { view: "features", label: tx("Features") },
  { view: "activity", label: tx("Activity") },
];

export const SETTINGS_VIEWS: View[] = ["settings", ...PAGES.map((p) => p.view)];

/** The sidebar while in Settings: back to where you were, then quick links
 * to each section and the other settings pages. */
export function SettingsNav({ view, features, section, onBack, onJump, onView }: {
  view: View;
  features: Set<FeatureId>;
  /** The section being read on the Settings page. */
  section: string | null;
  onBack: () => void;
  onJump: (id: string) => void;
  onView: (v: View) => void;
}) {
  return (
    <>
      <div className="new-chat-row">
        <button className="btn primary block" onClick={onBack}>
          ← {t("Back")}
        </button>
      </div>
      <nav className="nav settings-nav" aria-label={t("Settings")}>
        <div className="side-head">
          <span>{t("Settings")}</span>
        </div>
        {settingsSections(features).map((s) => (
          <button key={s.id} className={view === "settings" && section === s.id ? "active" : ""} aria-current={view === "settings" && section === s.id ? "location" : undefined} onClick={() => onJump(s.id)}>
            {t(s.label, { app: APP_NAME })}
          </button>
        ))}
        <div className="side-head">
          <span>{t("More")}</span>
        </div>
        {PAGES.map((p) => (
          <button key={p.view} className={view === p.view ? "active" : ""} aria-current={view === p.view ? "page" : undefined} onClick={() => onView(p.view)}>
            {t(p.label)}
          </button>
        ))}
      </nav>
    </>
  );
}
