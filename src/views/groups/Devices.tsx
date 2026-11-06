// SPDX-License-Identifier: AGPL-3.0-only
// Which microphone, speakers and camera calls use, kept on this PC; and
// each person's volume and "mute for me", which only this PC hears.
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { t } from "../../i18n";
import { useDismiss } from "../../useDismiss";

export type DeviceChoice = { mic: string; speaker: string; camera: string };
const KEY = "sulcus.devices";
const SEEN = "sulcus.devicesSeen";
const VOLUMES = "sulcus.volumes";
export const DEVICES_EVENT = "sulcus:devices";

function read<T>(key: string, empty: T): T {
  try {
    return { ...empty, ...JSON.parse(localStorage.getItem(key) ?? "null") };
  } catch {
    return empty;
  }
}

function write(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    // Kept for this session only.
  }
}

export const devices = (): DeviceChoice => read(KEY, { mic: "", speaker: "", camera: "" });

export function setDevices(d: Partial<DeviceChoice>) {
  write(KEY, { ...devices(), ...d });
  window.dispatchEvent(new Event(DEVICES_EVENT));
}

export const micConstraint = (): MediaTrackConstraints => ({ echoCancellation: true, noiseSuppression: true, ...(devices().mic ? { deviceId: { ideal: devices().mic } } : {}) });
export const cameraConstraint = (): MediaTrackConstraints => ({ width: 640, height: 360, ...(devices().camera ? { deviceId: { ideal: devices().camera } } : {}) });

/** Each person's volume for this PC only: 0 is muted for me, 1 normal, up to 2. */
export const volumes = (): Record<string, number> => read(VOLUMES, {} as Record<string, number>);
export function setVolume(who: string, v: number) {
  write(VOLUMES, { ...volumes(), [who]: Math.max(0, Math.min(2, v)) });
  window.dispatchEvent(new Event(DEVICES_EVENT));
}

async function list(): Promise<MediaDeviceInfo[]> {
  let all = await navigator.mediaDevices.enumerateDevices();
  // Names show only after the microphone has been allowed once.
  if (all.some((d) => d.kind === "audioinput" && !d.label)) {
    try {
      const s = await navigator.mediaDevices.getUserMedia({ audio: true });
      s.getTracks().forEach((tr) => tr.stop());
      all = await navigator.mediaDevices.enumerateDevices();
    } catch {
      // Listed without names.
    }
  }
  return all;
}

/** Voice and video settings: microphone (with a level meter), speakers, camera. */
export function DevicesDialog({ onClose, extra }: { onClose: () => void; extra?: React.ReactNode }) {
  const [all, setAll] = useState<MediaDeviceInfo[]>([]);
  const [d, setD] = useState(devices);
  const [level, setLevel] = useState(0);
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box);
  useEffect(() => {
    const load = () => list().then(setAll).catch(() => setAll([]));
    load();
    navigator.mediaDevices.addEventListener("devicechange", load);
    return () => navigator.mediaDevices.removeEventListener("devicechange", load);
  }, []);
  // A live meter for the chosen microphone, so it's easy to tell it works.
  useEffect(() => {
    let stop = () => {};
    navigator.mediaDevices
      .getUserMedia({ audio: micConstraint() })
      .then((s) => {
        const ctx = new AudioContext();
        const an = ctx.createAnalyser();
        ctx.createMediaStreamSource(s).connect(an);
        const buf = new Uint8Array(an.fftSize);
        const id = window.setInterval(() => {
          an.getByteTimeDomainData(buf);
          let sum = 0;
          for (const v of buf) sum += ((v - 128) / 128) ** 2;
          setLevel(Math.min(1, Math.sqrt(sum / buf.length) * 4));
        }, 80);
        stop = () => {
          window.clearInterval(id);
          s.getTracks().forEach((tr) => tr.stop());
          ctx.close().catch(() => {});
        };
      })
      .catch(() => setLevel(0));
    return () => stop();
  }, [d.mic]);
  const pick = (k: keyof DeviceChoice, v: string) => {
    setD((x) => ({ ...x, [k]: v }));
    setDevices({ [k]: v });
  };
  const options = (kind: MediaDeviceKind) => all.filter((x) => x.kind === kind && x.deviceId && x.deviceId !== "default" && x.deviceId !== "communications");
  const canPickSpeaker = "setSinkId" in HTMLMediaElement.prototype;
  return createPortal(
    <div className="social-modal" role="dialog" aria-label={t("Voice and video")}>
      <div className="social-modal-box devices-box" ref={box}>
        <h3>{t("Voice and video")}</h3>
        <label>
          {t("Microphone")}
          <select value={d.mic} onChange={(e) => pick("mic", e.target.value)}>
            <option value="">{t("Windows default")}</option>
            {options("audioinput").map((x) => (
              <option key={x.deviceId} value={x.deviceId}>
                {x.label || t("Microphone")}
              </option>
            ))}
          </select>
        </label>
        <div className="mic-meter" aria-label={t("Microphone level")}>
          <span style={{ width: `${Math.round(level * 100)}%` }} />
        </div>
        <label>
          {t("Speakers or headphones")}
          <select value={d.speaker} disabled={!canPickSpeaker} onChange={(e) => pick("speaker", e.target.value)}>
            <option value="">{t("Windows default")}</option>
            {options("audiooutput").map((x) => (
              <option key={x.deviceId} value={x.deviceId}>
                {x.label || t("Speakers")}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("Camera")}
          <select value={d.camera} onChange={(e) => pick("camera", e.target.value)}>
            <option value="">{t("Windows default")}</option>
            {options("videoinput").map((x) => (
              <option key={x.deviceId} value={x.deviceId}>
                {x.label || t("Camera")}
              </option>
            ))}
          </select>
        </label>
        <p className="small muted">{t("Changes apply at once, also in a call you're in. When a new microphone or headset is plugged in, SulcusAI asks whether to use it.")}</p>
        <div className="row">
          {extra}
          <span className="grow" />
          <button className="primary" onClick={onClose}>
            {t("Done")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** When a microphone or speakers are plugged in: asks whether to use them. */
export function NewDevicePrompt() {
  const [found, setFound] = useState<MediaDeviceInfo | null>(null);
  useEffect(() => {
    const check = async () => {
      const all = await navigator.mediaDevices.enumerateDevices().catch(() => [] as MediaDeviceInfo[]);
      const audio = all.filter((x) => (x.kind === "audioinput" || x.kind === "audiooutput") && x.deviceId && x.deviceId !== "default" && x.deviceId !== "communications");
      const seen: string[] = read(SEEN, { ids: [] as string[] }).ids;
      const fresh = audio.find((x) => !seen.includes(x.deviceId) && x.label);
      write(SEEN, { ids: [...new Set([...seen, ...audio.map((x) => x.deviceId)])] });
      // The first look only learns what's there.
      if (fresh && seen.length > 0) setFound(fresh);
    };
    check();
    navigator.mediaDevices.addEventListener("devicechange", check);
    return () => navigator.mediaDevices.removeEventListener("devicechange", check);
  }, []);
  if (!found) return null;
  const mic = found.kind === "audioinput";
  return createPortal(
    <div className="device-prompt" role="alertdialog" aria-label={t("New device")}>
      <strong>{mic ? t("New microphone found") : t("New speakers or headphones found")}</strong>
      <span className="small">{found.label}</span>
      <div className="row">
        <button
          className="primary small"
          onClick={() => {
            setDevices(mic ? { mic: found.deviceId } : { speaker: found.deviceId });
            setFound(null);
          }}
        >
          {t("Use it")}
        </button>
        <button className="small" onClick={() => setFound(null)}>
          {t("Not now")}
        </button>
      </div>
    </div>,
    document.body,
  );
}
