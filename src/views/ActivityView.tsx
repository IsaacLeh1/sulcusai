// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { api, errorText, type Action } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

const CATEGORIES: { id: string; label: string; icon: string }[] = [
  { id: "all", label: "Everything", icon: "" },
  { id: "network", label: "Left this PC", icon: "🌐" },
  { id: "model", label: "Models", icon: "🧠" },
  { id: "privacy", label: "Privacy", icon: "🛡" },
  { id: "security", label: "Security", icon: "🔐" },
  { id: "chat", label: "Chats", icon: "💬" },
];

function icon(category: string) {
  return CATEGORIES.find((c) => c.id === category)?.icon || "•";
}

function when(ms: number) {
  return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function ActivityView({ toast }: { toast: PushToast }) {
  const [actions, setActions] = useState<Action[] | null>(null);
  const [filter, setFilter] = useState("all");
  const [query, setQuery] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);

  const load = () => api.actions(1000).then(setActions).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (actions ?? []).filter((a) => (filter === "all" || a.category === filter) && (!q || a.summary.toLowerCase().includes(q)));
  }, [actions, filter, query]);

  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>Activity</h1>
            <p className="muted">Everything SulcusAI has done on its own, including anything that went over the internet. It never records what you say in chats.</p>
          </div>
        </header>

        <div className="filters">
          {CATEGORIES.map((c) => (
            <button key={c.id} className={`chip ${filter === c.id ? "active" : ""}`} onClick={() => setFilter(c.id)}>
              {c.icon && <span aria-hidden>{c.icon} </span>}
              {c.label}
            </button>
          ))}
          <input className="input search" placeholder="Search activity" value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search activity" />
        </div>

        <section className="card activity">
          {actions === null && <p className="muted">Loading…</p>}
          {actions !== null && shown.length === 0 && <p className="muted">Nothing here yet.</p>}
          <ol className="activity-list">
            {shown.map((a) => (
              <li key={a.id}>
                <span className="activity-icon" aria-hidden>{icon(a.category)}</span>
                <span className="activity-summary">{a.summary}</span>
                <time className="muted small" dateTime={new Date(a.at).toISOString()}>{when(a.at)}</time>
              </li>
            ))}
          </ol>
        </section>

        {actions && actions.length > 0 && (
          <button className="btn ghost danger" onClick={() => setConfirmClear(true)}>
            Clear activity
          </button>
        )}
      </div>

      {confirmClear && (
        <Modal title="Clear the activity log?" onClose={() => setConfirmClear(false)}>
          <p>This permanently deletes the log on this PC. A new entry will note that it was cleared.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmClear(false)}>Cancel</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                await api.clearActions();
                setConfirmClear(false);
                load();
              }}
            >
              Clear
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
