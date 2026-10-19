// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { api, errorText, type Action } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

const CATEGORIES: { id: string; label: string }[] = [
  { id: "all", label: tx("Everything") },
  { id: "network", label: tx("Left this PC") },
  { id: "model", label: tx("Models") },
  { id: "privacy", label: tx("Privacy") },
  { id: "security", label: tx("Security") },
  { id: "chat", label: tx("Chats") },
  { id: "people", label: tx("People") },
];

const PERIODS: { id: string; label: string; days: number | null }[] = [
  { id: "day", label: tx("Today"), days: 0 },
  { id: "week", label: tx("Past week"), days: 7 },
  { id: "month", label: tx("Past month"), days: 30 },
  { id: "all", label: tx("All time"), days: null },
];

/** Where a period starts: today from midnight, the others counted back from now. */
function periodStart(days: number | null): number | undefined {
  if (days === null) return undefined;
  if (days === 0) {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  }
  return Date.now() - days * 86_400_000;
}

function when(ms: number) {
  return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function ActivityView({ toast }: { toast: PushToast }) {
  const [actions, setActions] = useState<Action[] | null>(null);
  const [filter, setFilter] = useState("all");
  const [period, setPeriod] = useState("week");
  const [query, setQuery] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);

  const load = () => {
    const days = PERIODS.find((p) => p.id === period)?.days ?? null;
    api.actions(days === null ? 5000 : 2000, periodStart(days)).then(setActions).catch((e) => toast(errorText(e), "error"));
  };
  useEffect(() => {
    setActions(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [period]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (actions ?? []).filter((a) => (filter === "all" || a.category === filter) && (!q || a.summary.toLowerCase().includes(q)));
  }, [actions, filter, query]);

  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>{t("Activity")}</h1>
            <p className="muted">{t("Everything SulcusAI has done on its own, including anything that went over the internet. It never records what you say in chats.")}</p>
          </div>
        </header>

        <div className="mode-switch activity-period" role="group" aria-label={t("Period")}>
          {PERIODS.map((p) => (
            <button key={p.id} className={`mode ${period === p.id ? "active" : ""}`} aria-pressed={period === p.id} onClick={() => setPeriod(p.id)}>
              {t(p.label)}
            </button>
          ))}
        </div>

        <div className="filters">
          {CATEGORIES.map((c) => (
            <button key={c.id} className={`chip ${filter === c.id ? "active" : ""}`} onClick={() => setFilter(c.id)}>
              {t(c.label)}
            </button>
          ))}
          <input className="input search" placeholder={t("Search activity")} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t("Search activity")} />
        </div>

        <section className="card activity">
          {actions === null && <p className="muted">{t("Loading…")}</p>}
          {actions !== null && shown.length === 0 && <p className="muted">{period === "all" ? t("Nothing here yet.") : t("Nothing in this period.")}</p>}
          {actions !== null && shown.length > 0 && <p className="muted small">{shown.length === 1 ? t("1 entry") : t("{n} entries", { n: shown.length })}</p>}
          <ol className="activity-list">
            {shown.map((a) => (
              <li key={a.id}>
                <span className="activity-summary">{a.summary}</span>
                <time className="muted small" dateTime={new Date(a.at).toISOString()}>{when(a.at)}</time>
              </li>
            ))}
          </ol>
        </section>

        {actions && actions.length > 0 && (
          <button className="btn ghost danger" onClick={() => setConfirmClear(true)}>
            {t("Clear activity")}
          </button>
        )}
      </div>

      {confirmClear && (
        <Modal title={t("Clear the activity log?")} onClose={() => setConfirmClear(false)}>
          <p>{t("This permanently deletes the log on this PC. A new entry will note that it was cleared.")}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmClear(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                await api.clearActions();
                setConfirmClear(false);
                load();
              }}
            >
              {t("Clear")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
