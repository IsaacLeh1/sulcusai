// SPDX-License-Identifier: AGPL-3.0-only
// A MIDI clip's notes as sound: written as a Standard MIDI File and played
// on the sampled instruments Compose uses (SpessaSynth, rendered offline),
// so a MIDI track mixes like any other.
import { WorkletSynthesizer } from "spessasynth_lib";
import processorUrl from "spessasynth_lib/dist/spessasynth_processor.min.js?url";
import { BasicMIDI } from "spessasynth_core";
import { soundBank } from "../compose/player";
import type { Note } from "./model";

/** General MIDI drums: played on channel 10 (program 128 here). */
export const DRUMS = 128;

const TICKS = 480;
const TEMPO = 500_000; // a quarter note each half second: ticks are 1/960 s

function varLen(n: number): number[] {
  const out = [n & 0x7f];
  let v = n >> 7;
  while (v > 0) {
    out.unshift((v & 0x7f) | 0x80);
    v >>= 7;
  }
  return out;
}

/** Notes (seconds) as a one-track MIDI file, on `program` (or drums). */
export function midiFile(notes: Note[], program: number, name = "Track"): Uint8Array {
  const ch = program === DRUMS ? 9 : 0;
  const tick = (s: number) => Math.max(0, Math.round((s * TICKS * 1_000_000) / TEMPO));
  const events: { at: number; bytes: number[]; order: number }[] = [];
  for (const n of notes) {
    const pitch = Math.max(0, Math.min(127, Math.round(n.pitch)));
    const vel = Math.max(1, Math.min(127, Math.round(n.velocity)));
    events.push({ at: tick(n.start), bytes: [0x90 | ch, pitch, vel], order: 1 });
    events.push({ at: tick(Math.max(n.end, n.start + 0.02)), bytes: [0x80 | ch, pitch, 0], order: 0 });
  }
  // Note-offs before note-ons at the same moment.
  events.sort((a, b) => a.at - b.at || a.order - b.order);
  const title = Array.from(new TextEncoder().encode(name.slice(0, 60)));
  const track: number[] = [0, 0xff, 0x03, ...varLen(title.length), ...title, 0, 0xff, 0x51, 0x03, (TEMPO >> 16) & 0xff, (TEMPO >> 8) & 0xff, TEMPO & 0xff];
  if (ch !== 9) track.push(0, 0xc0 | ch, Math.max(0, Math.min(127, program)));
  let last = 0;
  for (const e of events) {
    track.push(...varLen(e.at - last), ...e.bytes);
    last = e.at;
  }
  track.push(0, 0xff, 0x2f, 0);
  const head = [0x4d, 0x54, 0x68, 0x64, 0, 0, 0, 6, 0, 0, 0, 1, (TICKS >> 8) & 0xff, TICKS & 0xff];
  const len = track.length;
  return new Uint8Array([...head, 0x4d, 0x54, 0x72, 0x6b, (len >> 24) & 0xff, (len >> 16) & 0xff, (len >> 8) & 0xff, len & 0xff, ...track]);
}

/** A MIDI file's parts: each channel with notes, its instrument (drums on channel 10) and notes in seconds. */
export function readMidiFile(bytes: ArrayBuffer): { channel: number; program: number; notes: Note[] }[] {
  const midi = BasicMIDI.fromArrayBuffer(bytes);
  const programs = new Map<number, number>();
  for (const track of midi.tracks) {
    for (const e of track.events) {
      const ch = e.statusByte & 0x0f;
      if ((e.statusByte & 0xf0) === 0xc0 && !programs.has(ch)) programs.set(ch, e.data[0]);
    }
  }
  return midi
    .getNoteTimes(0.05)
    .map((list, channel) => ({
      channel,
      program: channel === 9 ? DRUMS : programs.get(channel) ?? 0,
      notes: list.map((n) => ({ pitch: n.midiNote, start: n.start, end: n.start + Math.max(0.03, n.length), velocity: n.velocity })),
    }))
    .filter((c) => c.notes.length > 0);
}

/** Notes played now (musical typing, a note added in the piano roll), on any instrument. */
export class LiveSynth {
  private ctx = new AudioContext();
  private synth: WorkletSynthesizer | null = null;
  private starting: Promise<void> | null = null;
  private program = -1;

  ready(): Promise<void> {
    this.starting ??= (async () => {
      const sounds = await soundBank();
      await this.ctx.audioWorklet.addModule(processorUrl);
      const synth = new WorkletSynthesizer(this.ctx);
      synth.connect(this.ctx.destination);
      await synth.soundBankManager.addSoundBank(sounds.slice(0), "main");
      await synth.isReady;
      this.synth = synth;
    })().catch((e) => {
      this.starting = null;
      throw e;
    });
    return this.starting;
  }

  async on(program: number, pitch: number, velocity = 100) {
    await this.ready();
    await this.ctx.resume();
    const s = this.synth!;
    const ch = program === DRUMS ? 9 : 0;
    if (ch === 0 && program !== this.program) {
      s.programChange(0, program);
      this.program = program;
    }
    s.noteOn(ch, pitch, velocity);
  }

  off(program: number, pitch: number) {
    this.synth?.noteOff(program === DRUMS ? 9 : 0, pitch);
  }

  /** A short note, heard once. */
  async tap(program: number, pitch: number) {
    await this.on(program, pitch);
    setTimeout(() => this.off(program, pitch), 300);
  }

  close() {
    this.synth?.destroy();
    this.ctx.close();
  }
}

/** A MIDI clip's sound, `seconds` long, at 44.1 kHz stereo. */
export async function renderNotes(notes: Note[], program: number, seconds: number): Promise<AudioBuffer> {
  const rate = 44100;
  const length = Math.max(1, Math.ceil((seconds + 0.5) * rate));
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length, sampleRate: rate });
  if (!notes.length) return ctx.startRendering();
  const bytes = midiFile(notes, program);
  const midi = BasicMIDI.fromArrayBuffer(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer);
  const sounds = (await soundBank()).slice(0);
  await ctx.audioWorklet.addModule(processorUrl);
  const synth = new WorkletSynthesizer(ctx);
  synth.connect(ctx.destination);
  await synth.startOfflineRender({ midiSequence: midi, loopCount: 0, soundBankList: [{ bankOffset: 0, soundBankBuffer: sounds }], sequencerOptions: { skipToFirstNoteOn: false } });
  return ctx.startRendering();
}
