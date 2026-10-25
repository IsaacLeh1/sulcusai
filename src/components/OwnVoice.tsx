// SPDX-License-Identifier: AGPL-3.0-only
// Your voice: the person reads a short script once (lines said in different
// ways) and sings a little; narration can then read in their voice and
// songs can be sung in it. The recordings stay on this PC, encrypted.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, on, type OwnVoice, type OwnVoiceLine, type OwnVoicesView, type OwnVoiceTake } from "../api";
import { bytes } from "../format";
import { language, t, tx } from "../i18n";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { saveAs } from "./MediaBits";
import type { PushToast } from "./Toasts";

/** How to say each line, and its words. The song's lines are SulcusAI's own. */
const SCRIPT: Record<string, { how: string; words: string }> = {
  intro: { how: tx("Introduce yourself, in your usual voice"), words: tx("Hi, this is my voice. I'm recording a few lines so my assistant can read and sing the way I sound.") },
  everyday: { how: tx("Like talking to a friend"), words: tx("I'll pick up a few groceries on the way home, so tell me if we need anything else for dinner tonight.") },
  story: { how: tx("Like reading a story aloud"), words: tx("The lighthouse keeper climbed the stairs one last time, lit the lamp, and watched the little boats find their way home through the fog.") },
  question: { how: tx("As real questions"), words: tx("Wait, are we meeting at seven or at eight? And did anyone remember to book the table?") },
  excited: { how: tx("Excited, with energy"), words: tx("We did it! I honestly can't believe it worked on the very first try!") },
  calm: { how: tx("Slow and calm"), words: tx("Take a slow breath in, hold it for a moment, and let it out gently. There's nothing you need to do right now.") },
  sounds: { how: tx("Clearly, every word"), words: tx("A jazzy zebra quickly chewed six thick slices of fresh orange while the vacuum hummed in the yellow hallway.") },
  numbers: { how: tx("Numbers and a date"), words: tx("My appointment is on March twenty-third at four fifteen, at 1280 West Maple Avenue, apartment nine.") },
  hold: { how: tx("Sing “ah” and hold one comfortable note for about five seconds"), words: tx("Aaah…") },
  scale: { how: tx("Sing “la” up five notes and back down, like a warm-up"), words: tx("La la la la la, la la la la la") },
  song: { how: tx("Sing 20 to 60 seconds of a song you know well, or these lines to any tune"), words: tx("Morning light on the window, coffee on the stove, every little sound outside is singing me back home.") },
  soft_loud: { how: tx("Sing one line softly, then the same line in full voice"), words: tx("Light on the water, light in the sky.") },
};

const NOTES: Record<string, string> = {
  quiet: tx("A bit quiet: move closer to the microphone."),
  loud: tx("It distorted: move back a little or sing softer."),
  noisy: tx("Background noise came through: a quieter room helps."),
};

/** The recorded voices and what they can do (loaded once per mount). */
export function useOwnVoices(): OwnVoicesView | null {
  const [v, setV] = useState<OwnVoicesView | null>(null);
  useEffect(() => {
    api.ownVoices().then(setV).catch(() => {});
  }, []);
  return v;
}

function wavUrl(b64: string): string {
  const raw = atob(b64);
  const buf = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) buf[i] = raw.charCodeAt(i);
  return URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
}

export function OwnVoicePanel({ toast }: { toast: PushToast }) {
  const [view, setView] = useState<OwnVoicesView | null>(null);
  const [picked, setPicked] = useState("");
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState(t("My voice"));
  const [agreed, setAgreed] = useState(false);
  const [recording, setRecording] = useState<string | null>(null);
  const [level, setLevel] = useState(0);
  const [secs, setSecs] = useState(0);
  const [saving, setSaving] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [tryText, setTryText] = useState(t("This is how I sound when SulcusAI reads something for me."));
  const [trying, setTrying] = useState(false);
  // The last sentence heard: its sound (to play and keep) and words.
  const [tried, setTried] = useState<{ url: string; b64: string; text: string } | null>(null);
  const player = useRef<HTMLAudioElement | null>(null);
  const stopping = useRef(false);
  // Recordings the person already has, being added: which file and what's happening.
  const [adding, setAdding] = useState<{ file: string; step: string; n: number; of: number } | null>(null);
  const [addKind, setAddKind] = useState<"sing" | "speak">("sing");
  useEffect(() => {
    const sub = on("own_voice:adding", (d) => setAdding({ file: d.file, step: d.step, n: d.n, of: d.of }));
    return () => {
      sub.then((un) => un());
    };
  }, []);

  const take = useCallback((v: OwnVoicesView) => {
    setView(v);
    setPicked((p) => (v.voices.some((x) => x.id === p) ? p : v.voices[v.voices.length - 1]?.id ?? ""));
  }, []);
  useEffect(() => {
    api.ownVoices().then(take).catch((e) => toast(errorText(e), "error"));
    return () => {
      // Leaving the page while recording throws the take away.
      api.ownVoiceCancel().catch(() => {});
      player.current?.pause();
    };
  }, [take, toast]);

  const voice: OwnVoice | undefined = view?.voices.find((v) => v.id === picked);

  const stop = useCallback(async () => {
    if (!recording || stopping.current) return;
    stopping.current = true;
    const line = recording;
    setRecording(null);
    setSaving(line);
    try {
      take(await api.ownVoiceStop(t(SCRIPT[line]?.words ?? "")));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(null);
      stopping.current = false;
    }
  }, [recording, take, toast]);

  useEffect(() => {
    const lv = on("own_voice:level", (d) => {
      setLevel(d.level);
      setSecs(d.seconds);
    });
    const full = on("own_voice:full", () => void stop());
    return () => {
      lv.then((un) => un());
      full.then((un) => un());
    };
  }, [stop]);

  const record = async (line: string) => {
    if (!voice) return;
    player.current?.pause();
    try {
      setLevel(0);
      setSecs(0);
      await api.ownVoiceRecord(voice.id, line);
      setRecording(line);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const play = async (line: string) => {
    if (!voice) return;
    try {
      player.current?.pause();
      const a = new Audio(wavUrl(await api.ownVoiceTake(voice.id, line)));
      player.current = a;
      await a.play();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const create = async () => {
    try {
      take(await api.ownVoiceCreate(name, agreed));
      setCreating(false);
      setAgreed(false);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const tryIt = async () => {
    if (!voice || !tryText.trim()) return;
    setTrying(true);
    try {
      const b64 = await api.ownVoiceTry(voice.id, tryText, language());
      setTried({ url: wavUrl(b64), b64, text: tryText });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setTrying(false);
    }
  };

  const addFiles = async () => {
    if (!voice) return;
    const picked = await openFile({ multiple: true, filters: [{ name: t("Recordings"), extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "wma", "mp4"] }] });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    if (paths.length === 0) return;
    player.current?.pause();
    setAdding({ file: "", step: "reading", n: 1, of: paths.length });
    try {
      const r = await api.ownVoiceUpload(voice.id, paths, addKind === "sing");
      take(r);
      if (r.problems.length) toast(r.problems.join("\n"), "error");
      else toast(t("Added. Your recordings are used before the lines below."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setAdding(null);
    }
  };

  // The sentence heard, kept in the gallery as a narration (and saved where
  // the person picks, with `save`).
  const keep = async (save: boolean) => {
    if (!tried) return;
    try {
      const seconds = Math.max(0, atob(tried.b64).length - 44) / 2 / 24000;
      const item = await api.mediaAdd({ data: tried.b64, mime: "audio/wav", op: "narrate", prompt: tried.text, seconds });
      if (save) await saveAs(item, toast);
      else toast(t("Kept in the gallery (Studio, Sound)."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  if (!view) return null;

  const newVoice = (
    <div className="form own-voice-new">
      <label>
        {t("Whose voice is it?")}
        <input className="input" value={name} maxLength={60} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="row small">
        <input type="checkbox" checked={agreed} onChange={(e) => setAgreed(e.target.checked)} />
        {t("This is my own voice, or the person whose voice it is has said yes.")}
      </label>
      <div className="row">
        <button className="btn primary" disabled={!agreed || !name.trim()} onClick={create}>{t("Start recording")}</button>
        {view.voices.length > 0 && <button className="btn ghost" onClick={() => setCreating(false)}>{t("Cancel")}</button>}
      </div>
    </div>
  );

  const lineCard = (l: OwnVoiceLine) => {
    const s = SCRIPT[l.id];
    const done = voice?.takes.find((x) => x.line === l.id);
    const live = recording === l.id;
    const busy = (recording !== null && !live) || saving !== null || adding !== null;
    const shown = s ? t(s.words) : "";
    // What speech recognition heard, when it's clearly not the line as written.
    const heardOther = done && l.kind === "speak" && done.words && done.words.toLowerCase().replace(/[^\p{L}\p{N} ]/gu, "") !== shown.toLowerCase().replace(/[^\p{L}\p{N} ]/gu, "");
    return (
      <div key={l.id} className={`own-voice-line ${done ? "done" : ""} ${live ? "live" : ""}`}>
        <div className="row">
          <span className="small muted">{s ? t(s.how) : l.id}</span>
          <span className="spacer" />
          {done && !live && <span className="small own-voice-check">{t("Recorded, {s} s", { s: done.seconds.toFixed(1) })}</span>}
        </div>
        <p className="own-voice-words">{shown}</p>
        {live && (
          <div className="own-voice-meter" aria-label={t("Loudness")}>
            <span style={{ width: `${Math.round(level * 100)}%` }} />
            <em className="small">{t("{s} s of up to {max} s", { s: secs.toFixed(1), max: l.max_s })}</em>
          </div>
        )}
        {done && !live && done.notes.length > 0 && (
          <p className="small warn-text">{done.notes.map((n) => (NOTES[n] ? t(NOTES[n]) : n)).join(" ")}</p>
        )}
        {heardOther && !live && <p className="small muted">{t("Heard: “{words}”", { words: done!.words })}</p>}
        <div className="row">
          {live ? (
            <button className="btn primary small" onClick={() => void stop()}>{t("Stop")}</button>
          ) : saving === l.id ? (
            <span className="small muted">{t("Saving…")}</span>
          ) : (
            <button className="btn small" disabled={busy} onClick={() => record(l.id)}>{done ? t("Record again") : t("Record")}</button>
          )}
          {done && !live && saving !== l.id && (
            <>
              <button className="btn ghost small" disabled={busy} onClick={() => play(l.id)}>{t("Listen")}</button>
              <button
                className="btn ghost small"
                disabled={busy}
                onClick={() => voice && api.ownVoiceTakeDelete(voice.id, l.id).then(take).catch((e) => toast(errorText(e), "error"))}
              >
                {t("Remove")}
              </button>
            </>
          )}
        </div>
      </div>
    );
  };

  // A recording the person added: its file, kind, length and how clear it is.
  const addedRow = (x: OwnVoiceTake) => (
    <div key={x.line} className="own-voice-added-row">
      <div className="row wrap">
        <strong className="small ellipsis">{x.file || t("A recording")}</strong>
        <span className="small muted">
          {x.kind === "speak" ? t("Talking") : t("Singing")} · {t("{s} s", { s: x.seconds.toFixed(0) })} · {t("{p}% voice", { p: Math.round((x.score ?? 0) * 100) })}
        </span>
        <span className="spacer" />
        <button className="btn ghost small" disabled={recording !== null || adding !== null} onClick={() => play(x.line)}>{t("Listen")}</button>
        <button
          className="btn ghost small"
          disabled={recording !== null || adding !== null}
          onClick={() => voice && api.ownVoiceTakeDelete(voice.id, x.line).then(take).catch((e) => toast(errorText(e), "error"))}
        >
          {t("Remove")}
        </button>
      </div>
      {x.notes.length > 0 && <p className="small warn-text">{x.notes.map((n) => (NOTES[n] ? t(NOTES[n]) : n)).join(" ")}</p>}
      {x.kind === "speak" && x.words && <p className="small muted">{t("Heard: “{words}”", { words: x.words })}</p>}
    </div>
  );
  const STEPS: Record<string, string> = { reading: tx("Reading it…"), separating: tx("Taking your voice out of the music…"), choosing: tx("Finding the clearest part…") };
  const added = voice?.takes.filter((x) => x.line.startsWith("added-")) ?? [];

  const speakLines = view.lines.filter((l) => l.kind === "speak");
  const singLines = view.lines.filter((l) => l.kind === "sing");
  const count = (lines: OwnVoiceLine[]) => lines.filter((l) => voice?.takes.some((x) => x.line === l.id)).length;

  return (
    <div className="own-voice">
      <p className="muted small">
        {t("Record yourself once: narration can then read in your voice and way of speaking, and songs can be sung in your voice. The recordings stay on this PC, encrypted, and are never sent anywhere.")}
      </p>
      {view.voices.length === 0 || creating ? (
        newVoice
      ) : (
        <>
          <div className="row wrap">
            {view.voices.length > 1 && (
              <select value={picked} onChange={(e) => setPicked(e.target.value)} aria-label={t("Voice")} disabled={recording !== null}>
                {view.voices.map((v) => (
                  <option key={v.id} value={v.id}>{v.name}</option>
                ))}
              </select>
            )}
            {voice && renaming === null && <strong>{view.voices.length === 1 ? voice.name : ""}</strong>}
            {voice && renaming !== null ? (
              <>
                <input className="input" value={renaming} maxLength={60} autoFocus aria-label={t("Name")} onChange={(e) => setRenaming(e.target.value)} />
                <button
                  className="btn small"
                  disabled={!renaming.trim()}
                  onClick={() => api.ownVoiceRename(voice.id, renaming).then((v) => { take(v); setRenaming(null); }).catch((e) => toast(errorText(e), "error"))}
                >
                  {t("Save")}
                </button>
                <button className="btn ghost small" onClick={() => setRenaming(null)}>{t("Cancel")}</button>
              </>
            ) : (
              voice && <button className="btn ghost small" disabled={recording !== null} onClick={() => setRenaming(voice.name)}>{t("Rename")}</button>
            )}
            <span className="spacer" />
            <button className="btn ghost small" disabled={recording !== null} onClick={() => { setCreating(true); setName(""); }}>{t("+ Another voice")}</button>
            {voice && (
              <button
                className="btn ghost small danger"
                disabled={recording !== null}
                onClick={() => {
                  if (!deleting) return setDeleting(true);
                  setDeleting(false);
                  api.ownVoiceDelete(voice.id).then(take).catch((e) => toast(errorText(e), "error"));
                }}
                onBlur={() => setDeleting(false)}
              >
                {deleting ? t("Delete {name} and its recordings?", { name: voice.name }) : t("Delete")}
              </button>
            )}
          </div>

          {voice && (
            <>
              <h3>{t("Recordings you already have")}</h3>
              <p className="small muted">
                {t("Add songs you've sung, or recordings of you talking, instead of (or as well as) the lines below. From a song, your voice is taken out of the music. The clearest 15 seconds of each are kept, and the clearest recording is used first. The lines you've recorded stay as they are.")}
              </p>
              <div className="row wrap">
                <div className="filters tight">
                  <button className={`chip ${addKind === "sing" ? "active" : ""}`} onClick={() => setAddKind("sing")}>{t("Songs I sang")}</button>
                  <button className={`chip ${addKind === "speak" ? "active" : ""}`} onClick={() => setAddKind("speak")}>{t("Me talking")}</button>
                </div>
                <button className="btn" disabled={adding !== null || recording !== null} onClick={addFiles}>{t("Add recordings…")}</button>
                {adding && (
                  <span className="small muted">
                    {adding.file ? t("{file} ({n} of {of}): {step}", { file: adding.file, n: adding.n, of: adding.of, step: t(STEPS[adding.step] ?? adding.step) }) : t("Reading it…")}
                  </span>
                )}
              </div>
              {added.length > 0 && <div className="own-voice-added">{added.map(addedRow)}</div>}

              <h3>{t("Speaking ({done} of {n})", { done: count(speakLines), n: speakLines.length })}</h3>
              <p className="small muted">
                {voice.can_speak
                  ? t("Narration can read in this voice. More lines, said in different ways, make it sound more like you.")
                  : t("Read at least three lines for narration. Say them naturally, about a hand's width from the microphone, in a quiet room.")}
              </p>
              <div className="own-voice-lines">{speakLines.map(lineCard)}</div>

              <h3>{t("Singing ({done} of {n})", { done: count(singLines), n: singLines.length })}</h3>
              <p className="small muted">
                {count(singLines) > 0
                  ? t("Songs are sung in this voice from your singing.")
                  : voice.can_sing
                    ? t("Songs can already use your reading; a little singing makes them sound more like you.")
                    : t("Sing a little, and songs can be sung in your voice. It doesn't have to be good, just you.")}
              </p>
              <div className="own-voice-lines">{singLines.map(lineCard)}</div>

              <h3>{t("Hear it")}</h3>
              {voice.can_speak ? (
                <div className="form">
                  <textarea className="input" rows={2} value={tryText} aria-label={t("Text to hear in this voice")} onChange={(e) => setTryText(e.target.value)} />
                  <div className="row wrap">
                    <button className="btn" disabled={trying || !tryText.trim() || recording !== null} onClick={tryIt}>{trying ? t("Reading in your voice…") : t("Read it in this voice")}</button>
                    {tried && (
                      <>
                        <audio src={tried.url} controls controlsList="nodownload" autoPlay />
                        <button className="btn small" onClick={() => keep(true)}>{t("Save…")}</button>
                        <button className="btn ghost small" onClick={() => keep(false)}>{t("Keep in the gallery")}</button>
                      </>
                    )}
                  </div>
                  {view.speak_size > 0 && <p className="small muted">{t("The first time, the voice engine and its model download ({size}).", { size: bytes(view.speak_size) })}</p>}
                </div>
              ) : (
                <p className="small muted">{t("Read three lines above, then hear a sentence in your voice here.")}</p>
              )}
              <p className="small muted">
                {t("Use it in Narration (pick it as the voice) and in Music (Sung in). Singing downloads its model the first time ({size}).", { size: bytes(view.sing_size) })}
              </p>
            </>
          )}
        </>
      )}
    </div>
  );
}
