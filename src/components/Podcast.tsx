// SPDX-License-Identifier: AGPL-3.0-only
// Podcasts and readings with several voices: who talks, in which voice, and
// for a podcast, what it's about. The script is written and read on this PC
// (or by the cloud model picked to write it).
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, type MediaRequest, type OwnVoice, type TalkSpeaker, type TalkSpec, type Voice } from "../api";
import { bytes } from "../format";
import { language as uiLanguage, t, tx } from "../i18n";
import { ModelSelect } from "./ModelChoices";
import { useOwnVoices } from "./OwnVoice";

/** This PC's voices (natural and Windows), loaded once per mount. */
export function useVoices(): Voice[] {
  const [voices, setVoices] = useState<Voice[]>([]);
  useEffect(() => {
    api.voices().then(setVoices).catch(() => {});
  }, []);
  return voices;
}

/** A voice to read in: the person's recorded and made voices, then this PC's. */
export function VoiceSelect({ value, onChange, voices, own, label, pickForMe, each }: {
  value: string | null;
  onChange: (v: string | null) => void;
  voices: Voice[];
  own: OwnVoice[];
  label: string;
  /** Offer "Pick one for me" (a different voice for each person). */
  pickForMe?: boolean;
  /** Offer "Each speaker's own voice" (dubbing: "each"). */
  each?: boolean;
}) {
  return (
    <select value={value ?? ""} onChange={(e) => onChange(e.target.value || null)} aria-label={label} title={label}>
      {each && <option value="each">{t("Each speaker's own voice")}</option>}
      {pickForMe && <option value="">{t("Pick one for me")}</option>}
      {own.length > 0 && (
        <optgroup label={t("Your voices")}>
          {own.map((v) => (
            <option key={v.id} value={`own:${v.id}`}>{v.designed ? t("{name} (made)", { name: v.name }) : v.name}</option>
          ))}
        </optgroup>
      )}
      <optgroup label={t("Voices on this PC")}>
        {voices.map((v) => (
          <option key={v.id} value={v.id}>{v.name}</option>
        ))}
      </optgroup>
    </select>
  );
}

/** Languages a podcast can be written and read in (the voices read these best). */
export const TALK_LANGUAGES: [string, string][] = [
  ["en", tx("English")],
  ["es", tx("Spanish")],
  ["fr", tx("French")],
  ["de", tx("German")],
  ["it", tx("Italian")],
  ["pt", tx("Portuguese")],
  ["ru", tx("Russian")],
  ["ja", tx("Japanese")],
  ["ko", tx("Korean")],
  ["zh", tx("Chinese")],
];

/** The language of the app, when it's one the voices read, else English. */
export function talkLanguage(): string {
  const ui = (uiLanguage() ?? "en").split(/[-_]/)[0];
  return TALK_LANGUAGES.some(([c]) => c === ui) ? ui : "en";
}

export function LanguageSelect({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} aria-label={t("Language")} title={t("Language")}>
      {TALK_LANGUAGES.map(([c, n]) => (
        <option key={c} value={c}>{t(n)}</option>
      ))}
    </select>
  );
}

const STYLES: [NonNullable<TalkSpec["style"]>, string][] = [
  ["conversation", tx("A conversation")],
  ["interview", tx("An interview")],
  ["debate", tx("A friendly debate")],
  ["lesson", tx("A lesson")],
  ["story", tx("A story told together")],
];
const LENGTHS = [2, 5, 10, 15, 20, 30];
const MAX_PEOPLE = 4;
const DOC_TYPES = ["pdf", "docx", "pptx", "xlsx", "xls", "csv", "txt", "md", "markdown", "html", "htm", "json", "rtf"];

/** The people a script names ("Name:" at the start of lines), at most four (as the reader finds them). */
export function namesIn(script: string): string[] {
  const out: string[] = [];
  for (const raw of script.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("#")) continue;
    const at = line.indexOf(":");
    if (at < 0) continue;
    const name = line.slice(0, at).replace(/^[\s*_\->]+|[\s*_\->]+$/g, "");
    const rest = line.slice(at + 1).trim();
    const fits = name.length > 0 && name.length <= 32 && name.split(/\s+/).length <= 3 && /^\p{L}/u.test(name);
    if (fits && rest && name.toLowerCase() !== "title" && !out.some((n) => n.toLowerCase() === name.toLowerCase())) {
      out.push(name);
      if (out.length === MAX_PEOPLE) break;
    }
  }
  return out;
}

/** A row for each person: their name, voice and who they are. */
function People({ people, onChange, voices, own }: { people: TalkSpeaker[]; onChange: (p: TalkSpeaker[]) => void; voices: Voice[]; own: OwnVoice[] }) {
  const set = (i: number, patch: Partial<TalkSpeaker>) => onChange(people.map((p, j) => (j === i ? { ...p, ...patch } : p)));
  return (
    <div className="talk-people">
      {people.map((p, i) => (
        <div key={i} className="talk-person row wrap">
          <input className="input talk-name" value={p.name} maxLength={32} placeholder={t("Name")} aria-label={t("Name")} onChange={(e) => set(i, { name: e.target.value })} />
          <VoiceSelect value={p.voice ?? null} onChange={(v) => set(i, { voice: v })} voices={voices} own={own} label={t("{name}'s voice", { name: p.name || t("Their") })} pickForMe />
          <input className="input grow" value={p.about ?? ""} maxLength={160} placeholder={t("Who they are (the host, a chef, a skeptic…)")} aria-label={t("Who they are")} onChange={(e) => set(i, { about: e.target.value })} />
          {people.length > 1 && (
            <button className="icon-btn" onClick={() => onChange(people.filter((_, j) => j !== i))} aria-label={t("Remove {name}", { name: p.name || t("this person") })} title={t("Remove")}>
              ✕
            </button>
          )}
        </div>
      ))}
      {people.length < MAX_PEOPLE && (
        <button className="btn ghost small" onClick={() => onChange([...people, { name: "", voice: null, about: "" }])}>{t("+ Add someone")}</button>
      )}
    </div>
  );
}

/** A podcast: what it's about (words, files, the web), who talks in which voice, how long and how. */
export function PodcastComposer({ onStart, onYourVoice }: { onStart: (r: MediaRequest) => Promise<boolean>; onYourVoice: () => void }) {
  const voices = useVoices();
  const ownView = useOwnVoices();
  const own = (ownView?.voices ?? []).filter((v) => v.can_speak);
  const [topic, setTopic] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [research, setResearch] = useState(false);
  const [people, setPeople] = useState<TalkSpeaker[]>([
    { name: "Alex", voice: null, about: t("the host") },
    { name: "Sam", voice: null, about: t("the guest who knows the subject") },
  ]);
  const [minutes, setMinutes] = useState(5);
  const [style, setStyle] = useState<NonNullable<TalkSpec["style"]>>("conversation");
  const [lang, setLang] = useState(talkLanguage());
  const [writer, setWriter] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const addFiles = async () => {
    const picked = await openFile({ multiple: true, filters: [{ name: t("Documents"), extensions: DOC_TYPES }] });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    setFiles((f) => [...f, ...paths.filter((p) => !f.includes(p))].slice(0, 10));
  };

  const named = people.filter((p) => p.name.trim());
  const ready = (topic.trim() || files.length > 0) && named.length > 0 && !busy;
  const go = async () => {
    setBusy(true);
    const talk: TalkSpec = { speakers: named.map((p) => ({ ...p, name: p.name.trim() })), minutes, style, research, files, writer };
    const ok = await onStart({ op: "podcast", prompt: topic, language: lang, talk });
    setBusy(false);
    if (ok) {
      setTopic("");
      setFiles([]);
    }
  };
  const usesOwn = people.some((p) => p.voice?.startsWith("own:"));

  return (
    <div className="form podcast">
      <textarea
        className="input"
        rows={4}
        value={topic}
        placeholder={t("What it's about: a topic, your notes, or the points to cover. Add files below for it to talk about them.")}
        aria-label={t("What the podcast is about")}
        onChange={(e) => setTopic(e.target.value)}
        autoFocus
      />
      <div className="row wrap">
        <button className="btn small" onClick={addFiles}>{t("Add files…")}</button>
        {files.map((f) => (
          <span key={f} className="chip">
            {f.split(/[\\/]/).pop()}
            <button className="link small" onClick={() => setFiles(files.filter((x) => x !== f))} aria-label={t("Remove {name}", { name: f.split(/[\\/]/).pop() ?? "" })}>✕</button>
          </span>
        ))}
      </div>
      <label className="check small">
        <input type="checkbox" checked={research} onChange={(e) => setResearch(e.target.checked)} />
        {t("Look it up on the web first")}
      </label>
      {research && <p className="small muted">{t("A few searches go to your search engine and the top pages are read, for facts to talk about. They're listed under What left this PC.")}</p>}

      <h3>{t("Who talks")}</h3>
      <People people={people} onChange={setPeople} voices={voices} own={own} />
      {own.length === 0 && <button className="link small" onClick={onYourVoice}>{t("Use your own voice, or make one from a description…")}</button>}

      <div className="row wrap">
        <label className="small">
          {t("Length")}
          <select value={minutes} onChange={(e) => setMinutes(Number(e.target.value))}>
            {LENGTHS.map((m) => (
              <option key={m} value={m}>{t("about {m} min", { m })}</option>
            ))}
          </select>
        </label>
        <label className="small">
          {t("Kind")}
          <select value={style} onChange={(e) => setStyle(e.target.value as NonNullable<TalkSpec["style"]>)}>
            {STYLES.map(([id, label]) => (
              <option key={id} value={id}>{t(label)}</option>
            ))}
          </select>
        </label>
        <label className="small">
          {t("Language")}
          <LanguageSelect value={lang} onChange={setLang} />
        </label>
      </div>
      <label className="small">
        {t("Written by")}
        <ModelSelect value={writer} onChange={setWriter} label={t("The model that writes it")} />
      </label>
      <div className="row">
        <span className="small muted">{t("It's written first, then each line is read in its person's voice.")}</span>
        <span className="spacer" />
        <button className="btn primary" disabled={!ready} onClick={go}>{t("Make the podcast")}</button>
      </div>
      {usesOwn && (ownView?.speak_size ?? 0) > 0 && (
        <p className="small muted">{t("Read in your voices. The first time, the voice engine and its model download ({size}).", { size: bytes(ownView!.speak_size) })}</p>
      )}
    </div>
  );
}

/** For a script read in several voices: a voice for each name it uses. */
export function ScriptVoices({ script, picks, onPick, voices, own }: {
  script: string;
  picks: Record<string, string | null>;
  onPick: (name: string, voice: string | null) => void;
  voices: Voice[];
  own: OwnVoice[];
}) {
  const names = namesIn(script);
  if (names.length === 0) return <p className="small muted">{t("Start each line with who says it, like “Ana: Hello!”, and pick a voice for each person here.")}</p>;
  return (
    <div className="talk-people">
      {names.map((n) => (
        <div key={n} className="talk-person row">
          <strong className="talk-name">{n}</strong>
          <VoiceSelect value={picks[n] ?? null} onChange={(v) => onPick(n, v)} voices={voices} own={own} label={t("{name}'s voice", { name: n })} pickForMe />
        </div>
      ))}
    </div>
  );
}
