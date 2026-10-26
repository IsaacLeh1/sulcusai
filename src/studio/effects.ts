// SPDX-License-Identifier: AGPL-3.0-only
// A track's effects as Web Audio nodes, the same live and when the mix is
// rendered: EQ, compressor, reverb, delay, distortion, gate, chorus, phaser,
// tremolo, auto-pan, sweeping low- and high-pass filters, bit crusher,
// stereo width and limiter.
import type { Effect } from "./model";

export interface Chain {
  input: AudioNode;
  output: AudioNode;
  /** Settings changed while playing, without rebuilding. */
  update: (e: Effect) => void;
}

const GATE = `
class Gate extends AudioWorkletProcessor {
  static get parameterDescriptors() { return [{ name: "threshold", defaultValue: -45 }]; }
  constructor() { super(); this.env = 0; this.gain = 0; }
  process(inputs, outputs, params) {
    const input = inputs[0], output = outputs[0];
    if (!input.length) return true;
    const open = Math.pow(10, params.threshold[0] / 20);
    for (let i = 0; i < input[0].length; i++) {
      let peak = 0;
      for (let c = 0; c < input.length; c++) peak = Math.max(peak, Math.abs(input[c][i]));
      this.env = peak > this.env ? peak : this.env * 0.9995;
      const target = this.env > open ? 1 : 0;
      this.gain += (target - this.gain) * (target > this.gain ? 0.01 : 0.0005);
      for (let c = 0; c < output.length; c++) output[c][i] = (input[c] || input[0])[i] * this.gain;
    }
    return true;
  }
}
registerProcessor("sulcusai-gate", Gate);
`;
const gateUrl = URL.createObjectURL(new Blob([GATE], { type: "text/javascript" }));
const gateReady = new WeakMap<BaseAudioContext, Promise<void>>();

/** Readies the effects that need the audio thread (the gate) on a context. */
export function prepare(ctx: BaseAudioContext): Promise<void> {
  let p = gateReady.get(ctx);
  if (!p) {
    p = ctx.audioWorklet.addModule(gateUrl).catch(() => {});
    gateReady.set(ctx, p);
  }
  return p;
}

/** A room's echo: noise dying away over `secs`, a little different left and right. */
function impulse(ctx: BaseAudioContext, secs: number): AudioBuffer {
  const n = Math.floor(ctx.sampleRate * secs);
  const buf = ctx.createBuffer(2, n, ctx.sampleRate);
  for (let c = 0; c < 2; c++) {
    const d = buf.getChannelData(c);
    for (let i = 0; i < n; i++) d[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / n, 2.5);
  }
  return buf;
}

function curve(drive: number): Float32Array<ArrayBuffer> {
  const k = 1 + drive * 80;
  const out = new Float32Array(1024);
  for (let i = 0; i < out.length; i++) {
    const x = (i / (out.length - 1)) * 2 - 1;
    out[i] = ((1 + k) * x) / (1 + k * Math.abs(x));
  }
  return out;
}

/** A slow wave moving `params` (an LFO): `rate` cycles a second, each param by `amount`. */
function lfo(ctx: BaseAudioContext, rate: number, amount: number, params: AudioParam[], shape: OscillatorType = "sine") {
  const osc = ctx.createOscillator();
  osc.type = shape;
  osc.frequency.value = rate;
  const depth = ctx.createGain();
  depth.gain.value = amount;
  osc.connect(depth);
  for (const p of params) depth.connect(p);
  osc.start();
  return { osc, depth };
}

/** A staircase of 2^bits steps (the bit crusher's curve). */
function steps(bits: number): Float32Array<ArrayBuffer> {
  const levels = Math.pow(2, Math.max(1, Math.round(bits))) / 2;
  const out = new Float32Array(4096);
  for (let i = 0; i < out.length; i++) {
    const x = (i / (out.length - 1)) * 2 - 1;
    out[i] = Math.round(x * levels) / levels;
  }
  return out;
}

/** The input as two channels (a mono one copied to both), for effects that work on left and right apart. */
function stereo(ctx: BaseAudioContext, input: AudioNode) {
  const two = ctx.createGain();
  two.channelCount = 2;
  two.channelCountMode = "explicit";
  two.channelInterpretation = "speakers";
  input.connect(two);
  const split = ctx.createChannelSplitter(2);
  two.connect(split);
  return split;
}

/** A filter's Q from its resonance (0 to 1), as the synth's. */
const q = (resonance: number) => 0.5 * Math.pow(40, resonance);

/** Dry and wet mixed, the wet path already fed from the input: returns the node to connect onward. */
function mixed(ctx: BaseAudioContext, input: AudioNode, wetOut: AudioNode, mix: number) {
  const dry = ctx.createGain();
  const wet = ctx.createGain();
  const out = ctx.createGain();
  dry.gain.value = 1 - mix;
  wet.gain.value = mix;
  input.connect(dry).connect(out);
  wetOut.connect(wet).connect(out);
  return { out, dry, wet };
}

/** Dry and wet mixed: `wet` gets the input, returns the node to connect onward. */
function blend(ctx: BaseAudioContext, input: AudioNode, wetIn: AudioNode, wetOut: AudioNode, mix: number) {
  const dry = ctx.createGain();
  const wet = ctx.createGain();
  const out = ctx.createGain();
  dry.gain.value = 1 - mix;
  wet.gain.value = mix;
  input.connect(dry).connect(out);
  input.connect(wetIn);
  wetOut.connect(wet).connect(out);
  return { out, dry, wet };
}

function one(ctx: BaseAudioContext, e: Effect): Chain {
  const p = e.params;
  const input = ctx.createGain();
  if (e.bypass) return { input, output: input, update: () => {} };
  switch (e.kind) {
    case "eq": {
      const low = ctx.createBiquadFilter();
      low.type = "lowshelf";
      low.frequency.value = 200;
      const mid = ctx.createBiquadFilter();
      mid.type = "peaking";
      mid.Q.value = 1;
      const high = ctx.createBiquadFilter();
      high.type = "highshelf";
      high.frequency.value = 4000;
      const set = (x: Effect) => {
        low.gain.value = x.params.low;
        mid.gain.value = x.params.mid;
        mid.frequency.value = x.params.midFreq;
        high.gain.value = x.params.high;
      };
      set(e);
      input.connect(low).connect(mid).connect(high);
      return { input, output: high, update: set };
    }
    case "compressor": {
      const comp = ctx.createDynamicsCompressor();
      const makeup = ctx.createGain();
      const set = (x: Effect) => {
        comp.threshold.value = x.params.threshold;
        comp.ratio.value = x.params.ratio;
        comp.attack.value = x.params.attack;
        comp.release.value = x.params.release;
        comp.knee.value = 6;
        makeup.gain.value = Math.pow(10, x.params.makeup / 20);
      };
      set(e);
      input.connect(comp).connect(makeup);
      return { input, output: makeup, update: set };
    }
    case "reverb": {
      const conv = ctx.createConvolver();
      conv.buffer = impulse(ctx, p.size);
      const b = blend(ctx, input, conv, conv, p.mix);
      return {
        input,
        output: b.out,
        update: (x) => {
          b.dry.gain.value = 1 - x.params.mix;
          b.wet.gain.value = x.params.mix;
        },
      };
    }
    case "delay": {
      const delay = ctx.createDelay(2);
      const fb = ctx.createGain();
      delay.connect(fb).connect(delay);
      const b = blend(ctx, input, delay, delay, p.mix);
      const set = (x: Effect) => {
        delay.delayTime.value = x.params.time;
        fb.gain.value = x.params.feedback;
        b.dry.gain.value = 1 - x.params.mix;
        b.wet.gain.value = x.params.mix;
      };
      set(e);
      return { input, output: b.out, update: set };
    }
    case "distortion": {
      const shaper = ctx.createWaveShaper();
      shaper.curve = curve(p.drive);
      shaper.oversample = "2x";
      const b = blend(ctx, input, shaper, shaper, p.mix);
      return {
        input,
        output: b.out,
        update: (x) => {
          shaper.curve = curve(x.params.drive);
          b.dry.gain.value = 1 - x.params.mix;
          b.wet.gain.value = x.params.mix;
        },
      };
    }
    case "chorus": {
      // Two short delays swaying opposite ways, left and right.
      const split = stereo(ctx, input);
      const merge = ctx.createChannelMerger(2);
      const delays = [0, 1].map((c) => {
        const d = ctx.createDelay(0.1);
        d.delayTime.value = 0.02;
        split.connect(d, c);
        d.connect(merge, 0, c);
        return d;
      });
      const left = lfo(ctx, p.rate, 0.006 * p.depth, [delays[0].delayTime]);
      const right = lfo(ctx, p.rate, -0.006 * p.depth, [delays[1].delayTime]);
      const b = mixed(ctx, input, merge, p.mix);
      const set = (x: Effect) => {
        for (const l of [left, right]) l.osc.frequency.value = x.params.rate;
        left.depth.gain.value = 0.006 * x.params.depth;
        right.depth.gain.value = -0.006 * x.params.depth;
        b.dry.gain.value = 1 - x.params.mix;
        b.wet.gain.value = x.params.mix;
      };
      return { input, output: b.out, update: set };
    }
    case "phaser": {
      // Four all-pass stages whose turning point sweeps up and down.
      const stages = Array.from({ length: 4 }, () => {
        const f = ctx.createBiquadFilter();
        f.type = "allpass";
        f.frequency.value = 800;
        f.Q.value = 0.7;
        return f;
      });
      stages.reduce<AudioNode>((a, f) => a.connect(f), input);
      const sweep = lfo(ctx, p.rate, 700 * p.depth, stages.map((f) => f.frequency));
      const b = mixed(ctx, input, stages[3], p.mix);
      const set = (x: Effect) => {
        sweep.osc.frequency.value = x.params.rate;
        sweep.depth.gain.value = 700 * x.params.depth;
        b.dry.gain.value = 1 - x.params.mix;
        b.wet.gain.value = x.params.mix;
      };
      return { input, output: b.out, update: set };
    }
    case "tremolo": {
      const g = ctx.createGain();
      g.gain.value = 1 - p.depth / 2;
      input.connect(g);
      const wave = lfo(ctx, p.rate, p.depth / 2, [g.gain]);
      const set = (x: Effect) => {
        wave.osc.frequency.value = x.params.rate;
        wave.depth.gain.value = x.params.depth / 2;
        g.gain.value = 1 - x.params.depth / 2;
      };
      return { input, output: g, update: set };
    }
    case "autopan": {
      const pan = ctx.createStereoPanner();
      input.connect(pan);
      const wave = lfo(ctx, p.rate, p.depth, [pan.pan]);
      const set = (x: Effect) => {
        wave.osc.frequency.value = x.params.rate;
        wave.depth.gain.value = x.params.depth;
      };
      return { input, output: pan, update: set };
    }
    case "lowpass":
    case "highpass": {
      // Its cutoff, swept up and down `depth` (up to two octaves each way) `rate` times a second.
      const f = ctx.createBiquadFilter();
      f.type = e.kind;
      input.connect(f);
      const sweep = lfo(ctx, p.rate, 2400 * p.depth, [f.detune]);
      const set = (x: Effect) => {
        f.frequency.value = x.params.cutoff;
        f.Q.value = q(x.params.resonance);
        sweep.osc.frequency.value = x.params.rate;
        sweep.depth.gain.value = x.params.rate > 0 ? 2400 * x.params.depth : 0;
      };
      set(e);
      return { input, output: f, update: set };
    }
    case "bitcrusher": {
      const shaper = ctx.createWaveShaper();
      shaper.curve = steps(p.bits);
      const b = blend(ctx, input, shaper, shaper, p.mix);
      return {
        input,
        output: b.out,
        update: (x) => {
          shaper.curve = steps(x.params.bits);
          b.dry.gain.value = 1 - x.params.mix;
          b.wet.gain.value = x.params.mix;
        },
      };
    }
    case "width": {
      // Middle and sides apart, the sides made wider or narrower.
      const split = stereo(ctx, input);
      const mid = ctx.createGain();
      const side = ctx.createGain();
      const lToMid = ctx.createGain();
      const rToMid = ctx.createGain();
      const lToSide = ctx.createGain();
      const rToSide = ctx.createGain();
      lToMid.gain.value = rToMid.gain.value = lToSide.gain.value = 0.5;
      rToSide.gain.value = -0.5;
      split.connect(lToMid, 0).connect(mid);
      split.connect(rToMid, 1).connect(mid);
      split.connect(lToSide, 0).connect(side);
      split.connect(rToSide, 1).connect(side);
      const wide = ctx.createGain();
      wide.gain.value = p.width;
      side.connect(wide);
      const flip = ctx.createGain();
      flip.gain.value = -1;
      wide.connect(flip);
      const merge = ctx.createChannelMerger(2);
      mid.connect(merge, 0, 0);
      mid.connect(merge, 0, 1);
      wide.connect(merge, 0, 0);
      flip.connect(merge, 0, 1);
      return { input, output: merge, update: (x) => (wide.gain.value = x.params.width) };
    }
    case "limiter": {
      const lim = ctx.createDynamicsCompressor();
      lim.knee.value = 0;
      lim.ratio.value = 20;
      lim.attack.value = 0.002;
      lim.release.value = 0.1;
      lim.threshold.value = p.threshold;
      input.connect(lim);
      return { input, output: lim, update: (x) => (lim.threshold.value = x.params.threshold) };
    }
    case "gate": {
      try {
        const node = new AudioWorkletNode(ctx, "sulcusai-gate", { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [2] });
        const set = (x: Effect) => node.parameters.get("threshold")?.setValueAtTime(x.params.threshold, 0);
        set(e);
        input.connect(node);
        return { input, output: node, update: set };
      } catch {
        // The audio thread's gate isn't ready: passed through.
        return { input, output: input, update: () => {} };
      }
    }
  }
}

/** A track's effects in order; with none, a single pass-through node. */
export function chain(ctx: BaseAudioContext, effects: Effect[]): Chain {
  const input = ctx.createGain();
  let last: AudioNode = input;
  const parts = new Map<string, Chain>();
  for (const e of effects) {
    const c = one(ctx, e);
    last.connect(c.input);
    last = c.output;
    parts.set(e.id, c);
  }
  return { input, output: last, update: (e) => parts.get(e.id)?.update(e) };
}
