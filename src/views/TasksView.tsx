// SPDX-License-Identifier: AGPL-3.0-only
// Tasks: a to-do list with due dates, priorities, subtasks and reminders.
import { useEffect, useMemo, useState } from "react";
import { api, errorText, on, type ScheduleView, type Task } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";
import { BellIcon } from "../components/Icons";

type List = "today" | "upcoming" | "anytime" | "done";
const PRIORITY = [tx("None"), tx("Low"), tx("Medium"), tx("High")];

function startOfTomorrow(): number {
  const d = new Date();
  d.setHours(24, 0, 0, 0);
  return d.getTime();
}

export function dueLabel(task: Task): string {
  if (task.due === null) return "";
  const d = new Date(task.due);
  const today = new Date();
  const days = Math.round((new Date(d).setHours(0, 0, 0, 0) - new Date(today).setHours(0, 0, 0, 0)) / 86_400_000);
  const day = days === 0 ? t("Today") : days === 1 ? t("Tomorrow") : days === -1 ? t("Yesterday") : d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  return task.due_has_time ? `${day} ${d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}` : day;
}

function blank(title: string): Task {
  return { id: "", title, notes: "", subtasks: [], source: null, due: null, due_has_time: false, priority: 0, remind_at: null, done_at: null, created_at: 0 };
}

/** What a timed action may do by itself, as its approval card said it. */
const ACTION_WORDS: Record<string, string> = {
  message_person: tx("send a People message"),
  ask_person: tx("ask someone's assistant"),
  email_send: tx("send an email"),
  calendar_create_event: tx("add a calendar event"),
  create_task: tx("add a task"),
  complete_task: tx("mark a task done"),
  create_note: tx("write a note"),
  create_music: tx("make a song"),
  create_image: tx("make a picture"),
  create_video: tx("make a video"),
  message_chat: tx("ask another chat"),
  write_file: tx("write files in its folders"),
  edit_file: tx("change files in its folders"),
  run_command: tx("run commands"),
};

/** One-time actions set for a time ("message Josh at midnight: happy
 * birthday"): waiting to run, or done. They run by themselves while
 * SulcusAI is open, doing what was approved when they were set up. */
function TimedActions({ done, onOpenChat, toast }: { done: boolean; onOpenChat: (id: string) => void; toast: PushToast }) {
  const [all, setAll] = useState<ScheduleView[]>([]);
  const load = () =>
    api
      .schedules()
      .then((s) => setAll(s.filter((x) => x.cadence.kind === "once")))
      .catch(() => {});
  useEffect(() => {
    load();
    const sub = on("schedule:ran", () => load());
    return () => {
      sub.then((un) => un());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const act = async (f: () => Promise<unknown>) => {
    try {
      await f();
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const shown = all.filter((s) => (done ? !s.enabled && s.last_run !== null : s.enabled));
  if (shown.length === 0) return null;
  return (
    <section className="timed-actions">
      <h2 className="small">{done ? t("Done at a set time") : t("At a set time")}</h2>
      <ul className="task-list">
        {shown.map((s) => (
          <li key={s.id} className="timed-item">
            <div className="timed-text">
              <strong>{s.name}</strong>
              <span className="muted small block">
                {s.when}
                {s.actions.length > 0 && ` · ${t("may: {what}", { what: s.actions.map((a) => t(ACTION_WORDS[a] ?? a)).join(", ") })}`}
              </span>
              <span className="small block">{s.prompt}</span>
            </div>
            <div className="row">
              {done ? (
                s.last_chat && (
                  <button className="link small" onClick={() => onOpenChat(s.last_chat!)}>
                    {t("See what it did")}
                  </button>
                )
              ) : (
                <>
                  <button
                    className="btn ghost small"
                    onClick={() => act(() => api.runScheduleNow(s.id))}
                  >
                    {t("Do it now")}
                  </button>
                  <button className="btn ghost small danger" onClick={() => window.confirm(t("Cancel “{name}”? It won't run.", { name: s.name })) && act(() => api.deleteSchedule(s.id))}>
                    {t("Cancel")}
                  </button>
                </>
              )}
            </div>
          </li>
        ))}
      </ul>
      {!done && <p className="muted small">{t("These run by themselves at their time while SulcusAI is open (if it's closed then, as soon as it opens), and each is done once it has run.")}</p>}
    </section>
  );
}

export function TasksView({ onOpenChat, toast }: { onOpenChat: (id: string) => void; toast: PushToast }) {
  const [tasks, setTasks] = useState<Task[] | null>(null);
  const [list, setList] = useState<List>("today");
  const [quick, setQuick] = useState("");
  const [editing, setEditing] = useState<Task | null>(null);

  const load = () => api.tasks().then(setTasks).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    const subs = [on("task:reminder", () => load()), on("sync:changed", () => load())];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const groups = useMemo(() => {
    const tomorrow = startOfTomorrow();
    const open = (tasks ?? []).filter((x) => x.done_at === null);
    return {
      today: open.filter((x) => x.due !== null && x.due < tomorrow),
      upcoming: open.filter((x) => x.due !== null && x.due >= tomorrow),
      anytime: open.filter((x) => x.due === null),
      done: (tasks ?? []).filter((x) => x.done_at !== null).sort((a, b) => (b.done_at ?? 0) - (a.done_at ?? 0)),
    };
  }, [tasks]);

  /** Saves the task; false (and the error shown) if it didn't save. */
  const save = async (task: Task) => {
    try {
      await api.saveTask(task);
      load();
      return true;
    } catch (e) {
      toast(errorText(e), "error");
      return false;
    }
  };

  // "Call Sam Friday 3pm": a due date at the end is read and taken off.
  const addQuick = async () => {
    const text = quick.trim();
    // Nothing typed: the full form for a new task (the button was greyed out
    // until something was typed, and looked broken).
    if (!text) {
      setEditing(blank(""));
      return;
    }
    const task = blank(text);
    const words = text.split(/\s+/);
    for (let k = Math.max(1, words.length - 4); k < words.length; k++) {
      const due = await api.parseDue(words.slice(k).join(" "));
      if (due) {
        task.title = words.slice(0, k).join(" ");
        task.due = due.due;
        task.due_has_time = due.has_time;
        break;
      }
    }
    if (list === "today" && task.due === null) {
      const d = new Date();
      d.setHours(0, 0, 0, 0);
      task.due = d.getTime();
    }
    setQuick("");
    // Not saved: the text comes back to try again.
    if (!(await save(task))) setQuick((q) => q || text);
  };

  const shown = groups[list];
  const now = Date.now();
  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>{t("Tasks")}</h1>
            <p className="muted">{t("Your to-do list, on this PC. The assistant can add tasks for you, and meeting action items can become tasks.")}</p>
          </div>
        </header>
        <div className="mode-switch" role="tablist" aria-label={t("Task lists")}>
          {(["today", "upcoming", "anytime", "done"] as List[]).map((l) => (
            <button key={l} role="tab" aria-selected={list === l} className={`mode ${list === l ? "active" : ""}`} onClick={() => setList(l)}>
              {l === "today" ? t("Today") : l === "upcoming" ? t("Upcoming") : l === "anytime" ? t("Anytime") : t("Done")}
              {l !== "done" && groups[l].length > 0 && ` (${groups[l].length})`}
            </button>
          ))}
        </div>
        <TimedActions done={list === "done"} onOpenChat={onOpenChat} toast={toast} />
        {list !== "done" && (
          <form
            className="row quick-add"
            onSubmit={(e) => {
              e.preventDefault();
              addQuick();
            }}
          >
            <input className="input" value={quick} onChange={(e) => setQuick(e.target.value)} placeholder={t("Add a task, e.g. “Send the budget Friday 3pm”")} aria-label={t("New task")} />
            <button className="btn primary" type="submit" title={quick.trim() ? undefined : t("Add a task with a due date, notes and steps")}>{t("Add")}</button>
          </form>
        )}
        <ul className="task-list">
          {tasks && shown.length === 0 && <li className="muted small">{list === "done" ? t("Nothing finished yet.") : t("Nothing here.")}</li>}
          {shown.map((task) => (
            <li key={task.id} className={`task ${task.done_at ? "done" : ""}`}>
              <input
                type="checkbox"
                checked={task.done_at !== null}
                onChange={(e) => save({ ...task, done_at: e.target.checked ? Date.now() : null })}
                aria-label={t("Done: {title}", { title: task.title })}
              />
              <button className="task-main" onClick={() => setEditing(task)}>
                <span className="task-title">
                  {task.priority > 0 && <span className={`prio p${task.priority}`} title={t("{priority} priority", { priority: t(PRIORITY[task.priority]) })}>{"!".repeat(task.priority)}</span>}
                  {task.title}
                </span>
                <span className="small muted">
                  {task.due !== null && <span className={task.due < now && !task.done_at && !(!task.due_has_time && task.due >= new Date().setHours(0, 0, 0, 0)) ? "overdue" : ""}>{dueLabel(task)}</span>}
                  {task.remind_at !== null && !task.done_at && <> · <BellIcon title={t("Has a reminder")} /></>}
                  {task.subtasks.length > 0 && ` · ${task.subtasks.filter((s) => s.done).length}/${task.subtasks.length}`}
                  {task.source && ` · ${t("from {label}", { label: task.source.label })}`}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </div>
      {editing && (
        <TaskEditor
          task={editing}
          onClose={() => setEditing(null)}
          onSave={async (task) => {
            // The editor stays open when it didn't save, so nothing typed is lost.
            if (await save(task)) setEditing(null);
          }}
          onDelete={async () => {
            // A new task that was never saved: nothing to delete.
            if (!editing.id) return setEditing(null);
            try {
              await api.deleteTask(editing.id);
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

function toLocalInput(ms: number | null, withTime: boolean): string {
  if (ms === null) return "";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  const date = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  return withTime ? `${date}T${pad(d.getHours())}:${pad(d.getMinutes())}` : date;
}

function TaskEditor({ task, onClose, onSave, onDelete }: { task: Task; onClose: () => void; onSave: (task: Task) => void; onDelete: () => void }) {
  const [draft, setDraft] = useState<Task>(task);
  const [withTime, setWithTime] = useState(task.due_has_time);
  const [sub, setSub] = useState("");
  const setDue = (v: string) => {
    if (!v) return setDraft({ ...draft, due: null, due_has_time: false });
    const d = new Date(withTime ? v : `${v}T00:00`);
    setDraft({ ...draft, due: d.getTime(), due_has_time: withTime });
  };
  return (
    <Modal title={t("Task")} onClose={onClose}>
      <div className="form">
        <input className="input" value={draft.title} onChange={(e) => setDraft({ ...draft, title: e.target.value })} aria-label={t("Task")} autoFocus />
        <div className="two-col">
          <label>
            {t("Due")}
            <input className="input" type={withTime ? "datetime-local" : "date"} value={toLocalInput(draft.due, withTime)} onChange={(e) => setDue(e.target.value)} />
          </label>
          <label>
            {t("Priority")}
            <select value={draft.priority} onChange={(e) => setDraft({ ...draft, priority: Number(e.target.value) })}>
              {PRIORITY.map((p, i) => <option key={p} value={i}>{t(p)}</option>)}
            </select>
          </label>
        </div>
        <label className="check">
          <input type="checkbox" checked={withTime} onChange={(e) => { setWithTime(e.target.checked); setDraft({ ...draft, due_has_time: e.target.checked }); }} />
          <span>{t("At a specific time")}</span>
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.remind_at !== null}
            disabled={draft.due === null}
            onChange={(e) => {
              if (!e.target.checked || draft.due === null) return setDraft({ ...draft, remind_at: null });
              const at = new Date(draft.due);
              if (!draft.due_has_time) at.setHours(9, 0, 0, 0);
              setDraft({ ...draft, remind_at: at.getTime() });
            }}
          />
          <span>{draft.due === null ? t("Remind me (set a due date first)") : draft.due_has_time ? t("Remind me at that time") : t("Remind me at 9 AM that day")}</span>
        </label>
        <label>
          {t("Notes")}
          <textarea className="input" rows={3} value={draft.notes} onChange={(e) => setDraft({ ...draft, notes: e.target.value })} />
        </label>
        <div>
          <strong className="small">{t("Subtasks")}</strong>
          <ul className="subtasks">
            {draft.subtasks.map((s, i) => (
              <li key={i}>
                <label className="check">
                  <input type="checkbox" checked={s.done} onChange={(e) => setDraft({ ...draft, subtasks: draft.subtasks.map((x, j) => (j === i ? { ...x, done: e.target.checked } : x)) })} />
                  <span>{s.title}</span>
                </label>
                <button className="link small" onClick={() => setDraft({ ...draft, subtasks: draft.subtasks.filter((_, j) => j !== i) })}>{t("Remove")}</button>
              </li>
            ))}
          </ul>
          <form
            className="row"
            onSubmit={(e) => {
              e.preventDefault();
              if (sub.trim()) setDraft({ ...draft, subtasks: [...draft.subtasks, { title: sub.trim(), done: false }] });
              setSub("");
            }}
          >
            <input className="input" value={sub} onChange={(e) => setSub(e.target.value)} placeholder={t("Add a step")} aria-label={t("New subtask")} />
            <button className="btn small" type="submit">{t("Add")}</button>
          </form>
        </div>
        {draft.source && <p className="muted small">{t("From {kind}: {label}", { kind: draft.source.kind, label: draft.source.label })}</p>}
      </div>
      <div className="modal-actions">
        {task.id && <button className="btn ghost danger" onClick={onDelete}>{t("Delete")}</button>}
        <span className="spacer" />
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={() => onSave(draft)} disabled={!draft.title.trim()}>{t("Save")}</button>
      </div>
    </Modal>
  );
}
