// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Voice and meetings: microphone, spoken language, voice, models, recordings.
import { useEffect, useState } from "react";
import { api, errorText, type AudioDevice, type SpeechView, type Voice, type VoiceSettings } from "../api";
import type { PushToast } from "./Toasts";

/** Languages speech recognition is set to; "auto" detects each time. */
const SPOKEN = [
  ["auto", "Detect automatically"], ["en", "English"], ["es", "Spanish"], ["fr", "French"], ["de", "German"],
  ["it", "Italian"], ["pt", "Portuguese"], ["nl", "Dutch"], ["pl", "Polish"], ["ru", "Russian"], ["uk", "Ukrainian"],
  ["tr", "Turkish"], ["ar", "Arabic"], ["hi", "Hindi"], ["zh", "Chinese"], ["ja", "Japanese"], ["ko", "Korean"],
  ["vi", "Vietnamese"], ["sv", "Swedish"], ["da", "Danish"], ["fi", "Finnish"], ["cs", "Czech"], ["el", "Greek"],
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
      <h2>Voice and meetings</h2>
      <p className="muted small">Speech is recognized and spoken on this PC. Audio never leaves it.</p>
      <div className="form">
        <label>
          Microphone
          <select value={s.mic ?? ""} onChange={(e) => save({ ...s, mic: e.target.value || null })}>
            <option value="">Windows default{mics.find((m) => m.default) ? ` (${mics.find((m) => m.default)!.name})` : ""}</option>
            {mics.map((m) => (
              <option key={m.id} value={m.id}>{m.name}</option>
            ))}
          </select>
        </label>
        <label className="check">
          <input type="checkbox" checked={s.voice_processing} onChange={(e) => save({ ...s, voice_processing: e.target.checked })} />
          <span>
            Echo cancellation and noise suppression
            <span className="muted small block">Uses Windows' voice processing where your PC has it. Helps when you use speakers instead of headphones.</span>
          </span>
        </label>
        <label>
          Language you speak
          <select value={s.language} onChange={(e) => save({ ...s, language: e.target.value })}>
            {SPOKEN.map(([code, name]) => (
              <option key={code} value={code}>{name}</option>
            ))}
          </select>
        </label>
        {installed.length > 1 && (
          <div className="two-col">
            <label>
              Model for meetings
              <select value={s.speech_model ?? ""} onChange={(e) => save({ ...s, speech_model: e.target.value || null })}>
                <option value="">Automatic (most accurate)</option>
                {installed.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
              </select>
            </label>
            <label>
              Model for dictation and voice chats
              <select value={s.live_model ?? ""} onChange={(e) => save({ ...s, live_model: e.target.value || null })}>
                <option value="">Automatic (quick)</option>
                {installed.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
              </select>
            </label>
          </div>
        )}
        <div className="two-col">
          <label>
            Voice for spoken replies
            <select value={s.voice ?? ""} onChange={(e) => save({ ...s, voice: e.target.value || null })}>
              <option value="">Match the language</option>
              {voices.map((v) => (
                <option key={v.id} value={v.id}>{v.name} ({v.language})</option>
              ))}
            </select>
          </label>
          <label>
            Speaking speed: {s.rate.toFixed(1)}×
            <input type="range" min={0.6} max={2} step={0.1} value={s.rate} onChange={(e) => setS({ ...s, rate: Number(e.target.value) })} onMouseUp={() => save(s)} onKeyUp={() => save(s)} aria-label="Speaking speed" />
          </label>
        </div>
        <div className="row">
          <button className="btn small" onClick={() => api.speak("Hi! This is how I'll sound when I read replies aloud.").catch((e) => toast(errorText(e), "error"))}>
            🔊 Test voice
          </button>
          <span className="muted small">More voices: Windows Settings › Time &amp; language › Speech › Add voices.</span>
        </div>
        <label className="check">
          <input type="checkbox" checked={s.keep_meeting_audio} onChange={(e) => save({ ...s, keep_meeting_audio: e.target.checked })} />
          <span>
            Keep meeting recordings
            <span className="muted small block">Saved encrypted, so you can replay any line of a transcript. About 230 MB per hour of meeting. Turn off to keep only the transcript and notes.</span>
          </span>
        </label>
      </div>
    </section>
  );
}
