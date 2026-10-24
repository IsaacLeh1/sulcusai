// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type Cadence, type ModelCard, type Project, type Schedule, type ScheduleView } from "../api";
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
    // A task ran on its own: its last run, next run and result change.
    const sub = on("schedule:ran", () => load());
    return () => {
      sub.then((un) => un());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const blank = (): Schedule => ({
    id: "",
    name: "",
    prompt: "",
    cadence: { kind: "days", weekdays: [0, 1, 2, 3, 4], times: ["08:00"] },
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
        <details className="card how-it-runs">
          <summary>{t("How tasks run without a model always loaded")}</summary>
          <ul className="small">
            <li>{t("SulcusAI looks at the list every 30 seconds while it's open, in a window or as the tray icon. That takes almost nothing: no model is loaded for it.")}</li>
            <li>{t("When a task is due, the model it uses loads, does the task and saves what it wrote as a chat, and you get a note. The model unloads again once it has been idle for a while (you set how long in Settings, Performance).")}</li>
            <li>{t("If the PC was off or SulcusAI was closed when a task was due, it runs once when SulcusAI starts again. A task waits while a chat is answering, then runs.")}</li>
            <li>{t("While SulcusAI is locked, tasks run only if “Keep working while locked” is on (Settings, Privacy & security).")}</li>
          </ul>
        </details>

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
  // Every day, weekdays and one weekday open as chosen days, the form's own.
  const [s, setS] = useState<Schedule>(() => ({ ...initial, cadence: asDays(initial.cadence) }));
  const c = s.cadence;
  const setCadence = (next: Cadence) => setS({ ...s, cadence: next });
  const onceValue = c.kind === "once" ? toLocalInput(c.at) : toLocalInput(Date.now() + 3_600_000);
  const times = c.kind === "days" || c.kind === "dates" ? c.times : ["08:00"];
  const setTimes = (next: string[]) => (c.kind === "days" || c.kind === "dates") && setCadence({ ...c, times: next });
  const [spread, setSpread] = useState({ n: 3, from: "08:00", to: "20:00" });
  const [newDate, setNewDate] = useState(() => new Date().toISOString().slice(0, 10));

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
          <input className="input" value={s.name} onChange={(e) => setS({ ...s, name: e.target.value })} placeholder={t("Morning summary")} aria-label={t("Morning summary")} maxLength={80} autoFocus />
        </label>
        <label>
          {t("What to do")}
          <textarea
            className="input"
            rows={4}
            value={s.prompt}
            onChange={(e) => setS({ ...s, prompt: e.target.value })}
            placeholder={t("Read the files in notes/ that changed this week and summarize them in five bullet points.")} aria-label={t("Read the files in notes/ that changed this week and summarize them in five bullet points.")}
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
                else if (k === "dates") setCadence({ kind: "dates", dates: [], times });
                else setCadence({ kind: "days", weekdays: [0, 1, 2, 3, 4], times });
              }}
            >
              <option value="days">{t("On days of the week")}</option>
              <option value="dates">{t("On dates I pick")}</option>
              <option value="hourly">{t("Every hour")}</option>
              <option value="once">{t("Once")}</option>
            </select>
          </label>
          {c.kind === "hourly" && (
            <label>
              {t("Minute")}
              <input className="input" type="number" min={0} max={59} value={c.minute} onChange={(e) => setCadence({ kind: "hourly", minute: Number(e.target.value) })} />
            </label>
          )}
          {c.kind === "once" && (
            <label>
              {t("When")}
              <input className="input" type="datetime-local" value={onceValue} onChange={(e) => setCadence({ kind: "once", at: new Date(e.target.value).getTime() })} />
            </label>
          )}
        </div>
        {c.kind === "days" && (
          <fieldset className="schedule-part">
            <legend>{t("Days")}</legend>
            <div className="row wrap" role="group" aria-label={t("Days")}>
              {DAYS.map((d, i) => (
                <button
                  key={d}
                  type="button"
                  className={`chip ${c.weekdays.includes(i) ? "active" : ""}`}
                  aria-pressed={c.weekdays.includes(i)}
                  onClick={() => setCadence({ ...c, weekdays: c.weekdays.includes(i) ? c.weekdays.filter((w) => w !== i) : [...c.weekdays, i].sort() })}
                >
                  {t(d).slice(0, 3)}
                </button>
              ))}
            </div>
            <div className="row wrap small">
              <button type="button" className="link" onClick={() => setCadence({ ...c, weekdays: [0, 1, 2, 3, 4, 5, 6] })}>{t("Every day")}</button>
              <button type="button" className="link" onClick={() => setCadence({ ...c, weekdays: [0, 1, 2, 3, 4] })}>{t("Weekdays")}</button>
              <button type="button" className="link" onClick={() => setCadence({ ...c, weekdays: [5, 6] })}>{t("Weekends")}</button>
            </div>
          </fieldset>
        )}
        {c.kind === "dates" && (
          <fieldset className="schedule-part">
            <legend>{t("Dates")}</legend>
            <div className="row wrap">
              {[...c.dates].sort().map((d) => (
                <span key={d} className="chip active">
                  {new Date(`${d}T12:00:00`).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}
                  <button type="button" className="chip-x" aria-label={t("Remove {date}", { date: d })} onClick={() => setCadence({ ...c, dates: c.dates.filter((x) => x !== d) })}>
                    ×
                  </button>
                </span>
              ))}
            </div>
            <div className="row wrap">
              <input className="input" type="date" value={newDate} onChange={(e) => setNewDate(e.target.value)} aria-label={t("A date")} />
              <button type="button" className="btn small" disabled={!newDate || c.dates.includes(newDate)} onClick={() => setCadence({ ...c, dates: [...c.dates, newDate] })}>{t("Add this date")}</button>
            </div>
          </fieldset>
        )}
        {(c.kind === "days" || c.kind === "dates") && (
          <fieldset className="schedule-part">
            <legend>{t("Times of day")}</legend>
            <div className="row wrap">
              {times.map((tm, i) => (
                <span key={i} className="time-pick">
                  <input className="input" type="time" value={tm} aria-label={t("Time {n}", { n: i + 1 })} onChange={(e) => setTimes(times.map((x, j) => (j === i ? e.target.value : x)))} />
                  {times.length > 1 && (
                    <button type="button" className="chip-x" aria-label={t("Remove this time")} onClick={() => setTimes(times.filter((_, j) => j !== i))}>
                      ×
                    </button>
                  )}
                </span>
              ))}
              <button type="button" className="btn small" disabled={times.length >= 24} onClick={() => setTimes([...times, nextTime(times)])}>{t("+ Add a time")}</button>
            </div>
            <div className="row wrap small">
              <span>{t("Or")}</span>
              <input className="input narrow-num" type="number" min={1} max={24} value={spread.n} onChange={(e) => setSpread({ ...spread, n: Number(e.target.value) })} aria-label={t("Times a day")} />
              <span>{t("times a day, between")}</span>
              <input className="input" type="time" value={spread.from} onChange={(e) => setSpread({ ...spread, from: e.target.value })} aria-label={t("From")} />
              <span>{t("and")}</span>
              <input className="input" type="time" value={spread.to} onChange={(e) => setSpread({ ...spread, to: e.target.value })} aria-label={t("To")} />
              <button type="button" className="btn small" onClick={() => setTimes(spreadTimes(spread.n, spread.from, spread.to))}>{t("Spread them out")}</button>
            </div>
          </fieldset>
        )}
        <p className="small" role="status">{whenItRuns(c)}</p>
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

/** Every day, weekdays and one weekday as chosen days (the form shows those). */
export function asDays(c: Cadence): Cadence {
  switch (c.kind) {
    case "daily":
      return { kind: "days", weekdays: [0, 1, 2, 3, 4, 5, 6], times: [c.time] };
    case "weekdays":
      return { kind: "days", weekdays: [0, 1, 2, 3, 4], times: [c.time] };
    case "weekly":
      return { kind: "days", weekdays: [c.weekday], times: [c.time] };
    default:
      return c;
  }
}

const minutes = (hhmm: string) => {
  const [h, m] = hhmm.split(":").map(Number);
  return (h || 0) * 60 + (m || 0);
};
const hhmm = (mins: number) => `${String(Math.floor(mins / 60) % 24).padStart(2, "0")}:${String(Math.round(mins) % 60).padStart(2, "0")}`;

/** `n` times from `from` to `to`, evenly apart (one: at `from`). */
export function spreadTimes(n: number, from: string, to: string): string[] {
  const count = Math.max(1, Math.min(24, Math.round(n) || 1));
  const a = minutes(from);
  const b = Math.max(a, minutes(to));
  if (count === 1) return [hhmm(a)];
  return Array.from({ length: count }, (_, i) => hhmm(a + Math.round(((b - a) * i) / (count - 1))));
}

/** A time to add: an hour after the latest, or 08:00. */
function nextTime(times: string[]): string {
  if (times.length === 0) return "08:00";
  return hhmm(Math.min(23 * 60 + 59, Math.max(...times.map(minutes)) + 60));
}

/** When the task runs, in a sentence (or what's missing). */
export function whenItRuns(c: Cadence): string {
  const ts = (list: string[]) => {
    const sorted = [...new Set(list)].sort();
    return sorted.length > 1 ? t("{list} and {last}", { list: sorted.slice(0, -1).join(", "), last: sorted[sorted.length - 1] }) : sorted[0] ?? "";
  };
  switch (c.kind) {
    case "days": {
      if (c.weekdays.length === 0) return t("Pick at least one day.");
      if (c.times.length === 0) return t("Add at least one time.");
      const ws = [...new Set(c.weekdays)].sort();
      const days = ws.length === 7 ? t("every day") : ws.join() === "0,1,2,3,4" ? t("on weekdays") : ws.join() === "5,6" ? t("on weekends") : t("on {days}", { days: ws.map((w) => t(DAYS[w])).join(", ") });
      return t("Runs {days} at {times}: {n} times a week.", { days, times: ts(c.times), n: ws.length * new Set(c.times).size });
    }
    case "dates":
      if (c.dates.length === 0) return t("Pick at least one date.");
      if (c.times.length === 0) return t("Add at least one time.");
      return t("Runs on {n} date(s) at {times}, then stops.", { n: new Set(c.dates).size, times: ts(c.times) });
    case "hourly":
      return t("Runs every hour at :{minute}.", { minute: String(c.minute).padStart(2, "0") });
    case "once":
      return t("Runs once, then stops.");
    default:
      return "";
  }
}
