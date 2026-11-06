// SPDX-License-Identifier: AGPL-3.0-only
// Personal life: records from mail, plans and people, health and home, mind.
import { useEffect, useState } from "react";
import { errorText } from "../api";
import {
  breathing, choreRota, deletePersonal, leaveAt, listPersonal, parseIngredients, personalEnabled, personalOverview,
  profileCheck, savePersonal, scaleRecipe, refineMail, scanMail, setPersonalEnabled, shoppingList, tasksFromText, weekIndex,
  type Ingredient, type LooseTask, type PersonalKind, type PersonalOverview, type PersonalRecord,
} from "../api/personal";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

type Tab = "overview" | "mail" | "plans" | "health" | "mind";

/** Fields each kind asks for; "date" fields set the record's date. */
const FIELDS: Partial<Record<PersonalKind, { key: string; label: () => string; type?: "number" | "date" | "text" }[]>> = {
  contact: [{ key: "name", label: () => t("Name") }, { key: "born", label: () => t("Birthday"), type: "date" }, { key: "phone", label: () => t("Phone") }],
  family: [{ key: "name", label: () => t("Name") }, { key: "note", label: () => t("Note") }],
  application: [{ key: "subject", label: () => t("Job and company") }, { key: "stage", label: () => t("Stage (applied, interview, offer, declined)") }],
  trip: [{ key: "subject", label: () => t("Where") }, { key: "date", label: () => t("Starts"), type: "date" }],
  food: [{ key: "what", label: () => t("What") }, { key: "calories", label: () => t("Calories"), type: "number" }, { key: "protein_g", label: () => t("Protein (g)"), type: "number" }],
  workout: [{ key: "what", label: () => t("What") }, { key: "minutes", label: () => t("Minutes"), type: "number" }],
  health: [{ key: "measure", label: () => t("Measure (weight, sleep, pulse...)") }, { key: "value", label: () => t("Value"), type: "number" }],
  car: [
    { key: "name", label: () => t("Car") }, { key: "last_km", label: () => t("Km at last service"), type: "number" },
    { key: "now_km", label: () => t("Km now"), type: "number" }, { key: "every_km", label: () => t("Service every km"), type: "number" },
    { key: "every_days", label: () => t("Or every days"), type: "number" },
  ],
  plant: [{ key: "name", label: () => t("Plant") }, { key: "every_days", label: () => t("Water every days"), type: "number" }],
  pet: [{ key: "name", label: () => t("Pet and care") }, { key: "every_days", label: () => t("Every days"), type: "number" }],
  journal: [{ key: "text", label: () => t("Entry") }],
};

const KIND_NAMES: Record<PersonalKind, () => string> = {
  package: () => t("Deliveries"), booking: () => t("Bookings"), event: () => t("Events"), newsletter: () => t("Newsletters"),
  birthday: () => t("Birthdays"), application: () => t("Jobs and applications"), trip: () => t("Trips"), contact: () => t("Contacts"),
  family: () => t("Family"), food: () => t("Food"), workout: () => t("Workouts"), health: () => t("Health log"), recipe: () => t("Recipes"),
  meal_plan: () => t("Meal plans"), car: () => t("Car"), plant: () => t("Plants"), pet: () => t("Pets"), journal: () => t("Journal"),
  meditation: () => t("Meditation"),
};

const day = (ms: number) => new Date(ms).toLocaleDateString();

export function PersonalView({ toast }: { toast: PushToast }) {
  const [on, setOn] = useState<boolean | null>(null);
  const [tab, setTab] = useState<Tab>("overview");
  useEffect(() => {
    personalEnabled().then(setOn).catch(() => setOn(false));
  }, []);
  const flip = async (v: boolean) => {
    try {
      await setPersonalEnabled(v);
      setOn(v);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const tabs: [Tab, string][] = [
    ["overview", t("Today")], ["mail", t("From mail")], ["plans", t("Plans and people")], ["health", t("Health and home")], ["mind", t("Mind")],
  ];
  return (
    <div className="page personal-view">
      <h2>{t("Personal life")}</h2>
      <p className="muted">{t("Deliveries, trips, birthdays, health, home and journal, kept encrypted on this PC.")}</p>
      <label className="check">
        <input type="checkbox" checked={!!on} onChange={(e) => flip(e.target.checked)} /> {t("Turned on")}
      </label>
      {on && (
        <>
          <div className="seg" role="tablist" aria-label={t("Personal life")}>
            {tabs.map(([id, label]) => (
              <button key={id} role="tab" aria-selected={tab === id} className={tab === id ? "active" : ""} onClick={() => setTab(id)}>{label}</button>
            ))}
          </div>
          {tab === "overview" && <Overview toast={toast} />}
          {tab === "mail" && <FromMail toast={toast} />}
          {tab === "plans" && <Plans toast={toast} />}
          {tab === "health" && <Health toast={toast} />}
          {tab === "mind" && <Mind toast={toast} />}
        </>
      )}
    </div>
  );
}

function Overview({ toast }: { toast: PushToast }) {
  const [o, setO] = useState<PersonalOverview | null>(null);
  useEffect(() => {
    personalOverview().then(setO).catch((e) => toast(errorText(e), "error"));
  }, [toast]);
  if (!o) return null;
  const line = (label: string, items: string[]) =>
    items.length > 0 && (
      <div className="card">
        <h4>{label}</h4>
        <ul>{items.map((x, i) => <li key={i}>{x}</li>)}</ul>
      </div>
    );
  return (
    <div className="personal-overview">
      {line(t("On the way"), o.deliveries.map((r) => `${String(r.data.carrier ?? "")} ${String(r.data.tracking ?? "")} (${day(r.at)})`))}
      {line(t("Trips"), o.trips.map((x) => `${day(x.start)} to ${day(x.end)}: ${x.bookings.length} ${t("bookings")}`))}
      {line(t("Birthdays soon"), o.birthdays.map((b) => `${b.name}: ${b.date}${b.turns ? ` (${b.turns})` : ""}, ${b.days} ${t("days")}`))}
      {line(t("Time to follow up"), o.follow_ups.map((r) => String(r.data.subject ?? "")))}
      {line(t("Care due"), [...o.care_due, ...o.car_due.map((c) => `${c}: ${t("service due")}`)])}
      <div className="card">
        <h4>{t("Today")}</h4>
        <p>{t("Calories")}: {o.today.calories} · {t("Protein (g)")}: {o.today.protein_g} · {t("Workout minutes")}: {o.today.workout_min}</p>
        <p>{t("Journal streak")}: {o.journal_streak} · {t("Meditation this week")}: {o.meditation_min_week} min</p>
        <p className="muted">{o.journal_prompt}</p>
      </div>
    </div>
  );
}

/** A list of one kind with an add form built from FIELDS. */
function Records({ kind, toast }: { kind: PersonalKind; toast: PushToast }) {
  const [rows, setRows] = useState<PersonalRecord[]>([]);
  const [form, setForm] = useState<Record<string, string>>({});
  const load = () => listPersonal(kind).then(setRows).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind]);
  const fields = FIELDS[kind] ?? [];
  const add = async () => {
    const data: Record<string, unknown> = {};
    let at = Date.now();
    for (const f of fields) {
      const v = form[f.key] ?? "";
      if (!v) continue;
      data[f.key] = f.type === "number" ? Number(v) : v;
      if (f.type === "date" && kind !== "contact") at = new Date(v).getTime();
    }
    if (kind === "plant" || kind === "pet" || kind === "car") data.last_ms = Date.now();
    try {
      await savePersonal(kind, at, data);
      setForm({});
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const remove = async (id: string) => {
    try {
      await deletePersonal(id);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const done = async (r: PersonalRecord) => {
    await savePersonal(r.kind, r.at, { ...r.data, last_ms: Date.now() }, r.id).catch((e) => toast(errorText(e), "error"));
    load();
  };
  const caring = kind === "plant" || kind === "pet" || kind === "car";
  return (
    <div className="card">
      <h4>{KIND_NAMES[kind]()}</h4>
      {fields.length > 0 && (
        <div className="row wrap">
          {fields.map((f) => (
            <input key={f.key} className="input" type={f.type ?? "text"} placeholder={f.label()} value={form[f.key] ?? ""}
              onChange={(e) => setForm((s) => ({ ...s, [f.key]: e.target.value }))} />
          ))}
          <button className="btn small" onClick={add}>{t("Add")}</button>
        </div>
      )}
      <ul>
        {rows.map((r) => (
          <li key={r.id}>
            {day(r.at)}: {Object.entries(r.data).filter(([k, v]) => k !== "mail_id" && k !== "last_ms" && v !== null && v !== "").map(([, v]) => String(v)).join(" · ")}
            {caring && <button className="btn ghost small" onClick={() => done(r)}>{kind === "car" ? t("Serviced") : t("Done today")}</button>}
            <button className="btn ghost small" onClick={() => remove(r.id)}>{t("Delete")}</button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function FromMail({ toast }: { toast: PushToast }) {
  const [n, setN] = useState(0);
  const scan = async () => {
    try {
      const added = await scanMail();
      toast(`${added} ${t("new records from mail")}`, "info");
      setN((x) => x + 1);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const refine = async () => {
    try {
      const added = await refineMail();
      toast(`${added} ${t("more records from the Mail model")}`, "info");
      setN((x) => x + 1);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div key={n}>
      <p className="muted">{t("Reads mail already on this PC for packages, bookings, events, newsletters, birthdays and applications. Nothing is sent anywhere.")}</p>
      <button className="btn" onClick={scan}>{t("Read my mail")}</button>{" "}
      <button className="btn" onClick={refine} title={t("Asks the Mail model about mail with a date that wasn't sorted")}>{t("Sort the rest with the Mail model")}</button>
      {(["package", "booking", "event", "birthday", "application", "newsletter"] as PersonalKind[]).map((k) => <Records key={k} kind={k} toast={toast} />)}
    </div>
  );
}

function Plans({ toast }: { toast: PushToast }) {
  const [start, setStart] = useState("");
  const [travel, setTravel] = useState("20");
  const [leave, setLeave] = useState<number | null>(null);
  const [chores, setChores] = useState("");
  const [people, setPeople] = useState("");
  const [rota, setRota] = useState<{ chore: string; who: string }[]>([]);
  const split = (s: string) => s.split(",").map((x) => x.trim()).filter(Boolean);
  return (
    <div>
      <div className="card">
        <h4>{t("Leave on time")}</h4>
        <div className="row wrap">
          <input className="input" type="datetime-local" value={start} onChange={(e) => setStart(e.target.value)} />
          <input className="input" type="number" value={travel} onChange={(e) => setTravel(e.target.value)} placeholder={t("Travel minutes")} />
          <button className="btn small" disabled={!start} onClick={() => leaveAt(new Date(start).getTime(), Number(travel)).then(setLeave)}>{t("When to leave")}</button>
        </div>
        {leave !== null && <p>{t("Leave at")} {new Date(leave).toLocaleTimeString()}</p>}
      </div>
      <div className="card">
        <h4>{t("Family organizer")}</h4>
        <input className="input" placeholder={t("Chores, separated by commas")} value={chores} onChange={(e) => setChores(e.target.value)} />
        <input className="input" placeholder={t("People, separated by commas")} value={people} onChange={(e) => setPeople(e.target.value)} />
        <button className="btn small" onClick={() => choreRota(split(chores), split(people), weekIndex(Date.now())).then(setRota)}>{t("This week's chores")}</button>
        <ul>{rota.map((r) => <li key={r.chore}>{r.chore}: {r.who}</li>)}</ul>
      </div>
      {(["trip", "contact", "family", "application"] as PersonalKind[]).map((k) => <Records key={k} kind={k} toast={toast} />)}
    </div>
  );
}

function Health({ toast }: { toast: PushToast }) {
  const [recipe, setRecipe] = useState("");
  const [from, setFrom] = useState("4");
  const [to, setTo] = useState("2");
  const [out, setOut] = useState<Ingredient[]>([]);
  const [week, setWeek] = useState("");
  const [shop, setShop] = useState<Ingredient[]>([]);
  const show = (i: Ingredient) => `${i.qty} ${i.unit} ${i.name}`.replace(/\s+/g, " ");
  return (
    <div>
      <p className="muted">{t("Your own numbers, added up. This isn't medical advice.")}</p>
      {(["food", "workout", "health", "car", "plant", "pet"] as PersonalKind[]).map((k) => <Records key={k} kind={k} toast={toast} />)}
      <div className="card">
        <h4>{t("Recipes")}</h4>
        <textarea className="input" rows={4} placeholder={t("One ingredient per line, like 300 g flour")} value={recipe} onChange={(e) => setRecipe(e.target.value)} />
        <div className="row">
          <input className="input" type="number" value={from} onChange={(e) => setFrom(e.target.value)} title={t("Serves")} />
          <input className="input" type="number" value={to} onChange={(e) => setTo(e.target.value)} title={t("Make for")} />
          <button className="btn small" onClick={() => scaleRecipe(parseIngredients(recipe), Number(from), Number(to)).then(setOut)}>{t("Scale")}</button>
          <button className="btn small" onClick={() => savePersonal("recipe", Date.now(), { text: recipe, serves: Number(from) }).then(() => toast(t("Saved"), "info"), (e) => toast(errorText(e), "error"))}>{t("Save recipe")}</button>
        </div>
        <ul>{out.map((i, n) => <li key={n}>{show(i)}</li>)}</ul>
      </div>
      <div className="card">
        <h4>{t("Meal plan shopping list")}</h4>
        <textarea className="input" rows={5} placeholder={t("Each meal's ingredients, meals separated by a blank line")} value={week} onChange={(e) => setWeek(e.target.value)} />
        <button className="btn small" onClick={() => shoppingList(week.split(/\n\s*\n/).map(parseIngredients)).then(setShop)}>{t("Make the list")}</button>
        <ul>{shop.map((i, n) => <li key={n}>{show(i)}</li>)}</ul>
      </div>
    </div>
  );
}

function Mind({ toast }: { toast: PushToast }) {
  const [ramble, setRamble] = useState("");
  const [tasks, setTasks] = useState<LooseTask[]>([]);
  const [profile, setProfile] = useState("");
  const [tips, setTips] = useState<string[] | null>(null);
  const [phase, setPhase] = useState<string | null>(null);
  const [minutes, setMinutes] = useState("3");
  const meditate = async () => {
    const steps = await breathing(Number(minutes), 4);
    for (const s of steps) {
      setPhase(s.name);
      await new Promise((r) => setTimeout(r, s.seconds * 1000));
    }
    setPhase(null);
    await savePersonal("meditation", Date.now(), { minutes: Number(minutes) }).catch((e) => toast(errorText(e), "error"));
  };
  return (
    <div>
      <Records kind="journal" toast={toast} />
      <div className="card">
        <h4>{t("Meditation")}</h4>
        <div className="row">
          <input className="input" type="number" value={minutes} onChange={(e) => setMinutes(e.target.value)} title={t("Minutes")} />
          <button className="btn small" disabled={!!phase} onClick={meditate}>{t("Start box breathing")}</button>
        </div>
        {phase && <p className="personal-breath">{t(phase)}</p>}
      </div>
      <div className="card">
        <h4>{t("Tasks from rambling")}</h4>
        <textarea className="input" rows={4} placeholder={t("Write whatever is on your mind")} value={ramble} onChange={(e) => setRamble(e.target.value)} />
        <button className="btn small" onClick={() => tasksFromText(ramble).then(setTasks)}>{t("Find the tasks")}</button>
        <ul>{tasks.map((x, i) => <li key={i}>{x.title}{x.due ? ` (${x.due})` : ""}</li>)}</ul>
      </div>
      <div className="card">
        <h4>{t("Profile coach")}</h4>
        <textarea className="input" rows={6} placeholder={t("Paste your resume or profile")} value={profile} onChange={(e) => setProfile(e.target.value)} />
        <button className="btn small" onClick={() => profileCheck(profile).then(setTips)}>{t("Check it")}</button>
        {tips && (tips.length ? <ul>{tips.map((x) => <li key={x}>{x}</li>)}</ul> : <p>{t("Nothing to fix found.")}</p>)}
      </div>
    </div>
  );
}
