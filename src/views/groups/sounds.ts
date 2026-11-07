// SPDX-License-Identifier: AGPL-3.0-only
// Procedural audio cues for voice calls, mute toggles and group messaging.

let audioCtx: AudioContext | null = null;

function getContext(): AudioContext | null {
  try {
    if (!audioCtx) {
      const AudioCtx = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      if (AudioCtx) audioCtx = new AudioCtx();
    }
    if (audioCtx && audioCtx.state === "suspended") {
      audioCtx.resume().catch(() => {});
    }
    return audioCtx;
  } catch {
    return null;
  }
}

function playTone(freq: number, durationSec: number, delaySec = 0, gainLevel = 0.12, type: OscillatorType = "sine") {
  const ctx = getContext();
  if (!ctx) return;
  const start = ctx.currentTime + delaySec;
  const end = start + durationSec;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();

  osc.type = type;
  osc.frequency.setValueAtTime(freq, start);

  gain.gain.setValueAtTime(0.001, start);
  gain.gain.exponentialRampToValueAtTime(gainLevel, start + 0.015);
  gain.gain.exponentialRampToValueAtTime(0.0001, end);

  osc.connect(gain);
  gain.connect(ctx.destination);

  osc.start(start);
  osc.stop(end);
}

/** Ascending two-tone chime when entering a call. */
export function playJoinCall() {
  playTone(440, 0.12, 0, 0.15);
  playTone(659.25, 0.22, 0.1, 0.15);
}

/** Descending two-tone chime when disconnecting from a call. */
export function playLeaveCall() {
  playTone(659.25, 0.12, 0, 0.14);
  playTone(440, 0.22, 0.1, 0.14);
}

/** Three-tone chime when another person enters the active call. */
export function playPeerJoin() {
  playTone(587.33, 0.09, 0, 0.12);
  playTone(739.99, 0.09, 0.08, 0.12);
  playTone(880, 0.18, 0.16, 0.14);
}

/** Click tone on mic mute toggle. */
export function playMute(muted: boolean) {
  if (muted) {
    playTone(420, 0.05, 0, 0.12, "triangle");
    playTone(280, 0.08, 0.04, 0.12, "triangle");
  } else {
    playTone(280, 0.05, 0, 0.12, "triangle");
    playTone(420, 0.08, 0.04, 0.12, "triangle");
  }
}

/** Soft chime on audio deafen toggle. */
export function playDeafen(deafened: boolean) {
  if (deafened) {
    playTone(360, 0.06, 0, 0.12, "sine");
    playTone(220, 0.12, 0.05, 0.12, "sine");
  } else {
    playTone(220, 0.06, 0, 0.12, "sine");
    playTone(360, 0.12, 0.05, 0.12, "sine");
  }
}

let lastGroupMessageSoundAt = 0;

/** Crisp chime on incoming group message. */
export function playGroupMessage() {
  const now = Date.now();
  if (now - lastGroupMessageSoundAt < 500) return;
  lastGroupMessageSoundAt = now;
  const ctx = getContext();
  if (!ctx) return;
  const start = ctx.currentTime;
  const end = start + 0.16;

  const osc1 = ctx.createOscillator();
  const osc2 = ctx.createOscillator();
  const gain = ctx.createGain();

  osc1.type = "sine";
  osc1.frequency.setValueAtTime(880, start);
  osc2.type = "sine";
  osc2.frequency.setValueAtTime(1320, start);

  gain.gain.setValueAtTime(0.001, start);
  gain.gain.exponentialRampToValueAtTime(0.14, start + 0.01);
  gain.gain.exponentialRampToValueAtTime(0.0001, end);

  osc1.connect(gain);
  osc2.connect(gain);
  gain.connect(ctx.destination);

  osc1.start(start);
  osc2.start(start);
  osc1.stop(end);
  osc2.stop(end);
}
