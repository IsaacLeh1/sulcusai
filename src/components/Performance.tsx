// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type HeatState, type PerfMode, type PerfSettings, type PerfView } from "../api";
import type { PushToast } from "./Toasts";

const MODES: { id: PerfMode; icon: string; label: string; detail: string }[] = [
  { id: "cool", icon: "🍃", label: "Cool & quiet", detail: "About half the processor, less graphics memory, low priority. Models unload after 5 idle minutes. Best on a laptop." },
  { id: "balanced", icon: "⚖️", label: "Balanced", detail: "Uses what the PC can comfortably spare and leaves room for your other apps." },
  { id: "turbo", icon: "🚀", label: "Turbo", detail: "You choose how much it may use, up to what this PC can safely give." },
];

export const MODE_LABEL: Record<PerfMode, string> = { cool: "Cool & quiet", balanced: "Balanced", turbo: "Turbo" };

const gb = (bytes: number) => `${(bytes / 1024 ** 3).toFixed(1)} GB`;
const deg = (t: number | null | undefined) => (t == null ? "—" : `${Math.round(t)}°C`);

export function PerformanceSection({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PerfView | null>(null);
  const load = () => api.perfView().then(setV).catch(() => {});
  useEffect(() => {
    load();
    const t = setInterval(load, 10_000);
    const sub = on("perf:heat", () => load());
    return () => {
      clearInterval(t);
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
      <h2>Performance</h2>
      <p className="muted small">How much of this PC the AI may use, and keeping it from running hot. Changes apply from the next reply.</p>
      <div className="levels" role="radiogroup" aria-label="Performance mode">
        {MODES.map((m) => (
          <button key={m.id} role="radio" aria-checked={s.mode === m.id} className={`level ${s.mode === m.id ? "active" : ""}`} onClick={() => save({ ...s, mode: m.id, turbo: m.id === "turbo" ? turbo : s.turbo })}>
            <span className="conn-icon" aria-hidden>{m.icon}</span>
            <span>
              <strong>{m.label}</strong>
              {m.id === "balanced" && <span className="badge subtle">Default</span>}
              <span className="small muted block">{m.detail}</span>
            </span>
          </button>
        ))}
      </div>

      {s.mode === "turbo" && (
        <div className="form turbo">
          <Slider
            label="Processor cores"
            value={turbo.threads}
            min={1}
            max={c.threads}
            step={1}
            show={(n) => `${n} of ${v.cores}`}
            onDone={(n) => save({ ...s, turbo: { ...turbo, threads: n } })}
          />
          {c.has_gpu && (
            <Slider
              label="Graphics memory"
              value={Math.min(turbo.gpu_percent, c.gpu_percent)}
              min={30}
              max={c.gpu_percent}
              step={1}
              show={(n) => `${n}% (${((v.vram_gb * n) / 100).toFixed(1)} of ${v.vram_gb.toFixed(1)} GB)`}
              onDone={(n) => save({ ...s, turbo: { ...turbo, gpu_percent: n } })}
            />
          )}
          <Slider
            label="System memory for models"
            value={Math.min(turbo.ram_gb, c.ram_gb)}
            min={1}
            max={c.ram_gb}
            step={0.5}
            show={(n) => `${n.toFixed(1)} of ${v.ram_total_gb.toFixed(0)} GB`}
            onDone={(n) => save({ ...s, turbo: { ...turbo, ram_gb: n } })}
          />
          <p className="muted small">The limits stop where Windows and your screen still need room, so Turbo can't take more than this PC can give.</p>
        </div>
      )}

      <div className="perf-switches">
        <Toggle
          label="Adaptive cooling"
          detail="Watches the temperatures and steps down as the PC heats up: fewer processor threads when the processor is hot, less on the graphics card when it is. Steps back up once it cools."
          checked={s.adaptive}
          onChange={(adaptive) => save({ ...s, adaptive })}
        />
        <Toggle
          label="Heat guard"
          detail={`Pauses between steps while the PC is over its limit (now ${Math.round(v.base.gpu_temp_limit)}°C graphics, ${Math.round(v.base.cpu_temp_limit)}°C processor). A safety limit stays on even when this is off.`}
          checked={s.heat_guard}
          onChange={(heat_guard) => save({ ...s, heat_guard })}
        />
        {v.has_battery && (
          <Toggle
            label="Cool & quiet on battery"
            detail="Switches to Cool & quiet while unplugged."
            checked={s.cool_on_battery}
            onChange={(cool_on_battery) => save({ ...s, cool_on_battery })}
          />
        )}
      </div>

      <div className="perf-now small">
        <strong>Right now</strong>
        <span>
          {MODE_LABEL[v.limits.mode]}
          {v.limits.on_battery && v.limits.mode !== s.mode ? " (on battery)" : ""} · {v.limits.threads} threads
          {c.has_gpu ? ` · ${gb(v.limits.vram_bytes)} graphics memory` : ""}
        </span>
        <span>
          Graphics card {deg(v.temps.gpu)} · Processor {deg(v.temps.cpu)}
          {v.temps.gpu == null && v.temps.cpu == null && " (this PC doesn't report temperatures)"}
        </span>
        {s.adaptive && <HeatLine heat={v.heat} />}
      </div>
    </section>
  );
}

function HeatLine({ heat }: { heat: HeatState }) {
  if (heat.gpu_level === 0 && heat.cpu_level === 0) return <span className="muted">Adaptive cooling: running at full speed.</span>;
  const parts = [];
  if (heat.gpu_level > 0) parts.push(`graphics card at level ${heat.gpu_level} of 3`);
  if (heat.cpu_level > 0) parts.push(`processor at level ${heat.cpu_level} of 3`);
  return <span className="warn-text">Adaptive cooling has stepped down: {parts.join(", ")}.</span>;
}

function Toggle({ label, detail, checked, onChange }: { label: string; detail: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="toggle-row">
      <div>
        <strong>{label}</strong>
        <span className="small muted block">{detail}</span>
      </div>
      <label className="switch" title={checked ? "On" : "Off"}>
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
    const t = setInterval(load, 30_000);
    const sub = on("perf:heat", (h) => setLevel(Math.max(h.gpu_level, h.cpu_level)));
    window.addEventListener("perf-changed", load);
    return () => {
      clearInterval(t);
      window.removeEventListener("perf-changed", load);
      sub.then((u) => u());
    };
  }, []);
  if (!mode) return null;
  const icon = MODES.find((m) => m.id === mode)?.icon;
  return (
    <button className={`status-item perf-pill ${level > 0 ? "hot" : ""}`} onClick={onOpen} title={level > 0 ? `Running cooler to keep the PC's temperature down (level ${level} of 3)` : "Performance mode"}>
      <span aria-hidden>{level > 0 ? "🌡️" : icon}</span> {MODE_LABEL[mode]}
      {level > 0 && ` · cooling ${level}/3`}
    </button>
  );
}
