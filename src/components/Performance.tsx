// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type GpuLimit, type GpuNow, type HeatState, type PerfMode, type PerfSettings, type PerfView, type ProcessorFallback } from "../api";
import { bytes } from "../format";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

const MODES: { id: PerfMode; label: string; detail: string }[] = [
  { id: "cool", label: tx("Cool & quiet"), detail: tx("About half the processor, less of the chat kept in mind, low priority, and still the whole graphics card. Models unload after 5 idle minutes. Best on a laptop.") },
  { id: "balanced", label: tx("Balanced"), detail: tx("Uses what the PC can comfortably spare and leaves room for your other apps.") },
  { id: "turbo", label: tx("Turbo"), detail: tx("You choose how much it may use, up to what this PC can safely give.") },
];

export const MODE_LABEL: Record<PerfMode, string> = { cool: tx("Cool & quiet"), balanced: tx("Balanced"), turbo: tx("Turbo") };

/** Minutes an idle model stays loaded in each mode (0 = never unloaded); perf.rs `limits`. */
const MODE_UNLOAD: Record<PerfMode, number> = { cool: 5, balanced: 10, turbo: 0 };

const gb = (bytes: number) => `${(bytes / 1024 ** 3).toFixed(1)} GB`;
const deg = (temp: number | null | undefined) => (temp == null ? "-" : `${Math.round(temp)}°C`);

export function PerformanceSection({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PerfView | null>(null);
  const load = () => api.perfView().then(setV).catch(() => {});
  useEffect(() => {
    load();
    // Not while the window is hidden; caught up when it shows again.
    const t = setInterval(() => !document.hidden && load(), 10_000);
    const shown = () => !document.hidden && load();
    document.addEventListener("visibilitychange", shown);
    const sub = on("perf:heat", () => load());
    return () => {
      clearInterval(t);
      document.removeEventListener("visibilitychange", shown);
      sub.then((u) => u());
    };
  }, []);
  if (!v) return null;
  const s = v.settings;
  const c = v.ceiling;
  const turbo = s.turbo ?? { threads: c.threads, gpu_percent: c.gpu_percent, ram_gb: c.ram_gb };

  const save = async (next: PerfSettings) => {
    setV({ ...v, settings: next });
    try {
      await api.setPerf(next);
      window.dispatchEvent(new Event("perf-changed"));
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>{t("Performance")}</h2>
      <p className="muted small">{t("How much of this PC the AI may use, and keeping it from running hot. Changes apply from the next reply.")}</p>
      <div className="levels" role="radiogroup" aria-label={t("Performance mode")}>
        {MODES.map((m) => (
          <button key={m.id} role="radio" aria-checked={s.mode === m.id} className={`level ${s.mode === m.id ? "active" : ""}`} onClick={() => save({ ...s, mode: m.id, turbo: m.id === "turbo" ? turbo : s.turbo })}>
            <span>
              <strong>{t(m.label)}</strong>
              {m.id === "balanced" && <span className="badge subtle">{t("Default")}</span>}
              <span className="small muted block">{t(m.detail)}</span>
            </span>
          </button>
        ))}
      </div>

      {s.mode === "turbo" && (
        <div className="form turbo">
          <Slider
            label={t("Processor cores")}
            value={turbo.threads}
            min={1}
            max={c.threads}
            step={1}
            show={(n) => t("{n} of {total}", { n, total: v.cores })}
            onDone={(n) => save({ ...s, turbo: { ...turbo, threads: n } })}
          />
          {c.has_gpu && (
            <Slider
              label={t("Graphics memory")}
              value={Math.min(turbo.gpu_percent, c.gpu_percent)}
              min={30}
              max={c.gpu_percent}
              step={1}
              show={(n) => t("{n}% ({used} of {total} GB)", { n, used: ((v.vram_gb * n) / 100).toFixed(1), total: v.vram_gb.toFixed(1) })}
              onDone={(n) => save({ ...s, turbo: { ...turbo, gpu_percent: n } })}
            />
          )}
          <Slider
            label={t("System memory for models")}
            value={Math.min(turbo.ram_gb, c.ram_gb)}
            min={1}
            max={c.ram_gb}
            step={0.5}
            show={(n) => t("{n} of {total} GB", { n: n.toFixed(1), total: v.ram_total_gb.toFixed(0) })}
            onDone={(n) => save({ ...s, turbo: { ...turbo, ram_gb: n } })}
          />
          <p className="muted small">{t("The limits stop where Windows and your screen still need room, so Turbo can't take more than this PC can give.")}</p>
        </div>
      )}

      <div className="perf-switches">
        <Toggle
          label={t("Adaptive cooling")}
          detail={t("Watches the temperatures and steps down as the PC heats up: fewer processor threads when the processor is hot, short rests when the graphics card is. The model stays on the graphics card. Steps back up once it cools.")}
          checked={s.adaptive}
          onChange={(adaptive) => save({ ...s, adaptive })}
        />
        <Toggle
          label={t("Heat guard")}
          detail={t("Pauses between steps while the PC is over its limit (now {gpu}°C graphics, {cpu}°C processor). A safety limit stays on even when this is off.", { gpu: Math.round(v.base.gpu_temp_limit), cpu: Math.round(v.base.cpu_temp_limit) })}
          checked={s.heat_guard}
          onChange={(heat_guard) => save({ ...s, heat_guard })}
        />
        {v.has_battery && (
          <Toggle
            label={t("Cool & quiet on battery")}
            detail={t("Switches to Cool & quiet while unplugged.")}
            checked={s.cool_on_battery}
            onChange={(cool_on_battery) => save({ ...s, cool_on_battery })}
          />
        )}
        <div className="toggle-row">
          <div>
            <strong>{t("Free the graphics card when idle")}</strong>
            <span className="small muted block">
              {t("A loaded model keeps the card's memory until it's unloaded. Unloading frees it for games and other apps; the next reply then waits a few seconds for the model to load again.")}
            </span>
          </div>
          <select
            value={s.unload_right_away ? "now" : s.unload_after_mins == null ? "mode" : String(s.unload_after_mins)}
            aria-label={t("Free the graphics card when idle")}
            onChange={(e) =>
              e.target.value === "now"
                ? save({ ...s, unload_right_away: true })
                : save({ ...s, unload_right_away: false, unload_after_mins: e.target.value === "mode" ? null : Number(e.target.value) })
            }
          >
            <option value="now">{t("Right after each reply (the next one loads it again, a few seconds)")}</option>
            <option value="mode">{t("As the mode sets it ({n})", { n: MODE_UNLOAD[s.mode] === 0 ? t("never") : t("after {n} min", { n: MODE_UNLOAD[s.mode] }) })}</option>
            {[1, 2, 5, 10, 30].map((m) => (
              <option key={m} value={m}>
                {t("After {n} minutes idle", { n: m })}
              </option>
            ))}
            <option value="0">{t("Never (keep it loaded)")}</option>
          </select>
        </div>
        <Toggle
          label={t("Keep the model used before loaded too")}
          detail={t("Switching models, the one used before stays loaded while the graphics card has room for both, so switching back needs no loading. Anything that needs the card (pictures, video, a game) frees both.")}
          checked={s.keep_two ?? true}
          onChange={(keep_two) => save({ ...s, keep_two })}
        />
        <ProcessorFallbackRow toast={toast} />
      </div>

      <div className="perf-now small">
        <strong>{t("Right now")}</strong>
        <span>
          {t(MODE_LABEL[v.limits.mode])}
          {v.limits.on_battery && v.limits.mode !== s.mode ? " " + t("(on battery)") : ""} · {t("{n} threads", { n: v.limits.threads })}
          {c.has_gpu ? ` · ${t("{size} graphics memory", { size: gb(v.limits.vram_bytes) })}` : ""}
        </span>
        <span>
          {t("Graphics card {gpu} · Processor {cpu}", { gpu: deg(v.temps.gpu), cpu: deg(v.temps.cpu) })}
          {v.temps.gpu == null && v.temps.cpu == null && " " + t("(this PC doesn't report temperatures)")}
        </span>
        {s.adaptive && <HeatLine heat={v.heat} />}
      </div>
    </section>
  );
}

/** What happens when work meant for the graphics card can't go there. */
function ProcessorFallbackRow({ toast }: { toast: PushToast }) {
  const [value, setValue] = useState<ProcessorFallback | null>(null);
  useEffect(() => {
    api.processorFallback().then(setValue).catch(() => {});
  }, []);
  if (!value) return null;
  return (
    <div className="toggle-row">
      <div>
        <strong>{t("When the graphics card can't do a job")}</strong>
        <span className="small muted block">
          {t("Work runs on the graphics card first. When it can't (no card, an old driver, its memory full), the processor and system memory can do it instead: slower, often several times, and warmer. Why is kept in the app log.")}
        </span>
      </div>
      <select
        value={value}
        aria-label={t("When the graphics card can't do a job")}
        onChange={async (e) => {
          const next = e.target.value as ProcessorFallback;
          setValue(next);
          try {
            await api.setProcessorFallback(next);
          } catch (err) {
            toast(errorText(err), "error");
          }
        }}
      >
        <option value="ask">{t("Ask me first")}</option>
        <option value="always">{t("Use the processor")}</option>
        <option value="never">{t("Stop the job")}</option>
      </select>
    </div>
  );
}

function HeatLine({ heat }: { heat: HeatState }) {
  if (heat.gpu_level === 0 && heat.cpu_level === 0) return <span className="muted">{t("Adaptive cooling: running at full speed.")}</span>;
  const parts = [];
  if (heat.gpu_level > 0) parts.push(t("graphics card at level {n} of 3", { n: heat.gpu_level }));
  if (heat.cpu_level > 0) parts.push(t("processor at level {n} of 3", { n: heat.cpu_level }));
  return <span className="warn-text">{t("Adaptive cooling has stepped down: {parts}.", { parts: parts.join(", ") })}</span>;
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

function Slider(props: { label: string; value: number; min: number; max: number; step: number; show: (n: number) => string; onDone: (n: number) => void }) {
  const [n, setN] = useState(props.value);
  useEffect(() => setN(props.value), [props.value]);
  return (
    <label>
      <span className="row between">
        {props.label} <span className="muted small">{props.show(n)}</span>
      </span>
      <input
        type="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={n}
        onChange={(e) => setN(Number(e.target.value))}
        onPointerUp={() => props.onDone(n)}
        onKeyUp={() => props.onDone(n)}
      />
    </label>
  );
}

/** Status-bar pill: the mode, and a warning while adaptive cooling has stepped down. */
export function PerfPill({ onOpen }: { onOpen: () => void }) {
  const [mode, setMode] = useState<PerfMode | null>(null);
  const [level, setLevel] = useState(0);
  useEffect(() => {
    const load = () =>
      api
        .perfView()
        .then((v) => {
          setMode(v.limits.mode);
          setLevel(v.limits.heat_level);
        })
        .catch(() => {});
    load();
    // For the switch to Cool & quiet on battery. Not while the window is
    // hidden (it reads the temperatures too); caught up when it shows again.
    const t = setInterval(() => !document.hidden && load(), 30_000);
    const shown = () => !document.hidden && load();
    document.addEventListener("visibilitychange", shown);
    const sub = on("perf:heat", (h) => setLevel(Math.max(h.gpu_level, h.cpu_level)));
    window.addEventListener("perf-changed", load);
    return () => {
      clearInterval(t);
      document.removeEventListener("visibilitychange", shown);
      window.removeEventListener("perf-changed", load);
      sub.then((u) => u());
    };
  }, []);
  if (!mode) return null;
  return (
    <button className={`status-item perf-pill ${level > 0 ? "hot" : ""}`} onClick={onOpen} title={level > 0 ? t("Running cooler to keep the PC's temperature down (level {n} of 3)", { n: level }) : t("Performance mode")}>
      {t(MODE_LABEL[mode])}
      {level > 0 && ` · ${t("cooling {n}/3", { n: level })}`}
    </button>
  );
}

const GPU_STEPS = [100, 75, 50, 25, 0];

/** Top of a chat: how much of the model may run on the graphics card. The
 * rest runs on the processor: less load and memory on the card (for a game,
 * or to keep it cool), slower replies. Applies from the next reply. */
export function GpuLimitMenu({ toast }: { toast: PushToast }) {
  const [limit, setLimit] = useState<GpuLimit | null>(null);
  useEffect(() => {
    api.gpuLimit().then(setLimit).catch(() => {});
    const sub = on("gpu:limit", (p) => setLimit((l) => (l ? { ...l, percent: p.percent } : l)));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!limit?.has_gpu) return null;
  const steps = GPU_STEPS.includes(limit.percent) ? GPU_STEPS : [...GPU_STEPS, limit.percent].sort((a, b) => b - a);
  return (
    <label
      className="gpu-limit"
      title={t("How much of the model runs on the graphics card. Lower leaves the card freer and cooler (for a game, say). The processor is held back to match, so it doesn't run hot instead, and replies are slower. Applies from the next reply.")}
    >
      <span>{t("GPU")}</span>
      <select
        value={limit.percent}
        aria-label={t("How much of the model runs on the graphics card")}
        onChange={async (e) => {
          const percent = Number(e.target.value);
          try {
            setLimit(await api.setGpuLimit(percent));
            toast(percent === 0 ? t("From the next reply, the model runs on the processor only, on a quarter of it so it stays cool (slowly).") : t("From the next reply: up to {n}% of the model on the graphics card, and the processor held back to match.", { n: percent }), "success");
          } catch (err) {
            toast(errorText(err), "error");
          }
        }}
      >
        {steps.map((p) => (
          <option key={p} value={p}>
            {p === 0 ? t("Off") : `${p}%`}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Bottom bar: how much SulcusAI is using the graphics card, every few
 * seconds (not while the window is hidden), and, while it works, how busy
 * and hot the whole card is. Amber from 80°C. At rest it reads Windows'
 * counters only: the temperature comes from nvidia-smi, and starting that
 * wakes the card from idle. */
export function GpuPill({ onOpen }: { onOpen: () => void }) {
  const [g, setG] = useState<GpuNow | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => {
      if (document.hidden) return;
      api
        .gpuNow()
        .then((x) => alive && setG(x))
        .catch(() => {});
    };
    load();
    const timer = setInterval(load, 5000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);
  if (!g || (g.usage === null && g.temp === null && g.ours === null)) return null;
  const hot = (g.temp ?? 0) >= 80;
  // SulcusAI's own use; the whole card's while it works (its own is most of it then).
  const shown = g.working ? g.usage ?? g.ours : g.ours ?? g.usage;
  const others = g.usage !== null && g.ours !== null ? Math.max(0, g.usage - g.ours) : null;
  const parts = [
    g.ours !== null ? t("SulcusAI: {n}%", { n: Math.round(g.ours) }) : null,
    g.usage !== null ? t("all apps: {n}%", { n: Math.round(g.usage) }) : null,
    g.temp !== null ? `${Math.round(g.temp)}°C` : null,
    g.mem_used !== null && g.mem_total ? t("{used} of {total} memory in use", { used: bytes(g.mem_used), total: bytes(g.mem_total) }) : null,
    !g.working && others !== null && others >= 5 ? t("Other apps are using the card; SulcusAI is resting.") : null,
  ].filter(Boolean);
  return (
    <button className={`status-item perf-pill gpu-pill ${hot ? "hot" : ""}`} onClick={onOpen} title={`${g.name ?? t("Graphics card")}: ${parts.join(" · ")}`}>
      <span aria-hidden>▦</span> {t("GPU")}
      {shown !== null && <span className="gpu-num">{Math.round(shown)}%</span>}
      {g.temp !== null && <span className="gpu-num">{Math.round(g.temp)}°C</span>}
      {!g.working && <span className="gpu-num muted">{t("resting")}</span>}
    </button>
  );
}
