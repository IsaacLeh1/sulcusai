// SPDX-License-Identifier: AGPL-3.0-only
// The wavetable synth's sound, made on the audio thread (an AudioWorklet),
// the same live and when the mix is rendered: two wavetable oscillators with
// position, warp, unison and spread, a sub oscillator and noise, a
// multimode filter with drive, four envelopes, three LFOs and a mod matrix;
// poly or mono with legato and glide. Its wavetables come band-limited (one
// copy per octave), so high notes don't alias.
export const SYNTH_PROCESSOR = `
const SIZE = 2048, MASK = 2047, LEVELS = 11, BLOCK = 16;
const TABLES = new Map();
const TARGETS = ["pitch","osc1.pitch","osc2.pitch","osc1.pos","osc2.pos","osc1.warp","osc2.warp","osc1.level","osc2.level","sub.level","cutoff","resonance","volume","pan"];
const SOURCES = ["env3","env4","lfo1","lfo2","lfo3","velocity","modwheel","note","random"];
const WARPS = { none: 0, bend: 1, sync: 2, pwm: 3, mirror: 4 };
const FILTERS = { lowpass: 0, highpass: 1, bandpass: 2, notch: 3 };
const SHAPES = { sine: 0, triangle: 1, square: 2, saw: 3, random: 4 };

function hash(n) {
  let x = (n * 2654435761) >>> 0;
  x ^= x >>> 15; x = Math.imul(x, 2246822519) >>> 0; x ^= x >>> 13;
  return (x >>> 0) / 4294967296;
}

// Rises from where it is, falls away exponentially: reaches within 1% in the time given.
function envStep(e, env, sr) {
  if (e.stage === 1) {
    e.level += env.a > 0.0005 ? 1 / (env.a * sr) : 1;
    if (e.level >= 1) { e.level = 1; e.stage = 2; }
  } else if (e.stage === 2) {
    e.level += (env.s - e.level) * (env.d > 0.0005 ? 1 - Math.exp(-4.6 / (env.d * sr)) : 1);
  } else if (e.stage === 3) {
    e.level += -e.level * (env.r > 0.0005 ? 1 - Math.exp(-4.6 / (env.r * sr)) : 1);
    if (e.level < 1e-5) { e.level = 0; e.stage = 0; }
  }
  return e.level;
}

function lfoValue(shape, phase, seed) {
  const f = phase - Math.floor(phase);
  switch (shape) {
    case 0: return Math.sin(2 * Math.PI * f);
    case 1: return f < 0.5 ? 4 * f - 1 : 3 - 4 * f;
    case 2: return f < 0.5 ? 1 : -1;
    case 3: return 2 * f - 1;
    default: return hash(Math.floor(phase) + seed * 7919) * 2 - 1;
  }
}

function warpPhase(mode, amt, ph) {
  if (amt <= 0 || mode === 0) return ph;
  switch (mode) {
    case 1: return Math.pow(ph, Math.exp(amt * 2.2));
    case 2: { const r = ph * (1 + 7 * amt); return r - Math.floor(r); }
    case 3: { const w = 0.5 - 0.47 * amt; return ph < w ? 0.5 * ph / w : 0.5 + 0.5 * (ph - w) / (1 - w); }
    case 4: return amt * (1 - Math.abs(2 * ph - 1)) + (1 - amt) * ph;
  }
  return ph;
}

class WaveSynth extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = options.processorOptions || {};
    for (const [id, t] of Object.entries(o.tables || {})) TABLES.set(id, t);
    this.setPatch(o.patch);
    this.bpm = o.bpm || 120;
    this.endAt = o.endAt === undefined || o.endAt === null ? Infinity : o.endAt;
    this.queue = (o.events || []).slice().sort((a, b) => a.t - b.t);
    this.voices = [];
    this.stack = [];
    this.mod = 0;
    this.bend = 0;
    this.lastPitch = -1;
    this.counter = 0;
    this.noise = 22222;
    this.port.onmessage = (e) => {
      const m = e.data;
      if (m.type === "patch") this.setPatch(m.patch);
      else if (m.type === "tables") for (const [id, t] of Object.entries(m.tables)) TABLES.set(id, t);
      else if (m.type === "events") { this.queue.push(...m.events); this.queue.sort((a, b) => a.t - b.t); }
      else if (m.type === "now") this.queue.unshift({ t: 0, k: m.k, n: m.n, v: m.v });
      else if (m.type === "silence") { this.voices = []; this.stack = []; this.queue = []; this.endAt = -1; }
    };
  }

  setPatch(p) {
    this.p = p;
    const r = [];
    for (const m of (p && p.matrix) || []) {
      const s = SOURCES.indexOf(m.source), t = TARGETS.indexOf(m.target);
      if (s >= 0 && t >= 0 && m.amount) r.push([s, t, m.amount]);
    }
    this.routes = r;
  }

  noteOn(n, v, when) {
    const p = this.p;
    const mono = p.voice.mode === "mono";
    if (mono) {
      this.stack = this.stack.filter((x) => x !== n);
      this.stack.push(n);
      const voice = this.voices[0];
      if (voice && voice.gate && p.voice.legato) { voice.target = n; voice.note = n; return; }
      if (voice && voice.amp.stage !== 0) {
        voice.target = n; voice.note = n; voice.vel = v / 127; voice.gate = true;
        if (p.voice.glide <= 0) voice.pitch = n;
        for (const e of voice.envs) e.stage = 1;
        voice.amp.stage = 1; voice.born = when;
        return;
      }
      this.voices = [this.newVoice(n, v, when, p.voice.glide > 0 && this.lastPitch >= 0 ? this.lastPitch : n)];
      return;
    }
    const same = this.voices.find((x) => x.note === n && x.gate);
    if (same) same.gate = false, same.amp.stage = 3, same.envs.forEach((e) => (e.stage = 3));
    const most = Math.max(1, Math.min(16, p.voice.voices | 0 || 8));
    while (this.voices.length >= most) {
      // The oldest let-go voice gives way first, else the oldest.
      let k = this.voices.findIndex((x) => !x.gate);
      if (k < 0) k = 0;
      this.voices.splice(k, 1);
    }
    this.voices.push(this.newVoice(n, v, when, n));
  }

  noteOff(n) {
    const p = this.p;
    if (p.voice.mode === "mono") {
      this.stack = this.stack.filter((x) => x !== n);
      const voice = this.voices[0];
      if (!voice || voice.note !== n) return;
      if (this.stack.length) { voice.target = voice.note = this.stack[this.stack.length - 1]; return; }
      voice.gate = false; voice.amp.stage = 3; voice.envs.forEach((e) => (e.stage = 3));
      return;
    }
    for (const x of this.voices) if (x.note === n && x.gate) { x.gate = false; x.amp.stage = 3; x.envs.forEach((e) => (e.stage = 3)); }
  }

  newVoice(n, v, when, from) {
    const seed = ++this.counter;
    const phases = (k) => Array.from({ length: 7 }, (_, i) => (k === 1 && i === 0 ? 0 : hash(seed * 31 + i * 7 + k)));
    return {
      note: n, target: n, pitch: from, vel: v / 127, gate: true, born: when, seed,
      rnd: hash(seed * 977) * 2 - 1,
      ph: [phases(1), phases(2)], sub: 0,
      amp: { stage: 1, level: 0 },
      envs: [{ stage: 1, level: 0 }, { stage: 1, level: 0 }, { stage: 1, level: 0 }],
      f: [{ a: 0, b: 0 }, { a: 0, b: 0 }],
      g: 0, k: 1, a1: 0, a2: 0, a3: 0,
    };
  }

  // One oscillator's sample for one unison voice: position between frames, then within the frame.
  oscSample(t, level, pos, ph) {
    const frames = t.frames;
    const fp = pos * (frames - 1);
    const f0 = Math.min(frames - 1, Math.floor(fp)), f1 = Math.min(frames - 1, f0 + 1), fr = fp - f0;
    const x = ph * SIZE, xi = x | 0, xf = x - xi, i0 = xi & MASK, i1 = (xi + 1) & MASK;
    const d = t.data;
    const b0 = (f0 * LEVELS + level) * SIZE, b1 = (f1 * LEVELS + level) * SIZE;
    const s0 = d[b0 + i0] + (d[b0 + i1] - d[b0 + i0]) * xf;
    const s1 = d[b1 + i0] + (d[b1 + i1] - d[b1 + i0]) * xf;
    return s0 + (s1 - s0) * fr;
  }

  process(inputs, outputs) {
    const out = outputs[0];
    const L = out[0], R = out[1] || out[0];
    const n = L.length;
    const sr = sampleRate;
    const p = this.p;
    if (!p) return true;
    const t1 = TABLES.get(p.osc1.table), t2 = TABLES.get(p.osc2.table);
    for (let i = 0; i < n; i++) {
      const now = currentTime + i / sr;
      while (this.queue.length && this.queue[0].t <= now) {
        const e = this.queue.shift();
        if (e.k === 1) this.noteOn(e.n, e.v, now);
        else if (e.k === 0) this.noteOff(e.n);
        else if (e.k === 2) this.mod = e.v;
        else if (e.k === 3) this.bend = e.v;
        else if (e.k === 4) { for (const v of this.voices) { v.gate = false; v.amp.stage = 3; v.envs.forEach((x) => (x.stage = 3)); } this.stack = []; }
      }
      let l = 0, r = 0;
      for (let vi = 0; vi < this.voices.length; vi++) {
        const v = this.voices[vi];
        if ((this.counter + i) % BLOCK === 0 || v.ctl === undefined) this.control(v, now, t1, t2);
        const c = v.ctl;
        let sl = 0, sr2 = 0;
        for (let o = 0; o < 2; o++) {
          const osc = o === 0 ? p.osc1 : p.osc2;
          const tab = o === 0 ? t1 : t2;
          if (!osc.on || !tab || c.level[o] <= 0) continue;
          const ph = v.ph[o];
          const uni = c.uni[o];
          for (let u = 0; u < uni; u++) {
            let x = ph[u] + c.inc[o][u];
            x -= Math.floor(x);
            ph[u] = x;
            const s = this.oscSample(tab, c.lvl[o], c.pos[o], warpPhase(c.warp[o], c.warpAmt[o], x)) * c.level[o];
            sl += s * c.panL[o][u];
            sr2 += s * c.panR[o][u];
          }
        }
        if (p.sub.on && c.subLevel > 0) {
          v.sub += c.subInc;
          v.sub -= Math.floor(v.sub);
          const s = (p.sub.shape === "square" ? (v.sub < 0.5 ? 1 : -1) : Math.sin(2 * Math.PI * v.sub)) * c.subLevel;
          sl += s * 0.7071; sr2 += s * 0.7071;
        }
        if (p.noise > 0) {
          this.noise ^= this.noise << 13; this.noise ^= this.noise >>> 17; this.noise ^= this.noise << 5;
          const s = ((this.noise >>> 0) / 4294967296 * 2 - 1) * p.noise * 0.5;
          sl += s; sr2 += s;
        }
        if (p.filter.on) {
          sl = this.filter(v, v.f[0], sl, c.ftype);
          sr2 = this.filter(v, v.f[1], sr2, c.ftype);
        }
        if (p.filter.drive > 0) {
          const d = 1 + 6 * p.filter.drive;
          sl = Math.tanh(sl * d) / (1 + p.filter.drive);
          sr2 = Math.tanh(sr2 * d) / (1 + p.filter.drive);
        }
        // The amp envelope sample by sample (no clicks), the rest each block.
        const amp = envStep(v.amp, p.amp, sr) * c.gain;
        l += sl * amp * c.outL;
        r += sr2 * amp * c.outR;
        if (v.amp.stage === 0) { this.voices.splice(vi, 1); vi--; }
      }
      L[i] = l * p.level;
      R[i] = r * p.level;
    }
    this.counter += n;
    return !(currentTime > this.endAt && !this.voices.length && !this.queue.length);
  }

  // A state-variable filter (trapezoidal), one channel of a voice.
  filter(v, st, x, ft) {
    const v3 = x - st.b;
    const v1 = v.a1 * st.a + v.a2 * v3;
    const v2 = st.b + v.a2 * st.a + v.a3 * v3;
    st.a = 2 * v1 - st.a;
    st.b = 2 * v2 - st.b;
    return ft === 0 ? v2 : ft === 1 ? x - v.k * v1 - v2 : ft === 2 ? v.k * v1 : x - v.k * v1;
  }

  // What changes slowly for a voice, each block: its pitch, mod sources and targets, filter, levels.
  control(v, now, t1, t2) {
    const p = this.p;
    const sr = sampleRate;
    const dt = BLOCK / sr;
    for (let k = 0; k < 3; k++) {
      const env = k === 0 ? p.fenv : k === 1 ? p.env3 : p.env4;
      const e = v.envs[k];
      for (let s = 0; s < BLOCK; s++) envStep(e, env, sr);
    }
    if (v.pitch !== v.target) {
      const g = p.voice.glide > 0.001 ? 1 - Math.exp(-4.6 * dt / p.voice.glide) : 1;
      v.pitch += (v.target - v.pitch) * g;
      if (Math.abs(v.target - v.pitch) < 0.001) v.pitch = v.target;
    }
    this.lastPitch = v.target;
    const src = v.src || (v.src = new Float64Array(SOURCES.length));
    src[0] = v.envs[1].level; src[1] = v.envs[2].level;
    src[5] = v.vel; src[6] = this.mod; src[7] = (v.note - 60) / 48; src[8] = v.rnd;
    for (let k = 0; k < 3; k++) {
      const lfo = p.lfos[k];
      const rate = lfo.sync ? this.bpm / 60 / Math.max(0.0625, lfo.div) : lfo.rate;
      const t = lfo.retrig ? now - v.born : now;
      src[2 + k] = lfoValue(SHAPES[lfo.shape] ?? 0, t * rate, k + (lfo.retrig ? v.seed : 0));
    }
    const m = v.m || (v.m = new Float64Array(TARGETS.length));
    m.fill(0);
    for (const [s, t, a] of this.routes) m[t] += src[s] * a;
    const base = v.pitch + p.voice.octave * 12 + p.voice.semi + p.voice.fine / 100 + this.bend * 2 + m[0] * 24;
    const c = v.ctl || (v.ctl = { inc: [[], []], uni: [1, 1], level: [0, 0], lvl: [0, 0], pos: [0, 0], warp: [0, 0], warpAmt: [0, 0], panL: [[], []], panR: [[], []] });
    for (let o = 0; o < 2; o++) {
      const osc = o === 0 ? p.osc1 : p.osc2;
      const tab = o === 0 ? t1 : t2;
      const pitch = base + osc.octave * 12 + osc.semi + osc.fine / 100 + m[1 + o] * 24;
      const hz = 440 * Math.pow(2, (pitch - 69) / 12);
      const uni = Math.max(1, Math.min(7, osc.unison | 0));
      c.uni[o] = uni;
      c.pos[o] = Math.max(0, Math.min(1, osc.pos + m[3 + o]));
      c.warp[o] = WARPS[osc.warp] || 0;
      c.warpAmt[o] = Math.max(0, Math.min(1, osc.warpAmt + m[5 + o]));
      c.level[o] = Math.max(0, Math.min(1, osc.level + m[7 + o])) / Math.sqrt(uni);
      // Fewer harmonics for higher notes (and for warps that brighten), so nothing folds back.
      const bright = c.warp[o] === 2 ? 1 + 7 * c.warpAmt[o] : c.warp[o] ? 1 + 2 * c.warpAmt[o] : 1;
      const room = (sr * 0.45) / (hz * bright * Math.pow(2, Math.abs(osc.detune) / 1200));
      c.lvl[o] = room >= 1024 ? 0 : Math.max(0, Math.min(LEVELS - 1, Math.ceil(Math.log2(1024 / Math.max(1, room)))));
      for (let u = 0; u < uni; u++) {
        const spread = uni === 1 ? 0 : (2 * u) / (uni - 1) - 1;
        c.inc[o][u] = (hz * Math.pow(2, (osc.detune * spread) / 1200)) / sr;
        const pan = Math.max(-1, Math.min(1, osc.pan + osc.spread * spread));
        c.panL[o][u] = Math.cos(((pan + 1) * Math.PI) / 4);
        c.panR[o][u] = Math.sin(((pan + 1) * Math.PI) / 4);
      }
      if (!tab) c.level[o] = 0;
    }
    const subHz = 440 * Math.pow(2, (base + p.sub.octave * 12 - 69) / 12);
    c.subInc = subHz / sr;
    c.subLevel = Math.max(0, Math.min(1, p.sub.level + m[9]));
    const vel = 1 - p.voice.velocity * (1 - v.vel);
    c.gain = Math.max(0, vel * (1 + m[12]));
    const pan = Math.max(-1, Math.min(1, m[13]));
    c.outL = Math.cos(((pan + 1) * Math.PI) / 4) * Math.SQRT2;
    c.outR = Math.sin(((pan + 1) * Math.PI) / 4) * Math.SQRT2;
    const f = p.filter;
    c.ftype = FILTERS[f.type] ?? 0;
    const oct = f.env * v.envs[0].level + f.keyTrack * (v.note - 60) / 12 + m[10] * 6;
    const fc = Math.max(20, Math.min(sr * 0.45, f.cutoff * Math.pow(2, oct)));
    const res = Math.max(0, Math.min(1, f.resonance + m[11]));
    const q = 0.5 * Math.pow(40, res);
    v.g = Math.tan((Math.PI * fc) / sr);
    v.k = 1 / q;
    v.a1 = 1 / (1 + v.g * (v.g + v.k));
    v.a2 = v.g * v.a1;
    v.a3 = v.g * v.a2;
  }
}
registerProcessor("sulcusai-wavesynth", WaveSynth);
`;
