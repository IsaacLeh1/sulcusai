// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type StorageView } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Settings → Where models are kept: move them to a roomier drive. */
export function ModelStorage({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<StorageView | null>(null);
  const [moving, setMoving] = useState<{ done: number; total: number } | null>(null);

  useEffect(() => {
    api.storageView().then(setV).catch(() => {});
    const sub = on("models:moving", (p) => {
      if (p.error) {
        setMoving(null);
        toast(t("The models couldn't be moved: {error}", { error: p.error }), "error");
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

  const move = async (dest: string) => {
    try {
      await api.moveModels(dest);
      setMoving({ done: 0, total: v.used || 1 });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>{t("Where models are kept")}</h2>
      <p className="muted small">{t("Models are large. Keep them on the drive with the most room; the app moves them for you and restarts.")}</p>
      <p className="small">
        <code>{v.models_dir}</code> · {t("{size} used", { size: bytes(v.used) })}
      </p>
      <ul className="teach-list small">
        {v.drives.map((d) => (
          <li key={d.mount}>
            <span>
              <strong>{d.mount}</strong> <span className="muted">· {t("{free} free of {total}", { free: bytes(d.free), total: bytes(d.total) })}</span>
            </span>
          </li>
        ))}
      </ul>
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
        <div className="row wrap">
          <button
            className="btn"
            onClick={async () => {
              const dir = await openFile({ directory: true, multiple: false });
              if (typeof dir === "string") move(dir);
            }}
          >
            {t("Move models to…")}
          </button>
          {v.models_dir !== v.default_dir && (
            <button className="btn ghost" onClick={() => move(v.default_dir)}>
              {t("Move back to the app's folder")}
            </button>
          )}
        </div>
      )}
      {v.error && <p className="small warn-text">{v.error}</p>}
    </section>
  );
}
