// SPDX-License-Identifier: AGPL-3.0-only
// Custom models (Suno's): ACE-Step 1.5 Base taught the person's own songs.
// Picked for a song in the song form; trained from song files or gallery songs.
import { useCallback, useEffect, useState } from "react";
import { open as openFiles } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type CustomModelsView, type MediaItem, type MediaRequest } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import { waitLabel } from "../media";
import { InfoTip } from "./InfoTip";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

export function useCustomModels(): [CustomModelsView | null, () => void] {
  const [view, setView] = useState<CustomModelsView | null>(null);
  const load = useCallback(() => {
    api.musicCustomModels().then(setView).catch(() => {});
  }, []);
  useEffect(() => {
    load();
    const sub = on("media:trained", load);
    return () => {
      sub.then((u) => u());
    };
  }, [load]);
  return [view, load];
}

/** The song form's Custom model field: which one (if any), how much, and training another. */
export function CustomModelField({
  value,
  scale,
  onChange,
  onScale,
  gallery,
  onStart,
  onModels,
  toast,
}: {
  value: string;
  scale: number;
  onChange: (id: string) => void;
  onScale: (s: number) => void;
  gallery: MediaItem[];
  onStart: (r: MediaRequest) => Promise<boolean>;
  onModels: () => void;
  toast: PushToast;
}) {
  const [view, reload] = useCustomModels();
  const [training, setTraining] = useState(false);
  if (!view?.available) return null;
  const picked = view.models.find((m) => m.id === value);
  return (
    <div className="music-field">
      <div className="row wrap">
        <span className="music-label">{t("Custom model")}<InfoTip label={t("Custom model")} text={t("A music model you trained on your own songs or voice. Pick it to make songs in that sound; How much sets how strongly it's used. Train one with the link beside it.")} /></span>
        <select value={picked ? value : ""} onChange={(e) => onChange(e.target.value)} aria-label={t("Custom model")}>
          <option value="">{t("None")}</option>
          {view.models.map((m) => (
            <option key={m.id} value={m.id}>{t("{name} ({n} songs)", { name: m.name, n: m.songs })}</option>
          ))}
        </select>
        {picked && (
          <label className="row small music-length" title={t("How closely songs follow what it learned: lightly at the bottom, strongly at the top")}>
            {t("How much")}
            <input type="range" min={0.3} max={1.5} step={0.1} value={scale} onChange={(e) => onScale(+e.target.value)} />
            {scale.toFixed(1)}
          </label>
        )}
        <button className="link small" onClick={() => setTraining(true)}>{t("Train one on your songs or voice…")}</button>
      </div>
      {picked && <p className="small muted">{t("Made on ACE-Step 1.5 Base with what it learned from your songs.")}</p>}
      {picked && !view.base_installed && (
        <p className="callout small">
          {t("Custom models play on ACE-Step 1.5 Base.")} <button className="link" onClick={onModels}>{t("Get it")}</button>
        </p>
      )}
      {training && <TrainDialog view={view} gallery={gallery} onStart={onStart} onClose={() => setTraining(false)} onDeleted={(id) => { if (id === value) onChange(""); reload(); }} toast={toast} />}
    </div>
  );
}

function TrainDialog({ view, gallery, onStart, onClose, onDeleted, toast }: { view: CustomModelsView; gallery: MediaItem[]; onStart: (r: MediaRequest) => Promise<boolean>; onClose: () => void; onDeleted: (id: string) => void; toast: PushToast }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [songs, setSongs] = useState<string[]>([]);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [voice, setVoice] = useState("");
  const audio = gallery.filter((g) => g.kind === "audio" && !songs.includes(g.id));
  const voices = view.voices ?? [];
  const sung = voices.find((v) => v.id === voice)?.sung ?? 0;
  const n = files.length + songs.length + sung;
  // What's still needed before it can start, said plainly (a greyed-out
  // button alone didn't say why).
  const missing = [
    !view.nvidia ? t("an NVIDIA graphics card (8 GB or more)") : null,
    !name.trim() ? t("a name") : null,
    n === 0 ? t("at least one song, or a voice with singing") : null,
  ].filter(Boolean) as string[];
  // About 40 s a song to listen to and read, then the passes over them (more for few songs).
  const epochs = n <= 10 ? 300 : n <= 30 ? 150 : 80;
  const est = (view.ready ? 60 : 900) + n * 40 + epochs * n * 0.8;
  const pick = async () => {
    try {
      const got = await openFiles({ multiple: true, filters: [{ name: t("Songs"), extensions: ["mp3", "wav", "flac", "ogg", "opus", "m4a"] }] });
      const list = Array.isArray(got) ? got : got ? [got] : [];
      setFiles((f) => [...f, ...list.filter((x) => !f.includes(x))]);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const start = async () => {
    if (await onStart({ op: "train", prompt: name.trim(), train: { name: name.trim(), description: description.trim(), files, songs, voice: voice || null } })) {
      toast(t("Training started. It's in the jobs above; your custom model appears here when it's done."), "success");
      onClose();
    }
  };
  return (
    <Modal title={t("Custom models")} onClose={onClose}>
      {view.models.length > 0 && (
        <ul className="plain-list">
          {view.models.map((m) => (
            <li key={m.id} className="row">
              <strong>{m.name}</strong>
              <span className="small muted">{t("{n} songs · learned in {time}", { n: m.songs, time: waitLabel(m.minutes * 60) })}</span>
              <span className="spacer" />
              {deleting === m.id ? (
                <>
                  <button className="btn small" onClick={() => setDeleting(null)}>{t("Cancel")}</button>
                  <button className="btn small danger-fill" onClick={() => api.musicCustomModelDelete(m.id).then(() => { setDeleting(null); onDeleted(m.id); }).catch((e) => toast(errorText(e), "error"))}>{t("Delete")}</button>
                </>
              ) : (
                <button className="btn ghost danger small" onClick={() => setDeleting(m.id)}>{t("Delete")}</button>
              )}
            </li>
          ))}
        </ul>
      )}
      <h4>{t("Train one on your songs or voice")}</h4>
      <p className="small muted">{t("It learns the sound of songs you made or own, then new songs can be made in that sound. One song works; 3 or more teach its sound, 10 or more teach it best. Each song's words are heard first, so sung songs work best.")}</p>
      <div className="form">
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Its name, e.g. My songs")} aria-label={t("Name")} />
        <input className="input" value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t("Their style in a few words, e.g. warm acoustic country, male voice")} aria-label={t("Their style")} />
        <div className="row wrap">
          <button className="btn small" onClick={pick}>{t("Add song files…")}</button>
          {audio.length > 0 && (
            <select value="" onChange={(e) => e.target.value && setSongs((s) => [...s, e.target.value])} aria-label={t("Add a song from the gallery")}>
              <option value="">{t("Add a song from the gallery…")}</option>
              {audio.map((g) => (
                <option key={g.id} value={g.id}>{g.title || g.prompt || t("A song")}</option>
              ))}
            </select>
          )}
          <span className="small muted">{t("{n} songs", { n })}</span>
        </div>
        {voices.length > 0 && (
          <label className="small">
            {t("Your voice too")}
            <select value={voice} onChange={(e) => {
              setVoice(e.target.value);
              const v = voices.find((x) => x.id === e.target.value);
              if (v && !description.trim()) setDescription(t("sung by {name}", { name: v.name }));
            }} aria-label={t("Your voice too")}>
              <option value="">{t("None")}</option>
              {voices.map((v) => (
                <option key={v.id} value={v.id} disabled={v.sung === 0}>
                  {v.sung > 0 ? t("{name} ({n} singing recordings)", { name: v.name, n: v.sung }) : t("{name} (no singing recorded)", { name: v.name })}
                </option>
              ))}
            </select>
            <span className="muted">{t("Its singing recordings are learned with the songs, so songs can come out in that voice. Only singing is used: speaking teaches it speech, not singing. Record the singing lines in Your voice, or add songs where you sing.")}</span>
          </label>
        )}
        {n > 0 && (
          <ul className="plain-list small">
            {files.map((f) => (
              <li key={f} className="row">
                <span className="ellipsis">{f.split(/[\\/]/).pop()}</span>
                <span className="spacer" />
                <button className="link small" onClick={() => setFiles((x) => x.filter((y) => y !== f))}>{t("Remove")}</button>
              </li>
            ))}
            {songs.map((id) => {
              const g = gallery.find((x) => x.id === id);
              return (
                <li key={id} className="row">
                  <span className="ellipsis">{g?.title || g?.prompt || t("A song")}</span>
                  <span className="spacer" />
                  <button className="link small" onClick={() => setSongs((x) => x.filter((y) => y !== id))}>{t("Remove")}</button>
                </li>
              );
            })}
          </ul>
        )}
        {!view.nvidia && <p className="callout small">{t("Training needs an NVIDIA graphics card (8 GB or more).")}</p>}
        {view.nvidia && !view.ready && (
          <p className="small muted">
            {t("The first time, the trainer and its files download (about {size}). They download when training starts, or now:", { size: bytes(view.setup_bytes) })}{" "}
            <button className="btn small" onClick={async () => { if (await onStart({ op: "trainer", prompt: t("The trainer and its files") })) toast(t("Getting the trainer. It's in the jobs above."), "success"); }}>{t("Get the trainer now")}</button>
          </p>
        )}
        {n > 0 && n < 3 && <p className="small muted">{t("With {n} song it mostly learns that song; 3 or more teach its sound.", { n })}</p>}
        {missing.length > 0 && <p className="small warn-text">{t("To start, it still needs: {list}.", { list: missing.join(", ") })}</p>}
        {n >= 1 && <p className="small muted">{t("About {time} for {n} songs. The graphics card is busy meanwhile.", { time: waitLabel(est), n })}</p>}
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Close")}</button>
        <button className="btn primary" disabled={missing.length > 0} title={missing.length ? t("Still needs: {list}", { list: missing.join(", ") }) : undefined} onClick={start}>{t("Start training")}</button>
      </div>
    </Modal>
  );
}
