// SPDX-License-Identifier: AGPL-3.0-only
// Voice settings with a microphone test: a live meter with the gate line,
// record and play back through the same processing a call uses, and a
// suggested sensitivity from the room's own noise. Settings can mount it too.
import { useEffect, useRef, useState } from "react";
import { t } from "../../i18n";
import { social } from "./socialApi";
import { cameraConstraint } from "./Devices";
import { CAMERA_EVENT, cameraSettings, hasRoom, learnRoom, setCameraSettings, type CameraSettings } from "./camera";
import { accelFrom, MicChain, setVoiceSettings, VOICE_EVENT, voiceSettings, type Changer, type Level, type Preset, type VoiceSettings } from "./voice";

const toDb = (x: number) => (x <= 0 ? -100 : 20 * Math.log10(x));
/** -100..0 dB as 0..100% of the meter. */
const pct = (db: number) => Math.max(0, Math.min(100, db + 100));

export function useVoiceSettings(): [VoiceSettings, (v: Partial<VoiceSettings>) => void] {
  const [v, setV] = useState(voiceSettings);
  useEffect(() => {
    const on = () => setV(voiceSettings());
    window.addEventListener(VOICE_EVENT, on);
    return () => window.removeEventListener(VOICE_EVENT, on);
  }, []);
  return [v, setVoiceSettings];
}

export function VoicePanel() {
  const [v, set] = useVoiceSettings();
  const [level, setLevel] = useState<Level | null>(null);
  const [testing, setTesting] = useState(false);
  const [recording, setRecording] = useState(false);
  const [clip, setClip] = useState<string | null>(null);
  const [hear, setHear] = useState(false);
  const [err, setErr] = useState("");
  const [pickKey, setPickKey] = useState(false);
  const chain = useRef<MicChain | null>(null);
  const [ready, setReady] = useState<MicChain | null>(null);
  const floors = useRef<number[]>([]);

  // The test opens the mic with the same chain a call uses.
  useEffect(() => {
    if (!testing) return;
    let alive = true;
    let c: MicChain | null = null;
    MicChain.open(voiceSettings())
      .then((m) => {
        if (!alive) return m.close();
        c = m;
        chain.current = m;
        setReady(m);
        m.onLevel = (l) => {
          setLevel(l);
          floors.current.push(l.floor);
          if (floors.current.length > 100) floors.current.shift();
        };
      })
      .catch((e) => setErr(String(e?.message ?? e)));
    return () => {
      alive = false;
      c?.close();
      chain.current = null;
      setReady(null);
      setLevel(null);
    };
    // Reopened when the preset changes: it picks mono or stereo and the browser's processing.
  }, [testing, v.preset, v.echo, v.agc]);
  useEffect(() => chain.current?.apply(v), [v]);
  // Hearing yourself live: headphones only, or it feeds back.
  useEffect(() => {
    const c = ready;
    if (!c || !hear) return;
    const a = new Audio();
    a.srcObject = c.stream;
    a.play().catch(() => {});
    return () => {
      a.pause();
      a.srcObject = null;
    };
  }, [hear, ready]);

  const record = () => {
    const c = chain.current;
    if (!c) return;
    const rec = new MediaRecorder(c.stream);
    const parts: Blob[] = [];
    rec.ondataavailable = (e) => parts.push(e.data);
    rec.onstop = () => {
      if (clip) URL.revokeObjectURL(clip);
      setClip(URL.createObjectURL(new Blob(parts, { type: rec.mimeType })));
      setRecording(false);
    };
    rec.start();
    setRecording(true);
    window.setTimeout(() => rec.state !== "inactive" && rec.stop(), 5000);
  };

  // A line a little above the room's quietest moments.
  const suggested = floors.current.length ? Math.round(toDb(Math.min(...floors.current)) + 12) : null;

  const setPtt = (on: boolean, key = v.pttKey) => {
    set({ pushToTalk: on, pttKey: key });
    social.callPtt(on ? key : null).catch((e) => setErr(String(e)));
  };

  const check = (label: string, k: keyof VoiceSettings, hint?: string) => (
    <label className="row voice-row" title={hint}>
      <input type="checkbox" checked={!!v[k]} onChange={(e) => set({ [k]: e.target.checked } as Partial<VoiceSettings>)} />
      <span>{label}</span>
      {hint && <span className="small muted">{hint}</span>}
    </label>
  );

  const db = level ? toDb(level.level) : -100;
  return (
    <div className="voice-panel">
      <h3>{t("Voice")}</h3>
      <label className="voice-row">
        {t("Quality")}
        <select value={v.preset} onChange={(e) => set({ preset: e.target.value as Preset })}>
          <option value="voice">{t("Voice: clear speech")}</option>
          <option value="music">{t("Music: no processing, stereo")}</option>
          <option value="lowlag">{t("Gaming: low lag")}</option>
        </select>
      </label>
      {check(t("Echo cancelling and noise suppression"), "echo")}
      {check(t("Automatic gain"), "agc", t("Keeps quiet and loud voices level."))}
      {check(t("Extra noise removal"), "denoise", t("Takes out fans, keys and hum between words."))}
      {v.denoise && (
        <label className="voice-row">
          {t("Strength: {n}%", { n: Math.round(v.denoiseAmount * 100) })}
          <input type="range" min={10} max={100} value={Math.round(v.denoiseAmount * 100)} onChange={(e) => set({ denoiseAmount: Number(e.target.value) / 100 })} />
        </label>
      )}
      {check(t("Only send when I speak"), "gate", t("Sound under the line is not sent. The first word is kept."))}
      <div className="voice-meter" aria-label={t("Microphone level")}>
        <div className={level?.open ? "voice-meter-fill open" : "voice-meter-fill"} style={{ width: `${pct(db)}%` }} />
        {v.gate && <div className="voice-meter-line" style={{ left: `${pct(v.gateDb)}%` }} />}
      </div>
      {v.gate && (
        <label className="voice-row">
          {t("Sensitivity: {n} dB", { n: v.gateDb })}
          <input type="range" min={-90} max={-10} value={v.gateDb} onChange={(e) => set({ gateDb: Number(e.target.value) })} />
          {suggested !== null && (
            <button className="link" onClick={() => set({ gateDb: Math.max(-90, Math.min(-10, suggested)) })}>
              {t("Use suggested ({n} dB)", { n: suggested })}
            </button>
          )}
        </label>
      )}
      <label className="row voice-row">
        <input type="checkbox" checked={v.pushToTalk} onChange={(e) => setPtt(e.target.checked)} />
        <span>{t("Push to talk")}</span>
        {v.pushToTalk && (
          <button
            onKeyDown={(e) => {
              if (!pickKey) return;
              e.preventDefault();
              const a = accelFrom(e.nativeEvent);
              if (a) {
                setPickKey(false);
                setPtt(true, a);
              }
            }}
            onClick={() => setPickKey(true)}
          >
            {pickKey ? t("Press a key...") : t("Key: {key}", { key: v.pttKey })}
          </button>
        )}
      </label>
      {check(t("Remind me when I talk while muted"), "muteReminder")}
      {check(t("Place people left and right"), "spatial", t("Follows their place among the tiles."))}
      {check(t("Lower shared sound when someone talks"), "ducking")}
      {check(t("Bitrate follows the connection"), "adaptive", t("Higher and stereo when it allows, lower when it doesn't."))}

      <h4>{t("Voice changer")}</h4>
      <p className="small muted">{t("Runs on this PC. Only the changed voice leaves it.")}</p>
      <div className="row wrap">
        {(["off", "deep", "chipmunk", "robot", "radio", "pitch", "custom"] as Changer[]).map((c) => (
          <button key={c} className={v.changer === c ? "chip on" : "chip"} onClick={() => set({ changer: c })}>
            {changerName(c)}
          </button>
        ))}
      </div>
      {(v.changer === "pitch" || v.changer === "custom") && (
        <label className="voice-row">
          {t("Pitch: {n} semitones", { n: v.semitones })}
          <input type="range" min={-12} max={12} value={v.semitones} onChange={(e) => set({ semitones: Number(e.target.value) })} />
        </label>
      )}
      {v.changer === "custom" && (
        <>
          <label className="voice-row">
            {t("Robot: {n}%", { n: Math.round(v.robot * 100) })}
            <input type="range" min={0} max={100} value={Math.round(v.robot * 100)} onChange={(e) => set({ robot: Number(e.target.value) / 100 })} />
          </label>
          {check(t("Radio"), "radio")}
        </>
      )}
      {check(t("Show others a \"voice changed\" mark"), "markChanged")}

      <h4>{t("Microphone test")}</h4>
      <div className="row wrap">
        <button className={testing ? "primary" : ""} onClick={() => setTesting((x) => !x)}>
          {testing ? t("Stop test") : t("Start test")}
        </button>
        <button disabled={!testing || recording} onClick={record}>
          {recording ? t("Recording 5 s...") : t("Record and play back")}
        </button>
        <label className="row">
          <input type="checkbox" checked={hear} disabled={!testing} onChange={(e) => setHear(e.target.checked)} />
          <span>{t("Hear myself (headphones)")}</span>
        </label>
      </div>
      {clip && <audio src={clip} controls autoPlay />}
      {err && <p className="small danger">{err}</p>}
      <CameraPanel />
    </div>
  );
}

export function changerName(c: Changer): string {
  return { off: t("Off"), deep: t("Deep"), chipmunk: t("Chipmunk"), robot: t("Robot"), radio: t("Radio"), pitch: t("Pitch"), custom: t("Custom") }[c];
}

function CameraPanel() {
  const [c, setC] = useState(cameraSettings);
  const [learning, setLearning] = useState<number | null>(null);
  const [learned, setLearned] = useState(hasRoom);
  useEffect(() => {
    const on = () => setC(cameraSettings());
    window.addEventListener(CAMERA_EVENT, on);
    return () => window.removeEventListener(CAMERA_EVENT, on);
  }, []);
  const set = (x: Partial<CameraSettings>) => setCameraSettings(x);
  const canFrame = "FaceDetector" in window;
  // A short count so there's time to step out of view.
  const learn = async () => {
    let s: MediaStream | null = null;
    try {
      s = await navigator.mediaDevices.getUserMedia({ video: cameraConstraint() });
      for (let n = 5; n > 0; n--) {
        setLearning(n);
        await new Promise((r) => setTimeout(r, 1000));
      }
      setLearning(0);
      await learnRoom(s);
      setLearned(true);
    } finally {
      s?.getTracks().forEach((tr) => tr.stop());
      setLearning(null);
    }
  };
  return (
    <>
      <h4>{t("Camera")}</h4>
      <label className="voice-row">
        {t("Background")}
        <select value={c.background} onChange={(e) => set({ background: e.target.value as CameraSettings["background"] })}>
          <option value="off">{t("As it is")}</option>
          <option value="blur">{t("Blur")}</option>
          <option value="picture">{t("A picture")}</option>
        </select>
      </label>
      {c.background === "picture" && (
        <label className="voice-row">
          {t("Picture")}
          <input
            type="file"
            accept="image/*"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (!f) return;
              const r = new FileReader();
              r.onload = () => set({ picture: String(r.result) });
              r.readAsDataURL(f);
            }}
          />
        </label>
      )}
      {c.background !== "off" && (
        <div className="voice-row">
          <button onClick={learn} disabled={learning !== null}>
            {learning === null ? (learned ? t("Learn the empty room again") : t("Learn the empty room")) : learning > 0 ? t("Step out of view: {n}", { n: learning }) : t("Looking...")}
          </button>
          <span className="small muted">{t("Blur uses the camera's own when it has one. Otherwise, and for a picture, the empty room tells you apart from it; learn it again if the light or the camera moves.")}</span>
        </div>
      )}
      <label className="row voice-row">
        <input type="checkbox" checked={c.lighting} onChange={(e) => set({ lighting: e.target.checked })} />
        <span>{t("Lighting fix")}</span>
      </label>
      <label className="row voice-row" title={canFrame ? undefined : t("Face finding isn't available on this PC.")}>
        <input type="checkbox" checked={c.framing} disabled={!canFrame} onChange={(e) => set({ framing: e.target.checked })} />
        <span>{t("Framing follows my face")}</span>
      </label>
      <label className="row voice-row">
        <input type="checkbox" checked={c.mirror} onChange={(e) => set({ mirror: e.target.checked })} />
        <span>{t("Mirror my own view")}</span>
      </label>
    </>
  );
}
