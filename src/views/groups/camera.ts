// SPDX-License-Identifier: AGPL-3.0-only
// Camera effects, all on this PC: background blur or a picture behind you,
// lighting fix, framing that follows your face, and a mirrored own view.
// Without a segmentation model the background is learned: a frame of the
// empty room, and whatever differs from it is you.

export type CameraSettings = {
  background: "off" | "blur" | "picture";
  /** A data URL of the picture to put behind you. */
  picture: string;
  lighting: boolean;
  framing: boolean;
  mirror: boolean;
};

const KEY = "sulcus.camera";
const ROOM = "sulcus.cameraRoom";
export const CAMERA_EVENT = "sulcus:camera";
export const CAMERA_DEFAULTS: CameraSettings = { background: "off", picture: "", lighting: false, framing: false, mirror: false };

export function cameraSettings(): CameraSettings {
  try {
    return { ...CAMERA_DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "null") };
  } catch {
    return { ...CAMERA_DEFAULTS };
  }
}

export function setCameraSettings(c: Partial<CameraSettings>) {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...cameraSettings(), ...c }));
  } catch {
    // Kept for this session only.
  }
  window.dispatchEvent(new Event(CAMERA_EVENT));
}

/** Any effect that needs the frames redrawn before sending. */
export const needsFx = (c: CameraSettings) => c.background !== "off" || c.lighting || c.framing;

// The empty room, small: enough to tell you from it.
const MW = 160;
const MH = 90;

export function hasRoom(): boolean {
  try {
    return !!localStorage.getItem(ROOM);
  } catch {
    return false;
  }
}

/** Learns the empty room from a camera stream: call with you out of view. */
export async function learnRoom(stream: MediaStream): Promise<void> {
  const v = document.createElement("video");
  v.muted = true;
  v.srcObject = stream;
  await v.play();
  await new Promise((r) => setTimeout(r, 500));
  const c = document.createElement("canvas");
  c.width = MW;
  c.height = MH;
  const g = c.getContext("2d")!;
  // Several frames averaged so camera noise doesn't count as you.
  const sum = new Float32Array(MW * MH * 3);
  for (let n = 0; n < 8; n++) {
    g.drawImage(v, 0, 0, MW, MH);
    const d = g.getImageData(0, 0, MW, MH).data;
    for (let i = 0, j = 0; i < d.length; i += 4, j += 3) {
      sum[j] += d[i];
      sum[j + 1] += d[i + 1];
      sum[j + 2] += d[i + 2];
    }
    await new Promise((r) => setTimeout(r, 80));
  }
  const out = Array.from(sum, (x) => Math.round(x / 8));
  v.srcObject = null;
  try {
    localStorage.setItem(ROOM, JSON.stringify(out));
  } catch {
    // Too big for this PC's storage: it'll be asked again next time.
  }
}

function room(): Uint8ClampedArray | null {
  try {
    const r = JSON.parse(localStorage.getItem(ROOM) ?? "null");
    return Array.isArray(r) ? new Uint8ClampedArray(r) : null;
  } catch {
    return null;
  }
}

type Face = { boundingBox: DOMRectReadOnly };
type FaceDetectorCtor = new (o?: { fastMode?: boolean; maxDetectedFaces?: number }) => { detect: (s: CanvasImageSource) => Promise<Face[]> };

/** Asks the camera itself to blur (Windows camera effects) when it can. */
export async function nativeBlur(track: MediaStreamTrack, on: boolean): Promise<boolean> {
  const caps = (track.getCapabilities?.() ?? {}) as { backgroundBlur?: boolean[] };
  if (!caps.backgroundBlur?.includes(on)) return false;
  try {
    await track.applyConstraints({ advanced: [{ backgroundBlur: on } as MediaTrackConstraintSet] });
    return true;
  } catch {
    return false;
  }
}

/** The camera with effects drawn in, as a stream to send. */
export function cameraFx(src: MediaStream, s: CameraSettings, fps = 30): { stream: MediaStream; stop: () => void } {
  const video = document.createElement("video");
  video.muted = true;
  video.srcObject = src;
  video.play().catch(() => {});
  const canvas = document.createElement("canvas");
  const g = canvas.getContext("2d")!;
  const small = document.createElement("canvas");
  small.width = MW;
  small.height = MH;
  const sg = small.getContext("2d", { willReadFrequently: true })!;
  const mask = document.createElement("canvas");
  mask.width = MW;
  mask.height = MH;
  const mg = mask.getContext("2d")!;
  const maskData = mg.createImageData(MW, MH);
  const layer = document.createElement("canvas");
  const lg = layer.getContext("2d")!;
  const empty = room();
  let pic: HTMLImageElement | null = null;
  if (s.background === "picture" && s.picture) {
    pic = new Image();
    pic.src = s.picture;
  }
  const Detector = (window as unknown as { FaceDetector?: FaceDetectorCtor }).FaceDetector;
  const detector = s.framing && Detector ? new Detector({ fastMode: true, maxDetectedFaces: 1 }) : null;
  // The crop follows the face slowly, so it doesn't jump.
  const view = { x: 0, y: 0, w: 1, h: 1 };
  let target = { ...view };
  let detecting = false;
  let bright = 1;
  let frame = 0;

  const id = window.setInterval(() => {
    const vw = video.videoWidth;
    const vh = video.videoHeight;
    if (!vw || !vh) return;
    if (canvas.width !== vw || canvas.height !== vh) {
      canvas.width = layer.width = vw;
      canvas.height = layer.height = vh;
    }
    frame++;
    sg.drawImage(video, 0, 0, MW, MH);
    const px = sg.getImageData(0, 0, MW, MH).data;

    if (s.lighting && frame % 10 === 0) {
      let sum = 0;
      for (let i = 0; i < px.length; i += 16) sum += 0.299 * px[i] + 0.587 * px[i + 1] + 0.114 * px[i + 2];
      const mean = sum / (px.length / 16);
      // Aim for a mid-grey face; never more than double.
      const want = Math.max(0.8, Math.min(2, 120 / Math.max(mean, 1)));
      bright = bright * 0.8 + want * 0.2;
    }
    if (detector && !detecting && frame % 6 === 0) {
      detecting = true;
      detector
        .detect(video)
        .then((faces) => {
          const f = faces[0]?.boundingBox;
          if (!f) return (target = { x: 0, y: 0, w: 1, h: 1 });
          const w = Math.min(1, (f.width / vw) * 3);
          const h = w;
          const cx = (f.x + f.width / 2) / vw;
          const cy = (f.y + f.height / 2) / vh;
          target = { x: Math.max(0, Math.min(1 - w, cx - w / 2)), y: Math.max(0, Math.min(1 - h, cy - h / 2.4)), w, h };
        })
        .catch(() => {})
        .finally(() => (detecting = false));
    }
    view.x += (target.x - view.x) * 0.1;
    view.y += (target.y - view.y) * 0.1;
    view.w += (target.w - view.w) * 0.1;
    view.h += (target.h - view.h) * 0.1;
    const sx = view.x * vw;
    const sy = view.y * vh;
    const sw = view.w * vw;
    const sh = view.h * vh;
    const filter = s.lighting ? `brightness(${bright.toFixed(2)}) contrast(1.05)` : "none";

    if (s.background !== "off" && empty) {
      // You: where the frame differs from the empty room, softened at the edges.
      const md = maskData.data;
      for (let i = 0, j = 0; i < px.length; i += 4, j += 3) {
        const d = Math.abs(px[i] - empty[j]) + Math.abs(px[i + 1] - empty[j + 1]) + Math.abs(px[i + 2] - empty[j + 2]);
        md[i] = md[i + 1] = md[i + 2] = 0;
        md[i + 3] = d > 60 ? 255 : d > 35 ? ((d - 35) * 255) / 25 : 0;
      }
      mg.putImageData(maskData, 0, 0);
      // Background first.
      g.filter = s.background === "blur" ? `blur(${Math.round(vw / 60)}px) ${filter === "none" ? "" : filter}`.trim() : filter;
      if (s.background === "picture" && pic?.complete && pic.naturalWidth) g.drawImage(pic, 0, 0, vw, vh);
      else g.drawImage(video, sx, sy, sw, sh, 0, 0, vw, vh);
      // Then you, cut out by the mask.
      lg.globalCompositeOperation = "source-over";
      lg.filter = "blur(4px)";
      lg.clearRect(0, 0, vw, vh);
      lg.drawImage(mask, view.x * MW, view.y * MH, view.w * MW, view.h * MH, 0, 0, vw, vh);
      lg.filter = filter;
      lg.globalCompositeOperation = "source-in";
      lg.drawImage(video, sx, sy, sw, sh, 0, 0, vw, vh);
      g.filter = "none";
      g.drawImage(layer, 0, 0);
    } else {
      g.filter = filter;
      g.drawImage(video, sx, sy, sw, sh, 0, 0, vw, vh);
      g.filter = "none";
    }
  }, 1000 / fps);
  const stream = canvas.captureStream(fps);
  return {
    stream,
    stop: () => {
      window.clearInterval(id);
      stream.getTracks().forEach((tr) => tr.stop());
      video.srcObject = null;
    },
  };
}
