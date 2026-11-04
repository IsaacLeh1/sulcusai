// SPDX-License-Identifier: AGPL-3.0-only
import type { Project } from "../api";
import { t } from "../i18n";
import { BookIcon } from "../components/Icons";
import type { View } from "../App";

/** The Projects list in the sidebar, with its new-project button. */
export function SidebarProjects({ projects, view, activeProject, onNew, onOpen, onMenu }: {
  projects: Project[];
  view: View;
  activeProject: string | null;
  onNew: () => void;
  onOpen: (id: string) => void;
  onMenu: (m: { p: Project; x: number; y: number }) => void;
}) {
  return (
    <div className="side-section">
      <div className="side-head">
        <span>{t("Projects")}</span>
        <button className="icon-btn" title={t("New project")} aria-label={t("New project")} onClick={onNew}>+</button>
      </div>
      {projects.map((p) => (
        <button
          key={p.id}
          className={`chat-item ${view === "project" && activeProject === p.id ? "active" : ""}`}
          onClick={() => onOpen(p.id)}
          onContextMenu={(e) => {
            e.preventDefault();
            onMenu({ p, x: e.clientX, y: e.clientY });
          }}
        >
          <BookIcon />
          <span className="ellipsis">{p.name}</span>
        </button>
      ))}
    </div>
  );
}
