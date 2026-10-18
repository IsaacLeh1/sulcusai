// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type StorageView } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Bytes kept free beyond the models, so the drive doesn't fill to the brim. */
const MARGIN = 512 * 1024 * 1024;

/** Where models are kept: pick a drive (or any folder); the app moves them
 * there and restarts. Shown on the Models page and in Settings. */
export function ModelStorage({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<StorageView | null>(null);
  const [moving, setMoving] = useState<{ done: number; total: number } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);

  useEffect(() => {
    api.storageView().then(setV).catch(() => {});
    const sub = on("models:moving", (p) => {
      if (p.error) {
        setMoving(null);
        toast(t("The models couldn't be moved: {error}", { error: errorText(p.error) }), "error");
        api.storageView().then(setV).catch(() => {});
      } else if (p.finished) {
        toast(t("Models moved. Restarting…"), "success");
      } else if (p.total) {
        setMoving({ done: p.done ?? 0, total: p.total });
      }
    });
    return () => {
      sub.then((un) => un());
    };
  }, [toast]);
  if (!v) return null;

  const lower = (s: string) => s.toLowerCase();
  const onDrive = (dir: string, mount: string) => lower(dir).startsWith(lower(mount));
  const current = v.drives.find((d) => onDrive(v.models_dir, d.mount))?.mount ?? null;
  // A drive's folder: the app's own on its drive, else "<drive>\SulcusAI models".
  const folderOn = (mount: string) => (onDrive(v.default_dir, mount) ? v.default_dir : `${mount.replace(/[\\/]+$/, "")}\\SulcusAI models`);

  const move = async (dest: string) => {
    setConfirm(null);
    try {
      await api.moveModels(dest);
      setMoving({ done: 0, total: v.used || 1 });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card storage-card">
      <h2>{t("Where models are saved")}</h2>
      <p className="muted small">
        {t("New models download here, and the ones installed are moved for you (the app restarts after). Models found in other apps, like Ollama, stay where they are.")}
      </p>
      <p className="small storage-now">
        <span className="muted">{t("Now:")}</span> <code title={v.models_dir}>{v.models_dir}</code> · {t("{size} used", { size: bytes(v.used) })}
      </p>
      {moving || v.moving ? (
        <div>
          <span className="small">{t("Moving the models…")}</span>
          {moving && (
            <div className="progress">
              <span style={{ width: `${Math.round((moving.done / Math.max(1, moving.total)) * 100)}%` }} />
            </div>
          )}
        </div>
      ) : (
        <>
          <div className="drive-grid">
            {v.drives.map((d) => {
              const here = d.mount === current;
              const fits = d.free >= v.used + MARGIN;
              const usedPart = d.total > 0 ? Math.min(100, ((d.total - d.free) / d.total) * 100) : 0;
              return (
                <div key={d.mount} className={`drive ${here ? "current" : ""}`}>
                  <div className="row between">
                    <strong className="drive-name">💽 {d.mount.replace(/[\\/]+$/, "")}</strong>
                    {here && <span className="badge">{t("In use")}</span>}
                  </div>
                  <div className="drive-bar" aria-hidden>
                    <span style={{ width: `${usedPart}%` }} />
                  </div>
                  <span className="muted small">{t("{free} free of {total}", { free: bytes(d.free), total: bytes(d.total) })}</span>
                  {!here && (
                    <button className="btn small" disabled={!fits} onClick={() => setConfirm(folderOn(d.mount))} title={fits ? folderOn(d.mount) : t("Needs {size} free", { size: bytes(v.used + MARGIN) })}>
                      {fits ? t("Use this drive") : t("Not enough room")}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
          <div className="row wrap">
            <button
              className="btn ghost small"
              onClick={async () => {
                const dir = await openFile({ directory: true, multiple: false });
                if (typeof dir === "string") setConfirm(dir);
              }}
            >
              {t("Choose a folder…")}
            </button>
            {v.models_dir !== v.default_dir && (
              <button className="btn ghost small" onClick={() => setConfirm(v.default_dir)}>
                {t("Move back to the app's folder")}
              </button>
            )}
          </div>
          {confirm && (
            <div className="hint-box small move-confirm">
              <p>{t("Move {size} of models to {folder}? The app restarts when it's done.", { size: bytes(v.used), folder: confirm })}</p>
              <div className="row">
                <button className="btn primary small" onClick={() => move(confirm)}>{t("Move them")}</button>
                <button className="btn small" onClick={() => setConfirm(null)}>{t("Cancel")}</button>
              </div>
            </div>
          )}
        </>
      )}
      {v.error && <p className="small warn-text">{v.error}</p>}
    </section>
  );
}
