// SPDX-License-Identifier: AGPL-3.0-only
// People in a window of its own, opened from the People page or by
// Shift+clicking the People button. While SulcusAI is locked it shows only
// that; unlocking happens in the main window.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type SecurityStatus } from "../api";
import { Toasts, useToasts } from "../components/Toasts";
import { t } from "../i18n";
import { PeopleView } from "./PeopleView";

export default function PeopleWindow() {
  const toasts = useToasts();
  const [security, setSecurity] = useState<SecurityStatus | null>(null);
  // Actions that failed with nothing to catch them say so (see main.tsx).
  useEffect(() => {
    const show = (e: Event) => toasts.push(errorText((e as CustomEvent<string>).detail), "error");
    window.addEventListener("sulcus:error", show);
    return () => window.removeEventListener("sulcus:error", show);
  }, [toasts.push]);

  const refresh = useCallback(() => api.security().then(setSecurity).catch(() => {}), []);
  useEffect(() => {
    refresh();
    const sub = on("security:locked", refresh);
    return () => {
      sub.then((un) => un());
    };
  }, [refresh]);
  // Unlocked in the main window: noticed within a few seconds.
  useEffect(() => {
    if (!security?.locked) return;
    const timer = window.setInterval(refresh, 3000);
    return () => window.clearInterval(timer);
  }, [security?.locked, refresh]);

  return (
    <div className="people-window">
      {security?.locked ? (
        <div className="empty">
          <p className="muted">{t("SulcusAI is locked. Unlock it in the main window.")}</p>
        </div>
      ) : (
        security && <PeopleView toast={toasts.push} inWindow />
      )}
      <Toasts toasts={toasts.list} onDismiss={toasts.dismiss} />
    </div>
  );
}
