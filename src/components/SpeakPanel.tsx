// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, type MediaItem, type MediaRequest, type Voice } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { useOwnVoices } from "./OwnVoice";
import { LanguageSelect, talkLanguage, VoiceSelect } from "./Podcast";

/** The longest voice a picture talks or sings to (media/mod.rs SPEAK_SECS). */
const LONGEST = 60;

/** A picture made to talk or sing (Wan 2.2 S2V): to words read aloud in a
 * voice, a song or sound from the gallery, or a sound file. */
export function SpeakPanel({ run }: { run: (r: Omit<MediaRequest, "source">) => void }) {
  const [from, setFrom] = useState<"words" | "sound" | "file">("words");
  const [words, setWords] = useState("");
  const [voice, setVoice] = useState<string | null>(null);
  const [voices, setVoices] = useState<Voice[]>([]);
  const own = (useOwnVoices()?.voices ?? []).filter((v) => v.can_speak);
  const [language, setLanguage] = useState(talkLanguage());
  const [sounds, setSounds] = useState<MediaItem[]>([]);
  const [sound, setSound] = useState("");
  const [file, setFile] = useState<string | null>(null);
  const [described, setDescribed] = useState("");
  useEffect(() => {
    api.voices().then(setVoices).catch(() => {});
    api.mediaList("audio", null, 200).then((a) => setSounds(a ?? [])).catch(() => {});
  }, []);
  const picked = sounds.find((s) => s.id === sound);
  // About 2.5 words a second read aloud.
  const seconds = from === "words" ? words.trim().split(/\s+/).filter(Boolean).length / 2.5 : from === "sound" ? picked?.seconds ?? 0 : null;
  const ready = from === "words" ? !!words.trim() : from === "sound" ? !!sound : !!file;
  const go = () =>
    run({
      op: "speak",
      prompt: described,
      say: from === "words" ? words : null,
      sound: from === "sound" ? sound : null,
      sound_file: from === "file" ? file : null,
      voice: from === "words" ? voice : null,
      language,
      seconds: seconds ? Math.min(LONGEST, Math.max(1, seconds)) : undefined,
    });
  return (
    <div className="form speak-panel">
      <div className="filters tight">
        <button className={`chip ${from === "words" ? "active" : ""}`} onClick={() => setFrom("words")}>{t("Words")}</button>
        <button className={`chip ${from === "sound" ? "active" : ""}`} onClick={() => setFrom("sound")}>{t("A song or sound")}</button>
        <button className={`chip ${from === "file" ? "active" : ""}`} onClick={() => setFrom("file")}>{t("A file")}</button>
      </div>
      {from === "words" && (
        <>
          <textarea className="input" rows={3} value={words} onChange={(e) => setWords(e.target.value)} placeholder={t("What it says")} aria-label={t("What it says")} />
          <div className="ve-keep small">
            <span className="ve-bound">
              {t("Voice")}
              <VoiceSelect value={voice} onChange={setVoice} voices={voices} own={own} label={t("Voice")} pickForMe />
            </span>
            <LanguageSelect value={language} onChange={setLanguage} />
          </div>
        </>
      )}
      {from === "sound" && (
        <select value={sound} onChange={(e) => setSound(e.target.value)} aria-label={t("A song or sound")}>
          <option value="">{sounds.length ? t("Pick one…") : t("No songs or sounds in the gallery yet")}</option>
          {sounds.map((s) => (
            <option key={s.id} value={s.id}>{(s.title || s.prompt || t("A sound")).slice(0, 60)} · {clock(s.seconds)}</option>
          ))}
        </select>
      )}
      {from === "file" && (
        <div className="row">
          <button
            className="btn small"
            onClick={async () => {
              const p = await openFile({ multiple: false, filters: [{ name: t("Sound"), extensions: ["mp3", "wav", "m4a", "ogg", "flac", "aac"] }] });
              if (typeof p === "string") setFile(p);
            }}
          >
            {t("Pick a file…")}
          </button>
          {file && <span className="small muted">{file.split(/[\\/]/).pop()}</span>}
        </div>
      )}
      <input className="input" value={described} onChange={(e) => setDescribed(e.target.value)} placeholder={t("How it looks while it talks (optional), e.g. smiling, nodding")} aria-label={t("How it looks while it talks")} />
      <p className="small muted">
        {t("Its lips and face move to the voice, about 5 seconds at a time. It's slow: each 5 seconds can take half an hour on a 12 GB graphics card (less on bigger ones). Up to {n} seconds.", { n: LONGEST })}
        {seconds != null && seconds > LONGEST && " " + t("This one is longer, so it stops at {n} seconds.", { n: LONGEST })}
      </p>
      <div className="row">
        <span className="spacer" />
        <button className="btn primary small" disabled={!ready} onClick={go}>{t("Make it talk")}</button>
      </div>
    </div>
  );
}
