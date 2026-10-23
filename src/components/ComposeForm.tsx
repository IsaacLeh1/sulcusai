// SPDX-License-Identifier: AGPL-3.0-only
// Compose: what a song written as notes should be. The chat model plans it
// (sections, bars, chords, how each part plays, the words) in the time
// signature, tempo, key, style and era picked here, and the arranger writes
// its notes; it then opens in the editor (`ComposeEditor`).
import { useState } from "react";
import { api, errorText, type Composition } from "../api";
import { t, tx } from "../i18n";
import { NOTE_NAMES, TIMES } from "./ComposeEditor";
import type { PushToast } from "./Toasts";

/** Decades a song can be from: [value, name]. Shared with the Music tab. */
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
const STYLES = [
  tx("Pop"),
  tx("Rock"),
  tx("Folk"),
  tx("Ballad"),
  tx("Country"),
  tx("R&B"),
  tx("Soul"),
  tx("Funk"),
  tx("Hip hop"),
  tx("Jazz"),
  tx("Blues"),
  tx("Electronic"),
  tx("Reggae"),
  tx("Latin"),
  tx("Gospel"),
  tx("Waltz"),
];
const LENGTHS: [number, string][] = [
  [4, tx("Short")],
  [6, tx("Medium")],
  [8, tx("Long")],
];
const VOICES: [string, string][] = [
  ["", tx("As described")],
  ["female", tx("Female")],
  ["male", tx("Male")],
  ["duet", tx("Duet")],
  ["choir", tx("Choir")],
];

export function ComposeForm({ onWritten, toast }: { onWritten: (c: Composition) => void; toast: PushToast }) {
  const [description, setDescription] = useState("");
  const [time, setTime] = useState("4/4");
  const [beats, setBeats] = useState(5);
  const [unit, setUnit] = useState(8);
  const [bpm, setBpm] = useState<number | null>(null);
  const [key, setKey] = useState("");
  const [style, setStyle] = useState("");
  const [era, setEra] = useState("");
  const [sections, setSections] = useState(6);
  const [vocals, setVocals] = useState(true);
  const [voice, setVoice] = useState("");
  const [busy, setBusy] = useState(false);

  const write = async () => {
    if (!description.trim() || busy) return;
    const [b, u] = time === "other" ? [beats, unit] : time.split("/").map(Number);
    setBusy(true);
    try {
      onWritten(await api.composeWrite({ description, beats: b, unit: u, bpm, key: key || null, style: style || null, sections, vocals, voice, era }));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="form compose-form">
      <p className="small muted">
        {t("Write a song as notes: the chat model plans its sections, chords and tune, and you can change any note before having the music model make it real.")}
      </p>
      <textarea
        className="input"
        rows={3}
        value={description}
        onChange={(e) => setDescription(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && (e.ctrlKey || e.metaKey) && write()}
        placeholder={t("Describe the song, e.g. a hopeful folk song about leaving home, with a big last chorus")}
      />
      <div className="row wrap">
        <label className="small row">
          {t("Time signature")}
          <select className="input" value={time} onChange={(e) => setTime(e.target.value)}>
            {TIMES.map(([b, u]) => (
              <option key={`${b}/${u}`} value={`${b}/${u}`}>{`${b}/${u}`}</option>
            ))}
            <option value="other">{t("Other…")}</option>
          </select>
        </label>
        {time === "other" && (
          <span className="small row">
            <input type="number" className="input narrow" min={1} max={16} value={beats} aria-label={t("Beats in a bar")} onChange={(e) => setBeats(Math.max(1, Math.min(16, +e.target.value || 1)))} />
            /
            <select className="input" value={unit} aria-label={t("The note that counts a beat")} onChange={(e) => setUnit(+e.target.value)}>
              {[2, 4, 8, 16].map((u) => (
                <option key={u} value={u}>{u}</option>
              ))}
            </select>
          </span>
        )}
        <label className="small row">
          {t("Tempo")}
          <input
            type="number"
            className="input narrow"
            min={40}
            max={220}
            placeholder={t("Auto")}
            value={bpm ?? ""}
            onChange={(e) => setBpm(e.target.value ? Math.max(40, Math.min(220, +e.target.value)) : null)}
          />
        </label>
        <label className="small row">
          {t("Key")}
          <select className="input" value={key} onChange={(e) => setKey(e.target.value)}>
            <option value="">{t("Auto")}</option>
            {NOTE_NAMES.flatMap((n) => [`${n} major`, `${n} minor`]).map((k) => (
              <option key={k} value={k}>{k.replace("major", t("major")).replace("minor", t("minor"))}</option>
            ))}
          </select>
        </label>
        <label className="small row">
          {t("Style")}
          <input className="input" list="compose-styles" value={style} placeholder={t("Any")} onChange={(e) => setStyle(e.target.value)} />
          <datalist id="compose-styles">
            {STYLES.map((s) => (
              // The style goes to the models in English; its name shows in the person's language.
              <option key={s} value={s} label={t(s)} />
            ))}
          </datalist>
        </label>
      </div>
      <div className="music-field">
        <span className="music-label">{t("Era")}</span>
        <div className="filters tight wrap">
          <button className={`chip ${era === "" ? "active" : ""}`} onClick={() => setEra("")}>{t("Any")}</button>
          {ERAS.map(([v, name]) => (
            <button key={v} className={`chip ${era === v ? "active" : ""}`} onClick={() => setEra(era === v ? "" : v)}>{t(name)}</button>
          ))}
        </div>
      </div>
      <div className="row wrap">
        <span className="small">{t("Length")}</span>
        <span className="filters tight">
          {LENGTHS.map(([n, name]) => (
            <button key={n} className={`chip ${sections === n ? "active" : ""}`} onClick={() => setSections(n)}>{t(name)}</button>
          ))}
        </span>
        <label className="small row">
          <input type="checkbox" checked={vocals} onChange={(e) => setVocals(e.target.checked)} /> {t("Words to sing")}
        </label>
        {vocals && (
          <label className="small row">
            {t("Who sings it when it's made real")}
            <select className="input" value={voice} onChange={(e) => setVoice(e.target.value)}>
              {VOICES.map(([v, name]) => (
                <option key={v} value={v}>{t(name)}</option>
              ))}
            </select>
          </label>
        )}
        <span className="spacer" />
        <button className="btn primary" disabled={!description.trim() || busy} onClick={write}>
          {busy ? t("Writing the arrangement…") : t("Write it")}
        </button>
      </div>
    </div>
  );
}
