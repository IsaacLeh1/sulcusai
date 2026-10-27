// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, type MediaItem, type MediaRequest, type VideoEdit, type Voice } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { InfoTip } from "./InfoTip";
import { useOwnVoices } from "./OwnVoice";
import { LanguageSelect, TALK_LANGUAGES, VoiceSelect } from "./Podcast";

/** A video's edits, each made as a new video beside it: cut to a stretch,
 * other clips joined after it, a sound track (or its sound off), a sound
 * made for it by the music model, and its speech dubbed into another
 * language. `video` is the player, for the "here" buttons. */
export function VideoEditor({ item, run, video }: { item: MediaItem; run: (r: Omit<MediaRequest, "source">) => void; video: () => HTMLVideoElement | null }) {
  const [open, setOpen] = useState<"edit" | "sound" | "dub" | "captions" | null>(null);
  const [captionsIn, setCaptionsIn] = useState("same");
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(item.seconds || 0);
  const [join, setJoin] = useState<string[]>([]);
  const [track, setTrack] = useState<string>("");
  const [trackFile, setTrackFile] = useState<string | null>(null);
  const [mode, setMode] = useState<"replace" | "mix">("replace");
  const [volume, setVolume] = useState(1);
  const [mute, setMute] = useState(false);
  const [videos, setVideos] = useState<MediaItem[]>([]);
  const [sounds, setSounds] = useState<MediaItem[]>([]);
  const [described, setDescribed] = useState("");
  const [language, setLanguage] = useState("es");
  const [voice, setVoice] = useState<string | null>(null);
  const [voices, setVoices] = useState<Voice[]>([]);
  const own = (useOwnVoices()?.voices ?? []).filter((v) => v.can_speak);
  useEffect(() => {
    if (!open) return;
    api.mediaList("video", null, 200).then((v) => setVideos((v ?? []).filter((x) => x.id !== item.id))).catch(() => {});
    api.mediaList("audio", null, 200).then((a) => setSounds(a ?? [])).catch(() => {});
    if (open === "dub") api.voices().then(setVoices).catch(() => {});
  }, [open, item.id]);
  const here = () => video()?.currentTime ?? 0;
  const length = item.seconds || end;
  const edit = (): VideoEdit => ({
    trim: start > 0.05 || end < length - 0.05 ? [start, end] : null,
    join,
    audio: track || null,
    audio_file: track ? null : trackFile,
    audio_mode: mode,
    volume,
    mute: mute && !track && !trackFile,
  });
  const changes = start > 0.05 || end < length - 0.05 || join.length > 0 || !!track || !!trackFile || mute;
  return (
    <div className="video-editor">
      <div className="filters tight">
        <button className={`chip ${open === "edit" ? "active" : ""}`} onClick={() => setOpen(open === "edit" ? null : "edit")}>{t("Cut, join, sound")}</button>
        <button className={`chip ${open === "sound" ? "active" : ""}`} onClick={() => setOpen(open === "sound" ? null : "sound")}>{t("Make sound for it")}</button>
        <button className={`chip ${open === "dub" ? "active" : ""}`} onClick={() => setOpen(open === "dub" ? null : "dub")}>{t("Dub it")}</button>
        <button className={`chip ${open === "captions" ? "active" : ""}`} onClick={() => setOpen(open === "captions" ? null : "captions")}>{t("Captions")}</button>
      </div>
      {open === "edit" && (
        <div className="form">
          <div className="ve-keep small">
            <span className="music-label">{t("Keep")}</span>
            <span className="ve-bound">
              {t("from")}
              <input className="input num" type="number" min={0} max={end} step={0.1} value={start.toFixed(1)} onChange={(e) => setStart(Math.max(0, Math.min(+e.target.value, end - 0.1)))} aria-label={t("Keep from (seconds)")} />
              <button className="link small" onClick={() => setStart(Math.min(here(), end - 0.1))} title={t("Where the video is now")}>{t("here")}</button>
            </span>
            <span className="ve-bound">
              {t("to")}
              <input className="input num" type="number" min={start} max={length} step={0.1} value={end.toFixed(1)} onChange={(e) => setEnd(Math.max(start + 0.1, Math.min(+e.target.value, length)))} aria-label={t("Keep to (seconds)")} />
              <button className="link small" onClick={() => setEnd(Math.max(here(), start + 0.1))} title={t("Where the video is now")}>{t("here")}</button>
            </span>
            <span className="muted">{t("{time} of {all}", { time: clock(end - start), all: clock(length) })}</span>
          </div>
          <div className="row wrap small">
            <span className="music-label">{t("Join after it")}</span>
            <select value="" onChange={(e) => e.target.value && setJoin((j) => [...j, e.target.value])} aria-label={t("A clip to join after it")}>
              <option value="">{videos.length ? t("Add a clip…") : t("No other videos in the gallery")}</option>
              {videos.map((v) => (
                <option key={v.id} value={v.id}>{(v.prompt || t("A video")).slice(0, 60)} · {clock(v.seconds)}</option>
              ))}
            </select>
            {join.map((id, i) => (
              <span key={`${id}-${i}`} className="chip active">
                {i + 1}. {(videos.find((v) => v.id === id)?.prompt || t("A video")).slice(0, 24)}
                <button className="chip-x" onClick={() => setJoin((j) => j.filter((_, k) => k !== i))} aria-label={t("Remove")}>✕</button>
              </span>
            ))}
          </div>
          <div className="row wrap small">
            <span className="music-label">
              {t("Sound")}
              <InfoTip label={t("Sound")} text={t("Put a song or sound from the gallery (or a file) on the video: in place of its sound, or mixed with it. Or take its sound off.")} />
            </span>
            <select value={track} onChange={(e) => (setTrack(e.target.value), setTrackFile(null))} aria-label={t("A song or sound")}>
              <option value="">{trackFile ? trackFile.split(/[\\/]/).pop() : t("Its own sound")}</option>
              {sounds.map((s) => (
                <option key={s.id} value={s.id}>{(s.title || s.prompt || t("A sound")).slice(0, 60)} · {clock(s.seconds)}</option>
              ))}
            </select>
            <button
              className="link small"
              onClick={async () => {
                const p = await openFile({ multiple: false, filters: [{ name: t("Sound"), extensions: ["mp3", "wav", "m4a", "ogg", "flac", "aac"] }] });
                if (typeof p === "string") (setTrackFile(p), setTrack(""));
              }}
            >
              {t("From a file…")}
            </button>
            {(track || trackFile) && (
              <>
                <div className="filters tight">
                  <button className={`chip ${mode === "replace" ? "active" : ""}`} onClick={() => setMode("replace")}>{t("In place of its sound")}</button>
                  <button className={`chip ${mode === "mix" ? "active" : ""}`} onClick={() => setMode("mix")}>{t("Mixed in")}</button>
                </div>
                <label className="row music-length">
                  {t("Loudness")}
                  <input type="range" min={0} max={2} step={0.05} value={volume} onChange={(e) => setVolume(+e.target.value)} />
                  {Math.round(volume * 100)}%
                </label>
              </>
            )}
            {!track && !trackFile && (
              <label className="check">
                <input type="checkbox" checked={mute} onChange={(e) => setMute(e.target.checked)} />
                {t("Take its sound off")}
              </label>
            )}
          </div>
          <div className="row">
            <span className="spacer" />
            <button className="btn primary small" disabled={!changes} onClick={() => (run({ op: "video_edit", prompt: "", edit: edit() }), setOpen(null))}>
              {t("Make the edited video")}
            </button>
          </div>
        </div>
      )}
      {open === "sound" && (
        <div className="form">
          <p className="small muted">{t("The music model makes a sound as long as the video (a sound effect for a short clip, music for a longer one) and puts it on, mixed with its sound when it has some.")}</p>
          <div className="row">
            <input className="input grow" value={described} onChange={(e) => setDescribed(e.target.value)} placeholder={t("What it should sound like (optional), e.g. waves and seagulls")} />
            <button className="btn primary small" onClick={() => (run({ op: "video_sound", prompt: described }), setOpen(null))}>{t("Make it")}</button>
          </div>
        </div>
      )}
      {open === "captions" && (
        <div className="form">
          <p className="small muted">{t("What's said in it is written along the bottom, in time with it, in the language spoken or translated. You get a new copy with the captions, and their text is kept with it.")}</p>
          <div className="ve-keep small">
            <span className="ve-bound">
              {t("In")}
              <select value={captionsIn} onChange={(e) => setCaptionsIn(e.target.value)} aria-label={t("Captions in")}>
                <option value="same">{t("The language spoken")}</option>
                {TALK_LANGUAGES.map(([c, n]) => (
                  <option key={c} value={c}>{t(n)}</option>
                ))}
              </select>
            </span>
          </div>
          <div className="row">
            <span className="spacer" />
            <button className="btn primary small" onClick={() => (run({ op: "captions", prompt: "", language: captionsIn }), setOpen(null))}>{t("Add captions")}</button>
          </div>
        </div>
      )}
      {open === "dub" && (
        <div className="form">
          <p className="small muted">{t("What's said in it is heard, written down, translated and spoken again in the language you pick, in time with the video. The music and other sounds stay.")}</p>
          <div className="ve-keep small">
            <span className="ve-bound">
              {t("Into")}
              <LanguageSelect value={language} onChange={setLanguage} />
            </span>
            <span className="ve-bound">
              {t("Voice")}
              <VoiceSelect value={voice} onChange={setVoice} voices={voices} own={own} label={t("Voice")} pickForMe />
            </span>
          </div>
          <div className="row">
            <span className="spacer" />
            <button className="btn primary small" onClick={() => (run({ op: "dub", prompt: "", language, voice }), setOpen(null))}>{t("Dub it")}</button>
          </div>
        </div>
      )}
    </div>
  );
}
