// SPDX-License-Identifier: AGPL-3.0-only
// Pieces the studio and chats share: tiles, job progress, the audio player.
import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, toBase64, type MadeStats, type MediaItem, type MediaJob } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { encodeWav, fileUrl, JOB_LABEL, makeStill, useMediaUrl, waitLabel } from "../media";
import { onPhone } from "../phone/env";
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
        thumb ? <img src={thumb} alt={item.prompt || t("Picture")} loading="lazy" /> : <span className="media-ph" aria-hidden>{t("Picture")}</span>
      ) : videoStill ? (
        <>
          <img src={videoStill} alt={item.prompt || t("Video")} loading="lazy" />
          <span className="media-play" aria-hidden>▶ {clock(item.seconds)}</span>
        </>
      ) : (
        <span className="media-ph" aria-hidden>
          {item.kind === "video" ? t("Video") : item.op === "narrate" ? t("Narration") : item.op === "sound" ? t("Sound effect") : t("Music")}
          <span className="small">{clock(item.seconds)}</span>
        </span>
      )}
      {item.favorite && <span className="media-fav" aria-label={t("Favorite")}>★</span>}
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
          <strong>{JOB_LABEL[job.op] ? t(JOB_LABEL[job.op]) : t("Working")}</strong>
          {job.prompt && <span className="muted"> · {job.prompt}</span>}
        </span>
        <button className="link" onClick={() => api.mediaCancel(job.id)}>{job.status === "queued" ? t("Remove") : t("Stop")}</button>
      </div>
      <div className={`progress ${job.status === "queued" ? "indeterminate" : ""}`}>
        <span style={{ width: job.status === "queued" ? undefined : `${Math.max(3, pct)}%` }} />
      </div>
      <span className="small muted">
        {job.status === "queued"
          ? t("Waiting for the job before it · takes {time}", { time: waitLabel(job.est_secs) })
          : `${job.stage}${job.eta_secs != null ? ` · ${t("{time} left", { time: waitLabel(job.eta_secs) })}` : ""}`}
      </span>
      {job.status === "running" && <JobDevice job={job} />}
    </div>
  );
}

/** What's making it right now: the card or processor by name, how busy it
 * is and (the card) how warm; the card is shown too while the processor works. */
function JobDevice({ job }: { job: MediaJob }) {
  if (!job.on) return null;
  if (job.on === "cloud") return <div className="job-device small muted">{t("Making it in the cloud with {model}", { model: job.device ?? t("a cloud model") })}</div>;
  const card = job.gpu_usage != null || job.gpu_temp != null;
  return (
    <div className="job-device">
      <span className="small ellipsis">
        {t("Making it on")} <strong>{job.device ?? (job.on === "processor" ? t("the processor") : t("the graphics card"))}</strong>
      </span>
      {job.on === "processor" ? (
        <>
          <UseBar label={t("Processor")} usage={job.cpu_usage} temp={null} />
          {card && <UseBar label={t("Graphics card")} usage={job.gpu_usage} temp={job.gpu_temp} />}
        </>
      ) : (
        <UseBar label={t("Graphics card")} usage={job.gpu_usage} temp={job.gpu_temp} />
      )}
    </div>
  );
}

/** How busy a card or processor is, and how warm, while a job runs. */
export function UseBar({ label, usage, temp }: { label: string; usage: number | null; temp: number | null }) {
  // Under 70 °C is fine, to 83 warm, above that hot (most cards slow down near 85-90).
  const heat = temp == null ? "" : temp >= 83 ? "hot" : temp >= 70 ? "warm" : "cool";
  return (
    <div className="gpu-bar" title={t("Updated every 2 seconds")}>
      <span className="small muted gpu-label">{label}</span>
      <div className="gpu-meter" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(usage ?? 0)}>
        <span style={{ width: `${Math.min(100, Math.max(2, usage ?? 0))}%` }} />
      </div>
      <span className="small">{usage != null ? t("{n}% busy", { n: Math.round(usage) }) : "—"}</span>
      {temp != null && <span className={`small gpu-temp ${heat}`}>{Math.round(temp)} °C</span>}
    </div>
  );
}

/** "Took 41 s on NVIDIA GeForce RTX 5070 · about 32 prompt tokens · 8 steps · up to 71 °C, 88% busy on average". */
export function madeLine(s: MadeStats | null | undefined): string {
  if (!s) return "";
  const parts = [
    s.device
      ? t("Took {time} on {device}", { time: took(s.secs), device: s.device })
      : s.on === "cloud"
        ? t("Took {time} in the cloud", { time: took(s.secs) })
        : s.on === "processor"
          ? t("Took {time} on the processor", { time: took(s.secs) })
          : t("Took {time} on the graphics card", { time: took(s.secs) }),
  ];
  if (s.prompt_tokens) parts.push(t("about {n} prompt tokens", { n: s.prompt_tokens }));
  if (s.audio_tokens) parts.push(t("{n} audio tokens", { n: s.audio_tokens.toLocaleString() }));
  if (s.frames) parts.push(t("{n} frames", { n: s.frames }));
  if (s.steps) parts.push(t("{n} steps", { n: s.steps }));
  if (s.gpu_peak_temp != null || s.gpu_avg_usage != null) {
    const temp = s.gpu_peak_temp != null ? t("up to {n} °C", { n: Math.round(s.gpu_peak_temp) }) : "";
    const busy = s.gpu_avg_usage != null ? t("{n}% busy on average", { n: Math.round(s.gpu_avg_usage) }) : "";
    parts.push([temp, busy].filter(Boolean).join(", "));
  } else if (s.cpu_avg_usage != null) {
    parts.push(t("processor {n}% busy on average", { n: Math.round(s.cpu_avg_usage) }));
  }
  return parts.join(" · ");
}

/** "41 s", "3 min 12 s". */
function took(secs: number): string {
  const s = Math.max(0, Math.round(secs * 10) / 10);
  if (s < 10) return t("{s} s", { s: s.toFixed(1) });
  if (s < 60) return t("{s} s", { s: Math.round(s) });
  const m = Math.floor(s / 60);
  return t("{m} min {s} s", { m, s: Math.round(s - m * 60) });
}

/** The line under something made: how long it took and what it used. */
export function MadeStatsLine({ item }: { item: MediaItem }) {
  const line = madeLine(item.stats);
  return line ? <p className="made-stats small muted">{line}</p> : null;
}

/** The listener on a sung song: what came through, and the unclear parts made again. */
export function SongListener({ item, onListen, onRemake }: { item: MediaItem; onListen: () => void; onRemake: () => void }) {
  const r = item.review;
  return (
    <div className="song-listener small">
      <div className="row">
        <strong>{t("Listener")}</strong>
        {r && r.words > 0 && <span className="muted">{t("{heard} of {words} words heard", { heard: r.heard, words: r.words })}</span>}
        <span className="spacer" />
        <button className="btn small" onClick={onListen}>{r ? t("Listen again") : t("Listen to it")}</button>
      </div>
      {!r && <p className="muted">{t("Hears which words come through and where. The unclear parts can then be made again.")}</p>}
      {r && (
        <ul className="listener-notes">
          {r.words === 0 && <li>{t("Install a speech model (Models, Speech) and the listener also hears which words come through.")}</li>}
          {r.words > 0 && r.unclear.length === 0 && <li>{t("The words come through clearly.")}</li>}
          {r.unclear.map(([a, b]) => (
            <li key={a}>{t("The words from {from} to {to} aren't clear.", { from: clock(a), to: clock(b) })}</li>
          ))}
        </ul>
      )}
      {r && r.unclear.length > 0 && (
        <button className="btn primary small" onClick={onRemake}>{t("Make the unclear parts again")}</button>
      )}
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
  // On a paired phone it's saved to the phone, as its browser downloads files.
  if (onPhone()) {
    try {
      const url = await fileUrl(item);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${base}.${ext}`;
      a.click();
    } catch (e) {
      toast(errorText(e), "error");
    }
    return;
  }
  const path = await save({ defaultPath: `${base}.${ext}`, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] });
  if (!path) return;
  try {
    await api.mediaExport(item.id, path);
    toast(t("Saved."), "success");
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
      toast(t("The trimmed part is saved in the gallery."), "success");
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
      <canvas ref={canvas} className="wave" onClick={seek} aria-label={t("Waveform: click to jump")} />
      {dur > 0 && (
        <>
          <div className="trim-row">
            <label className="small">
              {t("Start {time}", { time: clock(range[0]) })}
              <input type="range" min={0} max={dur} step={0.1} value={range[0]} onChange={(e) => setRange([Math.min(+e.target.value, range[1] - 0.5), range[1]])} />
            </label>
            <label className="small">
              {t("End {time}", { time: clock(range[1]) })}
              <input type="range" min={0} max={dur} step={0.1} value={range[1]} onChange={(e) => setRange([range[0], Math.max(+e.target.value, range[0] + 0.5)])} />
            </label>
          </div>
          <div className="row">
            <label className="row small">
              <input type="checkbox" checked={loop} onChange={(e) => setLoop(e.target.checked)} /> {t("Loop")}
            </label>
            <span className="small muted">{clock(now)} / {clock(dur)}</span>
            <span className="spacer" />
            {trimmed && (
              <button className="btn small" disabled={busy || !buf} onClick={saveTrim}>
                {t("Save the trimmed part")}
              </button>
            )}
            <button className="btn small" onClick={() => saveAs(item, toast)}>{t("Save as…")}</button>
          </div>
        </>
      )}
    </div>
  );
}

/** What the assistant made, shown in its tool card. */
export function MediaInline({ items, onOpen, toast }: { items: MediaItem[]; onOpen?: (i: MediaItem) => void; toast?: PushToast }) {
  return (
    <div className="media-inline">
      {items.map((i) => (
        <div key={i.id} className="media-inline-one">
          <InlineOne item={i} onOpen={onOpen} />
          <MadeStatsLine item={i} />
          {toast && (
            <button className="btn ghost small" onClick={() => saveAs(i, toast)}>
              {t("Save…")}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

function InlineOne({ item, onOpen }: { item: MediaItem; onOpen?: (i: MediaItem) => void }) {
  const src = useMediaUrl(item, item.kind === "image");
  if (!src) return <div className="media-inline-ph">…</div>;
  if (item.kind === "image")
    return (
      <button className="media-inline-img" onClick={() => onOpen?.(item)} title={t("Open")}>
        <img src={src} alt={item.prompt || t("Picture")} />
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
        {src && <img src={src} alt={item.prompt || t("Picture")} />}
        <MadeStatsLine item={item} />
        <div className="row">
          <span className="small muted ellipsis">{item.prompt}</span>
          <span className="spacer" />
          <button className="btn small" onClick={() => saveAs(item, toast)}>{t("Save as…")}</button>
          <button className="btn small" onClick={onClose}>{t("Close")}</button>
        </div>
      </div>
    </div>
  );
}
