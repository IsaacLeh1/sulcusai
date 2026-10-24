// SPDX-License-Identifier: AGPL-3.0-only
// Compose's sound: a composition's notes played on sampled instruments (the
// MuseScore General sound set, downloaded once) by SpessaSynth, in the window.
// The same player renders the demo that's kept in the gallery and that
// "Make it real" hands to the music model.
import { Sequencer, WorkletSynthesizer } from "spessasynth_lib";
import processorUrl from "spessasynth_lib/dist/spessasynth_processor.min.js?url";
import { BasicMIDI } from "spessasynth_core";
import { api, type Composition } from "../api";
import { tx } from "../i18n";

let bank: Promise<ArrayBuffer> | null = null;

/** The sound set's bytes, fetched once a session (downloaded once ever). */
export function soundBank(): Promise<ArrayBuffer> {
  bank ??= api.composeSounds().catch((e) => {
    bank = null;
    throw e;
  });
  return bank;
}

/** Seconds a beat lasts: the tempo counts quarter notes, a beat is the time signature's unit. */
export function secondsPerBeat(plan: { bpm: number; unit: number }) {
  return (60 / Math.max(20, plan.bpm)) * (4 / Math.max(1, plan.unit));
}

/** The MIDI channel a part plays on (as the app writes the file: drums on 10). */
export function channelOf(c: Composition, part: string) {
  if (part === "drums") return 9;
  const melodic = c.parts.filter((p) => p.name !== "drums").map((p) => p.name);
  const i = melodic.indexOf(part);
  return i < 9 ? i : i + 1;
}

/** Plays a composition, from any point, with parts muted or heard. */
export class ComposePlayer {
  private ctx = new AudioContext();
  private synth: WorkletSynthesizer | null = null;
  private seq: Sequencer | null = null;
  private starting: Promise<void> | null = null;
  private songLoaded: (() => void) | null = null;
  /** The instruments are loaded (playing starts at once). */
  loaded = false;
  onEnd: (() => void) | null = null;

  /** Ready to play: the processor, the synthesizer and the sound set. */
  ready(): Promise<void> {
    this.starting ??= (async () => {
      const sounds = await soundBank();
      await this.ctx.audioWorklet.addModule(processorUrl);
      const synth = new WorkletSynthesizer(this.ctx);
      synth.connect(this.ctx.destination);
      // A copy: the processor may take the bytes it's sent.
      await synth.soundBankManager.addSoundBank(sounds.slice(0), "main");
      await synth.isReady;
      const seq = new Sequencer(synth, { skipToFirstNoteOn: false });
      seq.loopCount = 0;
      seq.eventHandler.addEvent("songChange", "compose", () => {
        this.songLoaded?.();
        this.songLoaded = null;
      });
      seq.eventHandler.addEvent("songEnded", "compose", () => this.onEnd?.());
      this.synth = synth;
      this.seq = seq;
      this.loaded = true;
    })().catch((e) => {
      this.starting = null;
      throw e;
    });
    return this.starting;
  }

  /** Plays the composition from `from` seconds, the parts in `muted` silent. */
  async play(c: Composition, from: number, muted: Set<string>) {
    await this.ready();
    const seq = this.seq!;
    const synth = this.synth!;
    await this.ctx.resume();
    const midi = await api.composeMidi(c, null);
    // Loaded when the sequencer says so (or after a moment, whatever it says).
    const done = new Promise<void>((r) => {
      this.songLoaded = r;
      setTimeout(r, 3000);
    });
    seq.loadNewSongList([{ binary: midi }]);
    await done;
    for (const p of c.parts) synth.midiChannels[channelOf(c, p.name)]?.setSystemParameter("isMuted", muted.has(p.name));
    seq.currentTime = from;
    seq.play();
  }

  /** A part heard or not while playing. */
  mute(c: Composition, part: string, muted: boolean) {
    this.synth?.midiChannels[channelOf(c, part)]?.setSystemParameter("isMuted", muted);
  }

  stop() {
    this.seq?.pause();
    this.synth?.stopAll(true);
  }

  /** Where it's playing, in seconds. */
  get time() {
    return this.seq?.currentTime ?? 0;
  }

  /** Jumps to `t` seconds, playing on (a loop going round). */
  seek(t: number) {
    if (this.seq) this.seq.currentTime = t;
  }

  /** One note heard now (an added or moved note), on its part's instrument. */
  async preview(c: Composition, part: string, pitch: number) {
    await this.ready();
    await this.ctx.resume();
    const synth = this.synth!;
    const ch = channelOf(c, part);
    const p = c.parts.find((x) => x.name === part);
    if (!p) return;
    if (part !== "drums") synth.programChange(ch, programOf(p.instrument));
    synth.midiChannels[ch]?.setSystemParameter("isMuted", false);
    synth.noteOn(ch, pitch, 100);
    setTimeout(() => synth.noteOff(ch, pitch), 350);
  }

  close() {
    this.stop();
    this.synth?.destroy();
    this.ctx.close();
  }
}

/** The composition as heard, rendered (a WAV-ready buffer at 44.1 kHz). */
export async function renderDemo(c: Composition, seconds: number): Promise<AudioBuffer> {
  const rate = 44100;
  // Everything fetched first: the offline render starts as soon as the synthesizer exists.
  const midi = BasicMIDI.fromArrayBuffer(await api.composeMidi(c, null));
  const sounds = (await soundBank()).slice(0);
  const ctx = new OfflineAudioContext({ numberOfChannels: 2, length: Math.ceil((seconds + 2) * rate), sampleRate: rate });
  await ctx.audioWorklet.addModule(processorUrl);
  const synth = new WorkletSynthesizer(ctx);
  synth.connect(ctx.destination);
  await synth.startOfflineRender({ midiSequence: midi, loopCount: 0, soundBankList: [{ bankOffset: 0, soundBankBuffer: sounds }], sequencerOptions: { skipToFirstNoteOn: false } });
  return ctx.startRendering();
}

/** Instruments a part can play, by name: [name, General MIDI program]. As `compose::program` reads them. */
export const INSTRUMENTS: [string, number][] = [
  [tx("piano"), 0],
  [tx("electric piano"), 4],
  [tx("organ"), 16],
  [tx("acoustic guitar"), 25],
  [tx("nylon guitar"), 24],
  [tx("clean guitar"), 27],
  [tx("distorted guitar"), 30],
  [tx("acoustic bass"), 32],
  [tx("electric bass"), 33],
  [tx("fretless bass"), 35],
  [tx("synth bass"), 38],
  [tx("violin"), 40],
  [tx("cello"), 42],
  [tx("strings"), 48],
  [tx("choir"), 52],
  [tx("voice"), 52],
  [tx("trumpet"), 56],
  [tx("brass"), 61],
  [tx("sax"), 65],
  [tx("flute"), 73],
  [tx("synth lead"), 80],
  [tx("pad"), 89],
  [tx("bells"), 14],
  [tx("marimba"), 12],
];
export const KITS = [tx("standard kit"), tx("room kit"), tx("rock kit"), tx("electronic kit"), tx("808 kit"), tx("jazz kit"), tx("brush kit"), tx("orchestra kit")];

/** The program an instrument's name plays (the closest named one). */
export function programOf(name: string) {
  const n = name.toLowerCase().trim();
  // The longest name it holds ("electric piano" before "piano").
  const longest = [...INSTRUMENTS].sort((a, b) => b[0].length - a[0].length);
  return (INSTRUMENTS.find(([k]) => k === n) ?? longest.find(([k]) => n.includes(k)))?.[1] ?? 0;
}
