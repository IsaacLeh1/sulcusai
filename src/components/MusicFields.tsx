// SPDX-License-Identifier: AGPL-3.0-only
// The song options of the Music tab, shared with Compose's Make it real:
// what to pick from (styles, moods, emotions, instruments, voices, harmonies,
// ways of singing, structure) and the controls that pick them.
import { useState } from "react";
import type { MusicSpec, VocalEffect } from "../api";
import { t, tx } from "../i18n";

/** Styles, moods, instruments and beats to pick from (any can be typed too). */
export const GENRES = [
  tx("Pop"), tx("Rock"), tx("Hip hop"), tx("R&B"), tx("Country"), tx("Folk"), tx("Jazz"), tx("Blues"), tx("Classical"), tx("Electronic"),
  tx("House"), tx("Techno"), tx("Lo-fi"), tx("Metal"), tx("Punk"), tx("Indie"), tx("Reggae"), tx("Latin"), tx("K-pop"), tx("Soul"),
  tx("Funk"), tx("Gospel"), tx("Ambient"), tx("Soundtrack"), tx("Synthwave"), tx("Drum and bass"),
];
export const MOODS = [tx("Happy"), tx("Sad"), tx("Energetic"), tx("Calm"), tx("Dark"), tx("Romantic"), tx("Epic"), tx("Dreamy"), tx("Angry"), tx("Hopeful"), tx("Nostalgic"), tx("Playful")];
export const INSTRUMENTS = [
  tx("Piano"), tx("Acoustic guitar"), tx("Electric guitar"), tx("Bass"), tx("Drums"), tx("Strings"), tx("Violin"), tx("Cello"), tx("Synth"), tx("Brass"),
  tx("Saxophone"), tx("Trumpet"), tx("Flute"), tx("Organ"), tx("Harmonica"), tx("Ukulele"), tx("Banjo"), tx("808 bass"), tx("Choir"), tx("Percussion"),
];
export const BEATS = [tx("Steady"), tx("Four on the floor"), tx("Swing"), tx("Shuffle"), tx("Half-time"), tx("Trap"), tx("Boom bap"), tx("Breakbeat"), tx("Reggaeton"), tx("Waltz")];
/** Pace presets, in beats a minute. */
export const PACES: [string, number][] = [[tx("Slow"), 70], [tx("Relaxed"), 90], [tx("Medium"), 105], [tx("Upbeat"), 120], [tx("Fast"), 140], [tx("Very fast"), 170]];
export const NOTES = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/** The parts the music engine knows: [what it's called there, what people call it]. */
export const PARTS: [string, string][] = [
  ["vocals", tx("Vocals")], ["backing_vocals", tx("Backing vocals")], ["drums", tx("Drums")], ["bass", tx("Bass")], ["guitar", tx("Guitar")], ["keyboard", tx("Keys")],
  ["percussion", tx("Percussion")], ["strings", tx("Strings")], ["synth", tx("Synth")], ["brass", tx("Brass")], ["woodwinds", tx("Woodwinds")], ["fx", tx("Effects")],
];
export type SongTask = "cover" | "repaint" | "extend" | "add" | "extract" | "complete";
export const TASKS: [SongTask, string, string][] = [
  ["cover", tx("Restyle it"), tx("The same song in another style: describe how it should sound.")],
  ["repaint", tx("Make part of it again"), tx("Pick the part; the rest stays as it is.")],
  ["extend", tx("Make it longer"), tx("More music after its end, in the same style.")],
  ["add", tx("Add a part"), tx("Vocals, drums, guitar… added on top of the song.")],
  ["extract", tx("Keep only one part"), tx("Takes one part out of the mix, such as the vocals or the drums.")],
  ["complete", tx("Build a full song around it"), tx("For a lone part (a vocal, a guitar riff): the rest of the band added.")],
];

/** Styles whose vocals the music model blurs or buries (synthwave's words weren't heard however it was asked). */
export const SOFT_VOCALS = ["Synthwave", "Ambient", "Techno", "House", "Lo-fi", "Drum and bass"];

/** How the lead sings. */
export const VOCAL_STYLES: [string, string][] = [
  ["smooth", tx("Smooth")], ["powerful", tx("Powerful")], ["breathy", tx("Breathy")], ["raspy", tx("Raspy")],
  ["soulful", tx("Soulful")], ["falsetto", tx("Falsetto")], ["whispered", tx("Whispered")], ["belting", tx("Belting")],
];
/** The feeling in the words and the voice. */
export const EMOTIONS = [
  tx("Joyful"), tx("Heartbroken"), tx("Longing"), tx("Hopeful"), tx("Lonely"), tx("In love"), tx("Angry"),
  tx("Defiant"), tx("Bittersweet"), tx("Nostalgic"), tx("Grateful"), tx("Empowered"), tx("Anxious"), tx("At peace"),
];
/** Ways of singing on parts of a song: [id, name]. */
export const VOCAL_EFFECTS: [string, string][] = [
  ["vibrato", tx("Heavy vibrato")], ["belting", tx("Belting")], ["falsetto", tx("Falsetto")], ["screamed", tx("Screamed (screamo)")],
  ["growled", tx("Growled")], ["raspy", tx("Raspy and gritty")], ["breathy", tx("Breathy")], ["whispered", tx("Whispered")],
  ["spoken", tx("Spoken word")], ["rapped", tx("Rapped")], ["autotune", tx("Auto-tune")], ["runs", tx("Runs and riffs")], ["operatic", tx("Operatic")],
];
/** Where in the song: [id, name]. */
export const EFFECT_PLACES: [string, string][] = [
  ["all", tx("Whole song")], ["verse", tx("Verses")], ["pre-chorus", tx("Pre-choruses")], ["chorus", tx("Choruses")], ["bridge", tx("Bridge")], ["final-chorus", tx("Last chorus")],
];

/** Ways of singing, each on a part of the song: a screamed chorus, heavy vibrato throughout. */
export function VocalEffects({ value, onChange }: { value: VocalEffect[]; onChange: (v: VocalEffect[]) => void }) {
  const [how, setHow] = useState("vibrato");
  const [place, setPlace] = useState("all");
  const name = (list: [string, string][], id: string) => t(list.find(([v]) => v === id)?.[1] ?? id);
  const added = value.some((f) => f.how === how && f.on === place);
  return (
    <div className="music-field">
      <span className="music-label">{t("Vocal effects")}</span>
      {value.length > 0 && (
        <div className="filters tight wrap">
          {value.map((f, i) => (
            <button key={`${f.how}-${f.on}`} className="chip active" onClick={() => onChange(value.filter((_, j) => j !== i))} title={t("Remove")}>
              {name(VOCAL_EFFECTS, f.how)} · {name(EFFECT_PLACES, f.on)} ×
            </button>
          ))}
        </div>
      )}
      <div className="row wrap">
        <select value={how} onChange={(e) => setHow(e.target.value)} aria-label={t("Vocal effect")}>
          {VOCAL_EFFECTS.map(([v, n]) => (
            <option key={v} value={v}>{t(n)}</option>
          ))}
        </select>
        <select value={place} onChange={(e) => setPlace(e.target.value)} aria-label={t("Where in the song")}>
          {EFFECT_PLACES.map(([v, n]) => (
            <option key={v} value={v}>{t(n)}</option>
          ))}
        </select>
        <button className="btn ghost small" disabled={added || value.length >= 6} onClick={() => onChange([...value, { how, on: place }])}>{t("Add")}</button>
      </div>
    </div>
  );
}

/** Who sings the harmonies. */
export const HARMONY_VOICES: [MusicSpec["harmony_voice"] | "", string][] = [["", tx("None")], ["female", tx("Female")], ["male", tx("Male")], ["mixed", tx("Male and female")], ["choir", tx("Choir")]];
/** How they're sung: [id, name, what it means]. */
export const HARMONY_STYLES: [NonNullable<MusicSpec["harmony_style"]>, string, string][] = [
  ["thirds", tx("Close harmony"), tx("A third above the lead: the classic pop harmony")],
  ["octave", tx("Octaves"), tx("The melody doubled an octave apart")],
  ["stacked", tx("Stacked"), tx("Rich gospel-style stacks of voices")],
  ["call", tx("Call and response"), tx("The harmony singers answer the lead")],
  ["oohs", tx("Oohs and aahs"), tx("Wordless backing voices")],
];
export const HARMONY_WHERE: [NonNullable<MusicSpec["harmony_where"]>, string][] = [["chorus", tx("Choruses")], ["throughout", tx("Throughout")], ["ending", tx("Last chorus")]];
export const HARMONY_INSTRUMENTS = [tx("Strings"), tx("Piano"), tx("Synth pad"), tx("Brass"), tx("Organ"), tx("Electric guitar"), tx("Flute"), tx("Choir pad")];
export const LEAD_INSTRUMENTS = [tx("Electric guitar"), tx("Acoustic guitar"), tx("Piano"), tx("Saxophone"), tx("Synth"), tx("Violin"), tx("Trumpet"), tx("Flute"), tx("Harmonica")];
export const ENERGIES: [NonNullable<MusicSpec["energy"]>, string][] = [["steady", tx("Steady")], ["builds", tx("Builds up")], ["big-chorus", tx("Big chorus")], ["quiet", tx("Quiet and intimate")]];
export const ENDINGS: [NonNullable<MusicSpec["ending"]>, string][] = [["fade", tx("Fade out")], ["big-finish", tx("Big finish")], ["hard-stop", tx("Hard stop")]];
export const PRODUCTIONS: [string, string][] = [
  ["studio", tx("Polished studio")], ["live", tx("Live band")], ["lo-fi", tx("Lo-fi")], ["acoustic", tx("Acoustic")], ["modern", tx("Modern")],
];
/** How lines rhyme: [id, name, what it means]. */
export const RHYMES: [NonNullable<MusicSpec["rhyme"]>, string, string][] = [
  ["aabb", "AABB", tx("Lines rhyme in pairs")],
  ["abab", "ABAB", tx("Every other line rhymes")],
  ["abcb", "ABCB", tx("The second and fourth lines rhyme")],
  ["aaaa", "AAAA", tx("All the lines of a section rhyme")],
  ["free", tx("No rhyme"), tx("Free verse")],
];
/** Sections a song can have: [the music model's tag, what people call it, seconds it takes]. */
export const SECTIONS: [string, string, number][] = [
  ["Intro", tx("Intro"), 15], ["Instrumental Intro", tx("Instrumental intro"), 10], ["Verse", tx("Verse"), 30], ["Pre-Chorus", tx("Pre-chorus"), 15], ["Chorus", tx("Chorus"), 30],
  ["Bridge", tx("Bridge"), 15], ["Solo", tx("Solo"), 15], ["Instrumental", tx("Instrumental break"), 10], ["Outro", tx("Outro"), 15],
];
/** Song shapes to start from. */
export const SHAPES: [string, string, string[]][] = [
  ["verse-chorus", tx("Verse and chorus"), ["Verse", "Chorus", "Verse", "Chorus"]],
  ["verse-chorus-bridge", tx("With a bridge"), ["Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus"]],
  ["intro-outro", tx("Intro to outro"), ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"]],
  ["build-drop", tx("Build-up and drop"), ["Intro", "Verse", "Pre-Chorus", "Chorus", "Verse", "Pre-Chorus", "Chorus"]],
  ["aaba", tx("AABA (classic)"), ["Verse", "Verse", "Bridge", "Verse"]],
  ["story", tx("Story (no chorus)"), ["Verse", "Verse", "Verse"]],
];

/** The length a structure needs: sung sections 7.5 s a line, played ones less. */
export function structureSeconds(sections: string[]): number {
  const s = sections.reduce((n, x) => n + (SECTIONS.find(([id]) => id === x)?.[2] ?? 20), 0);
  return Math.min(600, Math.max(10, Math.ceil(s / 10) * 10));
}

/** The song's sections in order: a shape to start from, then added, moved or taken out one by one. */
export function SongSections({ value, onChange }: { value: string[]; onChange: (v: string[]) => void }) {
  const name = (id: string) => t(SECTIONS.find(([s]) => s === id)?.[1] ?? id);
  const move = (i: number, d: number) => {
    const j = i + d;
    if (j < 0 || j >= value.length) return;
    const next = [...value];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  };
  const choruses = value.filter((s) => s === "Chorus").length;
  return (
    <div className="music-field">
      <span className="music-label">{t("Song structure")}</span>
      <div className="filters tight wrap">
        <button className={`chip ${value.length === 0 ? "active" : ""}`} onClick={() => onChange([])}>{t("Any")}</button>
        {SHAPES.map(([id, label, list]) => (
          <button key={id} className={`chip ${value.join("|") === list.join("|") ? "active" : ""}`} onClick={() => onChange([...list])}>{t(label)}</button>
        ))}
      </div>
      {value.length > 0 && (
        <ol className="song-sections">
          {value.map((s, i) => (
            <li key={`${i}-${s}`} className="song-section">
              <button className="icon-btn small" disabled={i === 0} onClick={() => move(i, -1)} aria-label={t("Move {name} earlier", { name: name(s) })} title={t("Earlier")}>‹</button>
              <span>{name(s)}</span>
              <button className="icon-btn small" disabled={i === value.length - 1} onClick={() => move(i, 1)} aria-label={t("Move {name} later", { name: name(s) })} title={t("Later")}>›</button>
              <button className="icon-btn small" onClick={() => onChange(value.filter((_, j) => j !== i))} aria-label={t("Take out {name}", { name: name(s) })} title={t("Take out")}>×</button>
            </li>
          ))}
        </ol>
      )}
      <div className="row wrap">
        {SECTIONS.map(([id, label]) => (
          <button key={id} className="btn ghost small" onClick={() => onChange([...value, id])} disabled={value.length >= 24}>+ {t(label)}</button>
        ))}
      </div>
      {value.length > 0 && (
        <p className="small muted">
          {choruses > 1 ? `${t("The chorus {n} times", { n: choruses })} · ` : choruses === 1 ? `${t("The chorus once")} · ` : ""}
          {t("{n} sections, about {time} long", { n: value.length, time: mmss(structureSeconds(value)) })}
        </p>
      )}
    </div>
  );
}

/** Chips to pick one or none: [value, name, what it means?]. */
export function Chips<T extends string>({ label, options, value, onChange, any = true }: { label: string; options: [T, string, string?][]; value: T | ""; onChange: (v: T | "") => void; any?: boolean }) {
  return (
    <div className="music-field">
      <span className="music-label">{label}</span>
      <div className="filters tight wrap">
        {any && <button className={`chip ${value === "" ? "active" : ""}`} onClick={() => onChange("")}>{t("Any")}</button>}
        {options.map(([v, name, why]) => (
          <button key={v} className={`chip ${value === v ? "active" : ""}`} onClick={() => onChange(value === v && any ? "" : v)} title={why ? t(why) : undefined}>{t(name)}</button>
        ))}
      </div>
    </div>
  );
}

/** Seconds of song a sung line needs to be heard (media/lyrics.rs `SECS_PER_LINE`). */
export const SECS_PER_LINE = 7.5;

/** Sung lines in lyrics: not the [Verse] tags or blank lines. */
export function sungLines(lyrics: string): number {
  return lyrics.split("\n").map((l) => l.trim()).filter((l) => l && !l.startsWith("[")).length;
}

/** A length that fits the lyrics, in steps of 10 seconds, at most 10 minutes. */
export function fitFor(lyrics: string): number {
  return Math.min(600, Math.ceil((sungLines(lyrics) * SECS_PER_LINE) / 10) * 10);
}

/** Seconds as "1:05". */
export function mmss(s: number): string {
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return `${m}:${String(r).padStart(2, "0")}`;
}

/** A row of chips to pick one (or none) from, plus a box for anything else. */
export function PickOne({ label, options, value, onChange, other }: { label: string; options: string[]; value: string; onChange: (v: string) => void; other?: string }) {
  const custom = value !== "" && !options.some((o) => t(o) === value || o === value);
  return (
    <div className="music-field">
      <span className="music-label">{label}</span>
      <div className="filters tight wrap">
        {options.map((o) => (
          <button key={o} className={`chip ${value === o || value === t(o) ? "active" : ""}`} onClick={() => onChange(value === o || value === t(o) ? "" : o)}>
            {t(o)}
          </button>
        ))}
        {other && <input className="input chip-input" value={custom ? value : ""} placeholder={other} aria-label={other} onChange={(e) => onChange(e.target.value)} />}
      </div>
    </div>
  );
}

/** Chips to pick several from, plus a box to add others. */
export function PickMany({ label, options, value, onChange, other }: { label: string; options: string[]; value: string[]; onChange: (v: string[]) => void; other: string }) {
  const [typed, setTyped] = useState("");
  const toggle = (o: string) => onChange(value.includes(o) ? value.filter((x) => x !== o) : [...value, o]);
  const extra = value.filter((v) => !options.includes(v));
  return (
    <div className="music-field">
      <span className="music-label">{label}</span>
      <div className="filters tight wrap">
        {options.map((o) => (
          <button key={o} className={`chip ${value.includes(o) ? "active" : ""}`} onClick={() => toggle(o)}>
            {t(o)}
          </button>
        ))}
        {extra.map((o) => (
          <button key={o} className="chip active" onClick={() => toggle(o)} title={t("Remove")}>
            {o} ×
          </button>
        ))}
        <input
          className="input chip-input"
          value={typed}
          placeholder={other}
          aria-label={other}
          onChange={(e) => setTyped(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && typed.trim()) {
              onChange([...value, typed.trim()]);
              setTyped("");
            }
          }}
        />
      </div>
    </div>
  );
}

/** Decades a song can be from: [value, name]. */
export const ERAS: [string, string][] = [
  ["1950s", tx("1950s")],
  ["1960s", tx("1960s")],
  ["1970s", tx("1970s")],
  ["1980s", tx("1980s")],
  ["1990s", tx("1990s")],
  ["2000s", tx("2000s")],
  ["2010s", tx("2010s")],
  ["2020s", tx("2020s")],
];

/** Languages the vocals can be sung in: [code, name]. */
export const LANGUAGES: [string, string][] = [
  ["auto", tx("Any language")],
  ["en", tx("English")],
  ["es", tx("Spanish")],
  ["fr", tx("French")],
  ["de", tx("German")],
  ["it", tx("Italian")],
  ["pt", tx("Portuguese")],
  ["ja", tx("Japanese")],
  ["ko", tx("Korean")],
  ["zh", tx("Chinese")],
];

/** A list picked from chips, as one value ("Pop, Rock") and back. */
export const joined = (list: string[]): string | null => (list.length ? list.join(", ") : null);
export const listed = (value?: string | null): string[] => (value ?? "").split(",").map((x) => x.trim()).filter(Boolean);
