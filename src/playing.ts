// SPDX-License-Identifier: AGPL-3.0-only
// Whether the window is making sound, so it isn't suspended mid-song: an
// <audio> or <video> playing, or a Web Audio graph running (the song view,
// the Studio, Compose's player).

const contexts: WeakRef<BaseAudioContext>[] = [];

/** Notes every AudioContext made from now on. */
export function trackAudioContexts(win: { AudioContext?: typeof AudioContext } = window) {
  const Original = win.AudioContext;
  if (!Original || (Original as { tracked?: boolean }).tracked) return;
  class Tracked extends Original {
    static tracked = true;
    constructor(options?: AudioContextOptions) {
      super(options);
      contexts.push(new WeakRef(this));
    }
  }
  win.AudioContext = Tracked;
}

type Media = { paused: boolean; ended: boolean };

export function playing(media: Iterable<Media> = document.querySelectorAll<HTMLMediaElement>("audio, video")): boolean {
  for (const m of media) if (!m.paused && !m.ended) return true;
  return contexts.some((r) => r.deref()?.state === "running");
}
