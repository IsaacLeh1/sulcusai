// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on } from "../api";
import { t } from "../i18n";
import { useOwnVoices } from "./OwnVoice";
import type { PushToast } from "./Toasts";

type LiveState = "off" | "loading" | "on";

/** Studio › Your voice: talk into the microphone and hear yourself in another voice. */
export function LiveVoice({ toast }: { toast: PushToast }) {
  const own = useOwnVoices();
  const voices = (own?.voices ?? []).filter((v) => v.can_speak || v.can_sing);
  const [voice, setVoice] = useState("");
  const [semitones, setSemitones] = useState(0);
  const [state, setState] = useState<LiveState>("off");
  const [level, setLevel] = useState(0);
  const [behind, setBehind] = useState(false);

  useEffect(() => {
    const sub = on("live_voice", (e) => {
      if (e.state === "loading") setState("loading");
      else if (e.state === "on") setState("on");
      else if (e.state === "level") {
        setLevel(e.level ?? 0);
        setBehind(!!e.behind);
      } else if (e.state === "error") {
        setState("off");
        toast(errorText(e.error), "error");
      } else if (e.state === "stopped") setState("off");
    });
    return () => {
      sub.then((un) => un());
    };
  }, [toast]);
  useEffect(() => {
    if (!voice && voices[0]) setVoice(voices[0].id);
  }, [voice, voices]);

  const start = () => api.liveVoiceStart(voice, semitones).catch((e) => toast(errorText(e), "error"));
  return (
    <section className="card">
      <h2>{t("Live voice changer")}</h2>
      <p className="muted small">
        {t("Talk into your microphone and hear yourself in one of these voices, about a second and a half later. It runs on your graphics card, here on this PC. Use headphones so the microphone doesn't hear the speakers. To use it in a call, set the call's microphone to a virtual audio cable and Windows' sound output to that cable.")}
      </p>
      {voices.length === 0 ? (
        <p className="muted small">{t("Add a voice above first: record or upload one, or make one from words.")}</p>
      ) : (
        <div className="row wrap">
          <select value={voice} onChange={(e) => setVoice(e.target.value)} disabled={state !== "off"} aria-label={t("Voice")}>
            {voices.map((v) => (
              <option key={v.id} value={v.id}>
                {v.name}
              </option>
            ))}
          </select>
          <label className="check small">
            {t("Pitch")}
            <input type="range" min={-12} max={12} step={1} value={semitones} disabled={state !== "off"} onChange={(e) => setSemitones(+e.target.value)} aria-label={t("Pitch")} />
            <span className="muted">{semitones > 0 ? `+${semitones}` : semitones}</span>
          </label>
          <span className="spacer" />
          {state === "off" ? (
            <button className="btn primary" disabled={!voice} onClick={start}>
              {t("Start")}
            </button>
          ) : (
            <button className="btn" onClick={() => api.liveVoiceStop()}>
              {state === "loading" ? t("Getting ready…") : t("Stop")}
            </button>
          )}
        </div>
      )}
      {state === "on" && (
        <p className="small muted">
          <span className="voice-level" style={{ display: "inline-block", width: `${Math.min(100, level * 800)}px`, height: 6, background: "var(--accent)", borderRadius: 3, marginRight: 8 }} />
          {behind ? t("This graphics card is falling behind: words may break up.") : t("On: talk, and you'll hear the new voice.")}
        </p>
      )}
      <p className="muted small">{t("Pitch moves the voice up or down: +12 for a man's voice sounding like a woman's, -12 the other way.")}</p>
    </section>
  );
}
