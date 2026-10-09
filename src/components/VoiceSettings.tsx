// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Voice and meetings: microphone, spoken language, voice, models, recordings.
import { useEffect, useState } from "react";
import { api, errorText, type AudioDevice, type SpeechView, type Voice, type VoiceSettings } from "../api";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

/** Languages speech recognition is set to; "auto" detects each time. */
const SPOKEN = [
  ["auto", tx("Detect automatically")], ["en", tx("English")], ["es", tx("Spanish")], ["fr", tx("French")], ["de", tx("German")],
  ["it", tx("Italian")], ["pt", tx("Portuguese")], ["nl", tx("Dutch")], ["pl", tx("Polish")], ["ru", tx("Russian")], ["uk", tx("Ukrainian")],
  ["tr", tx("Turkish")], ["ar", tx("Arabic")], ["hi", tx("Hindi")], ["zh", tx("Chinese")], ["ja", tx("Japanese")], ["ko", tx("Korean")],
  ["vi", tx("Vietnamese")], ["sv", tx("Swedish")], ["da", tx("Danish")], ["fi", tx("Finnish")], ["cs", tx("Czech")], ["el", tx("Greek")],
];

export function VoiceSection({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<VoiceSettings | null>(null);
  const [mics, setMics] = useState<AudioDevice[]>([]);
  const [voices, setVoices] = useState<Voice[]>([]);
  const [speech, setSpeech] = useState<SpeechView | null>(null);

  useEffect(() => {
    api.voiceSettings().then(setS);
    api.audioDevices().then((d) => setMics(d.inputs)).catch(() => {});
    api.voices().then(setVoices).catch(() => {});
    api.speechView().then(setSpeech).catch(() => {});
  }, []);

  const save = async (next: VoiceSettings) => {
    setS(next);
    try {
      setS(await api.setVoiceSettings(next));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  if (!s) return null;
  const installed = (speech?.models ?? []).filter((m) => m.installed);

  return (
    <section className="card">
      <h2>{t("Voice and meetings")}</h2>
      <p className="muted small">{t("Speech is recognized and spoken on this PC. Audio never leaves it.")}</p>
      <div className="form">
        <label>
          {t("Microphone")}
          <select value={s.mic ?? ""} onChange={(e) => save({ ...s, mic: e.target.value || null })}>
            <option value="">{mics.find((m) => m.default) ? t("Windows default ({name})", { name: mics.find((m) => m.default)!.name }) : t("Windows default")}</option>
            {mics.map((m) => (
              <option key={m.id} value={m.id}>{m.name}</option>
            ))}
          </select>
        </label>
        <label className="check">
          <input type="checkbox" checked={s.voice_processing} onChange={(e) => save({ ...s, voice_processing: e.target.checked })} />
          <span>
            {t("Echo cancellation and noise suppression")}
            <span className="muted small block">{t("Uses Windows' voice processing where your PC has it. Helps when you use speakers instead of headphones.")}</span>
          </span>
        </label>
        <label>
          {t("Language you speak")}
          <select value={s.language} onChange={(e) => save({ ...s, language: e.target.value })}>
            {SPOKEN.map(([code, name]) => (
              <option key={code} value={code}>{t(name)}</option>
            ))}
          </select>
        </label>
        {installed.length > 1 && (
          <div className="two-col">
            <label>
              {t("Model for meetings")}
              <select value={s.speech_model ?? ""} onChange={(e) => save({ ...s, speech_model: e.target.value || null })}>
                <option value="">{t("Automatic (most accurate)")}</option>
                {installed.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
              </select>
            </label>
            <label>
              {t("Model for dictation and voice chats")}
              <select value={s.live_model ?? ""} onChange={(e) => save({ ...s, live_model: e.target.value || null })}>
                <option value="">{t("Automatic (quick)")}</option>
                {installed.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
              </select>
            </label>
          </div>
        )}
        <div className="two-col">
          <label>
            {t("Voice for spoken replies")}
            <select value={s.voice ?? ""} onChange={(e) => save({ ...s, voice: e.target.value || null })}>
              <option value="">{t("Automatic (natural if installed, matching the language)")}</option>
              {voices.map((v) => (
                <option key={v.id} value={v.id}>{v.name} ({v.language})</option>
              ))}
            </select>
          </label>
          <label>
            {t("Speaking speed: {rate}×", { rate: s.rate.toFixed(1) })}
            <input type="range" min={0.6} max={2} step={0.1} value={s.rate} onChange={(e) => setS({ ...s, rate: Number(e.target.value) })} onMouseUp={() => save(s)} onKeyUp={() => save(s)} aria-label={t("Speaking speed")} />
          </label>
        </div>
        <div className="row">
          <button className="btn small" onClick={() => api.speak(t("Hi! This is how I'll sound when I read replies aloud.")).catch((e) => toast(errorText(e), "error"))}>
            {t("🔊 Test voice")}
          </button>
          <span className="muted small">{t("For natural voices, install them from Models › Speech.")}</span>
        </div>
        <label className="check">
          <input type="checkbox" checked={s.keep_meeting_audio} onChange={(e) => save({ ...s, keep_meeting_audio: e.target.checked })} />
          <span>
            {t("Keep meeting recordings")}
            <span className="muted small block">{t("Saved encrypted, so you can replay any line of a transcript. About 230 MB per hour of meeting. Turn off to keep only the transcript and notes.")}</span>
          </span>
        </label>
      </div>
    </section>
  );
}
