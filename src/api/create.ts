// SPDX-License-Identifier: AGPL-3.0-only
// Creating media: storyboards, batches, dialogue tags, sound effects, cuts by transcript, lip-sync, regions, mic check.
import { invoke } from "@tauri-apps/api/core";

export type Likeness = "nobody" | "myself" | "someone";
export interface LikenessClaim {
  face?: Likeness;
  voice?: Likeness;
  permission_from_them?: boolean;
  public_figure?: boolean;
}
export interface Shot { n: number; heading: string; action: string; dialogue: string[]; seconds: number; prompt: string }
export interface BatchItem { row: number; name: string; prompt: string; negative: string; seed: number | null; width: number | null; height: number | null }
export interface DialoguePart { speaker: string; text: string; delivery: string[]; sounds: string[]; pause_after: number }
export interface SoundEffect { prompt: string; seconds: number; looped: boolean }
export interface CutWord { text: string; start: number; end: number; removed?: boolean }
export interface Region { x: number; y: number; w: number; h: number; prompt: string }
export interface MicCheck { peak_db: number; speech_db: number; noise_db: number; clipped: number; hum: boolean; advice: string[] }

export const create = {
  consent: (claim: LikenessClaim) => invoke<void>("create_consent_check", { claim }),
  storyboard: (script: string, style: string, maxSeconds?: number) => invoke<Shot[]>("create_storyboard", { script, style, maxSeconds }),
  batch: (csv: string) => invoke<BatchItem[]>("create_batch_parse", { csv }),
  dialogue: (script: string, narrator: string) => invoke<DialoguePart[]>("create_dialogue_parts", { script, narrator }),
  sfx: (request: string) => invoke<SoundEffect>("create_sfx_plan", { request }),
  textCut: (words: CutWord[], length: number, strikeFillers = false) => invoke<[Array<[number, number]>, string]>("create_text_cut", { words, length, strikeFillers }),
  lipsync: (lines: Array<{ start: number; end: number }>, length: number, claim: LikenessClaim) => invoke<Array<{ start: number; end: number; lines: number[] }>>("create_lipsync_pieces", { lines, length, claim }),
  regionScene: (base: string, regions: Region[]) => invoke<string>("create_region_scene", { base, regions }),
  micCheck: (samples: number[], rate: number) => invoke<MicCheck>("create_mic_check", { samples, rate }),
};

/** Toggles a word struck or kept, for the transcript editor. */
export function toggleWord(words: CutWord[], i: number): CutWord[] {
  return words.map((w, k) => (k === i ? { ...w, removed: !w.removed } : w));
}

/** Mono samples from a recorded buffer, downmixed, for the mic check. */
export function monoSamples(channels: Float32Array[]): number[] {
  if (channels.length === 0) return [];
  const n = channels[0].length;
  const out = new Array<number>(n);
  for (let i = 0; i < n; i++) {
    let s = 0;
    for (const c of channels) s += c[i] ?? 0;
    out[i] = s / channels.length;
  }
  return out;
}
