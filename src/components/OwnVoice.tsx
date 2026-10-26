// SPDX-License-Identifier: AGPL-3.0-only
// Your voice: the person reads a short script once (lines said in different
// ways) and sings a little; narration can then read in their voice and
// songs can be sung in it. The recordings stay on this PC, encrypted.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, on, type CheckedRecording, type OwnVoice, type OwnVoiceLine, type OwnVoicesView, type OwnVoiceTake } from "../api";
import { bytes } from "../format";
import { language, t, tx } from "../i18n";
import { Busy } from "./Busy";
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

/** A voice from recordings the person already has, with no lines to read: each file is rated, and once there's enough clear voice the voice can be created. */
function FromRecordings({ view, onCreated, onCancel, toast }: { view: OwnVoicesView; onCreated: (v: OwnVoicesView) => void; onCancel?: () => void; toast: PushToast }) {
  const [kind, setKind] = useState<"speak" | "sing">("speak");
  const [checked, setChecked] = useState<(CheckedRecording & { use: boolean })[]>([]);
  const [reading, setReading] = useState<{ file: string; step: string; n: number; of: number } | null>(null);
  const [name, setName] = useState(t("My voice"));
  const [agreed, setAgreed] = useState(false);
  useEffect(() => {
    const sub = on("own_voice:adding", (d) => d.voice === "" && setReading({ file: d.file, step: d.step, n: d.n, of: d.of }));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  const add = async () => {
    const picked = await openFile({ multiple: true, filters: [{ name: t("Recordings"), extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "wma", "mp4"] }] });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    if (paths.length === 0) return;
    setReading({ file: "", step: "reading", n: 1, of: paths.length });
    try {
      const found = await api.ownVoiceCheck(paths, kind === "sing");
      setChecked((c) => [...c, ...found.map((f) => ({ ...f, use: !!f.token && f.quality >= 0.25 }))]);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setReading(null);
    }
  };
  const used = checked.filter((c) => c.use && c.token);
  const speech = used.filter((c) => c.kind === "speak").reduce((a, c) => a + c.seconds * c.clear, 0);
  const singing = used.filter((c) => c.kind === "sing").reduce((a, c) => a + c.seconds * c.clear, 0);
  const enough = speech >= view.speech_enough || singing > 0;
  const level = speech >= view.speech_best ? t("Plenty: narration should sound like you.") : speech >= view.speech_enough ? t("Enough for narration. More clear speech makes it sound more like you.") : t("Not enough yet: {n} of {need} s of clear speech.", { n: Math.round(speech), need: view.speech_enough });
  const create = async () => {
    try {
      onCreated(await api.ownVoiceCreateFrom(name, agreed, used.map((c) => c.token!)));
      toast(t("Voice created. Pick it in Narration, Podcast and Music."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const rating = (q: number) => (q >= 0.6 ? t("Clear") : q >= 0.25 ? t("Usable") : t("Poor"));
  const STEPS: Record<string, string> = {
    reading: tx("Reading it…"),
    getting_speakers: tx("Getting the speaker model (once)…"),
    speakers: tx("Finding who talks most…"),
    choosing: tx("Finding the clearest part…"),
    separating: tx("Taking out any music under the voice…"),
    getting_words: tx("Getting speech recognition (once)…"),
    hearing: tx("Hearing the words…"),
  };
  const listen = async (token: string) => {
    try {
      new Audio(wavUrl(await api.ownVoiceCheckedTake(token))).play();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form own-voice-from">
      <p className="small muted">
        {t("Make a voice from recordings you already have, with nothing to read: clips of you talking (a voice memo, a video's sound, a talk you gave) or songs you sang. Each is rated for how clear your voice is in it. Use recordings of only you, without other people talking.")}
      </p>
      <div className="row wrap">
        <div className="seg">
          <button className={kind === "speak" ? "active" : ""} onClick={() => setKind("speak")}>{t("Me talking")}</button>
          <button className={kind === "sing" ? "active" : ""} onClick={() => setKind("sing")}>{t("Songs I sang")}</button>
        </div>
        <button className="btn" disabled={reading !== null} onClick={add}>{t("Add recordings…")}</button>
        {reading && (
          <span className="small muted">
            {reading.file ? t("{file} ({n} of {of}): {step}", { file: reading.file, n: reading.n, of: reading.of, step: t(STEPS[reading.step] ?? reading.step) }) : t("Reading it…")}
          </span>
        )}
      </div>
      {checked.length > 0 && (
        <div className="own-voice-checked">
          {checked.map((c, i) => (
            <label key={i} className={`own-voice-checked-row ${c.token ? "" : "unusable"}`}>
              <input type="checkbox" disabled={!c.token} checked={c.use} onChange={(e) => setChecked((all) => all.map((x, j) => (j === i ? { ...x, use: e.target.checked } : x)))} />
              <span className="grow">
                <strong className="small ellipsis">{c.file}</strong>
                <span className="small muted">
                  {" "}
                  {c.kind === "speak" ? t("Talking") : t("Singing")}
                  {c.token && ` · ${t("{s} s kept", { s: c.seconds.toFixed(0) })} · ${t("{p}% voice", { p: Math.round(c.clear * 100) })}`}
                </span>
                {c.problem && <span className="small warn-text"> {c.problem}</span>}
                {c.several && <span className="small warn-text"> {t("Others talk in this too: only whoever talks most was kept. Listen to check it's you.")}</span>}
                {c.kind === "speak" && c.token && !c.words && <span className="small warn-text"> {t("Its words couldn't be heard, so the voice is copied only roughly from it.")}</span>}
                {c.notes.length > 0 && <span className="small warn-text"> {c.notes.map((n) => (NOTES[n] ? t(NOTES[n]) : n)).join(" ")}</span>}
              </span>
              {c.token && (
                <button className="btn ghost small" onClick={(e) => { e.preventDefault(); listen(c.token!); }}>{t("Listen")}</button>
              )}
              {c.token && (
                <span className={`quality q-${c.quality >= 0.6 ? "good" : c.quality >= 0.25 ? "ok" : "poor"}`} title={t("How clear and usable it is")}>
                  <span className="quality-bar"><span style={{ width: `${Math.round(c.quality * 100)}%` }} /></span>
                  <span className="small">{rating(c.quality)}</span>
                </span>
              )}
            </label>
          ))}
        </div>
      )}
      {checked.length > 0 && (
        <div className="own-voice-readiness">
          <div className="progress"><span style={{ width: `${Math.min(100, Math.round((speech / view.speech_best) * 100))}%` }} /></div>
          <span className="small">{level}</span>
          {singing > 0 && <span className="small muted">{t("Songs: {s} s of singing, so songs can be sung in it.", { s: Math.round(singing) })}</span>}
        </div>
      )}
      <label>
        {t("Whose voice is it?")}
        <input className="input" value={name} maxLength={60} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="check small">
        <input type="checkbox" checked={agreed} onChange={(e) => setAgreed(e.target.checked)} />
        {t("This is my own voice, or the person whose voice it is has said yes.")}
      </label>
      <div className="row">
        <button className="btn primary" disabled={!enough || !agreed || !name.trim()} onClick={create}>{t("Create voice")}</button>
        {onCancel && <button className="btn ghost" onClick={onCancel}>{t("Cancel")}</button>}
        {!enough && checked.length > 0 && <span className="small muted">{t("Add more recordings of you talking to create it.")}</span>}
      </div>
      <p className="small muted">{t("You can still read the script's lines afterwards, or add more recordings, to make it more accurate.")}</p>
    </div>
  );
}

/** Languages the voice maker reads its sample in. */
const DESIGN_LANGUAGES: [string, string][] = [
  ["en", tx("English")], ["es", tx("Spanish")], ["fr", tx("French")], ["de", tx("German")], ["it", tx("Italian")],
  ["pt", tx("Portuguese")], ["ru", tx("Russian")], ["ja", tx("Japanese")], ["ko", tx("Korean")], ["zh", tx("Chinese")],
];

/** A voice made from a description: three to choose from, each reading the same sample; the one kept sounds the same every time. */
function DesignVoice({ size, onKept, onCancel, toast }: { size: number; onKept: (v: OwnVoicesView) => void; onCancel?: () => void; toast: PushToast }) {
  const ui = (language() ?? "en").split(/[-_]/)[0];
  const [description, setDescription] = useState("");
  const [lang, setLang] = useState(DESIGN_LANGUAGES.some(([c]) => c === ui) ? ui : "en");
  const [making, setMaking] = useState<{ stage: string; fraction: number } | null>(null);
  const [made, setMade] = useState<{ url: string; seconds: number }[]>([]);
  const [pick, setPick] = useState(0);
  const [name, setName] = useState("");
  useEffect(() => {
    const sub = on("own_voice:designing", (d) => setMaking(d));
    return () => {
      sub.then((un) => un());
      api.ownVoiceDesignCancel().catch(() => {});
    };
  }, []);
  const make = async () => {
    setMaking({ stage: t("Starting"), fraction: 0 });
    setMade([]);
    try {
      const samples = await api.ownVoiceDesign(description, lang);
      setMade(samples.map((s) => ({ url: wavUrl(s.wav), seconds: s.seconds })));
      setPick(0);
    } catch (e) {
      const msg = errorText(e);
      if (!/cancel/i.test(msg)) toast(msg, "error");
    } finally {
      setMaking(null);
    }
  };
  const keep = async () => {
    try {
      onKept(await api.ownVoiceDesignKeep(name, pick));
      toast(t("Kept. Pick it as a voice in Narration and Podcast."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form own-voice-design">
      <p className="small muted">
        {t("Describe how the voice sounds: a man's or a woman's, their age, pitch, pace, mood and accent. It's made on this PC from your words; it isn't a copy of a real person's voice.")}
      </p>
      <textarea
        className="input"
        rows={3}
        maxLength={400}
        value={description}
        placeholder={t("A warm older woman with a low, calm voice, speaking slowly and kindly, with a slight British accent")}
        aria-label={t("Describe the voice")}
        onChange={(e) => setDescription(e.target.value)}
      />
      <div className="row wrap">
        <select value={lang} onChange={(e) => setLang(e.target.value)} aria-label={t("Language of the sample")} title={t("Language of the sample")}>
          {DESIGN_LANGUAGES.map(([c, n]) => (
            <option key={c} value={c}>{t(n)}</option>
          ))}
        </select>
        {making ? (
          <>
            <span className="small muted">{making.fraction > 0 && making.fraction < 1 ? `${making.stage} ${Math.round(making.fraction * 100)}%` : making.stage}</span>
            <button className="btn ghost small" onClick={() => api.ownVoiceDesignCancel().catch(() => {})}>{t("Stop")}</button>
          </>
        ) : (
          <button className="btn primary" disabled={description.trim().length < 4} onClick={make}>{made.length ? t("Make three more") : t("Make three voices")}</button>
        )}
        {onCancel && !making && <button className="btn ghost" onClick={onCancel}>{t("Cancel")}</button>}
      </div>
      {size > 0 && <p className="small muted">{t("The first time, the voice maker downloads ({size}).", { size: bytes(size) })}</p>}
      {made.length > 0 && (
        <>
          <div className="own-voice-made">
            {made.map((m, i) => (
              <label key={m.url} className={`own-voice-made-row row ${pick === i ? "picked" : ""}`}>
                <input type="radio" name="made-voice" checked={pick === i} onChange={() => setPick(i)} />
                <span className="small">{t("Voice {n}", { n: i + 1 })}</span>
                <audio src={m.url} controls controlsList="nodownload" />
              </label>
            ))}
          </div>
          <div className="row wrap">
            <input className="input" value={name} maxLength={60} placeholder={t("Name it")} aria-label={t("Name")} onChange={(e) => setName(e.target.value)} />
            <button className="btn primary" disabled={!name.trim()} onClick={keep}>{t("Keep voice {n}", { n: pick + 1 })}</button>
          </div>
        </>
      )}
    </div>
  );
}

export function OwnVoicePanel({ toast }: { toast: PushToast }) {
  const [view, setView] = useState<OwnVoicesView | null>(null);
  const [picked, setPicked] = useState("");
  const [creating, setCreating] = useState(false);
  // Making a voice from a description instead of recording one.
  const [designing, setDesigning] = useState(false);
  // Making a voice from recordings the person has, with nothing to read.
  const [fromFiles, setFromFiles] = useState(false);
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
  // What hearing a sentence is doing (downloading the first time, then reading), and why it failed.
  const [tryStage, setTryStage] = useState<{ stage: string; fraction: number; downloading: boolean; since: number } | null>(null);
  const [tryError, setTryError] = useState<string | null>(null);
  const [, tick] = useState(0);
  useEffect(() => {
    const sub = on("own_voice:trying", (d) => setTryStage((s) => ({ stage: d.stage, fraction: d.fraction, downloading: d.downloading, since: s?.since ?? Date.now() })));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  // The time so far, each second while it works.
  useEffect(() => {
    if (!trying) return;
    const id = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(id);
  }, [trying]);
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
    setTryError(null);
    setTryStage({ stage: t("Starting"), fraction: -1, downloading: false, since: Date.now() });
    try {
      const b64 = await api.ownVoiceTry(voice.id, tryText, language());
      setTried({ url: wavUrl(b64), b64, text: tryText });
    } catch (e) {
      setTryError(errorText(e));
    } finally {
      setTrying(false);
      setTryStage(null);
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

  const ways = (
    <div className="seg own-voice-ways" role="tablist" aria-label={t("How to make the voice")}>
      <button role="tab" aria-selected={!fromFiles && !designing} className={!fromFiles && !designing ? "active" : ""} onClick={() => { setFromFiles(false); setDesigning(false); setCreating(true); }}>{t("Read a few lines")}</button>
      <button role="tab" aria-selected={fromFiles} className={fromFiles ? "active" : ""} onClick={() => { setFromFiles(true); setDesigning(false); }}>{t("Use recordings I have")}</button>
      {view && view.design_size > 0 && (
        <button role="tab" aria-selected={designing} className={designing ? "active" : ""} onClick={() => { setDesigning(true); setFromFiles(false); }}>{t("Describe a voice")}</button>
      )}
    </div>
  );

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
  const STEPS: Record<string, string> = {
    reading: tx("Reading it…"),
    getting_speakers: tx("Getting the speaker model (once)…"),
    speakers: tx("Finding who talks most…"),
    choosing: tx("Finding the clearest part…"),
    separating: tx("Taking your voice out of the music…"),
    getting_words: tx("Getting speech recognition (once)…"),
    hearing: tx("Hearing the words…"),
  };
  const added = voice?.takes.filter((x) => x.line.startsWith("added-")) ?? [];

  const speakLines = view.lines.filter((l) => l.kind === "speak");
  const singLines = view.lines.filter((l) => l.kind === "sing");
  const count = (lines: OwnVoiceLine[]) => lines.filter((l) => voice?.takes.some((x) => x.line === l.id)).length;

  return (
    <div className="own-voice">
      <p className="muted small">
        {t("Record yourself once: narration can then read in your voice and way of speaking, and songs can be sung in your voice. The recordings stay on this PC, encrypted, and are never sent anywhere.")}
      </p>
      {(designing || fromFiles || view.voices.length === 0 || creating) && ways}
      {fromFiles ? (
        <FromRecordings
          view={view}
          onCreated={(v) => {
            take(v);
            setFromFiles(false);
            setCreating(false);
            setPicked(v.voices[v.voices.length - 1]?.id ?? "");
          }}
          onCancel={view.voices.length > 0 ? () => setFromFiles(false) : undefined}
          toast={toast}
        />
      ) : designing ? (
        <DesignVoice
          size={view.design_size}
          onKept={(v) => {
            take(v);
            setDesigning(false);
            setPicked(v.voices[v.voices.length - 1]?.id ?? "");
          }}
          onCancel={view.voices.length > 0 ? () => setDesigning(false) : undefined}
          toast={toast}
        />
      ) : view.voices.length === 0 || creating ? (
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
            {view.design_size > 0 && <button className="btn ghost small" disabled={recording !== null} onClick={() => setDesigning(true)}>{t("+ Make one from words")}</button>}
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

          {voice?.designed && (
            <div className="own-voice-designed">
              <p className="small">{t("Made from: “{description}”", { description: voice.designed })}</p>
              <button className="btn ghost small" onClick={() => play("added-1")}>{t("Listen to its sample")}</button>
            </div>
          )}
          {voice && !voice.designed && (
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
            </>
          )}
          {voice && (
            <>
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
                  {trying && tryStage && (
                    <div className="own-voice-trying">
                      <Busy
                        stage={tryStage.stage}
                        fraction={tryStage.downloading && tryStage.fraction >= 0 ? tryStage.fraction : null}
                        since={tryStage.since}
                        hint={tryStage.downloading ? undefined : t("Working: the first reading loads the voice model, which can take a minute. Nothing else is needed from you.")}
                      />
                    </div>
                  )}
                  {tryError && !trying && <p className="small warn-text" role="alert">{tryError}</p>}
                  {view.speak_size > 0 && <p className="small muted">{t("The first time, the voice engine and its model download ({size}).", { size: bytes(view.speak_size) })}</p>}
                </div>
              ) : (
                <p className="small muted">{t("Read three lines above, then hear a sentence in your voice here.")}</p>
              )}
              <p className="small muted">
                {t("Use it in Narration and Podcast (pick it as the voice) and in Music (Sung in). Singing downloads its model the first time ({size}).", { size: bytes(view.sing_size) })}
              </p>
            </>
          )}
        </>
      )}
    </div>
  );
}
