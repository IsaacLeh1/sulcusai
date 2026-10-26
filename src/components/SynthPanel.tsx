// SPDX-License-Identifier: AGPL-3.0-only
// The wavetable synth's controls (Suno Studio's synth): two wavetable
// oscillators, a sub and noise, the filter, four envelopes, three LFOs, the
// mod matrix and how it's played; factory and saved sounds; wavetables made
// from a sound file or from a clip.
import { useEffect, useRef, useState } from "react";
import { api, errorText } from "../api";
import { t, tx } from "../i18n";
import { frameOf, initPatch, PRESETS, TABLES, type Env, type Lfo, type MadeTable, type ModSource, type ModTarget, type Osc, type SynthPatch } from "../studio/synth";

const WARPS: [Osc["warp"], string][] = [["none", tx("No warp")], ["bend", tx("Bend")], ["sync", tx("Sync")], ["pwm", tx("Pulse width")], ["mirror", tx("Mirror")]];
const SOURCES: [ModSource, string][] = [["env3", tx("Envelope 3")], ["env4", tx("Envelope 4")], ["lfo1", tx("LFO 1")], ["lfo2", tx("LFO 2")], ["lfo3", tx("LFO 3")], ["velocity", tx("Velocity")], ["modwheel", tx("Mod wheel")], ["note", tx("Note (key)")], ["random", tx("Random per note")]];
const TARGETS: [ModTarget, string][] = [["pitch", tx("Pitch")], ["osc1.pitch", tx("Osc 1 pitch")], ["osc2.pitch", tx("Osc 2 pitch")], ["osc1.pos", tx("Osc 1 position")], ["osc2.pos", tx("Osc 2 position")], ["osc1.warp", tx("Osc 1 warp")], ["osc2.warp", tx("Osc 2 warp")], ["osc1.level", tx("Osc 1 level")], ["osc2.level", tx("Osc 2 level")], ["sub.level", tx("Sub level")], ["cutoff", tx("Cutoff")], ["resonance", tx("Resonance")], ["volume", tx("Volume")], ["pan", tx("Pan")]];
const DIVS: [number, string][] = [[16, tx("4 bars")], [8, tx("2 bars")], [4, tx("1 bar")], [2, "1/2"], [1, "1/4"], [0.5, "1/8"], [0.25, "1/16"], [1 / 3, "1/8T"], [0.125, "1/32"]];
const SHAPES: [Lfo["shape"], string][] = [["sine", tx("Sine")], ["triangle", tx("Triangle")], ["square", tx("Square")], ["saw", tx("Saw")], ["random", tx("Random")]];

/** One setting as a slider: straight, on a curve (`curve` > 1 gives the low end more room) or logarithmic. */
function Param({ label, value, min, max, step = 0, curve = 1, log, unit = "", digits = 2, onChange, onDone }: { label: string; value: number; min: number; max: number; step?: number; curve?: number; log?: boolean; unit?: string; digits?: number; onChange: (v: number) => void; onDone: () => void }) {
  const toU = (v: number) => (log ? Math.log(v / min) / Math.log(max / min) : Math.pow((v - min) / (max - min), 1 / curve));
  const fromU = (u: number) => {
    const v = log ? min * Math.pow(max / min, u) : min + (max - min) * Math.pow(u, curve);
    return step ? Math.round(v / step) * step : v;
  };
  const shown = Math.abs(value) >= 1000 ? `${(value / 1000).toFixed(1)}k` : value.toFixed(step >= 1 || Math.abs(value) >= 100 ? 0 : Math.abs(value) >= 10 ? 1 : digits);
  return (
    <label className="synth-param" title={`${label}: ${shown}${unit}`}>
      <span className="synth-param-name">{label}</span>
      <input type="range" min={0} max={1} step={0.001} value={Math.max(0, Math.min(1, toU(value) || 0))} onChange={(e) => onChange(fromU(+e.target.value))} onPointerUp={onDone} onKeyUp={onDone} aria-label={label} />
      <span className="synth-param-value mono">{shown}{unit}</span>
    </label>
  );
}

/** A frame of a wavetable, drawn small. */
function Frame({ table, pos, made }: { table: string; pos: number; made?: Record<string, MadeTable> }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = ref.current;
    const f = frameOf(table, pos, made);
    if (!c) return;
    const g = c.getContext("2d")!;
    g.clearRect(0, 0, c.width, c.height);
    if (!f) return;
    g.strokeStyle = "#6d5dfc";
    g.lineWidth = 1.5;
    g.beginPath();
    for (let x = 0; x < c.width; x++) {
      const v = f[Math.floor((x / c.width) * f.length)];
      const y = c.height / 2 - v * (c.height / 2 - 2);
      if (x) g.lineTo(x, y);
      else g.moveTo(x, y);
    }
    g.stroke();
  }, [table, pos, made]);
  return <canvas ref={ref} width={140} height={44} className="synth-frame" />;
}

export function SynthPanel({ track, patch, made, onPatch, onMakeTable, canCapture, toast }: {
  /** The track it plays on, by name. */
  track: string;
  patch: SynthPatch;
  made: Record<string, MadeTable>;
  /** The sound changed: `done` when a slider is let go (one step to undo). */
  onPatch: (p: SynthPatch, done: boolean) => void;
  /** A wavetable made for an oscillator: from a sound file, or from the selected clip. */
  onMakeTable: (from: "file" | "clip") => Promise<string | null>;
  canCapture: boolean;
  toast: (text: string, kind: "error" | "success") => void;
}) {
  const [saved, setSaved] = useState<SynthPatch[]>([]);
  const [name, setName] = useState("");
  useEffect(() => {
    api.studioSynthPresets().then((list) => setSaved(list as SynthPatch[])).catch(() => {});
  }, []);
  const set = (change: (p: SynthPatch) => void, done = true) => {
    const next: SynthPatch = structuredClone(patch);
    change(next);
    onPatch(next, done);
  };
  const done = () => onPatch(patch, true);
  const osc = (k: "osc1" | "osc2", label: string) => {
    const o = patch[k];
    const live = (f: (o: Osc) => void) => set((p) => f(p[k]), false);
    return (
      <div className="synth-box">
        <div className="row">
          <label className="row small"><input type="checkbox" checked={o.on} onChange={(e) => set((p) => (p[k].on = e.target.checked))} /> <strong>{label}</strong></label>
          <select
            value={o.table}
            onChange={async (e) => {
              const v = e.target.value;
              if (v === "#file" || v === "#clip") {
                const id = await onMakeTable(v === "#file" ? "file" : "clip");
                if (id) set((p) => ((p[k].table = id), (p[k].pos = 0)));
                return;
              }
              set((p) => (p[k].table = v));
            }}
            aria-label={t("Wavetable")}
          >
            {Object.entries(TABLES).map(([id, tb]) => (
              <option key={id} value={id}>{t(tb.label)}</option>
            ))}
            {Object.entries(made).map(([id, tb]) => (
              <option key={id} value={id}>{tb.name}</option>
            ))}
            <option value="#file">{t("New from a sound file…")}</option>
            {canCapture && <option value="#clip">{t("New from the selected clip")}</option>}
          </select>
        </div>
        <Frame table={o.table} pos={o.pos} made={made} />
        <Param label={t("Position")} value={o.pos} min={0} max={1} onChange={(v) => live((x) => (x.pos = v))} onDone={done} />
        <div className="row small">
          <select value={o.warp} onChange={(e) => set((p) => (p[k].warp = e.target.value as Osc["warp"]))} aria-label={t("Warp")}>
            {WARPS.map(([w, n]) => (
              <option key={w} value={w}>{t(n)}</option>
            ))}
          </select>
        </div>
        {o.warp !== "none" && <Param label={t("Warp")} value={o.warpAmt} min={0} max={1} onChange={(v) => live((x) => (x.warpAmt = v))} onDone={done} />}
        <Param label={t("Level")} value={o.level} min={0} max={1} onChange={(v) => live((x) => (x.level = v))} onDone={done} />
        <Param label={t("Octave")} value={o.octave} min={-3} max={3} step={1} onChange={(v) => live((x) => (x.octave = v))} onDone={done} />
        <Param label={t("Semitones")} value={o.semi} min={-12} max={12} step={1} onChange={(v) => live((x) => (x.semi = v))} onDone={done} />
        <Param label={t("Fine")} value={o.fine} min={-100} max={100} step={1} unit="¢" onChange={(v) => live((x) => (x.fine = v))} onDone={done} />
        <Param label={t("Unison")} value={o.unison} min={1} max={7} step={1} onChange={(v) => live((x) => (x.unison = v))} onDone={done} />
        <Param label={t("Detune")} value={o.detune} min={0} max={100} step={1} unit="¢" onChange={(v) => live((x) => (x.detune = v))} onDone={done} />
        <Param label={t("Spread")} value={o.spread} min={0} max={1} onChange={(v) => live((x) => (x.spread = v))} onDone={done} />
        <Param label={t("Pan")} value={o.pan} min={-1} max={1} onChange={(v) => live((x) => (x.pan = v))} onDone={done} />
      </div>
    );
  };
  const envBox = (k: "amp" | "fenv" | "env3" | "env4", label: string) => {
    const e = patch[k];
    const live = (f: (e: Env) => void) => set((p) => f(p[k]), false);
    return (
      <div className="synth-env">
        <strong className="small">{label}</strong>
        <Param label={t("Attack")} value={e.a} min={0} max={10} curve={3} unit=" s" onChange={(v) => live((x) => (x.a = v))} onDone={done} />
        <Param label={t("Decay")} value={e.d} min={0} max={10} curve={3} unit=" s" onChange={(v) => live((x) => (x.d = v))} onDone={done} />
        <Param label={t("Sustain")} value={e.s} min={0} max={1} onChange={(v) => live((x) => (x.s = v))} onDone={done} />
        <Param label={t("Release")} value={e.r} min={0} max={10} curve={3} unit=" s" onChange={(v) => live((x) => (x.r = v))} onDone={done} />
      </div>
    );
  };
  const lfoBox = (i: 0 | 1 | 2) => {
    const l = patch.lfos[i];
    return (
      <div className="synth-env" key={i}>
        <div className="row small">
          <strong>{t("LFO {n}", { n: i + 1 })}</strong>
          <select value={l.shape} onChange={(e) => set((p) => (p.lfos[i].shape = e.target.value as Lfo["shape"]))} aria-label={t("Shape")}>
            {SHAPES.map(([s, n]) => (
              <option key={s} value={s}>{t(n)}</option>
            ))}
          </select>
        </div>
        <div className="row small">
          <label className="row small"><input type="checkbox" checked={l.sync} onChange={(e) => set((p) => (p.lfos[i].sync = e.target.checked))} /> {t("To the tempo")}</label>
          <label className="row small"><input type="checkbox" checked={l.retrig} onChange={(e) => set((p) => (p.lfos[i].retrig = e.target.checked))} /> {t("Each note")}</label>
        </div>
        {l.sync ? (
          <select value={l.div} onChange={(e) => set((p) => (p.lfos[i].div = +e.target.value))} aria-label={t("Rate")}>
            {DIVS.map(([d, n]) => (
              <option key={n} value={d}>{t(n)}</option>
            ))}
          </select>
        ) : (
          <Param label={t("Rate")} value={l.rate} min={0.02} max={40} log unit=" Hz" onChange={(v) => set((p) => (p.lfos[i].rate = v), false)} onDone={done} />
        )}
      </div>
    );
  };
  const presetValue = [...PRESETS, ...saved].find((x) => x.name === patch.name) ? patch.name : "";
  return (
    <div className="synth-panel">
      <div className="row wrap">
        <strong>{t("Wavetable synth on {track}", { track })}</strong>
        <select
          value={presetValue}
          onChange={(e) => {
            const found = [...PRESETS, ...saved].find((x) => x.name === e.target.value);
            if (found) onPatch(structuredClone(found), true);
          }}
          aria-label={t("Sound")}
        >
          <option value="">{t("Sounds…")}</option>
          <optgroup label={t("Factory")}>
            {PRESETS.map((x) => (
              <option key={x.name} value={x.name}>{x.name}</option>
            ))}
          </optgroup>
          {saved.length > 0 && (
            <optgroup label={t("Saved")}>
              {saved.map((x) => (
                <option key={`s-${x.name}`} value={x.name}>{x.name}</option>
              ))}
            </optgroup>
          )}
        </select>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Name to save it as")} aria-label={t("Name to save it as")} />
        <button
          className="btn small"
          disabled={!name.trim()}
          onClick={async () => {
            const p = { ...patch, name: name.trim() };
            try {
              await api.studioSynthPresetSave(p);
              setSaved((s) => [...s.filter((x) => x.name !== p.name), p]);
              onPatch(p, true);
              setName("");
              toast(t("Saved as {name}.", { name: p.name }), "success");
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        >
          {t("Save the sound")}
        </button>
        {saved.some((x) => x.name === patch.name) && (
          <button
            className="btn ghost small"
            onClick={async () => {
              try {
                await api.studioSynthPresetDelete(patch.name);
                setSaved((s) => s.filter((x) => x.name !== patch.name));
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            {t("Delete saved sound")}
          </button>
        )}
        <button className="btn ghost small" onClick={() => onPatch(initPatch(), true)}>{t("Start over")}</button>
      </div>
      <div className="synth-grid">
        {osc("osc1", t("Osc 1"))}
        {osc("osc2", t("Osc 2"))}
        <div className="synth-box">
          <div className="row">
            <label className="row small"><input type="checkbox" checked={patch.sub.on} onChange={(e) => set((p) => (p.sub.on = e.target.checked))} /> <strong>{t("Sub")}</strong></label>
            <select value={patch.sub.shape} onChange={(e) => set((p) => (p.sub.shape = e.target.value as "sine" | "square"))} aria-label={t("Shape")}>
              <option value="sine">{t("Sine")}</option>
              <option value="square">{t("Square")}</option>
            </select>
          </div>
          <Param label={t("Octave")} value={patch.sub.octave} min={-2} max={0} step={1} onChange={(v) => set((p) => (p.sub.octave = v), false)} onDone={done} />
          <Param label={t("Level")} value={patch.sub.level} min={0} max={1} onChange={(v) => set((p) => (p.sub.level = v), false)} onDone={done} />
          <Param label={t("Noise")} value={patch.noise} min={0} max={1} onChange={(v) => set((p) => (p.noise = v), false)} onDone={done} />
          <div className="row">
            <label className="row small"><input type="checkbox" checked={patch.filter.on} onChange={(e) => set((p) => (p.filter.on = e.target.checked))} /> <strong>{t("Filter")}</strong></label>
            <select value={patch.filter.type} onChange={(e) => set((p) => (p.filter.type = e.target.value as SynthPatch["filter"]["type"]))} aria-label={t("Filter")}>
              <option value="lowpass">{t("Low-pass")}</option>
              <option value="highpass">{t("High-pass")}</option>
              <option value="bandpass">{t("Band-pass")}</option>
              <option value="notch">{t("Notch")}</option>
            </select>
          </div>
          <Param label={t("Cutoff")} value={patch.filter.cutoff} min={20} max={20000} log unit=" Hz" onChange={(v) => set((p) => (p.filter.cutoff = v), false)} onDone={done} />
          <Param label={t("Resonance")} value={patch.filter.resonance} min={0} max={1} onChange={(v) => set((p) => (p.filter.resonance = v), false)} onDone={done} />
          <Param label={t("Envelope")} value={patch.filter.env} min={-6} max={6} unit=" oct" onChange={(v) => set((p) => (p.filter.env = v), false)} onDone={done} />
          <Param label={t("Key follow")} value={patch.filter.keyTrack} min={0} max={1} onChange={(v) => set((p) => (p.filter.keyTrack = v), false)} onDone={done} />
          <Param label={t("Drive")} value={patch.filter.drive} min={0} max={1} onChange={(v) => set((p) => (p.filter.drive = v), false)} onDone={done} />
        </div>
        <div className="synth-box">
          {envBox("amp", t("Amp envelope"))}
          {envBox("fenv", t("Filter envelope"))}
        </div>
        <div className="synth-box">
          {envBox("env3", t("Envelope 3"))}
          {envBox("env4", t("Envelope 4"))}
        </div>
        <div className="synth-box">
          {lfoBox(0)}
          {lfoBox(1)}
          {lfoBox(2)}
        </div>
        <div className="synth-box">
          <strong className="small">{t("Modulation")}</strong>
          {patch.matrix.map((m, i) => (
            <div key={i} className="synth-route">
              <select value={m.source} onChange={(e) => set((p) => (p.matrix[i].source = e.target.value as ModSource))} aria-label={t("From")}>
                {SOURCES.map(([s, n]) => (
                  <option key={s} value={s}>{t(n)}</option>
                ))}
              </select>
              <select value={m.target} onChange={(e) => set((p) => (p.matrix[i].target = e.target.value as ModTarget))} aria-label={t("To")}>
                {TARGETS.map(([s, n]) => (
                  <option key={s} value={s}>{t(n)}</option>
                ))}
              </select>
              <Param label={t("Amount")} value={m.amount} min={-1} max={1} onChange={(v) => set((p) => (p.matrix[i].amount = v), false)} onDone={done} />
              <button className="btn ghost small" onClick={() => set((p) => p.matrix.splice(i, 1))} aria-label={t("Remove")}>✕</button>
            </div>
          ))}
          {patch.matrix.length < 8 && (
            <button className="btn ghost small" onClick={() => set((p) => p.matrix.push({ source: "lfo1", target: "cutoff", amount: 0.3 }))}>{t("+ Add a route")}</button>
          )}
        </div>
        <div className="synth-box">
          <strong className="small">{t("Playing")}</strong>
          <div className="row small">
            <select value={patch.voice.mode} onChange={(e) => set((p) => (p.voice.mode = e.target.value as "poly" | "mono"))} aria-label={t("Voices")}>
              <option value="poly">{t("Chords (poly)")}</option>
              <option value="mono">{t("One note (mono)")}</option>
            </select>
            {patch.voice.mode === "mono" && (
              <label className="row small"><input type="checkbox" checked={patch.voice.legato} onChange={(e) => set((p) => (p.voice.legato = e.target.checked))} /> {t("Legato")}</label>
            )}
          </div>
          {patch.voice.mode === "poly" && <Param label={t("Voices")} value={patch.voice.voices} min={1} max={16} step={1} onChange={(v) => set((p) => (p.voice.voices = v), false)} onDone={done} />}
          <Param label={t("Glide")} value={patch.voice.glide} min={0} max={2} curve={2} unit=" s" onChange={(v) => set((p) => (p.voice.glide = v), false)} onDone={done} />
          <Param label={t("Octave")} value={patch.voice.octave} min={-3} max={3} step={1} onChange={(v) => set((p) => (p.voice.octave = v), false)} onDone={done} />
          <Param label={t("Semitones")} value={patch.voice.semi} min={-12} max={12} step={1} onChange={(v) => set((p) => (p.voice.semi = v), false)} onDone={done} />
          <Param label={t("Fine")} value={patch.voice.fine} min={-100} max={100} step={1} unit="¢" onChange={(v) => set((p) => (p.voice.fine = v), false)} onDone={done} />
          <Param label={t("Velocity")} value={patch.voice.velocity} min={0} max={1} onChange={(v) => set((p) => (p.voice.velocity = v), false)} onDone={done} />
          <Param label={t("Volume")} value={patch.level} min={0} max={1} onChange={(v) => set((p) => (p.level = v), false)} onDone={done} />
        </div>
      </div>
    </div>
  );
}
