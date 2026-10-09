// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Cadence, type ModelCard, type Project, type Schedule, type ScheduleView } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

const DAYS = [tx("Monday"), tx("Tuesday"), tx("Wednesday"), tx("Thursday"), tx("Friday"), tx("Saturday"), tx("Sunday")];

function when(ms: number | null) {
  return ms ? new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "—";
}

interface Props {
  installed: ModelCard[];
  onOpenChat: (id: string) => void;
  toast: PushToast;
}

export function ScheduledView({ installed, onOpenChat, toast }: Props) {
  const [items, setItems] = useState<ScheduleView[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [editing, setEditing] = useState<Schedule | null>(null);
  const [runningId, setRunningId] = useState<string | null>(null);

  const load = () => api.schedules().then(setItems).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    api.projects().then(setProjects).catch(() => {});
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const blank = (): Schedule => ({
    id: "",
    name: "",
    prompt: "",
    cadence: { kind: "daily", time: "08:00" },
    model_id: null,
    project_id: null,
    allow_changes: false,
    enabled: true,
    last_run: null,
    next_run: null,
    last_chat: null,
  });

  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>{t("Scheduled")}</h1>
            <p className="muted">{t("Tasks that run on their own while SulcusAI is open, such as a morning summary of a notes folder. Each run is saved as a chat.")}</p>
          </div>
          <button className="btn primary" onClick={() => setEditing(blank())} disabled={installed.length === 0}>{t("+ New task")}</button>
        </header>

        <section className="card">
          {items === null && <p className="muted">{t("Loading…")}</p>}
          {items?.length === 0 && <p className="muted">{t("No scheduled tasks yet.")}</p>}
          {items?.map((s) => (
            <div key={s.id} className="setting-row schedule-row">
              <div>
                <strong>{s.name}</strong>
                {s.allow_changes && <span className="tag warn">{t("May change files")}</span>}
                <span className="small muted block">
                  {s.when} · {s.enabled ? t("next {time}", { time: when(s.next_run) }) : t("next paused")}
                  {s.last_run ? ` · ${t("last {time}", { time: when(s.last_run) })}` : ""}
                </span>
              </div>
              <div className="row">
                {s.last_chat && (
                  <button className="link" onClick={() => onOpenChat(s.last_chat!)}>{t("Last result")}</button>
                )}
                <button
                  className="btn small"
                  disabled={runningId !== null}
                  onClick={async () => {
                    setRunningId(s.id);
                    try {
                      const chatId = await api.runScheduleNow(s.id);
                      load();
                      onOpenChat(chatId);
                    } catch (e) {
                      toast(errorText(e), "error");
                    } finally {
                      setRunningId(null);
                    }
                  }}
                >
                  {runningId === s.id ? t("Running…") : t("Run now")}
                </button>
                <button className="btn ghost small" onClick={() => setEditing(s)}>{t("Edit")}</button>
                <label className="switch" title={s.enabled ? t("On") : t("Paused")}>
                  <input
                    type="checkbox"
                    checked={s.enabled}
                    onChange={async (e) => {
                      try {
                        await api.saveSchedule({ ...s, enabled: e.target.checked });
                        load();
                      } catch (err) {
                        toast(errorText(err), "error");
                      }
                    }}
                    aria-label={t("{name} on or paused", { name: s.name })}
                  />
                  <span />
                </label>
              </div>
            </div>
          ))}
        </section>
        <p className="muted small">{t("Tasks run only while the app is open and unlocked. A run that was missed while it was closed happens the next time it opens.")}</p>
      </div>
      {editing && (
        <ScheduleEditor
          initial={editing}
          installed={installed}
          projects={projects}
          onClose={() => setEditing(null)}
          onDelete={
            editing.id
              ? async () => {
                  await api.deleteSchedule(editing.id);
                  setEditing(null);
                  load();
                }
              : undefined
          }
          onSave={async (s) => {
            try {
              await api.saveSchedule(s);
              setEditing(null);
              load();
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        />
      )}
    </div>
  );
}

function ScheduleEditor({
  initial,
  installed,
  projects,
  onClose,
  onSave,
  onDelete,
}: {
  initial: Schedule;
  installed: ModelCard[];
  projects: Project[];
  onClose: () => void;
  onSave: (s: Schedule) => void;
  onDelete?: () => void;
}) {
  const [s, setS] = useState<Schedule>(initial);
  const c = s.cadence;
  const time = "time" in c ? c.time : "08:00";
  const setCadence = (next: Cadence) => setS({ ...s, cadence: next });
  const onceValue = c.kind === "once" ? toLocalInput(c.at) : toLocalInput(Date.now() + 3_600_000);

  return (
    <Modal title={initial.id ? t("Edit scheduled task") : t("New scheduled task")} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave(s);
        }}
      >
        <label>
          {t("Name")}
          <input className="input" value={s.name} onChange={(e) => setS({ ...s, name: e.target.value })} placeholder={t("Morning summary")} maxLength={80} autoFocus />
        </label>
        <label>
          {t("What to do")}
          <textarea
            className="input"
            rows={4}
            value={s.prompt}
            onChange={(e) => setS({ ...s, prompt: e.target.value })}
            placeholder={t("Read the files in notes/ that changed this week and summarize them in five bullet points.")}
          />
        </label>
        <div className="row wrap">
          <label>
            {t("How often")}
            <select
              value={c.kind}
              onChange={(e) => {
                const k = e.target.value as Cadence["kind"];
                if (k === "once") setCadence({ kind: "once", at: Date.now() + 3_600_000 });
                else if (k === "hourly") setCadence({ kind: "hourly", minute: 0 });
                else if (k === "weekly") setCadence({ kind: "weekly", weekday: 0, time });
                else setCadence({ kind: k, time });
              }}
            >
              <option value="once">{t("Once")}</option>
              <option value="hourly">{t("Every hour")}</option>
              <option value="daily">{t("Every day")}</option>
              <option value="weekdays">{t("Weekdays")}</option>
              <option value="weekly">{t("Every week")}</option>
            </select>
          </label>
          {c.kind === "weekly" && (
            <label>
              {t("Day")}
              <select value={c.weekday} onChange={(e) => setCadence({ ...c, weekday: Number(e.target.value) })}>
                {DAYS.map((d, i) => (
                  <option key={d} value={i}>{t(d)}</option>
                ))}
              </select>
            </label>
          )}
          {c.kind === "hourly" && (
            <label>
              {t("Minute")}
              <input className="input" type="number" min={0} max={59} value={c.minute} onChange={(e) => setCadence({ kind: "hourly", minute: Number(e.target.value) })} />
            </label>
          )}
          {(c.kind === "daily" || c.kind === "weekdays" || c.kind === "weekly") && (
            <label>
              {t("Time")}
              <input className="input" type="time" value={time} onChange={(e) => setCadence({ ...c, time: e.target.value })} />
            </label>
          )}
          {c.kind === "once" && (
            <label>
              {t("When")}
              <input className="input" type="datetime-local" value={onceValue} onChange={(e) => setCadence({ kind: "once", at: new Date(e.target.value).getTime() })} />
            </label>
          )}
        </div>
        <div className="row wrap">
          <label>
            {t("Model")}
            <select value={s.model_id ?? ""} onChange={(e) => setS({ ...s, model_id: e.target.value || null })}>
              <option value="">{t("Default model")}</option>
              {installed.map((m) => (
                <option key={m.id} value={m.id}>{m.name}</option>
              ))}
            </select>
          </label>
          <label>
            {t("Project")}
            <select value={s.project_id ?? ""} onChange={(e) => setS({ ...s, project_id: e.target.value || null })}>
              <option value="">{t("None")}</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
          </label>
        </div>
        <label className="check">
          <input type="checkbox" checked={s.allow_changes} onChange={(e) => setS({ ...s, allow_changes: e.target.checked })} />
          {t("Let it change files and run commands without asking")}
        </label>
        <p className="muted small">
          {s.allow_changes
            ? t("It runs in Bypass mode. File changes can be undone from its chat; commands can't.")
            : t("It can only read files in shared folders and report back. Nobody is there to approve changes.")}
        </p>
        <div className="modal-actions">
          {onDelete && <button type="button" className="btn ghost danger" onClick={onDelete}>{t("Delete")}</button>}
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!s.name.trim() || !s.prompt.trim()}>{t("Save")}</button>
        </div>
      </form>
    </Modal>
  );
}

function toLocalInput(ms: number) {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
