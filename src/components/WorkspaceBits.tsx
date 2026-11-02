// SPDX-License-Identifier: AGPL-3.0-only
// Small pieces of the workspace: the top bar, the new-project dialog and the
// page placeholder.
import { useEffect, useState } from "react";
import { api, errorText, type BrowsersView } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import { type PushToast } from "./Toasts";
import { FeedbackButton } from "./Feedback";
import { GlobeIcon, SidebarIcon, ViewerIcon } from "./Icons";

export function NewProjectDialog({ onClose, onCreate }: { onClose: () => void; onCreate: (name: string) => void }) {
  const [name, setName] = useState("");
  return (
    <Modal title={t("New project")} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) onCreate(name.trim());
        }}
      >
        <p className="muted small">{t("A project keeps its own instructions, folders and memories for a set of related chats.")}</p>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Project name, e.g. Thesis")} aria-label={t("Project name, e.g. Thesis")} autoFocus maxLength={80} />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!name.trim()}>{t("Create")}</button>
        </div>
      </form>
    </Modal>
  );
}


/** Across the top of the page: the web viewer's button, and the globe,
 * which opens the main browser (Chrome unless another was picked). */
export function TopBar({ sideHidden, onShowSide, viewerOpen, onViewer, toast }: { sideHidden: boolean; onShowSide: () => void; viewerOpen: boolean; onViewer: () => void; toast: PushToast }) {
  const [view, setView] = useState<BrowsersView | null>(null);
  useEffect(() => {
    const load = () => api.browsersView().then(setView).catch(() => {});
    load();
    window.addEventListener("browser-changed", load);
    return () => window.removeEventListener("browser-changed", load);
  }, []);
  const name = view?.current.kind === "app" ? view.current.name : view?.current.kind === "system" ? t("your default browser") : t("the web viewer");
  return (
    <div className="topbar">
      {sideHidden && (
        <button className="icon-btn side-show" title={t("Show the menu (Ctrl+B)")} aria-label={t("Show the menu")} onClick={onShowSide}>
          <SidebarIcon />
        </button>
      )}
      <FeedbackButton toast={toast} />
      <button className={`icon-btn web-btn${viewerOpen ? " active" : ""}`} title={t("Open the web viewer")} aria-label={t("Open the web viewer")} onClick={onViewer}>
        <ViewerIcon />
      </button>
      {view && (
        <button className="icon-btn web-btn" title={t("Open {name}", { name })} aria-label={t("Open {name}", { name })} onClick={() => api.openWeb(null).catch((e) => toast(errorText(e), "error"))}>
          <GlobeIcon />
        </button>
      )}
    </div>
  );
}

export function listNames(names: string[]): string {
  return names.length <= 1 ? names.join("") : t("{list} and {last}", { list: names.slice(0, -1).join(", "), last: names[names.length - 1] });
}

export function PageLoading() {
  return <div className="page"><p className="muted">{t("Loading…")}</p></div>;
}
