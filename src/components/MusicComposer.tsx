// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's song form: describing a song or working on one, its sound,
// words, structure and voice, and the songs it starts from.
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, type MediaItem, type Composition, type MediaModelCard, type MediaRequest, type MusicSpec, type Persona, type Taste, type VocalEffect } from "../api";
import { t } from "../i18n";
import { useMediaUrl, waitLabel } from "../media";
import { TIMES } from "./ComposeEditor";
import {
  BEATS, Chips, EMOTIONS, LANGUAGES, ENDINGS, ENERGIES, ERAS, GENRES, HARMONY_INSTRUMENTS, HARMONY_STYLES, HARMONY_VOICES, HARMONY_WHERE, INSTRUMENTS, LEAD_INSTRUMENTS, MOODS, NOTES,
  PACES, PARTS, PickMany, PickOne, PRODUCTIONS, RHYMES, SECS_PER_LINE, SOFT_VOCALS, SongSections, structureSeconds, TASKS, VOCAL_STYLES, VocalEffects, fitFor, mmss, sungLines,
  joined, listed, type SongTask,
} from "./MusicFields";
import type { PushToast } from "./Toasts";
import { useOwnVoices } from "./OwnVoice";
import { CustomModelField } from "./CustomModels";
import { Fold, InfoTip } from "./InfoTip";
import { ModelPick, useModelPick, DefaultLink } from "./StudioBits";

/** The song being worked on: picked from a file or the gallery, and heard. */
export function SongSource({ song, gallery, onPick, toast }: { song: MediaItem | null; gallery: MediaItem[]; onPick: (i: MediaItem | null) => void; toast: PushToast }) {
  const src = useMediaUrl(song);
  const [picking, setPicking] = useState(false);
  const fromFile = async () => {
    const picked = await openFile({ multiple: false, filters: [{ name: t("Songs and sounds"), extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "wma"] }] });
    if (typeof picked !== "string") return;
    try {
      onPick(await api.mediaImport(picked));
      setPicking(false);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const songs = gallery.filter((i) => i.kind === "audio");
  return (
    <div className="music-field">
      <span className="music-label">{t("The song")}</span>
      {song ? (
        <div className="row wrap">
          <strong className="small">{song.prompt || t("A song")}</strong>
          {song.seconds > 0 && <span className="small muted">{mmss(song.seconds)}</span>}
          <button className="link small" onClick={() => onPick(null)}>{t("Change")}</button>
          {src && <audio controls controlsList="nodownload" src={src} className="music-player" />}
        </div>
      ) : (
        <div className="row wrap">
          <button className="btn small" onClick={fromFile}>{t("From a file…")}</button>
          <button className="btn small" onClick={() => setPicking((p) => !p)} disabled={!songs.length}>{t("From the gallery")}</button>
          {!songs.length && <span className="small muted">{t("No songs in the gallery yet.")}</span>}
        </div>
      )}
      {picking && !song && (
        <div className="music-song-list">
          {songs.map((s) => (
            <button key={s.id} className="btn ghost small" onClick={() => { onPick(s); setPicking(false); }}>
              {s.title || s.prompt || t("A song")} {s.seconds > 0 ? `· ${mmss(s.seconds)}` : ""}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export function MusicComposer({
  models,
  defaultId,
  onSetDefault,
  onModels,
  onStart,
  gallery,
  toast,
  seed,
  onCompose,
  onYourVoice,
  canArt,
  reuse,
  inspire,
}: {
  models: MediaModelCard[];
  defaultId: string | null;
  onSetDefault: (id: string) => void;
  onModels: () => void;
  onStart: (r: MediaRequest) => Promise<boolean>;
  gallery: MediaItem[];
  toast: PushToast;
  /** A song picked in the viewer to work on. */
  seed?: { item: MediaItem; task: SongTask; reference?: MediaItem } | null;
  /** Written as notes first: opens in Compose's editor. */
  onCompose: (c: Composition) => void;
  /** Opens Your voice, to record one. */
  onYourVoice: () => void;
  /** A picture model is here to make cover art. */
  canArt: boolean;
  /** A song whose settings fill the form again. */
  reuse?: MediaItem | null;
  /** Songs whose shared sound fills the form (a playlist's). */
  inspire?: string[] | null;
}) {
  const [mode, setMode] = useState<"song" | "edit" | "sound">("song");
  // Sung in a recorded voice (the song as made is under "← The original").
  const ownVoices = (useOwnVoices()?.voices ?? []).filter((v) => v.can_sing);
  const [ownVoice, setOwnVoice] = useState("");
  const [prompt, setPrompt] = useState("");
  // Several styles, moods and feelings can be picked together.
  const [genre, setGenre] = useState<string[]>([]);
  const [mood, setMood] = useState<string[]>([]);
  const [instruments, setInstruments] = useState<string[]>([]);
  const [vocals, setVocals] = useState<"write" | "none" | "mine">("write");
  const [voice, setVoice] = useState("");
  const [lyrics, setLyrics] = useState("");
  const [writing, setWriting] = useState(false);
  const [language, setLanguage] = useState("auto");
  const [bpm, setBpm] = useState<number | null>(null);
  const [beat, setBeat] = useState("");
  const [meter, setMeter] = useState("");
  const [keyNote, setKeyNote] = useState("");
  const [keyMode, setKeyMode] = useState<"major" | "minor">("major");
  const [vocalStyle, setVocalStyle] = useState("");
  const [harmonyVoice, setHarmonyVoice] = useState<MusicSpec["harmony_voice"] | "">("");
  const [harmonyStyle, setHarmonyStyle] = useState<NonNullable<MusicSpec["harmony_style"]>>("thirds");
  const [harmonyWhere, setHarmonyWhere] = useState<NonNullable<MusicSpec["harmony_where"]>>("chorus");
  const [harmonyInstrument, setHarmonyInstrument] = useState("");
  const [harmonyLayer, setHarmonyLayer] = useState(false);
  const [leadInstrument, setLeadInstrument] = useState("");
  const [sections, setSections] = useState<string[]>([]);
  const [rhyme, setRhyme] = useState<NonNullable<MusicSpec["rhyme"]> | "">("");
  const [energy, setEnergy] = useState<NonNullable<MusicSpec["energy"]> | "">("");
  const [ending, setEnding] = useState<NonNullable<MusicSpec["ending"]> | "">("");
  const [production, setProduction] = useState("");
  const [era, setEra] = useState("");
  const [emotion, setEmotion] = useState<string[]>([]);
  const [vocalEffects, setVocalEffects] = useState<VocalEffect[]>([]);
  // An artist's style: the name typed, and what the chat model said of them.
  const [soundLike, setSoundLike] = useState("");
  const [artist, setArtist] = useState<{ name: string; sound: string; writing: string } | null>(null);
  const [lookingUp, setLookingUp] = useState(false);
  const key = keyNote ? `${keyNote} ${keyMode}` : "";
  const [seconds, setSeconds] = useState(60);
  // Auto: the song is as long as it needs (a jingle short, a full song about three minutes).
  const [autoLength, setAutoLength] = useState(true);
  // Vocals come out clearly about three times in four: two versions on the
  // graphics card (seconds each) make one clear one likely. Changed by hand, it stays.
  const [versionsSet, setVersions] = useState<number | null>(null);
  // Best of 3: three made, each listened to, the one whose words come through clearest kept.
  const [bestOf, setBestOf] = useState(false);
  // Suno's advanced options: a title, Weirdness and Style influence (null: the model's own), and styles left out.
  const [title, setTitle] = useState("");
  const [weird, setWeird] = useState<number | null>(null);
  const [influence, setInfluence] = useState<number | null>(null);
  const [exclude, setExclude] = useState("");
  // A custom model trained on the person's songs, and how much it's followed.
  const [customId, setCustomId] = useState("");
  const [customScale, setCustomScale] = useState(1);
  // Cover art for each song, made by the picture model after it (kept on for next time).
  const [coverArt, setCoverArtState] = useState(() => {
    try {
      return localStorage.getItem("sulcusai.songs.coverArt") === "1";
    } catch {
      return false;
    }
  });
  const setCoverArt = (on: boolean) => {
    setCoverArtState(on);
    try {
      localStorage.setItem("sulcusai.songs.coverArt", on ? "1" : "0");
    } catch {
      /* not kept */
    }
  };
  const [more, setMore] = useState(false);
  const [reference, setReference] = useState<MediaItem | null>(null);
  // Saved artists: one picked fills in their sound and voice; settings can be saved as one.
  const [personas, setPersonas] = useState<Persona[]>([]);
  const [personaId, setPersonaId] = useState("");
  const [naming, setNaming] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [voiceWords, setVoiceWords] = useState("");
  const [followVoice, setFollowVoice] = useState(false);
  const loadPersonas = () => api.musicPersonas().then(setPersonas).catch(() => {});
  useEffect(() => {
    loadPersonas();
  }, []);
  const persona = personas.find((p) => p.id === personaId);
  const [song, setSong] = useState<MediaItem | null>(null);
  const [task, setTask] = useState<SongTask>("cover");
  const [part, setPart] = useState("vocals");
  const [from, setFrom] = useState(0);
  const [to, setTo] = useState(30);
  const [extendBy, setExtendBy] = useState(30);
  const [strength, setStrength] = useState(0.5);
  useEffect(() => {
    if (!seed) return;
    setMode("edit");
    setSong(seed.item);
    setTask(seed.task);
    // A mashup: this song's tune and words in the other's sound.
    if (seed.reference) {
      setReference(seed.reference);
      setMore(true);
      setStrength(0.7);
    }
    if (seed.item.seconds) {
      setFrom(0);
      setTo(Math.min(30, Math.round(seed.item.seconds)));
    }
  }, [seed]);
  const parts = task === "add" || task === "extract" || task === "complete";
  const needOp = mode === "sound" ? "sound" : mode === "edit" && parts ? "parts" : "music";
  const fit = models.filter((m) => (m.can ?? ["music", "sound"]).includes(needOp));
  const [model, setModel] = useModelPick("music", fit.length ? fit : models, defaultId);
  const m = fit.find((x) => x.id === model) ?? fit[0];
  const versions = versionsSet ?? (mode === "song" && vocals !== "none" && m?.fit.on_gpu ? 2 : 1);
  const secs = mode === "sound" ? Math.min(Math.max(seconds, 10), 30) : seconds;
  const auto = mode === "song" && autoLength;
  // What Auto will likely come to, when it can be told before the song is made.
  const autoGuess = vocals === "mine" && sungLines(lyrics) > 0 ? fitFor(lyrics) + 5 : sections.length ? structureSeconds(sections) : null;
  const sings = mode === "song" ? vocals !== "none" : mode === "edit" && ((task === "add" && part.includes("vocals")) || task === "cover" || task === "repaint");
  const styled = mode === "song" || (mode === "edit" && (task === "cover" || task === "complete"));
  const singing = mode === "song" ? vocals !== "none" : sings;
  const harmonized = (singing && !!harmonyVoice) || !!harmonyInstrument;
  // Harmonies layered on afterwards need the model that adds parts.
  const canLayer = models.some((x) => (x.can ?? []).includes("parts"));
  const spec = (): MusicSpec => ({
    genre: joined(genre),
    mood: joined(mood),
    instruments,
    voice: voice || null,
    beat: beat && beat !== "Steady" ? beat : null,
    bpm,
    key: key || null,
    time_signature: meter || null,
    reference: reference?.id ?? null,
    vocal_style: singing ? vocalStyle || null : null,
    harmony_voice: singing && harmonyVoice ? harmonyVoice : null,
    harmony_instrument: harmonyInstrument || null,
    harmony_where: harmonized ? harmonyWhere : null,
    harmony_style: singing && harmonyVoice ? harmonyStyle : null,
    harmony_layer: harmonized && harmonyLayer && canLayer,
    lead_instrument: leadInstrument || null,
    sections,
    rhyme: singing && rhyme ? rhyme : null,
    energy: energy || null,
    ending: ending || null,
    production: production || null,
    era: era || null,
    auto_length: auto,
    own_voice: mode === "song" && singing && ownVoices.some((v) => v.id === ownVoice) ? ownVoice : null,
    best_of: mode === "song" && singing && bestOf,
    custom: mode === "song" && customId ? customId : null,
    custom_scale: mode === "song" && customId && customScale !== 1 ? customScale : null,
    cover_art: mode === "song" && canArt && coverArt,
    title: mode === "song" && title.trim() ? title.trim() : null,
    weirdness: mode !== "edit" && weird != null ? weird / 100 : null,
    influence: influence != null ? influence / 100 : null,
    exclude: exclude.trim() || null,
    persona: mode === "song" && personaId ? personaId : null,
    voice_detail: mode === "song" && personaId && voiceWords.trim() ? voiceWords.trim() : null,
    emotion: joined(emotion),
    vocal_effects: singing ? vocalEffects : [],
    sound_like: soundLike.trim() || null,
    // Looked up for this name (else it's looked up when the song is made).
    artist_sound: artist && artist.name === soundLike.trim() ? artist.sound : null,
    artist_writing: artist && artist.name === soundLike.trim() ? artist.writing : null,
    ...(mode === "edit" && song
      ? {
          task,
          audio: song.id,
          track: parts ? (task === "complete" ? null : part) : null,
          region: task === "repaint" ? [from, Math.max(to, from + 1)] : null,
          extend_by: task === "extend" ? extendBy : null,
          strength: task === "cover" ? strength : null,
        }
      : {}),
  });
  // Written as notes first (Compose): the plan follows every option picked
  // here, and Make it real starts from them.
  const [arranging, setArranging] = useState(false);
  const writeAsNotes = async () => {
    setArranging(true);
    try {
      const [beats, unit] = (meter || "4/4").split("/").map(Number);
      const s = spec();
      onCompose(
        await api.composeWrite({
          description: prompt.trim() || [joined(genre), joined(mood)].filter(Boolean).join(", ") || "a song",
          beats,
          unit,
          bpm,
          key: key || null,
          style: joined(genre),
          sections: s.sections?.length || (auto ? 6 : Math.max(2, Math.min(12, Math.round(secs / 25)))),
          vocals: vocals !== "none",
          voice,
          era,
          song: s,
          lyrics: vocals === "mine" && lyrics.trim() ? lyrics : null,
          language: language === "auto" ? null : language,
        }),
      );
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setArranging(false);
    }
  };
  // My taste: what the liked and made songs share, filled in on request.
  const [taste, setTaste] = useState<Taste | null>(null);
  const fillMyTaste = async (ids?: string[]) => {
    try {
      const s = await api.musicTaste(ids);
      if (!s.songs) return toast(t("Make or like a few songs first: My taste comes from them."), "info");
      setTaste(s);
      const known = (list: string[], v: string) => list.find((o) => o.toLowerCase() === v.toLowerCase()) ?? v;
      if (s.genres.length) setGenre(s.genres.map((g) => known(GENRES, g)));
      if (s.moods.length) setMood(s.moods.map((x) => known(MOODS, x)));
      if (s.instruments.length) setInstruments(s.instruments.map((i) => known(INSTRUMENTS, i)));
      if (s.voice) setVoice(s.voice);
      if (s.era) setEra(s.era);
      if (s.production) setProduction(s.production);
      if (s.avoid.length) setExclude(s.avoid.join(", "));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  // Lines selected in the lyrics, to be written again (whole lines: the selection widened to them).
  const [picked, setPicked] = useState<[number, number] | null>(null);
  const [how, setHow] = useState("");
  const [rewriting, setRewriting] = useState(false);
  const pickLines = (el: HTMLTextAreaElement) => {
    const { selectionStart: a, selectionEnd: b, value } = el;
    if (b - a < 2) return setPicked(null);
    const from = value.lastIndexOf("\n", a - 1) + 1;
    const end = value.indexOf("\n", b - (value[b - 1] === "\n" ? 1 : 0));
    setPicked([from, end < 0 ? value.length : end]);
  };
  const pickedLines = picked ? lyrics.slice(picked[0], picked[1]) : "";
  const rewriteLines = async () => {
    if (!picked) return;
    setRewriting(true);
    try {
      const fresh = await api.musicRewriteLines(lyrics, pickedLines, how, language === "auto" ? null : language);
      setLyrics(lyrics.slice(0, picked[0]) + fresh + lyrics.slice(picked[1]));
      setPicked([picked[0], picked[0] + fresh.length]);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setRewriting(false);
    }
  };
  const writeLyrics = async () => {
    setWriting(true);
    try {
      setLyrics(await api.musicWriteLyrics(prompt, spec(), auto ? null : secs, language === "auto" ? null : language));
      setVocals("mine");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWriting(false);
    }
  };
  // Their genre, voice, instruments and tempo filled in to see and change;
  // their sound and way of writing go with the song.
  const lookUpArtist = async () => {
    const name = soundLike.trim();
    if (!name) return;
    setLookingUp(true);
    try {
      const s = await api.musicArtistStyle(name);
      // "piano" lights the Piano chip rather than adding another.
      const known = (list: string[], v: string) => list.find((o) => o.toLowerCase() === v.toLowerCase()) ?? v;
      if (s.genre) setGenre(listed(s.genre).map((g) => known(GENRES, g)));
      if (s.voice) setVoice(s.voice);
      if (s.instruments.length) setInstruments(s.instruments.map((i) => known(INSTRUMENTS, i)));
      if (s.mood) setMood(listed(s.mood).map((x) => known(MOODS, x)));
      if (s.bpm) setBpm(s.bpm);
      setArtist({ name, sound: [s.vocal, s.production].filter(Boolean).join(", "), writing: s.writing });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setLookingUp(false);
    }
  };
  // An artist picked: their whole sound filled in, to see and change, and their voice song to follow.
  const pickArtist = (id: string) => {
    setPersonaId(id);
    setRemoving(false);
    const p = personas.find((x) => x.id === id);
    setVoiceWords(p?.voice_words ?? "");
    setFollowVoice(!!p?.follow_voice_song);
    if (!p) return;
    applySound(p.sound);
    if (p.language) setLanguage(p.language);
    setReference(p.follow_voice_song ? gallery.find((g) => g.id === p.voice_song) ?? null : null);
    if (vocals === "none") setVocals("write");
  };
  // A sound (an artist's, or a song's own settings) filled in, to see and change.
  const applySound = (s: MusicSpec) => {
    const [note, scale] = (s.key ?? "").split(" ");
    setGenre(listed(s.genre));
    setMood(listed(s.mood));
    setInstruments(s.instruments ?? []);
    setVoice(s.voice ?? "");
    setBeat(s.beat ?? "");
    setBpm(s.bpm ?? null);
    setKeyNote(note ?? "");
    setKeyMode(scale === "minor" ? "minor" : "major");
    setMeter(s.time_signature ?? "");
    setVocalStyle(s.vocal_style ?? "");
    setHarmonyVoice(s.harmony_voice ?? "");
    setHarmonyInstrument(s.harmony_instrument ?? "");
    if (s.harmony_style) setHarmonyStyle(s.harmony_style);
    if (s.harmony_where) setHarmonyWhere(s.harmony_where);
    setHarmonyLayer(!!s.harmony_layer);
    setLeadInstrument(s.lead_instrument ?? "");
    setRhyme(s.rhyme ?? "");
    setEnergy(s.energy ?? "");
    setEnding(s.ending ?? "");
    // A decade saved as the production (before the era had its own choice) is the era.
    const decade = /^\d{4}s$/.test(s.production ?? "");
    setProduction(decade ? "" : s.production ?? "");
    setEra(s.era ?? (decade ? s.production! : ""));
    setEmotion(listed(s.emotion));
    setVocalEffects(s.vocal_effects ?? []);
    setSoundLike(s.sound_like ?? "");
    setArtist(s.sound_like && s.artist_sound ? { name: s.sound_like, sound: s.artist_sound, writing: s.artist_writing ?? "" } : null);
  };
  // A playlist's sound (what its songs share), filled in for a new song.
  useEffect(() => {
    if (!inspire?.length) return;
    setMode("song");
    fillMyTaste(inspire);
  }, [inspire]); // eslint-disable-line react-hooks/exhaustive-deps
  // A song's own settings again (Suno's Reuse prompt): its description,
  // sound, words and choices, for another take or a change.
  useEffect(() => {
    if (!reuse) return;
    const s = reuse.music ?? {};
    setMode("song");
    setPrompt(reuse.prompt);
    applySound(s);
    setSections(s.sections ?? []);
    setAutoLength(!!s.auto_length);
    if (!s.auto_length && reuse.seconds) setSeconds(Math.round(reuse.seconds));
    setOwnVoice(s.own_voice ?? "");
    setPersonaId(s.persona ?? "");
    setWeird(s.weirdness != null ? Math.round(s.weirdness * 100) : null);
    setInfluence(s.influence != null ? Math.round(s.influence * 100) : null);
    setExclude(s.exclude ?? "");
    setCustomId(s.custom ?? "");
    setCustomScale(s.custom_scale ?? 1);
    setReference(s.reference ? gallery.find((g) => g.id === s.reference) ?? null : null);
    if (reuse.lyrics) {
      setLyrics(reuse.lyrics);
      setVocals("mine");
    } else setVocals("none");
  }, [reuse]); // eslint-disable-line react-hooks/exhaustive-deps
  // These settings as a new artist, or as the picked one's.
  const saveArtist = async (asNew: boolean) => {
    const name = asNew ? (naming ?? "").trim() : persona?.name ?? "";
    if (!name) return;
    try {
      const saved = await api.musicPersonaSave({
        id: asNew ? "" : persona?.id ?? "",
        name,
        voice_words: asNew ? "" : voiceWords,
        voice_song: followVoice && reference ? reference.id : asNew ? null : persona?.voice_song ?? null,
        seed: asNew ? null : persona?.seed ?? null,
        follow_voice_song: !asNew && followVoice,
        sound: spec(),
        language: language === "auto" ? null : language,
        created_at: 0,
        updated_at: 0,
      });
      await loadPersonas();
      setPersonaId(saved.id);
      setVoiceWords(saved.voice_words);
      setFollowVoice(saved.follow_voice_song);
      setNaming(null);
      toast(
        asNew ? t("Saved {name}, with a voice of their own: {voice}.", { name, voice: saved.voice_words }) : t("Saved the changes to {name}.", { name }),
        "success",
      );
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const removeArtist = async () => {
    if (!persona) return;
    if (!removing) return setRemoving(true);
    await api.musicPersonaDelete(persona.id).catch((e) => toast(errorText(e), "error"));
    setRemoving(false);
    setPersonaId("");
    await loadPersonas();
  };
  const voiceSong = persona?.voice_song ? gallery.find((g) => g.id === persona.voice_song) : undefined;
  const tag = (s: string) => setLyrics((l) => `${l}${l && !l.endsWith("\n") ? "\n\n" : ""}[${s}]\n`);
  const ready =
    mode === "sound"
      ? !!prompt.trim()
      : mode === "song"
        ? (!!prompt.trim() || genre.length > 0 || instruments.length > 0) && !(vocals === "mine" && !lyrics.trim())
        : !!song && (task !== "cover" || !!prompt.trim() || genre.length > 0 || instruments.length > 0);
  const go = async () => {
    const ok = await onStart({
      op: mode === "sound" ? "sound" : "music",
      prompt,
      seconds: mode === "edit" ? (task === "extend" ? extendBy : undefined) : auto ? undefined : secs,
      instrumental: mode === "song" ? vocals === "none" : undefined,
      lyrics: sings && (mode === "edit" || vocals === "mine") && lyrics.trim() ? lyrics : null,
      language: sings && language !== "auto" ? language : null,
      model_id: m?.id,
      count: mode === "song" ? versions : undefined,
      music: mode === "sound" ? null : spec(),
    });
    if (ok && mode !== "edit") {
      setPrompt("");
      setTitle("");
    }
  };
  // Each layered harmony is another pass, by a slower model (about twice the song's time).
  const layers = mode === "song" && harmonyLayer && canLayer ? (singing && harmonyVoice ? 1 : 0) + (harmonyInstrument ? 1 : 0) : 0;
  const songSecs = mode === "edit" ? Math.max(song?.seconds ?? 60, 30) : auto ? autoGuess ?? 150 : secs;
  // Singing the first version again in a recorded voice: about twice the song's length.
  const revoicing = mode === "song" && !!spec().own_voice ? 2 * songSecs : 0;
  // Best of 3: each version listened to (about a quarter of its length).
  const listening = mode === "song" && bestOf && singing ? versions * songSecs * 0.25 : 0;
  const est = m ? (m.fit.est_secs * songSecs * (mode === "song" ? versions : 1) * (1 + 2 * layers)) / 30 + revoicing + listening : 0;
  const keyTime = (
    <div className="music-field">
      <span className="music-label">{t("Key and time")}</span>
      <div className="row wrap">
        <select value={keyNote} onChange={(e) => setKeyNote(e.target.value)} aria-label={t("Key")}>
          <option value="">{t("Any key")}</option>
          {NOTES.map((n) => (
            <option key={n} value={n}>{n}</option>
          ))}
        </select>
        {keyNote && (
          <div className="filters tight" role="group" aria-label={t("Major or minor")}>
            <button className={`chip ${keyMode === "major" ? "active" : ""}`} onClick={() => setKeyMode("major")}>{t("Major")}</button>
            <button className={`chip ${keyMode === "minor" ? "active" : ""}`} onClick={() => setKeyMode("minor")}>{t("Minor")}</button>
          </div>
        )}
        <select value={meter} onChange={(e) => setMeter(e.target.value)} aria-label={t("Time signature")}>
          <option value="">{t("Any time signature")}</option>
          {TIMES.map(([b, u]) => `${b}/${u}`).map((x) => (
            <option key={x} value={x}>{x}</option>
          ))}
        </select>
      </div>
      {keyNote && <p className="small muted">{keyMode === "major" ? t("Major keys sound bright and settled.") : t("Minor keys sound darker or more emotional.")}</p>}
    </div>
  );
  return (
    <div className="form music-composer">
      <div className="mode-switch">
        <button className={`mode ${mode === "song" ? "active" : ""}`} onClick={() => setMode("song")}>{t("New song")}</button>
        <button className={`mode ${mode === "edit" ? "active" : ""}`} onClick={() => setMode("edit")}>{t("From a song you have")}</button>
        <button className={`mode ${mode === "sound" ? "active" : ""}`} onClick={() => setMode("sound")}>{t("Sound effect")}</button>
      </div>

      {mode === "song" && (
        <CustomModelField value={customId} scale={customScale} onChange={setCustomId} onScale={setCustomScale} gallery={gallery} onStart={onStart} onModels={onModels} toast={toast} />
      )}

      {mode === "song" && (
        <div className="music-field">
          <span className="music-label">{t("Artist")}<InfoTip label={t("Artist")} text={t("Your saved artists: the same voice, seed and sound go into each of their songs, so new songs sound like the same singer. Save one from settings you like.")} /></span>
          <div className="row wrap">
            <select value={personaId} onFocus={loadPersonas} onChange={(e) => pickArtist(e.target.value)} aria-label={t("Artist")}>
              <option value="">{t("No saved artist")}</option>
              {personas.map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
            {persona && naming === null && (
              <>
                <button className="btn ghost small" onClick={() => saveArtist(false)}>{t("Save changes to {name}", { name: persona.name })}</button>
                <button className="btn ghost small danger" onClick={removeArtist}>{removing ? t("Remove {name}?", { name: persona.name }) : t("Remove")}</button>
              </>
            )}
            {naming === null ? (
              <button className="btn ghost small" onClick={() => setNaming("")}>{t("Save these settings as an artist")}</button>
            ) : (
              <>
                <input className="input" value={naming} autoFocus placeholder={t("Artist name")} aria-label={t("Artist name")} onChange={(e) => setNaming(e.target.value)} onKeyDown={(e) => e.key === "Enter" && void saveArtist(true)} />
                <button className="btn small" disabled={!naming.trim()} onClick={() => saveArtist(true)}>{t("Save")}</button>
                <button className="btn ghost small" onClick={() => setNaming(null)}>{t("Cancel")}</button>
              </>
            )}
          </div>
          {persona ? (
            <>
              <label className="small row wrap">
                {t("Their voice:")}
                <input className="input grow" value={voiceWords} aria-label={t("Their voice")} onChange={(e) => setVoiceWords(e.target.value)} />
              </label>
              <label className="small row" title={t("Their songs follow the voice of their first song (or the one picked under More settings). Closer voice, but the words can come out less clear.")}>
                <input type="checkbox" checked={followVoice} onChange={(e) => setFollowVoice(e.target.checked)} disabled={!voiceSong} />
                {voiceSong ? t("Also follow the voice of “{song}”", { song: voiceSong.prompt || t("A song") }) : t("Also follow the voice of their first song, once it's made")}
              </label>
              <p className="small muted">{t("The same voice, seed and sound go into each of their songs, so they sound like one artist. Save the changes to keep edits.")}</p>
            </>
          ) : (
            <p className="small muted">{t("Save an artist to make more songs that sound like the same singer: their voice, style, instruments and way of writing are kept.")}</p>
          )}
        </div>
      )}

      {mode === "edit" && (
        <>
          <SongSource song={song} gallery={gallery} onPick={(s) => { setSong(s); if (s?.seconds) { setFrom(0); setTo(Math.min(30, Math.round(s.seconds))); } }} toast={toast} />
          <div className="music-field">
            <span className="music-label">{t("What to do with it")}</span>
            <div className="filters tight wrap">
              {TASKS.map(([id, label]) => (
                <button key={id} className={`chip ${task === id ? "active" : ""}`} onClick={() => setTask(id)}>{t(label)}</button>
              ))}
            </div>
            <p className="small muted">{t(TASKS.find(([id]) => id === task)![2])}</p>
          </div>
          {task === "repaint" && (
            <div className="row wrap">
              <label className="small">
                {t("From")} <input type="number" className="input num" min={0} max={song?.seconds || 600} value={from} onChange={(e) => setFrom(Math.max(0, +e.target.value))} /> s
              </label>
              <label className="small">
                {t("to")} <input type="number" className="input num" min={from + 1} max={song?.seconds || 600} value={to} onChange={(e) => setTo(+e.target.value)} /> s
              </label>
              {song && song.seconds > 0 && <span className="small muted">{t("of {length}", { length: mmss(song.seconds) })}</span>}
            </div>
          )}
          {task === "extend" && (
            <label className="small music-length">
              {t("Add")}
              <input type="range" min={5} max={300} step={5} value={extendBy} onChange={(e) => setExtendBy(+e.target.value)} />
              {mmss(extendBy)}
            </label>
          )}
          {(task === "add" || task === "extract") && (
            <PickOne label={task === "add" ? t("The part to add") : t("The part to keep")} options={PARTS.map(([, n]) => n)} value={PARTS.find(([id]) => id === part)?.[1] ?? ""} onChange={(v) => setPart(PARTS.find(([, n]) => n === v || t(n) === v)?.[0] ?? "vocals")} />
          )}
          {task === "cover" && (
            <label className="small music-length">
              {t("How close to the original")}
              <input type="range" min={0} max={1} step={0.05} value={strength} onChange={(e) => setStrength(+e.target.value)} />
              {strength < 0.35 ? t("loosely") : strength < 0.7 ? t("in between") : t("very close")}
            </label>
          )}
          {parts && !m && (
            <p className="callout small">
              {t("Adding, keeping or building around a part needs ACE-Step 1.5 Base (slower, and a 2.5 GB download beside ACE-Step 1.5).")}{" "}
              <button className="link" onClick={onModels}>{t("Get it")}</button>
            </p>
          )}
        </>
      )}

      {mode === "song" && (
        <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("Title (optional): else it's named by its hook")} aria-label={t("Title")} />
      )}
      <textarea
        className="input"
        rows={mode === "edit" ? 2 : 3}
        value={prompt}
        aria-label={t("Description")}
        placeholder={
          mode === "sound"
            ? t("Describe the sound, e.g. \"rain on a tin roof with distant thunder\"")
            : mode === "edit"
              ? t("Describe the result (optional for some changes), e.g. \"as a slow piano ballad\"")
              : t("What the song is about and how it sounds, e.g. \"a road trip through the mountains with old friends\"")
        }
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />

      {styled && (
        <div className="music-field">
          <span className="music-label">{t("Sound like an artist")}<InfoTip label={t("Sound like an artist")} text={t("Type a singer or band you like. Their style is read, and the song is written and arranged their way, sung by a voice of the same kind (not their real voice).")} /></span>
          <div className="row wrap">
            <input
              className="input grow"
              value={soundLike}
              placeholder={t("A singer or band, e.g. Lewis Capaldi")}
              aria-label={t("Sound like an artist")}
              onChange={(e) => setSoundLike(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void lookUpArtist()}
            />
            <button className="btn small" onClick={lookUpArtist} disabled={!soundLike.trim() || lookingUp}>{lookingUp ? t("Reading their style…") : t("Use their style")}</button>
          </div>
          {artist && artist.name === soundLike.trim() && artist.sound && (
            <p className="small muted">{t("Their sound, as the music model hears it: {sound}", { sound: artist.sound })}</p>
          )}
          <p className="small muted">
            {t("Written and arranged their way and sung by a voice of the same kind, not their own voice. You can also write it in the description: \"Sing as Taylor Swift, a song about bears\".")}
          </p>
        </div>
      )}

      {mode === "song" && (
        <div className="row wrap small">
          <button className="btn small" onClick={() => fillMyTaste()} title={t("Fill in the style, mood, instruments and voice of the songs you liked and made (and leave out what's in the ones marked Not for me)")}>
            {t("Use my taste")}
          </button>
          {taste && <span className="muted ellipsis">{tasteLine(taste)}</span>}
        </div>
      )}
      {styled && (
        <>
          <PickMany label={t("Style")} options={GENRES} value={genre} onChange={setGenre} other={t("Add another style (Enter)")} />
          <Fold title={t("Mood, feeling and instruments")} summary={[...mood, ...emotion, ...instruments].map((x) => t(x)).join(", ") || t("As the song suits")}>
            <PickMany label={t("Mood of the music")} options={MOODS} value={mood} onChange={setMood} other={t("Add another mood (Enter)")} />
            {(mode !== "song" || vocals !== "none") && <PickMany label={t("Emotion in the words and voice")} options={EMOTIONS} value={emotion} onChange={setEmotion} other={t("Add another feeling (Enter)")} />}
            <PickMany label={t("Instruments")} options={INSTRUMENTS} value={instruments} onChange={setInstruments} other={t("Add another (Enter)")} />
          </Fold>
        </>
      )}

      {mode === "song" && vocals !== "none" && (m?.id === "ace-step-1.5" || m?.id === "ace-step-1.5-small") && (
        <p className="callout small">
          {t("Voices from this model can sound synthetic and get buried under the music. For clear, natural vocals, use ACE-Step 1.5 XL (every word heard in tests) or ACE-Step 1.5 Clear voice (just as quick).")}{" "}
          <button className="link" onClick={onModels}>{t("Get them")}</button>
        </p>
      )}
      {mode === "song" && vocals !== "none" && genre.some((g) => SOFT_VOCALS.includes(g)) && (
        <p className="small muted">{t("Vocals in this style usually come out soft or blurred. For words you can make out, Pop, Rock, Folk or Country work best.")}</p>
      )}
      {mode === "song" && (
        <div className="music-field">
          <span className="music-label">{t("Vocals")}<InfoTip label={t("Vocals")} text={t("Lyrics written for me: words written from your description. My own lyrics: type or paste your words. Instrumental: no singing.")} /></span>
          <div className="row wrap">
            <div className="filters tight">
              {(["write", "mine", "none"] as const).map((v) => (
                <button key={v} className={`chip ${vocals === v ? "active" : ""}`} onClick={() => setVocals(v)}>
                  {{ write: t("Lyrics written for me"), mine: t("My own lyrics"), none: t("Instrumental") }[v]}
                </button>
              ))}
            </div>
            {vocals !== "none" && (
              <>
                <select value={voice} onChange={(e) => setVoice(e.target.value)} aria-label={t("Who sings")}>
                  <option value="">{t("Any voice")}</option>
                  <option value="female">{t("Female voice")}</option>
                  <option value="male">{t("Male voice")}</option>
                  <option value="duet">{t("Duet")}</option>
                  <option value="choir">{t("Choir")}</option>
                  <option value="rap">{t("Rap")}</option>
                </select>
                <select value={vocalStyle} onChange={(e) => setVocalStyle(e.target.value)} aria-label={t("How they sing")}>
                  <option value="">{t("Any singing style")}</option>
                  {VOCAL_STYLES.map(([v, n]) => (
                    <option key={v} value={v}>{t(n)}</option>
                  ))}
                </select>
                <select value={language} onChange={(e) => setLanguage(e.target.value)} aria-label={t("Language of the vocals")}>
                  {LANGUAGES.map(([c, n]) => (
                    <option key={c} value={c}>{t(n)}</option>
                  ))}
                </select>
              </>
            )}
          </div>
          {vocals !== "none" &&
            (ownVoices.length > 0 ? (
              <label className="small row wrap">
                {t("Sung in")}
                <InfoTip label={t("Sung in")} text={t("Pick one of your voices from Your voice. The song is made first, then sung again in that voice, in a key that fits its range, so it sounds like you.")} />
                <select value={ownVoice} onChange={(e) => setOwnVoice(e.target.value)} aria-label={t("Sung in")}>
                  <option value="">{t("The music model's singer")}</option>
                  {ownVoices.map((v) => (
                    <option key={v.id} value={v.id}>{v.designed ? t("{name} (made voice)", { name: v.name }) : t("{name} (recorded voice)", { name: v.name })}</option>
                  ))}
                </select>
                {ownVoice && <span className="muted">{t("The song is made first, then sung again in this voice, in a key that fits its range. The version before is under ← The original. It takes a few minutes more.")}</span>}
              </label>
            ) : (
              <button className="link small" onClick={onYourVoice}>{t("Sing it in your own voice…")}</button>
            ))}
        </div>
      )}

      {sings && (mode === "edit" || vocals !== "none") && (
        <div className="music-field">
          <div className="row wrap">
            <span className="music-label">{mode === "edit" ? t("Lyrics (optional)") : t("Lyrics")}</span>
            <span className="spacer" />
            {["Intro", "Verse", "Pre-Chorus", "Chorus", "Bridge", "Outro"].map((s) => (
              <button key={s} className="btn ghost small" onClick={() => { tag(s); if (mode === "song") setVocals("mine"); }}>+ {t(s)}</button>
            ))}
            <button className="btn small" onClick={writeLyrics} disabled={writing}>{writing ? t("Writing…") : t("Write lyrics for me")}</button>
          </div>
          {(vocals === "mine" || mode === "edit") && (
            <textarea
              className="input lyrics-box"
              rows={8}
              value={lyrics}
              aria-label={t("Lyrics")}
              placeholder={"[Verse]\n…\n\n[Chorus]\n…"}
              onChange={(e) => {
                setLyrics(e.target.value);
                setPicked(null);
              }}
              onSelect={(e) => pickLines(e.currentTarget)}
            />
          )}
          {picked && pickedLines.trim() && (vocals === "mine" || mode === "edit") && (
            <div className="row wrap small">
              <span className="muted">{t("{n} line(s) selected", { n: pickedLines.split("\n").filter((l) => l.trim() && !/^\[.*\]$/.test(l.trim())).length })}</span>
              <input className="input" value={how} onChange={(e) => setHow(e.target.value)} placeholder={t("How to change them (optional), e.g. more hopeful, about the sea")} aria-label={t("How to change them")} onKeyDown={(e) => e.key === "Enter" && rewriteLines()} />
              <button className="btn small" disabled={rewriting} onClick={rewriteLines}>{rewriting ? t("Writing…") : t("Write these lines again")}</button>
            </div>
          )}
          {mode === "song" && !auto && vocals === "mine" && sungLines(lyrics) > Math.round(secs / SECS_PER_LINE) * 1.25 && (
            <p className="callout small">
              {t("{n} lines need about {time} to be sung clearly; at {length} the words come out mumbled.", { n: sungLines(lyrics), time: mmss(fitFor(lyrics)), length: mmss(secs) })}{" "}
              <button className="link" onClick={() => setSeconds(fitFor(lyrics))}>{t("Make it {time}", { time: mmss(fitFor(lyrics)) })}</button>
            </p>
          )}
          {vocals === "write" && mode === "song" && <p className="small muted">{t("They're written when the song is made. To read and change them first, press Write lyrics for me.")}</p>}
        </div>
      )}

      {mode === "song" && (
        <Fold title={t("Voice details")} summary={[vocalEffects.length ? t("{n} vocal effects", { n: vocalEffects.length }) : "", rhyme ? t(rhyme) : "", harmonyVoice ? t("harmonies") : "", harmonyInstrument ? t(harmonyInstrument) : ""].filter(Boolean).join(", ") || t("As the song suits")} info={t("How the singing sounds: effects like vibrato or falsetto on parts of the song, how the lines rhyme, and backing harmonies.")}>
          {mode === "song" && vocals !== "none" && <VocalEffects value={vocalEffects} onChange={setVocalEffects} />}
          {mode === "song" && vocals !== "none" && <Chips label={t("Rhyme")} options={RHYMES} value={rhyme} onChange={setRhyme} />}
          {mode === "song" && (
            <div className="music-field">
              <span className="music-label">{t("Harmonies")}</span>
              {vocals !== "none" && (
                <div className="filters tight wrap" role="group" aria-label={t("Harmony singers")}>
                  <span className="small muted">{t("Singers:")}</span>
                  {HARMONY_VOICES.map(([v, n]) => (
                    <button key={v || "none"} className={`chip ${harmonyVoice === v ? "active" : ""}`} onClick={() => setHarmonyVoice(v)}>{t(n)}</button>
                  ))}
                </div>
              )}
              {vocals !== "none" && harmonyVoice && (
                <div className="filters tight wrap" role="group" aria-label={t("How the harmonies are sung")}>
                  <span className="small muted">{t("Sung:")}</span>
                  {HARMONY_STYLES.map(([v, n, why]) => (
                    <button key={v} className={`chip ${harmonyStyle === v ? "active" : ""}`} onClick={() => setHarmonyStyle(v)} title={t(why)}>{t(n)}</button>
                  ))}
                </div>
              )}
            </div>
          )}
          {mode === "song" && <PickOne label={t("Harmony instrument")} options={HARMONY_INSTRUMENTS} value={harmonyInstrument} onChange={setHarmonyInstrument} other={t("Other instrument")} />}
          {mode === "song" && harmonized && (
            <div className="music-field">
              <div className="filters tight wrap" role="group" aria-label={t("Where the harmonies come in")}>
                <span className="small muted">{t("Where:")}</span>
                {HARMONY_WHERE.map(([v, n]) => (
                  <button key={v} className={`chip ${harmonyWhere === v ? "active" : ""}`} onClick={() => setHarmonyWhere(v)}>{t(n)}</button>
                ))}
              </div>
              <label className="check small">
                <input type="checkbox" checked={harmonyLayer && canLayer} disabled={!canLayer} onChange={(e) => setHarmonyLayer(e.target.checked)} />
                {t("Layer them on afterwards, for fuller harmonies (takes longer)")}
              </label>
              {!canLayer && (
                <p className="small muted">
                  {t("Layering needs ACE-Step 1.5 Base.")} <button className="link" onClick={onModels}>{t("Get it")}</button>
                </p>
              )}
              {vocals === "mine" && harmonyVoice && <p className="small muted">{t("In your lyrics, put what the harmony singers sing in round brackets, e.g. We'll find our way (find our way).")}</p>}
            </div>
          )}
        </Fold>
      )}
      {mode === "song" && (
        <Fold title={t("Structure, tempo and key")} summary={[sections.length ? t("{n} sections", { n: sections.length }) : "", bpm !== null ? t("{n} beats a minute", { n: bpm }) : "", beat ? t(beat) : "", keyNote ? `${keyNote} ${keyMode === "major" ? t("Major") : t("Minor")}` : "", meter, leadInstrument ? t(leadInstrument) : "", energy ? t(energy) : "", ending ? t(ending) : "", era ? t(era) : "", production ? t(production) : ""].filter(Boolean).join(", ") || t("As the song suits")} info={t("The song's sections in order, its speed, beat, key and time signature, lead instrument, energy, ending, era and production. Left as they are, the song decides.")}>
          {mode === "song" && (
            <SongSections
              value={sections}
              onChange={(v) => {
                setSections(v);
                // The length follows the structure (it can still be changed after).
                if (v.length) setSeconds(structureSeconds(v));
              }}
            />
          )}
          {mode === "song" && (
            <div className="music-field">
              <span className="music-label">{t("Tempo")}</span>
              <div className="row wrap">
                <div className="filters tight">
                  <button className={`chip ${bpm === null ? "active" : ""}`} onClick={() => setBpm(null)}>{t("Auto")}</button>
                  {PACES.map(([label, b]) => (
                    <button key={label} className={`chip ${bpm === b ? "active" : ""}`} onClick={() => setBpm(b)}>{t(label)}</button>
                  ))}
                </div>
                {bpm !== null && (
                  <label className="small music-length">
                    <input type="range" min={50} max={200} value={bpm} onChange={(e) => setBpm(+e.target.value)} aria-label={t("Beats a minute")} />
                    {t("{n} beats a minute", { n: bpm })}
                  </label>
                )}
              </div>
            </div>
          )}

          {mode === "song" && <PickOne label={t("Beat")} options={BEATS} value={beat} onChange={setBeat} other={t("Other beat")} />}

          {keyTime}

          {mode === "song" && (
            <>
              <PickOne label={t("Lead instrument")} options={LEAD_INSTRUMENTS} value={leadInstrument} onChange={setLeadInstrument} other={t("Other instrument")} />
              <Chips label={t("Energy")} options={ENERGIES} value={energy} onChange={setEnergy} />
              <Chips label={t("Ending")} options={ENDINGS} value={ending} onChange={setEnding} />
              <Chips label={t("Era")} options={ERAS} value={era} onChange={setEra} />
              <Chips label={t("Production")} options={PRODUCTIONS} value={production} onChange={setProduction} />
            </>
          )}
        </Fold>
      )}
      {mode === "edit" && keyTime}

      {mode !== "sound" && (
        <button className="link small music-more" onClick={() => setMore((x) => !x)} aria-expanded={more}>
          {more ? t("Fewer settings") : t("More settings: weirdness, style influence, leave out, a song to sound like")}
        </button>
      )}
      {more && mode !== "sound" && (
        <div className="row wrap small">
          {mode === "song" && (
            <label className="row music-length" title={t("How far from the usual: surprising sounds, words and structure at the top, by the book at the bottom")}>
              {t("Weirdness")}
              <InfoTip label={t("Weirdness")} text={t("How far the song strays from what's usual for its style. Low keeps to familiar chords, structure and words; high tries surprising sounds, harmonies and turns. Auto picks for the style.")} />
              <input type="range" min={0} max={100} step={5} value={weird ?? 50} onChange={(e) => setWeird(+e.target.value)} />
              {weird == null ? t("Auto") : `${weird}%`}
            </label>
          )}
          <label className="row music-length" title={t("How closely the music model follows the description and style: loosely at the bottom, strictly at the top")}>
            {t("Style influence")}
            <InfoTip label={t("Style influence")} text={t("How closely the song follows your description and the styles you picked. Low takes them as a loose hint, so the music model adds its own ideas; high sticks to them closely, which can sound stiffer.")} />
            <input type="range" min={0} max={100} step={5} value={influence ?? 50} onChange={(e) => setInfluence(+e.target.value)} />
            {influence == null ? t("Auto") : `${influence}%`}
          </label>
          {(weird != null || influence != null) && (
            <button className="link small" onClick={() => { setWeird(null); setInfluence(null); }}>{t("Back to Auto")}</button>
          )}
        </div>
      )}
      {more && mode !== "sound" && (
        <input className="input" value={exclude} onChange={(e) => setExclude(e.target.value)} placeholder={t("Leave out (optional), e.g. drums, autotune, saxophone")} aria-label={t("Leave out")} />
      )}
      {more && mode !== "sound" && (
        <div className="row wrap">
          <span className="small">{t("Sound like:")}</span>
          <select value={reference?.id ?? ""} onChange={(e) => setReference(gallery.find((g) => g.id === e.target.value) ?? null)} aria-label={t("A song to sound like")}>
            <option value="">{t("No song")}</option>
            {gallery.filter((g) => g.kind === "audio").map((g) => (
              <option key={g.id} value={g.id}>{g.title || g.prompt || t("A song")}</option>
            ))}
          </select>
        </div>
      )}

      <div className="row wrap">
        {mode === "song" && (
          <button
            className={`chip ${auto ? "active" : ""}`}
            onClick={() => setAutoLength((a) => !a)}
            title={t("The song is as long as it needs: a length named in the description, your lyrics or the structure, or the lyrics written for it")}
          >
            {t("Auto length")}
          </button>
        )}
        {auto && <span className="small muted">{autoGuess ? t("about {time}", { time: mmss(autoGuess) }) : t("fits the song")}</span>}
        {mode !== "edit" && !auto && (
          <label className="small music-length">
            {t("Length")}
            <input
              type="range"
              min={mode === "sound" ? 10 : 10}
              max={mode === "sound" ? 30 : 600}
              step={mode === "sound" ? 5 : 10}
              value={secs}
              onChange={(e) => setSeconds(+e.target.value)}
              aria-label={t("Length")}
            />
            {mmss(secs)}
          </label>
        )}
        {mode === "song" && (
          <select
            value={bestOf && singing ? "best" : versions}
            onChange={(e) => {
              const best = e.target.value === "best";
              setBestOf(best);
              setVersions(best ? 3 : +e.target.value);
            }}
            aria-label={t("Versions")}
            title={t("Make several versions at once and keep the one you like")}
          >
            <option value={1}>{t("1 version")}</option>
            <option value={2}>{t("2 versions")}</option>
            <option value={3}>{t("3 versions")}</option>
            {singing && <option value="best">{t("Best of 3 (keeps the clearest)")}</option>}
          </select>
        )}
        {mode === "song" && canArt && (
          <label className="check small" title={t("A square picture for each song from the picture model, made right after it")}>
            <input type="checkbox" checked={coverArt} onChange={(e) => setCoverArt(e.target.checked)} /> {t("Cover art")}
          </label>
        )}
        <ModelPick models={fit.length ? fit : models} value={m?.id ?? ""} onChange={setModel} onMore={onModels} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel(est)}</span>}
        {mode === "song" && (
          <button className="btn" disabled={!ready || arranging} onClick={writeAsNotes} title={t("See and change every note in an editor first, then make it real with these settings")}>
            {arranging ? t("Writing the arrangement…") : t("Write it as notes first")}
          </button>
        )}
        <button className="btn primary" disabled={!ready || !m} onClick={go}>
          {mode === "sound" ? t("Make the sound") : mode === "edit" ? t("Make it") : t("Make the song")}
        </button>
      </div>
      {mode === "song" && (
        <p className="small muted">
          {t("Write it as notes first: the chat model arranges the song in bars and chords for every part, you can see, hear and change any note, then Make it real records it with these settings.")}
        </p>
      )}
      {mode === "sound" && <p className="small muted">{t("Sound effects come from the music model, so they're rough sketches rather than library-quality recordings.")}</p>}
      {mode === "song" && secs > 240 && <p className="small muted">{t("Long songs take a while: about {time} here.", { time: waitLabel(est) })}</p>}
    </div>
  );
}

/** My taste in a line: "country, folk · warm · acoustic guitar, pedal steel · male voice". */
export function tasteLine(s: Taste): string {
  return [s.genres.join(", "), s.moods.join(", "), s.instruments.join(", "), s.voice ? t("{voice} voice", { voice: s.voice }) : ""].filter(Boolean).join(" · ");
}
