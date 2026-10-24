// SPDX-License-Identifier: AGPL-3.0-only
// Compose's "Make it real": the music model records a song that follows the
// composition's notes. Every song option of the Music tab can be set here
// (who sings and how, the feeling, styles, moods, harmonies, production…)
// and the words changed, starting from what the song was written with.
import { useState } from "react";
import type { MediaModelCard, MusicSpec, Plan, VocalEffect } from "../api";
import { t } from "../i18n";
import {
  Chips, EMOTIONS, ENDINGS, ENERGIES, ERAS, GENRES, HARMONY_INSTRUMENTS, HARMONY_STYLES, HARMONY_VOICES, HARMONY_WHERE, INSTRUMENTS, LANGUAGES, LEAD_INSTRUMENTS,
  MOODS, PickMany, PickOne, PRODUCTIONS, VOCAL_STYLES, VocalEffects, joined, listed,
} from "./MusicFields";

/** The words of a plan, tagged by section, as the music model sings them (`Plan::lyrics`). */
export function lyricsOf(plan: Plan): string {
  return plan.sections.map((s) => (s.lines.length ? `[${s.name}]\n${s.lines.join("\n")}` : `[${s.name}]`)).join("\n\n");
}

/** The notes a singer sings (MIDI), as `compose::voice_range`. */
export function voiceRange(voice: string): [number, number] {
  const v = voice.trim().toLowerCase();
  return v === "male" || v === "man" || v === "rap" ? [48, 64] : v === "female" || v === "woman" ? [60, 76] : [55, 72];
}

/** Octaves (in semitones) the tune moves to fit a voice: none when it already fits. */
export function fitShift(pitches: number[], voice: string): number {
  if (!pitches.length || !voice) return 0;
  const [lo, hi] = voiceRange(voice);
  const sorted = [...pitches].sort((a, b) => a - b);
  if (sorted[0] >= lo - 2 && sorted[sorted.length - 1] <= hi + 2) return 0;
  const median = sorted[Math.floor(sorted.length / 2)];
  return 12 * Math.round(((lo + hi) / 2 - median) / 12);
}

export type RealOptions ={ strength: number; modelId: string | null; song: MusicSpec; lyrics: string | null; language: string | null };

export function MakeItReal({ plan, melody, models, defaultModel, busy, onCancel, onRecord }: {
  plan: Plan;
  /** The tune's pitches, to tell whether it moves for another voice. */
  melody: number[];
  /** Installed music models. */
  models: MediaModelCard[];
  defaultModel: string | null;
  busy: boolean;
  onCancel: () => void;
  onRecord: (o: RealOptions) => void;
}) {
  const start = plan.song ?? {};
  const words = lyricsOf(plan);
  const [lyrics, setLyrics] = useState(words);
  const sung = lyrics.split("\n").some((l) => l.trim() && !l.trim().startsWith("["));
  // In eighths: the music model follows the demo for that share of its 8 steps, rounded down.
  const [strength, setStrength] = useState(sung ? 0.375 : 0.5);
  const [model, setModel] = useState(models.find((m) => m.id === defaultModel)?.id ?? models[0]?.id ?? "");
  const [voice, setVoice] = useState(start.voice ?? plan.voice ?? "");
  const [vocalStyle, setVocalStyle] = useState(start.vocal_style ?? "");
  const [language, setLanguage] = useState(plan.language ?? "auto");
  const [emotion, setEmotion] = useState(listed(start.emotion));
  const [genre, setGenre] = useState(listed(start.genre ?? plan.style));
  const [mood, setMood] = useState(listed(start.mood));
  const [more, setMore] = useState(false);
  const [instruments, setInstruments] = useState<string[]>(start.instruments ?? []);
  const [effects, setEffects] = useState<VocalEffect[]>(start.vocal_effects ?? []);
  const [harmonyVoice, setHarmonyVoice] = useState<MusicSpec["harmony_voice"] | "">(start.harmony_voice ?? "");
  const [harmonyStyle, setHarmonyStyle] = useState<NonNullable<MusicSpec["harmony_style"]>>(start.harmony_style ?? "thirds");
  const [harmonyWhere, setHarmonyWhere] = useState<NonNullable<MusicSpec["harmony_where"]>>(start.harmony_where ?? "chorus");
  const [harmonyInstrument, setHarmonyInstrument] = useState(start.harmony_instrument ?? "");
  const [lead, setLead] = useState(start.lead_instrument ?? "");
  const [energy, setEnergy] = useState<NonNullable<MusicSpec["energy"]> | "">(start.energy ?? "");
  const [ending, setEnding] = useState<NonNullable<MusicSpec["ending"]> | "">(start.ending ?? "");
  const [era, setEra] = useState(start.era ?? plan.era ?? "");
  const [production, setProduction] = useState(start.production ?? "");

  const record = () =>
    onRecord({
      strength,
      modelId: model || null,
      lyrics: lyrics.trim() && lyrics.trim() !== words.trim() ? lyrics : null,
      language: sung && language !== "auto" ? language : null,
      song: {
        ...start,
        genre: joined(genre),
        mood: joined(mood),
        emotion: sung ? joined(emotion) : null,
        voice: sung ? voice || null : null,
        vocal_style: sung ? vocalStyle || null : null,
        vocal_effects: sung ? effects : [],
        instruments,
        harmony_voice: sung && harmonyVoice ? harmonyVoice : null,
        harmony_style: sung && harmonyVoice ? harmonyStyle : null,
        harmony_instrument: harmonyInstrument || null,
        harmony_where: (sung && harmonyVoice) || harmonyInstrument ? harmonyWhere : null,
        lead_instrument: lead || null,
        energy: energy || null,
        ending: ending || null,
        era: era || null,
        production: production || null,
      },
    });

  return (
    <div className="song-view-ask form make-real">
      <strong className="small">{t("Make it real")}</strong>
      <p className="small muted">
        {t("The music model records the song with real-sounding instruments, following your notes, and sings its words. It's a new song in the gallery; this composition stays as it is.")}
      </p>
      {models.length === 0 ? (
        <p className="small warn-box">{t("This needs a music model: install one in the Studio's Models.")}</p>
      ) : (
        <>
          {sung && (
            <div className="row wrap">
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
            </div>
          )}
          {sung && fitShift(melody, voice) !== 0 && (
            <p className="small muted">
              {fitShift(melody, voice) > 0
                ? t("The tune moves up {n} octave(s) to fit this voice, and the demo is recorded again.", { n: fitShift(melody, voice) / 12 })
                : t("The tune moves down {n} octave(s) to fit this voice, and the demo is recorded again.", { n: -fitShift(melody, voice) / 12 })}
            </p>
          )}
          {sung && <PickMany label={t("Emotion in the words and voice")} options={EMOTIONS} value={emotion} onChange={setEmotion} other={t("Add another feeling (Enter)")} />}
          <PickMany label={t("Style")} options={GENRES} value={genre} onChange={setGenre} other={t("Add another style (Enter)")} />
          <PickMany label={t("Mood of the music")} options={MOODS} value={mood} onChange={setMood} other={t("Add another mood (Enter)")} />
          <div className="music-field">
            <span className="music-label">{t("Lyrics")}</span>
            <textarea className="input lyrics-box" rows={8} value={lyrics} aria-label={t("Lyrics")} placeholder={"[Verse]\n…\n\n[Chorus]\n…"} onChange={(e) => setLyrics(e.target.value)} />
            <p className="small muted">{t("Change the words to sing your own; the notes stay as they are. Keep a line about every two bars so the words fit the tune.")}</p>
          </div>
          <button className="link small music-more" onClick={() => setMore((x) => !x)} aria-expanded={more}>
            {more ? t("Fewer settings") : t("More settings: instruments, ways of singing, harmonies, production")}
          </button>
          {more && (
            <>
              <PickMany label={t("Instruments")} options={INSTRUMENTS} value={instruments} onChange={setInstruments} other={t("Add another (Enter)")} />
              {sung && <VocalEffects value={effects} onChange={setEffects} />}
              {sung && (
                <div className="music-field">
                  <span className="music-label">{t("Harmonies")}</span>
                  <div className="filters tight wrap" role="group" aria-label={t("Harmony singers")}>
                    <span className="small muted">{t("Singers:")}</span>
                    {HARMONY_VOICES.map(([v, n]) => (
                      <button key={v || "none"} className={`chip ${harmonyVoice === v ? "active" : ""}`} onClick={() => setHarmonyVoice(v)}>{t(n)}</button>
                    ))}
                  </div>
                  {harmonyVoice && (
                    <div className="filters tight wrap" role="group" aria-label={t("How the harmonies are sung")}>
                      <span className="small muted">{t("Sung:")}</span>
                      {HARMONY_STYLES.map(([v, n, why]) => (
                        <button key={v} className={`chip ${harmonyStyle === v ? "active" : ""}`} onClick={() => setHarmonyStyle(v)} title={t(why)}>{t(n)}</button>
                      ))}
                    </div>
                  )}
                </div>
              )}
              <PickOne label={t("Harmony instrument")} options={HARMONY_INSTRUMENTS} value={harmonyInstrument} onChange={setHarmonyInstrument} other={t("Other instrument")} />
              {((sung && harmonyVoice) || harmonyInstrument) && (
                <div className="filters tight wrap" role="group" aria-label={t("Where the harmonies come in")}>
                  <span className="small muted">{t("Where:")}</span>
                  {HARMONY_WHERE.map(([v, n]) => (
                    <button key={v} className={`chip ${harmonyWhere === v ? "active" : ""}`} onClick={() => setHarmonyWhere(v)}>{t(n)}</button>
                  ))}
                </div>
              )}
              <PickOne label={t("Lead instrument")} options={LEAD_INSTRUMENTS} value={lead} onChange={setLead} other={t("Other instrument")} />
              <Chips label={t("Energy")} options={ENERGIES} value={energy} onChange={setEnergy} />
              <Chips label={t("Ending")} options={ENDINGS} value={ending} onChange={setEnding} />
              <Chips label={t("Era")} options={ERAS} value={era} onChange={setEra} />
              <Chips label={t("Production")} options={PRODUCTIONS} value={production} onChange={setProduction} />
            </>
          )}
          <label className="small row">
            {t("How closely it follows your notes")}
            <input type="range" min={0.125} max={0.875} step={0.125} value={strength} onChange={(e) => setStrength(+e.target.value)} />
            <span className="mono">{Math.round(strength * 8)}/8</span>
          </label>
          <p className="small muted">{t("Higher keeps closer to your tune, chords and timing but sings fewer of the words; lower sings more and plays more freely.")}</p>
          <div className="row wrap">
            {models.length > 1 && (
              <select value={model} onChange={(e) => setModel(e.target.value)} aria-label={t("Music model")}>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>{m.name}</option>
                ))}
              </select>
            )}
            <span className="spacer" />
            <button className="btn small" onClick={onCancel}>{t("Cancel")}</button>
            <button className="btn primary small" disabled={busy || !model} onClick={record}>{t("Record it")}</button>
          </div>
        </>
      )}
    </div>
  );
}
