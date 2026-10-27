// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { t } from "../i18n";

/** The strip at the top of the screen while the assistant works another
 * program: what it's doing, and Stop (tools/programs.rs). */
export default function WorkingBanner() {
  const [what, setWhat] = useState<string | null>(null);
  useEffect(() => {
    document.documentElement.classList.add("working-window");
    api.programsBanner().then((s) => setWhat(s?.what ?? null)).catch(() => {});
    const off = listen<{ what: string }>("programs:working", (e) => setWhat(e.payload.what));
    return () => {
      off.then((f) => f());
    };
  }, []);
  return (
    <div className="working-banner" role="status">
      <span className="working-dot" aria-hidden="true" />
      <span className="working-text">{t("SulcusAI: {what}", { what: what ? t(what) : t("Working in another program") })}</span>
      <button className="btn small danger" onClick={() => api.programsBannerStop().catch(() => {})}>{t("Stop")}</button>
    </div>
  );
}
