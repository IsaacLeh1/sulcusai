// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, type MediaItem } from "../api";
import { t } from "../i18n";
import { MediaTile } from "./MediaBits";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const FEWEST = 5;
const MOST = 40;

/** Teaching RealVisXL a person, pet, thing or style from the person's own photos. */
export function TrainLora({ onClose, onStarted, toast }: { onClose: () => void; onStarted: () => void; toast: PushToast }) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<"person" | "thing" | "style">("person");
  const [pictures, setPictures] = useState<MediaItem[]>([]);
  const [picked, setPicked] = useState<string[]>([]);
  const [starting, setStarting] = useState(false);
  useEffect(() => {
    api.mediaList("image", null, 200).then(setPictures).catch(() => {});
  }, []);
  const toggle = (id: string) => setPicked((p) => (p.includes(id) ? p.filter((x) => x !== id) : p.length >= MOST ? p : [...p, id]));
  const fromFiles = async () => {
    const files = await openFile({ multiple: true, filters: [{ name: t("Pictures"), extensions: ["png", "jpg", "jpeg", "webp"] }] });
    if (!Array.isArray(files)) return;
    try {
      const added: MediaItem[] = [];
      for (const f of files) added.push(await api.mediaImport(f));
      setPictures((all) => [...added, ...all]);
      setPicked((p) => [...p, ...added.map((i) => i.id)].slice(0, MOST));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const start = async () => {
    setStarting(true);
    try {
      await api.mediaStart({ op: "train_picture", prompt: name, picture_train: { name, kind, pictures: picked } });
      toast(t("Training started. It takes 10 to 30 minutes; the LoRA shows up under Your LoRAs when it's done."), "success");
      onStarted();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setStarting(false);
    }
  };

  return (
    <Modal title={t("Train a LoRA on your photos")} onClose={onClose}>
      <p className="muted small">
        {t("Pick {fewest} to {most} photos of one person, pet or thing (different places, light and angles), or of one style. RealVisXL learns it, here on your graphics card, and you can then put it in any new picture. The photos stay on this PC.", { fewest: FEWEST, most: MOST })}
      </p>
      <div className="row wrap">
        <input className="input" style={{ flex: 1 }} value={name} placeholder={t("Its name, e.g. Max the dog")} aria-label={t("Name")} onChange={(e) => setName(e.target.value)} maxLength={60} />
        <div className="filters tight">
          {(
            [
              ["person", t("A person")],
              ["thing", t("A pet or thing")],
              ["style", t("A style")],
            ] as const
          ).map(([k, label]) => (
            <button key={k} className={`chip ${kind === k ? "active" : ""}`} onClick={() => setKind(k)}>
              {label}
            </button>
          ))}
        </div>
      </div>
      <div className="row">
        <span className="small muted">{t("{n} picked", { n: picked.length })}</span>
        <span className="spacer" />
        <button className="btn small" onClick={fromFiles}>
          {t("Add photos from files…")}
        </button>
      </div>
      <div className="media-grid small-grid">
        {pictures.map((i) => (
          <div key={i.id} className={`pick-tile ${picked.includes(i.id) ? "picked" : ""}`}>
            <MediaTile item={i} onOpen={() => toggle(i.id)} />
          </div>
        ))}
      </div>
      {pictures.length === 0 && <p className="muted small">{t("No pictures in the gallery yet: add photos from files.")}</p>}
      <div className="row">
        <span className="spacer" />
        <button className="btn" onClick={onClose}>
          {t("Cancel")}
        </button>
        <button className="btn primary" disabled={starting || !name.trim() || picked.length < FEWEST} onClick={start}>
          {t("Train")}
        </button>
      </div>
    </Modal>
  );
}
