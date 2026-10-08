// SPDX-License-Identifier: AGPL-3.0-only
// Calendar: an agenda of events (on this PC, or from CalDAV accounts) and
// tasks that are due. Events on this PC work Offline.
import { useEffect, useMemo, useState } from "react";
import { api, errorText, on, type CalAccount, type CalEvent, type Task } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

const DAY = 86_400_000;
const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

export function CalendarView({ toast }: { toast: PushToast }) {
  const [from, setFrom] = useState(() => startOfDay(new Date()));
  const days = 14;
  const [events, setEvents] = useState<CalEvent[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [accounts, setAccounts] = useState<CalAccount[]>([]);
  const [creating, setCreating] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<CalEvent | null>(null);

  const load = () => {
    api.calendarEvents(from, from + days * DAY).then(setEvents).catch((e) => toast(errorText(e), "error"));
    api.tasks().then(setTasks).catch(() => setTasks([]));
    api.calendarAccounts().then(setAccounts).catch(() => {});
  };
  useEffect(() => {
    load();
    const sub = on("calendar:synced", () => load());
    return () => {
      sub.then((u) => u());
    };
  }, [from]); // eslint-disable-line react-hooks/exhaustive-deps

  const byDay = useMemo(() => {
    const out: { day: number; events: CalEvent[]; tasks: Task[] }[] = [];
    for (let i = 0; i < days; i++) {
      const day = startOfDay(new Date(from + i * DAY + DAY / 2));
      const next = day + DAY;
      out.push({
        day,
        events: events.filter((e) => e.start < next && e.end > day),
        tasks: tasks.filter((t) => !t.done_at && t.due !== null && t.due >= day && t.due < next),
      });
    }
    return out;
  }, [events, tasks, from]);

  const calName = (e: CalEvent) => {
    if (!e.account_id) return "This PC";
    const a = accounts.find((x) => x.id === e.account_id);
    return a?.calendars.find((c) => c.href === e.calendar)?.name ?? a?.name ?? "";
  };

  const sync = async () => {
    setSyncing(true);
    try {
      await api.syncCalendars();
      load();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSyncing(false);
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>Calendar</h1>
          <p className="muted">{new Date(from).toLocaleDateString([], { month: "long", day: "numeric" })} – {new Date(from + (days - 1) * DAY).toLocaleDateString([], { month: "long", day: "numeric" })}</p>
        </div>
        <div className="row">
          <button className="btn ghost small" onClick={() => setFrom(from - 7 * DAY)} aria-label="Earlier">←</button>
          <button className="btn small" onClick={() => setFrom(startOfDay(new Date()))}>Today</button>
          <button className="btn ghost small" onClick={() => setFrom(from + 7 * DAY)} aria-label="Later">→</button>
          {accounts.length > 0 && <button className="btn ghost small" onClick={sync} disabled={syncing}>{syncing ? "Syncing…" : "↻ Sync"}</button>}
          <button className="btn primary small" onClick={() => setCreating(startOfDay(new Date()))}>+ New event</button>
        </div>
      </header>

      <div className="agenda">
        {byDay.map(({ day, events: evs, tasks: ts }) => {
          const isToday = day === startOfDay(new Date());
          return (
            <section key={day} className={`agenda-day ${isToday ? "today" : ""}`}>
              <button className="agenda-date" onClick={() => setCreating(day)} title="Add an event on this day">
                <strong>{new Date(day).toLocaleDateString([], { weekday: "short" })}</strong>
                <span>{new Date(day).getDate()}</span>
              </button>
              <div className="agenda-items">
                {evs.length === 0 && ts.length === 0 && <span className="muted small">—</span>}
                {evs.map((e) => (
                  <div key={e.id} className="agenda-event">
                    <span className="agenda-time small">{e.all_day ? "All day" : `${time(e.start)} – ${time(e.end)}`}</span>
                    <span className="grow">
                      <strong>{e.title}</strong>
                      {e.location && <span className="muted small"> · {e.location}</span>}
                      {e.attendees.length > 0 && <span className="muted small block">With {e.attendees.join(", ")}</span>}
                    </span>
                    <span className="muted small">{calName(e)}</span>
                    {!e.recurring && <button className="icon-btn" title="Delete" aria-label={`Delete ${e.title}`} onClick={() => setConfirmDelete(e)}>×</button>}
                  </div>
                ))}
                {ts.map((t) => (
                  <div key={t.id} className="agenda-event task">
                    <span className="agenda-time small">{t.due_has_time && t.due ? time(t.due) : "Due"}</span>
                    <span className="grow">✅ {t.title}</span>
                    <span className="muted small">Tasks</span>
                  </div>
                ))}
              </div>
            </section>
          );
        })}
      </div>

      <section className="card">
        <h2>Calendar accounts</h2>
        <p className="muted small">Events you add here stay on this PC unless you pick an account's calendar. Accounts sync with Local AI + Web.</p>
        {accounts.map((a) => (
          <div key={a.id} className="row between">
            <span>
              <strong>{a.name}</strong> <span className="muted small">{a.calendars.map((c) => c.name).join(", ")}{a.synced_at ? ` · synced ${new Date(a.synced_at).toLocaleString()}` : ""}</span>
            </span>
            <button
              className="btn ghost small danger"
              onClick={async () => {
                try {
                  await api.removeCalendarAccount(a.id);
                  load();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Remove
            </button>
          </div>
        ))}
        {adding ? <AddCalendar toast={toast} onDone={() => { setAdding(false); load(); }} /> : <button className="btn small" onClick={() => setAdding(true)}>+ Connect a calendar (iCloud, Fastmail, Nextcloud…)</button>}
      </section>

      {creating !== null && <NewEventDialog day={creating} accounts={accounts} onClose={() => setCreating(null)} onCreated={() => { setCreating(null); load(); }} toast={toast} />}
      {confirmDelete && (
        <Modal title="Delete this event?" onClose={() => setConfirmDelete(null)}>
          <p>“{confirmDelete.title}” will be deleted{confirmDelete.account_id ? " from its calendar account too" : ""}.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(null)}>Cancel</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                const id = confirmDelete.id;
                setConfirmDelete(null);
                try {
                  await api.deleteEvent(id);
                  load();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");
const dateInput = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

function NewEventDialog({ day, accounts, onClose, onCreated, toast }: { day: number; accounts: CalAccount[]; onClose: () => void; onCreated: () => void; toast: PushToast }) {
  const [title, setTitle] = useState("");
  const [date, setDate] = useState(dateInput(day));
  const [allDay, setAllDay] = useState(false);
  const [start, setStart] = useState("09:00");
  const [end, setEnd] = useState("10:00");
  const [location, setLocation] = useState("");
  const [notes, setNotes] = useState("");
  const [attendees, setAttendees] = useState("");
  const [calendar, setCalendar] = useState("");
  const [busy, setBusy] = useState(false);
  const cals = accounts.flatMap((a) => a.calendars.map((c) => ({ href: c.href, label: `${c.name} (${a.name})` })));

  const save = async () => {
    const [y, m, d] = date.split("-").map(Number);
    const at = (hm: string) => {
      const [h, mi] = hm.split(":").map(Number);
      return new Date(y, m - 1, d, h, mi).getTime();
    };
    const s = allDay ? new Date(y, m - 1, d).getTime() : at(start);
    const e = allDay ? s + DAY : Math.max(at(end), s + 15 * 60_000);
    setBusy(true);
    try {
      await api.createEvent({
        title,
        start: s,
        end: e,
        all_day: allDay,
        location,
        notes,
        attendees: attendees.split(/[,;]/).map((x) => x.trim()).filter(Boolean),
        calendar: calendar || null,
      });
      onCreated();
    } catch (err) {
      toast(errorText(err), "error");
      setBusy(false);
    }
  };

  return (
    <Modal title="New event" onClose={onClose}>
      <div className="form">
        <label>Title<input className="input" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus /></label>
        <div className="row">
          <label>Date<input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} /></label>
          {!allDay && <label>Starts<input className="input" type="time" value={start} onChange={(e) => setStart(e.target.value)} /></label>}
          {!allDay && <label>Ends<input className="input" type="time" value={end} onChange={(e) => setEnd(e.target.value)} /></label>}
        </div>
        <label className="row"><input type="checkbox" checked={allDay} onChange={(e) => setAllDay(e.target.checked)} /> All day</label>
        <label>Place<input className="input" value={location} onChange={(e) => setLocation(e.target.value)} /></label>
        <label>Notes<textarea className="input" rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} /></label>
        {cals.length > 0 && (
          <label>Calendar
            <select value={calendar} onChange={(e) => setCalendar(e.target.value)}>
              <option value="">This PC only</option>
              {cals.map((c) => <option key={c.href} value={c.href}>{c.label}</option>)}
            </select>
          </label>
        )}
        {calendar && (
          <label>Invite (email addresses)<input className="input" value={attendees} onChange={(e) => setAttendees(e.target.value)} placeholder="Your calendar may email them an invitation" /></label>
        )}
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>Cancel</button>
        <button className="btn primary" onClick={save} disabled={busy || !title.trim()}>{busy ? "Saving…" : "Add"}</button>
      </div>
    </Modal>
  );
}

function AddCalendar({ toast, onDone }: { toast: PushToast; onDone: () => void }) {
  const [presets, setPresets] = useState<{ name: string; url: string; note: string }[]>([]);
  const [pick, setPick] = useState(0);
  const [url, setUrl] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.calendarPresets().then((p) => {
      setPresets(p);
      setUrl(p[0]?.url ?? "");
    });
  }, []);
  const connect = async () => {
    setBusy(true);
    try {
      await api.addCalendarAccount({ name: presets[pick]?.name === "Other (CalDAV)" ? "" : presets[pick]?.name ?? "", url, username, password, calendars: [] });
      toast("Calendar connected.", "success");
      onDone();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="form">
      <label>Provider
        <select value={pick} onChange={(e) => { const i = Number(e.target.value); setPick(i); setUrl(presets[i]?.url ?? ""); }}>
          {presets.map((p, i) => <option key={p.name} value={i}>{p.name}</option>)}
        </select>
      </label>
      {presets[pick] && <p className="small hint-box">{presets[pick].note}</p>}
      <label>CalDAV address<input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…" /></label>
      <label>User name<input className="input" value={username} onChange={(e) => setUsername(e.target.value)} /></label>
      <label>Password or app password<input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="off" /></label>
      <div className="row">
        <button className="btn primary" onClick={connect} disabled={busy || !url || !username || !password}>{busy ? "Connecting…" : "Connect"}</button>
        <button className="btn" onClick={onDone}>Cancel</button>
      </div>
      <p className="muted small">Google and Microsoft calendars need their own sign-in, which is coming.</p>
    </div>
  );
}
