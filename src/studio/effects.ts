// SPDX-License-Identifier: AGPL-3.0-only
// A track's effects as Web Audio nodes, the same live and when the mix is
// rendered: EQ, compressor, reverb, delay, distortion and gate.
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
