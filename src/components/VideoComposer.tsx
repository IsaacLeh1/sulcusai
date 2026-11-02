// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's video form: describing a clip, the picture it starts from,
// its length and look.
import { useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, type MediaItem, type MediaModelCard, type MediaRequest } from "../api";
import { t, tx } from "../i18n";
import { useMediaUrl, waitLabel } from "../media";
import { MediaTile } from "./MediaBits";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";
import { PromptHelpPanel, usePromptHelp } from "./StudioExtras";
import { ModelPick, useModelPick, DefaultLink, type Shape } from "./StudioBits";

/** Camera moves (ids as in the core's `CAMERA`). */
const CAMERA: [string, string][] = [
  ["", tx("Camera: as described")],
  ["static", tx("Still camera")],
  ["pan_left", tx("Pan left")],
  ["pan_right", tx("Pan right")],
  ["tilt_up", tx("Tilt up")],
  ["tilt_down", tx("Tilt down")],
  ["zoom_in", tx("Zoom in")],
  ["zoom_out", tx("Zoom out")],
  ["dolly_in", tx("Move in")],
  ["dolly_out", tx("Pull back")],
  ["orbit", tx("Circle around")],
  ["tracking", tx("Follow the subject")],
  ["crane_up", tx("Rise up")],
  ["handheld", tx("Handheld")],
  ["drone", tx("Drone shot")],
];

export function VideoComposer({
  models,
  defaultId,
  onSetDefault,
  onModels,
  source,
  onPickSource,
  gallery,
  onStart,
  toast,
}: {
  models: MediaModelCard[];
  defaultId: string | null;
  onSetDefault: (id: string) => void;
  onModels: () => void;
  source: MediaItem | null;
  onPickSource: (i: MediaItem | null) => void;
  gallery: MediaItem[];
  onStart: (r: MediaRequest) => Promise<boolean>;
  toast: PushToast;
}) {
  const [prompt, setPrompt] = useState("");
  const [seconds, setSeconds] = useState(3);
  const [customLength, setCustomLength] = useState(false);
  const [shape, setShape] = useState<Shape>("landscape");
  const [model, setModel] = useModelPick("video", models, defaultId);
  const [picking, setPicking] = useState<"start" | "end" | null>(null);
  const [camera, setCamera] = useState("");
  const [end, setEnd] = useState<MediaItem | null>(null);
  const endThumb = useMediaUrl(end, true);
  const usable = source ? models.filter((m) => m.can.includes("image")) : models;
  const m = usable.find((x) => x.id === model) ?? usable[0];
  const thumb = useMediaUrl(source, true);
  const help = usePromptHelp(m?.id);
  // What the model makes at once; longer clips are made in parts, each going
  // on from the last frame of the one before (models that start from a picture).
  const once = m ? Math.max(1, Math.floor(((m.defaults.max_frames || 81) - 1) / Math.max(8, m.defaults.fps || 16))) : 5;
  const continues = !!m && m.can.includes("image") && !m.id.startsWith("cloud:");
  // Ending on a picture is one clip: the model makes it in one go.
  const most = continues && !end ? 600 : once;
  const length = Math.min(Math.max(1, seconds), most);
  const parts = Math.ceil(length / once);
  const importPicture = async () => {
    const picked = await openFile({ multiple: false, filters: [{ name: t("Pictures"), extensions: ["png", "jpg", "jpeg", "webp"] }] });
    if (typeof picked !== "string") return;
    try {
      const item = await api.mediaImport(picked);
      if (picking === "end") setEnd(item);
      else onPickSource(item);
      setPicking(null);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const go = async () => {
    const write = m && !m.id.startsWith("cloud:") && prompt.trim() ? { write_prompt: help.write } : {};
    const extra = { camera: camera || null, end: source && end && m?.can.includes("end") ? end.id : null };
    if (await onStart({ op: "video", prompt, seconds: length, shape, source: source?.id ?? null, model_id: m?.id, ...extra, ...write })) {
      setPrompt("");
      onPickSource(null);
      setEnd(null);
    }
  };
  return (
    <div className="form">
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder={source ? t("How should the picture move? E.g. \"the camera slowly pushes in, leaves drift in the wind\"") : t("Describe the clip: the subject, what happens, the camera and the style.")}
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />
      <div className="row wrap">
        {source ? (
          <span className="chip active row">
            {thumb && <img src={thumb} alt="" className="chip-thumb" />} {t("Starts from this picture")}
            <button className="link" onClick={() => (onPickSource(null), setEnd(null))}>✕</button>
          </span>
        ) : (
          <button className="btn small" onClick={() => setPicking("start")} title={t("Turn a picture into a video: it starts on your picture and comes to life")}>{t("Start from a picture…")}</button>
        )}
        {source &&
          m?.can.includes("end") &&
          (end ? (
            <span className="chip active row">
              {endThumb && <img src={endThumb} alt="" className="chip-thumb" />} {t("Ends on this picture")}
              <button className="link" onClick={() => setEnd(null)}>✕</button>
            </span>
          ) : (
            <button className="btn small" onClick={() => setPicking("end")} title={t("The clip goes from your first picture to this one")}>{t("End on a picture…")}</button>
          ))}
        <select value={camera} onChange={(e) => setCamera(e.target.value)} title={t("How the camera moves")}>
          {CAMERA.map(([id, label]) => (
            <option key={id} value={id}>{t(label)}</option>
          ))}
        </select>
        {!source && (
          <div className="filters tight">
            {(["landscape", "portrait", "square"] as Shape[]).map((s) => (
              <button key={s} className={`chip ${shape === s ? "active" : ""}`} onClick={() => setShape(s)}>
                {({ landscape: t("▭ Landscape"), portrait: t("▯ Portrait"), square: t("◻ Square") } as Record<string, string>)[s]}
              </button>
            ))}
          </div>
        )}
        <select
          value={customLength ? "custom" : String(length)}
          onChange={(e) => {
            if (e.target.value === "custom") return setCustomLength(true);
            setCustomLength(false);
            setSeconds(+e.target.value);
          }}
          title={t("Length")}
        >
          {[2, 3, 5, 10, 20, 30, 60, 120].filter((s) => s <= most).map((s) => (
            <option key={s} value={s}>{s < 60 ? t("{s} seconds", { s }) : t("{m} min", { m: s / 60 })}</option>
          ))}
          {![2, 3, 5, 10, 20, 30, 60, 120].includes(length) && !customLength && <option value={length}>{t("{s} seconds", { s: length })}</option>}
          {continues && <option value="custom">{t("Any length…")}</option>}
        </select>
        {customLength && (
          <label className="check small">
            <input className="input tiny" type="number" min={1} max={most} value={seconds} onChange={(e) => setSeconds(Math.max(1, Math.min(most, +e.target.value || 1)))} aria-label={t("Seconds")} />
            {t("seconds")}
          </label>
        )}
        <ModelPick models={usable} value={m?.id ?? ""} onChange={setModel} onMore={onModels} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel((m.fit.est_secs * length) / Math.max(1, m.defaults.seconds || 3))}</span>}
        <button className="btn primary" disabled={(!prompt.trim() && !source) || !m} onClick={go}>{t("Make video")}</button>
      </div>
      {source && !m && <p className="small warn-text">{t("Bringing a picture to life needs Wan 2.2. Install it from Models.")}</p>}
      {parts > 1 && (
        <p className="small muted">
          {t("{model} makes about {n} seconds at a time, so this is made in {parts} parts, each going on from the last frame of the one before, then joined into one video.", { model: m?.name ?? "", n: once, parts })}
        </p>
      )}
      {!continues && m && seconds > once && <p className="small muted">{t("{model} makes clips up to {n} seconds. For longer ones, use Wan 2.2.", { model: m.name, n: once })}</p>}
      <PromptHelpPanel model={m} help={help} onStart={onStart} toast={toast} canCheck={false} />
      {picking && (
        <Modal title={picking === "end" ? t("End on a picture") : t("Start from a picture")} onClose={() => setPicking(null)}>
          <div className="row">
            <button className="btn small" onClick={importPicture}>{t("From a file…")}</button>
            <span className="small muted">{t("Or pick one from the gallery:")}</span>
          </div>
          <div className="media-grid small-grid">
            {gallery.filter((i) => i.kind === "image").map((i) => (
              <MediaTile
                key={i.id}
                item={i}
                onOpen={() => {
                  if (picking === "end") setEnd(i);
                  else onPickSource(i);
                  setPicking(null);
                }}
              />
            ))}
          </div>
          {!gallery.some((i) => i.kind === "image") && <p className="muted small">{t("No pictures in the gallery yet.")}</p>}
        </Modal>
      )}
    </div>
  );
}
