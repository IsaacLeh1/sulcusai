// SPDX-License-Identifier: AGPL-3.0-only
// Pause completely and Quit completely (pause.rs), and "Is it at zero?":
// every SulcusAI process with what it uses (zero.rs).
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type PauseSettings, type PauseView, type ZeroView } from "../api";
import { confirmed } from "../confirm";
import { t, tx } from "../i18n";
import { everyWhileVisible } from "../visible";
import { openSettings } from "./ConnectivityMenu";
import { PauseIcon } from "./Icons";
import type { PushToast } from "./Toasts";

// What pause.rs says is running, so each part is translated.
const RUNNING = [tx("a reply"), tx("a picture, video or music job"), tx("a download"), tx("teaching a model"), tx("a meeting recording"), tx("dictation or a voice chat")];

/** "a reply, a download" in the window's language. */
export function runningText(list: string): string {
  return list
    .split(", ")
    .map((part) => (RUNNING.includes(part) ? t(part) : part))
    .join(", ");
}

/** The question asked before stopping what's running; null to go ahead. */
export function busyOf(error: string): string | null {
  return error.startsWith("busy:") ? error.slice(5) : null;
}

async function pauseAsking(toast: PushToast, running?: string) {
  try {
    if (running === undefined) {
      await api.pauseNow();
      return;
    }
  } catch (e) {
    const busy = busyOf(errorText(e));
    if (busy === null) return toast(errorText(e), "error");
    running = busy;
  }
  if (!(await confirmed(t("Pausing completely stops {what}. Stop it and pause?", { what: runningText(running ?? "") })))) return;
  await api.pauseNow(true).catch((e) => toast(errorText(e), "error"));
}

/** The status bar's Pause button (Resume on a phone that kept its link). */
export function PauseButton({ toast }: { toast: PushToast }) {
  const [paused, setPaused] = useState(false);
  useEffect(() => {
    api
      .pauseView()
      .then((v) => setPaused(v.paused))
      .catch(() => {});
    const subs = [on("pause:asks", (p) => pauseAsking(toast, p.running)), on("zero:open", () => openSettings("zero")), on("pause:changed", (p) => setPaused(p.paused))];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [toast]);
  if (paused) {
    return (
      <button className="status-item pause-pill" onClick={() => api.resumeNow().catch((e) => toast(errorText(e), "error"))} title={t("SulcusAI is paused on the PC. Resume it.")}>
        {t("Resume")}
      </button>
    );
  }
  return (
    <button
      className="status-item pause-pill"
      onClick={() => pauseAsking(toast)}
      title={t("Pause completely: models unloaded, helpers closed, connections ended and the window put away. Click the tray icon or open SulcusAI to resume.")}
      aria-label={t("Pause completely")}
    >
      <PauseIcon size={13} /> {t("Pause")}
    </button>
  );
}

function Toggle({ label, detail, checked, onChange }: { label: string; detail: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="toggle-row">
      <div>
        <strong>{label}</strong>
        <span className="small muted block">{detail}</span>
      </div>
      <label className="switch" title={checked ? t("On") : t("Off")}>
        <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} aria-label={label} />
        <span />
      </label>
    </div>
  );
}

const num = (n: number, digits = 1) => n.toFixed(digits);

/** Settings: pausing and quitting, and the "Is it at zero?" view. */
export function QuietSection({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PauseView | null>(null);
  const [zero, setZero] = useState<ZeroView | null>(null);
  const [watching, setWatching] = useState(false);
  const load = useCallback(() => api.pauseView().then(setV).catch(() => {}), []);
  useEffect(() => {
    load();
  }, [load]);
  // Every two seconds while it's open and the window shows: the first look
  // only starts the count.
  useEffect(() => {
    if (!watching) return;
    const look = () => api.zeroView().then(setZero).catch(() => {});
    look();
    return everyWhileVisible(look, 2000);
  }, [watching]);
  useEffect(() => {
    const go = (e: Event) => (e as CustomEvent<string>).detail === "zero" && setWatching(true);
    window.addEventListener("open-settings", go);
    return () => window.removeEventListener("open-settings", go);
  }, []);
  if (!v) return null;
  const s = v.settings;
  const save = async (next: PauseSettings) => {
    setV({ ...v, settings: next });
    try {
      setV(await api.setPauseSettings(next));
    } catch (e) {
      toast(errorText(e), "error");
      load();
    }
  };

  return (
    <section className="card">
      <h2>{t("Pause and quit")}</h2>
      <p className="muted small">
        {t("Pause completely puts SulcusAI fully to sleep: models unloaded, helper programs closed, the phone link, People, Telegram and the wake word disconnected, and its windows put away. It wakes with one click on its tray icon, or by opening it. Quit completely ends everything.")}
      </p>
      <div className="row wrap">
        <button className="btn" onClick={() => pauseAsking(toast)}>
          {t("Pause completely")}
        </button>
        <button className="btn ghost" onClick={async () => (await confirmed(t("Quit SulcusAI completely?"))) && api.quitNow()}>
          {t("Quit completely")}
        </button>
      </div>
      <div className="perf-switches">
        <Toggle
          label={t("Pause and quit from the Start menu, and a hotkey")}
          detail={t("Right-click SulcusAI in the Start menu for Pause completely, Resume and Quit completely. {hotkey} pauses or resumes from anywhere.", { hotkey: v.hotkey })}
          checked={s.shortcuts}
          onChange={(shortcuts) => save({ ...s, shortcuts })}
        />
        <Toggle
          label={t("Keep the phone link while paused")}
          detail={t("The phone can then see that this PC is paused and wake it. It uses a small keep-alive every 25 seconds.")}
          checked={s.keep_phone}
          onChange={(keep_phone) => save({ ...s, keep_phone })}
        />
        <Toggle
          label={t("Run scheduled tasks after quitting")}
          detail={t("When SulcusAI quits, Windows' Task Scheduler starts it again in the tray for the next scheduled task, and it quits again when that's done. Tasks set off by mail, events or files still need SulcusAI open.")}
          checked={s.tasks_after_quit}
          onChange={(tasks_after_quit) => save({ ...s, tasks_after_quit })}
        />
      </div>

      <span id="settings-zero" className="settings-anchor" />
      <h3>{t("Is it at zero?")}</h3>
      <p className="muted small">{t("Every SulcusAI process, its window's and any engine's, with what each uses right now. Check it yourself while SulcusAI sits idle.")}</p>
      {!watching ? (
        <button className="btn ghost small" onClick={() => setWatching(true)}>
          {t("Show it")}
        </button>
      ) : !zero || zero.seconds === 0 ? (
        <p className="muted small">{t("Measuring…")}</p>
      ) : (
        <>
          <p className={zero.at_zero ? "ok-text" : "small"}>
            {zero.at_zero ? t("At zero: no processor work, no disk writes, nothing on the graphics card and no connections.") : t("Not at zero right now: see what's using something below.")}
          </p>
          <table className="zero-table small">
            <thead>
              <tr>
                <th>{t("Process")}</th>
                <th>{t("Processor (% of one core)")}</th>
                <th>{t("Memory (MB)")}</th>
                <th>{t("Disk writes (KB)")}</th>
                <th>{t("Graphics memory (MB)")}</th>
                <th>{t("Connections")}</th>
              </tr>
            </thead>
            <tbody>
              {[...zero.procs, zero.total].map((p, i) => (
                <tr key={p.pid + ":" + i} className={i === zero.procs.length ? "total" : ""}>
                  <td>{i === zero.procs.length ? t("All of SulcusAI") : p.name}</td>
                  <td>{num(p.cpu, 2)}</td>
                  <td>{num(p.memory_mb, 0)}</td>
                  <td>{num(p.written_kb)}</td>
                  <td>{num(p.card_mb)}</td>
                  <td>{p.connections}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="muted small">{t("Measured over the last {n} seconds. Looking costs a little itself.", { n: num(zero.seconds, 0) })}</p>
          {zero.still_on.length > 0 && (
            <>
              <strong className="small">{t("Still on because you turned it on")}</strong>
              <ul className="small">
                {zero.still_on.map((x) => (
                  <li key={x}>{x}</li>
                ))}
              </ul>
            </>
          )}
          <button className="btn ghost small" onClick={() => setWatching(false)}>
            {t("Stop measuring")}
          </button>
        </>
      )}
    </section>
  );
}
