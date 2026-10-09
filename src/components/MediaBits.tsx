// SPDX-License-Identifier: AGPL-3.0-only
// Pieces the studio and chats share: tiles, job progress, the audio player.
import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, toBase64, type MediaItem, type MediaJob } from "../api";
import { clock } from "../format";
import { encodeWav, fileUrl, JOB_LABEL, makeStill, useMediaUrl, waitLabel } from "../media";
import type { PushToast } from "./Toasts";

/** A gallery tile: the picture, or a card for a clip or sound. */
export function MediaTile({ item, onOpen, selected }: { item: MediaItem; onOpen: () => void; selected?: boolean }) {
  const thumb = useMediaUrl(item.kind === "image" || (item.kind === "video" && item.poster) ? item : null, true);
  // An older video without a still: take one now.
  const [still, setStill] = useState<string | null>(null);
  useEffect(() => {
    if (item.kind !== "video" || item.poster) return;
    let live = true;
    makeStill(item).then((u) => live && setStill(u));
    return () => {
      live = false;
    };
  }, [item.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const videoStill = item.kind === "video" ? thumb ?? still : null;
  return (
    <button className={`media-tile ${item.kind} ${selected ? "selected" : ""}`} onClick={onOpen} title={item.prompt || undefined}>
      {item.kind === "image" ? (
        thumb ? <img src={thumb} alt={item.prompt || "Picture"} loading="lazy" /> : <span className="media-ph" aria-hidden>🖼</span>
      ) : videoStill ? (
        <>
          <img src={videoStill} alt={item.prompt || "Video"} loading="lazy" />
          <span className="media-play" aria-hidden>▶ {clock(item.seconds)}</span>
        </>
      ) : (
        <span className="media-ph" aria-hidden>
          {item.kind === "video" ? "🎬" : item.op === "narrate" ? "🗣" : item.op === "sound" ? "🔔" : "🎵"}
          <span className="small">{clock(item.seconds)}</span>
        </span>
      )}
      {item.favorite && <span className="media-fav" aria-label="Favorite">★</span>}
      {item.prompt && <span className="media-cap ellipsis">{item.prompt}</span>}
    </button>
  );
}

/** A job waiting or running, with its progress and time left. */
export function JobCard({ job }: { job: MediaJob }) {
  const pct = Math.round(job.fraction * 100);
  return (
    <div className="job-card">
      <div className="progress-label">
        <span className="ellipsis">
          <strong>{JOB_LABEL[job.op] ?? "Working"}</strong>
          {job.prompt && <span className="muted"> · {job.prompt}</span>}
        </span>
        <button className="link" onClick={() => api.mediaCancel(job.id)}>{job.status === "queued" ? "Remove" : "Stop"}</button>
      </div>
      <div className={`progress ${job.status === "queued" ? "indeterminate" : ""}`}>
        <span style={{ width: job.status === "queued" ? undefined : `${Math.max(3, pct)}%` }} />
      </div>
      <span className="small muted">
        {job.status === "queued"
          ? `Waiting for the job before it · takes ${waitLabel(job.est_secs)}`
          : `${job.stage}${job.eta_secs != null ? ` · ${waitLabel(job.eta_secs)} left` : ""}`}
      </span>
    </div>
  );
}

/** Running jobs, kept up to date from the core's events. */
export function useMediaJobs(): MediaJob[] {
  const [jobs, setJobs] = useState<MediaJob[]>([]);
  useEffect(() => {
    api.mediaJobs().then(setJobs).catch(() => {});
    const subs = [
      on("media:progress", (j) => setJobs((all) => (all.some((x) => x.id === j.id) ? all.map((x) => (x.id === j.id ? j : x)) : [...all, j]))),
      on("media:done", (d) => setJobs((all) => all.filter((x) => x.id !== d.job_id))),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, []);
  return jobs;
}

/** Saves an item somewhere the user picks. */
export async function saveAs(item: MediaItem, toast: PushToast) {
  const ext = item.mime.split("/")[1]?.replace("mpeg", "mp3").replace("jpeg", "jpg") ?? "bin";
  const base = (item.prompt || item.op || "sulcusai").replace(/[^\w\- ]+/g, "").trim().slice(0, 40).replace(/\s+/g, "-") || "sulcusai";
  const path = await save({ defaultPath: `${base}.${ext}`, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] });
  if (!path) return;
  try {
    await api.mediaExport(item.id, path);
    toast("Saved.", "success");
  } catch (e) {
    toast(errorText(e), "error");
  }
}

/** Plays a sound or song: trim to a part, loop it, save the part. */
export function AudioPlayer({ item, toast, onSaved }: { item: MediaItem; toast: PushToast; onSaved?: (i: MediaItem) => void }) {
  const src = useMediaUrl(item);
  const audio = useRef<HTMLAudioElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [buf, setBuf] = useState<AudioBuffer | null>(null);
  const [range, setRange] = useState<[number, number]>([0, item.seconds || 0]);
  const [loop, setLoop] = useState(false);
  const [now, setNow] = useState(0);
  const [busy, setBusy] = useState(false);
  const dur = buf?.duration ?? item.seconds ?? 0;

  // Decode once, for the waveform and for saving a trimmed part.
  useEffect(() => {
    let live = true;
    fileUrl(item)
      .then((u) => fetch(u))
      .then((r) => r.arrayBuffer())
      .then((b) => new AudioContext().decodeAudioData(b))
      .then((d) => {
        if (!live) return;
        setBuf(d);
        setRange([0, d.duration]);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [item.id]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const c = canvas.current;
    if (!c || !buf) return;
    const ctx = c.getContext("2d")!;
    const w = (c.width = c.clientWidth * devicePixelRatio);
    const h = (c.height = c.clientHeight * devicePixelRatio);
    const data = buf.getChannelData(0);
    const step = Math.max(1, Math.floor(data.length / w));
    const style = getComputedStyle(c);
    ctx.clearRect(0, 0, w, h);
    for (let x = 0; x < w; x++) {
      let peak = 0;
      for (let i = x * step; i < (x + 1) * step && i < data.length; i += 4) peak = Math.max(peak, Math.abs(data[i]));
      const t = (x / w) * buf.duration;
      ctx.fillStyle = t >= range[0] && t <= range[1] ? style.getPropertyValue("--accent") || "#4b3fb5" : style.getPropertyValue("--border") || "#ccc";
      const bar = Math.max(1, peak * h * 0.9);
      ctx.fillRect(x, (h - bar) / 2, 1, bar);
    }
  }, [buf, range]);

  // Keep playback within the chosen part; loop it if asked.
  useEffect(() => {
    const a = audio.current;
    if (!a) return;
    const tick = () => {
      setNow(a.currentTime);
      if (a.currentTime >= range[1] - 0.02) {
        if (loop) a.currentTime = range[0];
        else {
          a.pause();
          a.currentTime = range[0];
        }
      }
    };
    a.addEventListener("timeupdate", tick);
    return () => a.removeEventListener("timeupdate", tick);
  }, [range, loop]);

  const seek = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const t = ((e.clientX - r.left) / r.width) * dur;
    if (audio.current) audio.current.currentTime = Math.min(Math.max(t, range[0]), range[1]);
  };

  const trimmed = range[0] > 0.05 || range[1] < dur - 0.05;
  const saveTrim = async () => {
    if (!buf) return;
    setBusy(true);
    try {
      const wav = encodeWav(buf, range[0], range[1]);
      const saved = await api.mediaAdd({ data: toBase64(wav), mime: "audio/wav", op: "trim", prompt: item.prompt, parent: item.id, seconds: range[1] - range[0] });
      toast("The trimmed part is saved in the gallery.", "success");
      onSaved?.(saved);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="audio-player">
      {src && <audio ref={audio} src={src} controls preload="auto" onPlay={() => audio.current && audio.current.currentTime < range[0] && (audio.current.currentTime = range[0])} />}
      <canvas ref={canvas} className="wave" onClick={seek} aria-label="Waveform: click to jump" />
      {dur > 0 && (
        <>
          <div className="trim-row">
            <label className="small">
              Start {clock(range[0])}
              <input type="range" min={0} max={dur} step={0.1} value={range[0]} onChange={(e) => setRange([Math.min(+e.target.value, range[1] - 0.5), range[1]])} />
            </label>
            <label className="small">
              End {clock(range[1])}
              <input type="range" min={0} max={dur} step={0.1} value={range[1]} onChange={(e) => setRange([range[0], Math.max(+e.target.value, range[0] + 0.5)])} />
            </label>
          </div>
          <div className="row">
            <label className="row small">
              <input type="checkbox" checked={loop} onChange={(e) => setLoop(e.target.checked)} /> Loop
            </label>
            <span className="small muted">{clock(now)} / {clock(dur)}</span>
            <span className="spacer" />
            {trimmed && (
              <button className="btn small" disabled={busy || !buf} onClick={saveTrim}>
                Save the trimmed part
              </button>
            )}
            <button className="btn small" onClick={() => saveAs(item, toast)}>Save as…</button>
          </div>
        </>
      )}
    </div>
  );
}

/** What the assistant made, shown in its tool card. */
export function MediaInline({ items, onOpen }: { items: MediaItem[]; onOpen?: (i: MediaItem) => void }) {
  return (
    <div className="media-inline">
      {items.map((i) => (
        <InlineOne key={i.id} item={i} onOpen={onOpen} />
      ))}
    </div>
  );
}

function InlineOne({ item, onOpen }: { item: MediaItem; onOpen?: (i: MediaItem) => void }) {
  const src = useMediaUrl(item, item.kind === "image");
  if (!src) return <div className="media-inline-ph">…</div>;
  if (item.kind === "image")
    return (
      <button className="media-inline-img" onClick={() => onOpen?.(item)} title="Open">
        <img src={src} alt={item.prompt || "Picture"} />
      </button>
    );
  if (item.kind === "video") return <video src={src} controls loop playsInline className="media-inline-video" />;
  return <audio src={src} controls className="media-inline-audio" />;
}

/** A full-size look at a picture from a chat. */
export function Lightbox({ item, onClose, toast }: { item: MediaItem; onClose: () => void; toast: PushToast }) {
  const src = useMediaUrl(item);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="modal-backdrop lightbox" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="lightbox-body">
        {src && <img src={src} alt={item.prompt || "Picture"} />}
        <div className="row">
          <span className="small muted ellipsis">{item.prompt}</span>
          <span className="spacer" />
          <button className="btn small" onClick={() => saveAs(item, toast)}>Save as…</button>
          <button className="btn small" onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
