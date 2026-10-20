// SPDX-License-Identifier: AGPL-3.0-only
// Calendar: events on this PC or from calendar accounts, and tasks that are
// due, in month, week and day views. Events on this PC work Offline.
import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorText, on, type CalAccount, type CalEvent, type Task } from "../api";
import { Modal } from "../components/Modal";
import { SignInButtons } from "../components/SignIn";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

const DAY = 86_400_000;
const HOUR_PX = 48;
const startOfDay = (d: Date | number) => {
  const x = new Date(d);
  return new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
};
const addDays = (ms: number, n: number) => {
  const d = new Date(ms);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n).getTime();
};
const startOfWeek = (ms: number) => addDays(startOfDay(ms), -new Date(ms).getDay());
const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const shortTime = (ms: number) => {
  const d = new Date(ms);
  return d.toLocaleTimeString([], d.getMinutes() ? { hour: "numeric", minute: "2-digit" } : { hour: "numeric" }).replace(/\s/g, "").toLowerCase();
};
const hourLabel = (h: number) => new Date(2000, 0, 1, h).toLocaleTimeString([], { hour: "numeric" });
const sameDay = (a: number, b: number) => startOfDay(a) === startOfDay(b);

type View = "day" | "week" | "month";
const VIEWS: { id: View; label: string }[] = [
  { id: "day", label: tx("Day") },
  { id: "week", label: tx("Week") },
  { id: "month", label: tx("Month") },
];
const VIEW_KEY = "sulcusai.calendar.view";
const savedView = (): View => {
  try {
    const v = localStorage.getItem(VIEW_KEY);
    if (v === "day" || v === "week" || v === "month") return v;
  } catch {
    // Not saved: start on the week.
  }
  return "week";
};

// One color per calendar: this PC is the accent; accounts' calendars use
// their own color or one of the logo's hues.
const HUES = ["#4b5df0", "#d02f8c", "#0f8b8d", "#8d44cf", "#c2410c", "#2d89ff", "#15803d"];

function range(view: View, at: number): [number, number] {
  if (view === "day") return [startOfDay(at), addDays(startOfDay(at), 1)];
  if (view === "week") return [startOfWeek(at), addDays(startOfWeek(at), 7)];
  const first = new Date(at);
  const gridStart = startOfWeek(new Date(first.getFullYear(), first.getMonth(), 1).getTime());
  return [gridStart, addDays(gridStart, 42)];
}

function title(view: View, at: number) {
  const d = new Date(at);
  if (view === "month") return d.toLocaleDateString([], { month: "long", year: "numeric" });
  if (view === "day") return d.toLocaleDateString([], { weekday: "long", month: "long", day: "numeric", year: "numeric" });
  const [a, b] = range("week", at);
  // "Oct 11 – 17, 2026", or "Oct 29 – Nov 4, 2026", as the language writes it.
  return new Intl.DateTimeFormat([], { month: "short", day: "numeric", year: "numeric" }).formatRange(new Date(a), new Date(addDays(b, -1)));
}

/** Side-by-side columns for events that overlap within one day. */
function layout(evs: CalEvent[], day: number): { e: CalEvent; top: number; height: number; col: number; cols: number }[] {
  const next = addDays(day, 1);
  const items = evs
    .map((e) => ({ e, s: Math.max(e.start, day), f: Math.min(e.end, next) }))
    .sort((a, b) => a.s - b.s || b.f - a.f);
  const out: { e: CalEvent; top: number; height: number; col: number; cols: number }[] = [];
  let group: { e: CalEvent; s: number; f: number; col: number }[] = [];
  let groupEnd = 0;
  const flush = () => {
    const cols = Math.max(1, ...group.map((g) => g.col + 1));
    for (const g of group) {
      const top = ((g.s - day) / 3_600_000) * HOUR_PX;
      const height = Math.max(((g.f - g.s) / 3_600_000) * HOUR_PX, 20);
      out.push({ e: g.e, top, height, col: g.col, cols });
    }
    group = [];
  };
  for (const it of items) {
    if (group.length && it.s >= groupEnd) flush();
    const taken = new Set(group.filter((g) => g.f > it.s).map((g) => g.col));
    let col = 0;
    while (taken.has(col)) col++;
    group.push({ ...it, col });
    groupEnd = Math.max(groupEnd, it.f);
  }
  if (group.length) flush();
  return out;
}

export function CalendarView({ toast }: { toast: PushToast }) {
  const [view, setViewState] = useState<View>(savedView);
  const [at, setAt] = useState(() => startOfDay(Date.now()));
  const [events, setEvents] = useState<CalEvent[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [accounts, setAccounts] = useState<CalAccount[]>([]);
  const [creating, setCreating] = useState<{ day: number; hour?: number } | null>(null);
  const [adding, setAdding] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [shown, setShown] = useState<CalEvent | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<CalEvent | null>(null);
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [now, setNow] = useState(Date.now());
  const [from, to] = range(view, at);

  const setView = (v: View) => {
    setViewState(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      // Kept for this visit only.
    }
  };
  const load = () => {
    api.calendarEvents(from, to).then(setEvents).catch((e) => toast(errorText(e), "error"));
    api.tasks().then(setTasks).catch(() => setTasks([]));
    api.calendarAccounts().then(setAccounts).catch(() => {});
  };
  useEffect(() => {
    load();
    const sub = on("calendar:synced", () => load());
    return () => {
      sub.then((u) => u());
    };
  }, [from, to]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(timer);
  }, []);

  // Calendars and their colors: this PC, then each account's calendars.
  const calendars = useMemo(() => {
    const list: { key: string; name: string; color: string; account?: string }[] = [{ key: "local", name: t("This PC"), color: "#6447e8" }];
    let i = 0;
    for (const a of accounts) {
      for (const c of a.calendars) {
        list.push({ key: c.href, name: c.name || a.name, color: c.color || HUES[i++ % HUES.length], account: a.name });
      }
    }
    return list;
  }, [accounts]);
  const calKey = (e: CalEvent) => (e.account_id ? e.calendar ?? e.account_id : "local");
  const colorOf = (e: CalEvent) => calendars.find((c) => c.key === calKey(e))?.color ?? HUES[0];
  const calName = (e: CalEvent) => calendars.find((c) => c.key === calKey(e))?.name ?? "";
  const visible = events.filter((e) => !hidden.has(calKey(e)));
  const dueTasks = tasks.filter((task) => !task.done_at && task.due !== null);

  const step = (dir: number) => {
    if (view === "day") setAt(addDays(at, dir));
    else if (view === "week") setAt(addDays(at, 7 * dir));
    else {
      const d = new Date(at);
      setAt(new Date(d.getFullYear(), d.getMonth() + dir, 1).getTime());
    }
  };
  const openDay = (day: number) => {
    setAt(day);
    setView("day");
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

  const chipStyle = (e: CalEvent) => ({ "--ev": colorOf(e) }) as React.CSSProperties;

  return (
    <div className="cal">
      <aside className="cal-rail">
        <button className="compose-btn" onClick={() => setCreating({ day: view === "month" ? startOfDay(Date.now()) : at })}>
          <span aria-hidden>＋</span> {t("Create")}
        </button>
        <MiniMonth at={at} today={now} onPick={(d) => setAt(d)} />
        <div className="cal-list">
          <span className="rail-label">{t("My calendars")}</span>
          {calendars.map((c) => (
            <label key={c.key} className="cal-check" title={c.account ? `${c.name} · ${c.account}` : c.name}>
              <input
                type="checkbox"
                checked={!hidden.has(c.key)}
                onChange={() => setHidden((h) => {
                  const n = new Set(h);
                  if (n.has(c.key)) n.delete(c.key);
                  else n.add(c.key);
                  return n;
                })}
                style={{ accentColor: c.color }}
              />
              <span className="cal-swatch" style={{ background: c.color }} />
              <span className="ellipsis">{c.name}</span>
            </label>
          ))}
          <button className="rail-link" onClick={() => setAdding(true)}>{t("+ Connect a calendar")}</button>
          {accounts.map((a) => (
            <div key={a.id} className="row between small cal-account">
              <span className="ellipsis muted" title={a.synced_at ? t("synced {time}", { time: new Date(a.synced_at).toLocaleString() }) : ""}>{a.name}</span>
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
                {t("Remove")}
              </button>
            </div>
          ))}
        </div>
      </aside>

      <section className="cal-main">
        <header className="cal-top">
          <button className="btn round-btn" onClick={() => setAt(startOfDay(Date.now()))}>{t("Today")}</button>
          <button className="icon-btn round" onClick={() => step(-1)} aria-label={t("Earlier")} title={t("Earlier")}>‹</button>
          <button className="icon-btn round" onClick={() => step(1)} aria-label={t("Later")} title={t("Later")}>›</button>
          <h1 className="cal-title">{title(view, at)}</h1>
          <span className="spacer" />
          {accounts.length > 0 && (
            <button className={`icon-btn round ${syncing ? "spin" : ""}`} onClick={sync} disabled={syncing} title={syncing ? t("Syncing…") : t("↻ Sync")} aria-label={t("↻ Sync")}>↻</button>
          )}
          <div className="seg" role="tablist" aria-label={t("View")}>
            {VIEWS.map((v) => (
              <button key={v.id} role="tab" aria-selected={view === v.id} className={view === v.id ? "active" : ""} onClick={() => setView(v.id)}>
                {t(v.label)}
              </button>
            ))}
          </div>
        </header>

        {view === "month" ? (
          <MonthGrid
            from={from}
            at={at}
            today={now}
            events={visible}
            tasks={dueTasks}
            chipStyle={chipStyle}
            onDay={openDay}
            onCreate={(day) => setCreating({ day })}
            onEvent={setShown}
          />
        ) : (
          <TimeGrid
            days={view === "day" ? [from] : Array.from({ length: 7 }, (_, i) => addDays(from, i))}
            today={now}
            events={visible}
            tasks={dueTasks}
            chipStyle={chipStyle}
            onDay={openDay}
            onCreate={(day, hour) => setCreating({ day, hour })}
            onEvent={setShown}
          />
        )}
      </section>

      {creating && <NewEventDialog day={creating.day} hour={creating.hour} accounts={accounts} onClose={() => setCreating(null)} onCreated={() => { setCreating(null); load(); }} toast={toast} />}
      {shown && (
        <Modal title={shown.title} onClose={() => setShown(null)}>
          <div className="event-details">
            <p className="row"><span className="cal-swatch" style={{ background: colorOf(shown) }} /> <span>{calName(shown)}</span></p>
            <p>
              {shown.all_day
                ? new Date(shown.start).toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" })
                : `${new Date(shown.start).toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" })} · ${time(shown.start)} – ${time(shown.end)}`}
              {shown.recurring && <span className="muted"> · {t("Repeats")}</span>}
            </p>
            {shown.location && <p className="row"><span className="muted">{t("Place")}</span> <span>{shown.location}</span></p>}
            {shown.attendees.length > 0 && <p className="muted small">{t("With {names}", { names: shown.attendees.join(", ") })}</p>}
            {shown.notes && <p className="pre-wrap">{shown.notes}</p>}
          </div>
          <div className="modal-actions">
            {!shown.recurring && (
              <button className="btn danger" onClick={() => { setConfirmDelete(shown); setShown(null); }}>{t("Delete")}</button>
            )}
            <button className="btn primary" onClick={() => setShown(null)}>{t("Close")}</button>
          </div>
        </Modal>
      )}
      {adding && (
        <Modal title={t("Connect a calendar")} onClose={() => setAdding(false)}>
          <AddCalendar toast={toast} onDone={() => { setAdding(false); load(); }} />
        </Modal>
      )}
      {confirmDelete && (
        <Modal title={t("Delete this event?")} onClose={() => setConfirmDelete(null)}>
          <p>{confirmDelete.account_id ? t("“{title}” will be deleted from its calendar account too.", { title: confirmDelete.title }) : t("“{title}” will be deleted.", { title: confirmDelete.title })}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(null)}>{t("Cancel")}</button>
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
              {t("Delete")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/** The small month in the rail, for jumping to a day. */
function MiniMonth({ at, today, onPick }: { at: number; today: number; onPick: (day: number) => void }) {
  const [month, setMonth] = useState(() => new Date(new Date(at).getFullYear(), new Date(at).getMonth(), 1).getTime());
  useEffect(() => setMonth(new Date(new Date(at).getFullYear(), new Date(at).getMonth(), 1).getTime()), [at]);
  const start = startOfWeek(month);
  const m = new Date(month);
  const days = Array.from({ length: 42 }, (_, i) => addDays(start, i));
  return (
    <div className="mini-month">
      <div className="row between">
        <strong className="small">{m.toLocaleDateString([], { month: "long", year: "numeric" })}</strong>
        <span>
          <button className="icon-btn" onClick={() => setMonth(new Date(m.getFullYear(), m.getMonth() - 1, 1).getTime())} aria-label={t("Earlier")}>‹</button>
          <button className="icon-btn" onClick={() => setMonth(new Date(m.getFullYear(), m.getMonth() + 1, 1).getTime())} aria-label={t("Later")}>›</button>
        </span>
      </div>
      <div className="mini-grid">
        {days.slice(0, 7).map((d) => <span key={`h${d}`} className="mini-dow">{new Date(d).toLocaleDateString([], { weekday: "narrow" })}</span>)}
        {days.map((d) => (
          <button
            key={d}
            className={`mini-day ${new Date(d).getMonth() !== m.getMonth() ? "other" : ""} ${sameDay(d, today) ? "today" : ""} ${sameDay(d, at) ? "picked" : ""}`}
            onClick={() => onPick(d)}
          >
            {new Date(d).getDate()}
          </button>
        ))}
      </div>
    </div>
  );
}

interface GridProps {
  today: number;
  events: CalEvent[];
  tasks: Task[];
  chipStyle: (e: CalEvent) => React.CSSProperties;
  onDay: (day: number) => void;
  onEvent: (e: CalEvent) => void;
}

function MonthGrid({ from, at, today, events, tasks, chipStyle, onDay, onCreate, onEvent }: GridProps & { from: number; at: number; onCreate: (day: number) => void }) {
  const month = new Date(at).getMonth();
  const days = Array.from({ length: 42 }, (_, i) => addDays(from, i));
  const MAX = 3;
  return (
    <div className="month">
      <div className="month-head">
        {days.slice(0, 7).map((d) => <span key={d}>{new Date(d).toLocaleDateString([], { weekday: "short" })}</span>)}
      </div>
      <div className="month-grid">
        {days.map((day) => {
          const next = addDays(day, 1);
          const evs = events.filter((e) => e.start < next && e.end > day).sort((a, b) => Number(b.all_day) - Number(a.all_day) || a.start - b.start);
          const ts = tasks.filter((task) => task.due! >= day && task.due! < next);
          const items = [...evs.map((e) => ({ e })), ...ts.map((task) => ({ task }))];
          const more = items.length - MAX;
          return (
            <div key={day} className={`month-cell ${new Date(day).getMonth() !== month ? "other" : ""}`} onClick={() => onCreate(day)}>
              <button className={`month-date ${sameDay(day, today) ? "today" : ""}`} onClick={(ev) => { ev.stopPropagation(); onDay(day); }}>
                {new Date(day).getDate() === 1 ? new Date(day).toLocaleDateString([], { month: "short", day: "numeric" }) : new Date(day).getDate()}
              </button>
              {items.slice(0, more > 0 ? MAX - 1 : MAX).map((it) =>
                "e" in it ? (
                  <button key={it.e.id} className={`cal-chip ${it.e.all_day ? "all-day" : ""}`} style={chipStyle(it.e)} onClick={(ev) => { ev.stopPropagation(); onEvent(it.e); }} title={it.e.title}>
                    {!it.e.all_day && <><i className="cal-chip-dot" /><span className="cal-chip-time">{shortTime(it.e.start)}</span></>}
                    <span className="ellipsis">{it.e.title}</span>
                  </button>
                ) : (
                  <span key={it.task.id} className="cal-chip task" title={it.task.title}><span className="cal-chip-box" aria-hidden /><span className="ellipsis">{it.task.title}</span></span>
                ),
              )}
              {more > 0 && <button className="cal-chip more" onClick={(ev) => { ev.stopPropagation(); onDay(day); }}>{t("{n} more", { n: more + 1 })}</button>}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function TimeGrid({ days, today, events, tasks, chipStyle, onDay, onCreate, onEvent }: GridProps & { days: number[]; onCreate: (day: number, hour: number) => void }) {
  const scroller = useRef<HTMLDivElement>(null);
  // Start the view at 7 AM (or a little before the current hour).
  useEffect(() => {
    const h = days.some((d) => sameDay(d, today)) ? Math.max(0, new Date(today).getHours() - 2) : 7;
    scroller.current?.scrollTo({ top: Math.min(h, 7) * HOUR_PX });
  }, [days.length]); // eslint-disable-line react-hooks/exhaustive-deps
  const hours = Array.from({ length: 24 }, (_, h) => h);
  const cols = { gridTemplateColumns: `56px repeat(${days.length}, minmax(0, 1fr))` };
  const allDay = (day: number) => {
    const next = addDays(day, 1);
    return [
      ...events.filter((e) => (e.all_day || e.end - e.start >= DAY) && e.start < next && e.end > day).map((e) => ({ e })),
      ...tasks.filter((task) => task.due! >= day && task.due! < next).map((task) => ({ task })),
    ];
  };
  const hasAllDay = days.some((d) => allDay(d).length > 0);
  return (
    <div className={`timegrid ${days.length === 1 ? "single" : ""}`}>
      <div className="tg-head" style={cols}>
        <span />
        {days.map((d) => (
          <button key={d} className={`tg-day ${sameDay(d, today) ? "today" : ""}`} onClick={() => onDay(d)}>
            <span className="tg-dow">{new Date(d).toLocaleDateString([], { weekday: "short" })}</span>
            <span className="tg-num">{new Date(d).getDate()}</span>
          </button>
        ))}
      </div>
      {hasAllDay && (
        <div className="tg-allday" style={cols}>
          <span className="tg-gutter small muted">{t("All day")}</span>
          {days.map((d) => (
            <div key={d} className="tg-allday-cell">
              {allDay(d).map((it) =>
                "e" in it ? (
                  <button key={it.e.id} className="cal-chip all-day" style={chipStyle(it.e)} onClick={() => onEvent(it.e)} title={it.e.title}><span className="ellipsis">{it.e.title}</span></button>
                ) : (
                  <span key={it.task.id} className="cal-chip task" title={it.task.title}><span className="cal-chip-box" aria-hidden /><span className="ellipsis">{it.task.title}</span></span>
                ),
              )}
            </div>
          ))}
        </div>
      )}
      <div className="tg-scroll" ref={scroller}>
        <div className="tg-body" style={cols}>
          <div className="tg-hours">
            {hours.map((h) => <span key={h} className="tg-hour" style={{ top: h * HOUR_PX }}>{h === 0 ? "" : hourLabel(h)}</span>)}
          </div>
          {days.map((day) => {
            const timed = events.filter((e) => !e.all_day && e.end - e.start < DAY && e.start < addDays(day, 1) && e.end > day);
            return (
              <div key={day} className="tg-col" style={{ height: 24 * HOUR_PX }}>
                {hours.map((h) => (
                  <button key={h} className="tg-slot" style={{ top: h * HOUR_PX, height: HOUR_PX }} onClick={() => onCreate(day, h)} aria-label={t("Add an event at {time}", { time: hourLabel(h) })} />
                ))}
                {layout(timed, day).map(({ e, top, height, col, cols: n }) => (
                  <button
                    key={e.id}
                    className={`tg-event ${height < 40 ? "short" : ""}`}
                    style={{ ...chipStyle(e), top, height: height - 2, left: `calc(${(col / n) * 100}% + 2px)`, width: `calc(${100 / n}% - 6px)` }}
                    onClick={() => onEvent(e)}
                    title={`${e.title} · ${time(e.start)} – ${time(e.end)}`}
                  >
                    <strong className="ellipsis">{e.title}</strong>
                    <span className="ellipsis">{height < 40 ? shortTime(e.start) : `${time(e.start)} – ${time(e.end)}${e.location ? ` · ${e.location}` : ""}`}</span>
                  </button>
                ))}
                {sameDay(day, today) && <span className="now-line" style={{ top: ((today - day) / 3_600_000) * HOUR_PX }} />}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");
const dateInput = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

/** What the event form starts with (an event read from an email). Empty
 * times are left for the person to fill in. */
export interface EventInitial {
  title?: string;
  /** YYYY-MM-DD */
  date?: string;
  /** HH:MM, or "" */
  start?: string;
  end?: string;
  allDay?: boolean;
  location?: string;
  notes?: string;
  /** A connected calendar's address; "" for this PC. */
  calendar?: string;
}

/** The name of the calendar an event went to: "Work (iCloud)", or This PC. */
export function calendarName(e: CalEvent, accounts: CalAccount[]): string {
  for (const a of accounts) {
    const c = a.calendars.find((x) => x.href === e.calendar);
    if (c) return `${c.name || a.name} (${a.name})`;
  }
  return t("This PC");
}

export function NewEventDialog({ day, hour, initial, heading, note, accounts, onClose, onCreated, toast }: {
  day: number;
  hour?: number;
  initial?: EventInitial;
  heading?: string;
  /** Shown above the form. */
  note?: string;
  accounts: CalAccount[];
  onClose: () => void;
  onCreated: (event: CalEvent) => void;
  toast: PushToast;
}) {
  const [title, setTitle] = useState(initial?.title ?? "");
  const [date, setDate] = useState(initial?.date || dateInput(day));
  const [allDay, setAllDay] = useState(initial?.allDay ?? false);
  // A time slot that was clicked, else 9 to 10.
  const [start, setStart] = useState(initial?.start ?? `${pad(hour ?? 9)}:00`);
  const [end, setEnd] = useState(initial?.end ?? `${pad(Math.min((hour ?? 9) + 1, 23))}:${(hour ?? 9) >= 23 ? "59" : "00"}`);
  const [location, setLocation] = useState(initial?.location ?? "");
  const [notes, setNotes] = useState(initial?.notes ?? "");
  const [attendees, setAttendees] = useState("");
  const [calendar, setCalendar] = useState(initial?.calendar ?? "");
  const [busy, setBusy] = useState(false);
  const cals = accounts.flatMap((a) => a.calendars.map((c) => ({ href: c.href, label: `${c.name} (${a.name})` })));

  const save = async () => {
    const [y, m, d] = date.split("-").map(Number);
    const at = (hm: string) => {
      const [h, mi] = hm.split(":").map(Number);
      return new Date(y, m - 1, d, h, mi).getTime();
    };
    const s = allDay ? new Date(y, m - 1, d).getTime() : at(start);
    // No end time: an hour.
    const e = allDay ? s + DAY : Math.max(end ? at(end) : s + 3_600_000, s + 15 * 60_000);
    setBusy(true);
    try {
      const made = await api.createEvent({
        title,
        start: s,
        end: e,
        all_day: allDay,
        location,
        notes,
        attendees: attendees.split(/[,;]/).map((x) => x.trim()).filter(Boolean),
        calendar: calendar || null,
      });
      onCreated(made);
    } catch (err) {
      toast(errorText(err), "error");
      setBusy(false);
    }
  };

  return (
    <Modal title={heading ?? t("New event")} onClose={onClose}>
      {note && <p className="small hint-box">{note}</p>}
      <div className="form">
        <label>{t("Title")}<input className="input" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus /></label>
        <div className="row">
          <label>{t("Date")}<input className="input" type="date" value={date} onChange={(e) => setDate(e.target.value)} /></label>
          {!allDay && <label>{t("Starts")}<input className="input" type="time" value={start} onChange={(e) => setStart(e.target.value)} /></label>}
          {!allDay && <label>{t("Ends")}<input className="input" type="time" value={end} onChange={(e) => setEnd(e.target.value)} /></label>}
        </div>
        <label className="check"><input type="checkbox" checked={allDay} onChange={(e) => setAllDay(e.target.checked)} /> {t("All day")}</label>
        <label>{t("Place")}<input className="input" value={location} onChange={(e) => setLocation(e.target.value)} /></label>
        <label>{t("Notes")}<textarea className="input" rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} /></label>
        {cals.length > 0 && (
          <label>{t("Calendar")}
            <select value={calendar} onChange={(e) => setCalendar(e.target.value)}>
              <option value="">{t("This PC only")}</option>
              {cals.map((c) => <option key={c.href} value={c.href}>{c.label}</option>)}
            </select>
          </label>
        )}
        {calendar && (
          <label>{t("Invite (email addresses)")}<input className="input" value={attendees} onChange={(e) => setAttendees(e.target.value)} placeholder={t("Your calendar may email them an invitation")} aria-label={t("Your calendar may email them an invitation")} /></label>
        )}
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={save} disabled={busy || !title.trim() || !date || (!allDay && !start)} title={!allDay && !start ? t("Add a start time, or make it all day.") : undefined}>{busy ? t("Saving…") : t("Add")}</button>
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
      toast(t("Calendar connected."), "success");
      onDone();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="form">
      <SignInButtons
        toast={toast}
        start={(p) => api.addCalendarAccountOAuth(p)}
        done={(p) => {
          toast(t("{provider} calendars connected.", { provider: p === "microsoft" ? "Outlook" : "Google" }), "success");
          onDone();
        }}
      />
      <p className="small muted or-line">{t("or connect a CalDAV calendar (iCloud, Fastmail, Nextcloud…)")}</p>
      <label>{t("Provider")}
        <select value={pick} onChange={(e) => { const i = Number(e.target.value); setPick(i); setUrl(presets[i]?.url ?? ""); }}>
          {presets.map((p, i) => <option key={p.name} value={i}>{p.name}</option>)}
        </select>
      </label>
      {presets[pick] && <p className="small hint-box">{presets[pick].note}</p>}
      <label>{t("CalDAV address")}<input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://…" /></label>
      <label>{t("User name")}<input className="input" value={username} onChange={(e) => setUsername(e.target.value)} /></label>
      <label>{t("Password or app password")}<input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="off" /></label>
      <div className="row">
        <button className="btn primary" onClick={connect} disabled={busy || !url || !username || !password}>{busy ? t("Connecting…") : t("Connect")}</button>
        <button className="btn" onClick={onDone}>{t("Cancel")}</button>
      </div>
    </div>
  );
}
