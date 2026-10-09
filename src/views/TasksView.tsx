// SPDX-License-Identifier: AGPL-3.0-only
// Tasks: a to-do list with due dates, priorities, subtasks and reminders.
import { useEffect, useMemo, useState } from "react";
import { api, errorText, on, type Task } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

type List = "today" | "upcoming" | "anytime" | "done";
const PRIORITY = ["None", "Low", "Medium", "High"];

function startOfTomorrow(): number {
  const d = new Date();
  d.setHours(24, 0, 0, 0);
  return d.getTime();
}

export function dueLabel(t: Task): string {
  if (t.due === null) return "";
  const d = new Date(t.due);
  const today = new Date();
  const days = Math.round((new Date(d).setHours(0, 0, 0, 0) - new Date(today).setHours(0, 0, 0, 0)) / 86_400_000);
  const day = days === 0 ? "Today" : days === 1 ? "Tomorrow" : days === -1 ? "Yesterday" : d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  return t.due_has_time ? `${day} ${d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}` : day;
}

function blank(title: string): Task {
  return { id: "", title, notes: "", subtasks: [], source: null, due: null, due_has_time: false, priority: 0, remind_at: null, done_at: null, created_at: 0 };
}

export function TasksView({ toast }: { toast: PushToast }) {
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
    const open = (tasks ?? []).filter((t) => t.done_at === null);
    return {
      today: open.filter((t) => t.due !== null && t.due < tomorrow),
      upcoming: open.filter((t) => t.due !== null && t.due >= tomorrow),
      anytime: open.filter((t) => t.due === null),
      done: (tasks ?? []).filter((t) => t.done_at !== null).sort((a, b) => (b.done_at ?? 0) - (a.done_at ?? 0)),
    };
  }, [tasks]);

  const save = async (t: Task) => {
    try {
      await api.saveTask(t);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  // "Call Sam Friday 3pm": a due date at the end is read and taken off.
  const addQuick = async () => {
    const text = quick.trim();
    if (!text) return;
    const t = blank(text);
    const words = text.split(/\s+/);
    for (let k = Math.max(1, words.length - 4); k < words.length; k++) {
      const due = await api.parseDue(words.slice(k).join(" "));
      if (due) {
        t.title = words.slice(0, k).join(" ");
        t.due = due.due;
        t.due_has_time = due.has_time;
        break;
      }
    }
    if (list === "today" && t.due === null) {
      const d = new Date();
      d.setHours(0, 0, 0, 0);
      t.due = d.getTime();
    }
    setQuick("");
    save(t);
  };

  const shown = groups[list];
  const now = Date.now();
  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>Tasks</h1>
            <p className="muted">Your to-do list, on this PC. The assistant can add tasks for you, and meeting action items can become tasks.</p>
          </div>
        </header>
        <div className="mode-switch" role="tablist" aria-label="Task lists">
          {(["today", "upcoming", "anytime", "done"] as List[]).map((l) => (
            <button key={l} role="tab" aria-selected={list === l} className={`mode ${list === l ? "active" : ""}`} onClick={() => setList(l)}>
              {l === "today" ? "Today" : l === "upcoming" ? "Upcoming" : l === "anytime" ? "Anytime" : "Done"}
              {l !== "done" && groups[l].length > 0 && ` (${groups[l].length})`}
            </button>
          ))}
        </div>
        {list !== "done" && (
          <form
            className="row quick-add"
            onSubmit={(e) => {
              e.preventDefault();
              addQuick();
            }}
          >
            <input className="input" value={quick} onChange={(e) => setQuick(e.target.value)} placeholder="Add a task, e.g. “Send the budget Friday 3pm”" aria-label="New task" />
            <button className="btn primary" type="submit" disabled={!quick.trim()}>Add</button>
          </form>
        )}
        <ul className="task-list">
          {tasks && shown.length === 0 && <li className="muted small">{list === "done" ? "Nothing finished yet." : "Nothing here."}</li>}
          {shown.map((t) => (
            <li key={t.id} className={`task ${t.done_at ? "done" : ""}`}>
              <input
                type="checkbox"
                checked={t.done_at !== null}
                onChange={(e) => save({ ...t, done_at: e.target.checked ? Date.now() : null })}
                aria-label={`Done: ${t.title}`}
              />
              <button className="task-main" onClick={() => setEditing(t)}>
                <span className="task-title">
                  {t.priority > 0 && <span className={`prio p${t.priority}`} title={`${PRIORITY[t.priority]} priority`}>{"!".repeat(t.priority)}</span>}
                  {t.title}
                </span>
                <span className="small muted">
                  {t.due !== null && <span className={t.due < now && !t.done_at && !(!t.due_has_time && t.due >= new Date().setHours(0, 0, 0, 0)) ? "overdue" : ""}>{dueLabel(t)}</span>}
                  {t.remind_at !== null && !t.done_at && " · 🔔"}
                  {t.subtasks.length > 0 && ` · ${t.subtasks.filter((s) => s.done).length}/${t.subtasks.length}`}
                  {t.source && ` · from ${t.source.label}`}
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
          onSave={async (t) => {
            await save(t);
            setEditing(null);
          }}
          onDelete={async () => {
            await api.deleteTask(editing.id);
            setEditing(null);
            load();
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

function TaskEditor({ task, onClose, onSave, onDelete }: { task: Task; onClose: () => void; onSave: (t: Task) => void; onDelete: () => void }) {
  const [t, setT] = useState<Task>(task);
  const [withTime, setWithTime] = useState(task.due_has_time);
  const [sub, setSub] = useState("");
  const setDue = (v: string) => {
    if (!v) return setT({ ...t, due: null, due_has_time: false });
    const d = new Date(withTime ? v : `${v}T00:00`);
    setT({ ...t, due: d.getTime(), due_has_time: withTime });
  };
  return (
    <Modal title="Task" onClose={onClose}>
      <div className="form">
        <input className="input" value={t.title} onChange={(e) => setT({ ...t, title: e.target.value })} aria-label="Task" autoFocus />
        <div className="two-col">
          <label>
            Due
            <input className="input" type={withTime ? "datetime-local" : "date"} value={toLocalInput(t.due, withTime)} onChange={(e) => setDue(e.target.value)} />
          </label>
          <label>
            Priority
            <select value={t.priority} onChange={(e) => setT({ ...t, priority: Number(e.target.value) })}>
              {PRIORITY.map((p, i) => <option key={p} value={i}>{p}</option>)}
            </select>
          </label>
        </div>
        <label className="check">
          <input type="checkbox" checked={withTime} onChange={(e) => { setWithTime(e.target.checked); setT({ ...t, due_has_time: e.target.checked }); }} />
          <span>At a specific time</span>
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={t.remind_at !== null}
            disabled={t.due === null}
            onChange={(e) => {
              if (!e.target.checked || t.due === null) return setT({ ...t, remind_at: null });
              const at = new Date(t.due);
              if (!t.due_has_time) at.setHours(9, 0, 0, 0);
              setT({ ...t, remind_at: at.getTime() });
            }}
          />
          <span>Remind me {t.due === null ? "(set a due date first)" : t.due_has_time ? "at that time" : "at 9 AM that day"}</span>
        </label>
        <label>
          Notes
          <textarea className="input" rows={3} value={t.notes} onChange={(e) => setT({ ...t, notes: e.target.value })} />
        </label>
        <div>
          <strong className="small">Subtasks</strong>
          <ul className="subtasks">
            {t.subtasks.map((s, i) => (
              <li key={i}>
                <label className="check">
                  <input type="checkbox" checked={s.done} onChange={(e) => setT({ ...t, subtasks: t.subtasks.map((x, j) => (j === i ? { ...x, done: e.target.checked } : x)) })} />
                  <span>{s.title}</span>
                </label>
                <button className="link small" onClick={() => setT({ ...t, subtasks: t.subtasks.filter((_, j) => j !== i) })}>Remove</button>
              </li>
            ))}
          </ul>
          <form
            className="row"
            onSubmit={(e) => {
              e.preventDefault();
              if (sub.trim()) setT({ ...t, subtasks: [...t.subtasks, { title: sub.trim(), done: false }] });
              setSub("");
            }}
          >
            <input className="input" value={sub} onChange={(e) => setSub(e.target.value)} placeholder="Add a step" aria-label="New subtask" />
            <button className="btn small" type="submit">Add</button>
          </form>
        </div>
        {t.source && <p className="muted small">From {t.source.kind}: {t.source.label}</p>}
      </div>
      <div className="modal-actions">
        {task.id && <button className="btn ghost danger" onClick={onDelete}>Delete</button>}
        <span className="spacer" />
        <button className="btn" onClick={onClose}>Cancel</button>
        <button className="btn primary" onClick={() => onSave(t)} disabled={!t.title.trim()}>Save</button>
      </div>
    </Modal>
  );
}
