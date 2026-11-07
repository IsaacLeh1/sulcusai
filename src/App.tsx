// SPDX-License-Identifier: AGPL-3.0-only
import { loadDrafts } from "./drafts";
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type SecurityStatus } from "./api";
import { t } from "./i18n";
import { LockScreen } from "./components/Security";
import { Toasts, useToasts } from "./components/Toasts";
import { RemoteOverlay } from "./components/RemoteOverlay";
import { useIdleLock } from "./idle";
import { Workspace } from "./Workspace";

export type View = "home" | "networks" | "chat" | "models" | "features" | "studio" | "meetings" | "translate" | "writing" | "notes" | "documents" | "study" | "review" | "compute" | "tasks" | "mail" | "calendar" | "memory" | "scheduled" | "connectors" | "people" | "shared" | "project" | "activity" | "settings" | "pc" | "personal" | "web" | "library";

export default function App() {
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  // Kept out here so unlocking returns to the same page and chat.
  const [view, setView] = useState<View>("home");
  const [activeChat, setActiveChat] = useState<string | null>(null);
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const toasts = useToasts();
  // Actions that failed with nothing to catch them say so (see main.tsx).
  // Unsent messages kept from before a restart (when that's turned on).
  useEffect(() => {
    loadDrafts();
  }, []);
  useEffect(() => {
    const show = (e: Event) => {
      const raw = (e as CustomEvent<unknown>).detail;
      // The plain words on the toast; the original for "Copy details".
      toasts.push(errorText(raw), "error", { details: typeof raw === "string" ? raw : JSON.stringify(raw) });
    };
    window.addEventListener("sulcus:error", show);
    return () => window.removeEventListener("sulcus:error", show);
  }, [toasts.push]);
  // A file dropped where nothing takes it would open in the window in the
  // app's place: dropping files is only for the places that take them.
  useEffect(() => {
    const keep = (e: DragEvent) => {
      if (e.dataTransfer?.types.includes("Files")) e.preventDefault();
    };
    window.addEventListener("dragover", keep);
    window.addEventListener("drop", keep);
    return () => {
      window.removeEventListener("dragover", keep);
      window.removeEventListener("drop", keep);
    };
  }, []);

  const refreshSecurity = useCallback(async () => setSecurity(await api.security()), []);
  useEffect(() => {
    refreshSecurity();
    const sub = on("security:locked", () => refreshSecurity());
    return () => {
      sub.then((un) => un());
    };
  }, [refreshSecurity]);

  useIdleLock(security?.lock_enabled && !security.locked ? security.auto_lock_minutes : 0, () => {
    api.lockNow().catch(() => {});
  });

  let body;
  if (!security) body = <div className="empty"><p className="muted">{t("Starting…")}</p></div>;
  else if (security.locked) body = <LockScreen status={security} onUnlocked={refreshSecurity} />;
  else
    body = (
      <Workspace
        security={security}
        onSecurityChanged={refreshSecurity}
        toast={toasts.push}
        nav={{ view, setView, activeChat, setActiveChat, activeProject, setActiveProject }}
      />
    );

  return (
    <>
      {body}
      <Toasts toasts={toasts.list} onDismiss={toasts.dismiss} />
      <RemoteOverlay />
    </>
  );
}
