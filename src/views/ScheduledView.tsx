// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Cadence, type ModelCard, type Project, type Schedule, type ScheduleView } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

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
            <h1>Scheduled</h1>
            <p className="muted">Tasks that run on their own while SulcusAI is open, such as a morning summary of a notes folder. Each run is saved as a chat.</p>
          </div>
          <button className="btn primary" onClick={() => setEditing(blank())} disabled={installed.length === 0}>+ New task</button>
        </header>

        <section className="card">
          {items === null && <p className="muted">Loading…</p>}
          {items?.length === 0 && <p className="muted">No scheduled tasks yet.</p>}
          {items?.map((s) => (
            <div key={s.id} className="setting-row schedule-row">
              <div>
                <strong>{s.name}</strong>
                {s.allow_changes && <span className="tag warn">May change files</span>}
                <span className="small muted block">
                  {s.when} · next {s.enabled ? when(s.next_run) : "paused"}
                  {s.last_run ? ` · last ${when(s.last_run)}` : ""}
                </span>
              </div>
              <div className="row">
                {s.last_chat && (
                  <button className="link" onClick={() => onOpenChat(s.last_chat!)}>Last result</button>
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
                  {runningId === s.id ? "Running…" : "Run now"}
                </button>
                <button className="btn ghost small" onClick={() => setEditing(s)}>Edit</button>
                <label className="switch" title={s.enabled ? "On" : "Paused"}>
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
                    aria-label={`${s.name} on or paused`}
                  />
                  <span />
                </label>
              </div>
            </div>
          ))}
        </section>
        <p className="muted small">Tasks run only while the app is open and unlocked. A run that was missed while it was closed happens the next time it opens.</p>
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
    <Modal title={initial.id ? "Edit scheduled task" : "New scheduled task"} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave(s);
        }}
      >
        <label>
          Name
          <input className="input" value={s.name} onChange={(e) => setS({ ...s, name: e.target.value })} placeholder="Morning summary" maxLength={80} autoFocus />
        </label>
        <label>
          What to do
          <textarea
            className="input"
            rows={4}
            value={s.prompt}
            onChange={(e) => setS({ ...s, prompt: e.target.value })}
            placeholder="Read the files in notes/ that changed this week and summarize them in five bullet points."
          />
        </label>
        <div className="row wrap">
          <label>
            How often
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
              <option value="once">Once</option>
              <option value="hourly">Every hour</option>
              <option value="daily">Every day</option>
              <option value="weekdays">Weekdays</option>
              <option value="weekly">Every week</option>
            </select>
          </label>
          {c.kind === "weekly" && (
            <label>
              Day
              <select value={c.weekday} onChange={(e) => setCadence({ ...c, weekday: Number(e.target.value) })}>
                {DAYS.map((d, i) => (
                  <option key={d} value={i}>{d}</option>
                ))}
              </select>
            </label>
          )}
          {c.kind === "hourly" && (
            <label>
              Minute
              <input className="input" type="number" min={0} max={59} value={c.minute} onChange={(e) => setCadence({ kind: "hourly", minute: Number(e.target.value) })} />
            </label>
          )}
          {(c.kind === "daily" || c.kind === "weekdays" || c.kind === "weekly") && (
            <label>
              Time
              <input className="input" type="time" value={time} onChange={(e) => setCadence({ ...c, time: e.target.value })} />
            </label>
          )}
          {c.kind === "once" && (
            <label>
              When
              <input className="input" type="datetime-local" value={onceValue} onChange={(e) => setCadence({ kind: "once", at: new Date(e.target.value).getTime() })} />
            </label>
          )}
        </div>
        <div className="row wrap">
          <label>
            Model
            <select value={s.model_id ?? ""} onChange={(e) => setS({ ...s, model_id: e.target.value || null })}>
              <option value="">Default model</option>
              {installed.map((m) => (
                <option key={m.id} value={m.id}>{m.name}</option>
              ))}
            </select>
          </label>
          <label>
            Project
            <select value={s.project_id ?? ""} onChange={(e) => setS({ ...s, project_id: e.target.value || null })}>
              <option value="">None</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
          </label>
        </div>
        <label className="check">
          <input type="checkbox" checked={s.allow_changes} onChange={(e) => setS({ ...s, allow_changes: e.target.checked })} />
          Let it change files and run commands without asking
        </label>
        <p className="muted small">
          {s.allow_changes
            ? "It runs in Bypass mode. File changes can be undone from its chat; commands can't."
            : "It can only read files in shared folders and report back. Nobody is there to approve changes."}
        </p>
        <div className="modal-actions">
          {onDelete && <button type="button" className="btn ghost danger" onClick={onDelete}>Delete</button>}
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn primary" disabled={!s.name.trim() || !s.prompt.trim()}>Save</button>
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
