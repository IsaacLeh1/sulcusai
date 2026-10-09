// SPDX-License-Identifier: AGPL-3.0-only
// Picture, video and music models: what fits this PC, install and remove.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type InstallProgress, type MediaKind, type MediaModelCard, type MediaView } from "../api";
import { bytes, percent } from "../format";
import { waitLabel } from "../media";
import type { PushToast } from "./Toasts";

const KINDS: { kind: MediaKind; title: string; per: string }[] = [
  { kind: "image", title: "Pictures", per: "a picture" },
  { kind: "upscale", title: "Upscaling", per: "a picture" },
  { kind: "background", title: "Background removal", per: "a picture" },
  { kind: "video", title: "Video", per: "a 3-second clip" },
  { kind: "music", title: "Music and sound", per: "a 30-second song" },
];

export function useMediaView(): [MediaView | null, () => void] {
  const [view, setView] = useState<MediaView | null>(null);
  const load = useCallback(() => {
    api.mediaView().then(setView).catch(() => {});
  }, []);
  useEffect(() => {
    load();
    const subs = [on("install:finished", load), on("features:changed", load), on("media:done", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [load]);
  return [view, load];
}

export function MediaModels({ progress, toast, only }: { progress: Record<string, InstallProgress>; toast: PushToast; only?: MediaKind[] }) {
  const [view, load] = useMediaView();
  if (!view) return <p className="muted">Loading…</p>;
  const busy = Object.keys(progress).length > 0;
  const kinds = KINDS.filter((k) => !only || only.includes(k.kind));
  return (
    <div className="media-models">
      {kinds.map(({ kind, title, per }) => {
        const list = view.models.filter((m) => m.kind === kind).sort((a, b) => b.quality - a.quality);
        if (list.length === 0 && kind !== "video") return null;
        return (
          <section key={kind}>
            <h3>{title}</h3>
            {kind === "video" && view.video_note && <p className="small warn-text">{view.video_note}</p>}
            {list.map((m) => (
              <MediaModelRow key={m.id} m={m} per={per} progress={progress[m.id]} busy={busy} toast={toast} onChanged={load} />
            ))}
          </section>
        );
      })}
      {view.hidden > 0 && <p className="small muted">{view.hidden} more {view.hidden === 1 ? "model needs" : "models need"} more than this PC has, so {view.hidden === 1 ? "it's" : "they're"} not shown.</p>}
    </div>
  );
}

function MediaModelRow({ m, per, progress, busy, toast, onChanged }: { m: MediaModelCard; per: string; progress?: InstallProgress; busy: boolean; toast: PushToast; onChanged: () => void }) {
  const [removing, setRemoving] = useState(false);
  const size = m.files.reduce((s, f) => s + f.size, 0);
  return (
    <article className="card media-model">
      <div className="row">
        <div className="grow">
          <strong>{m.name}</strong> <span className="muted small">· {m.publisher}</span>
          <span className="small block">{m.description}</span>
          <span className="small muted block">
            {m.fit.on_gpu ? "Runs on your graphics card" : "Runs on your processor"} · {waitLabel(m.fit.est_secs)} for {per} ·{" "}
            {m.installed ? `${bytes(size)} on disk` : m.fit.needed_bytes < size ? `${bytes(m.fit.needed_bytes)} to download (shares files you have)` : `${bytes(size)} download`} ·{" "}
            <a href={m.license.url} target="_blank" rel="noreferrer">{m.license.name}</a>
          </span>
        </div>
        {m.installed ? (
          <button
            className="btn ghost danger small"
            disabled={removing}
            onClick={async () => {
              setRemoving(true);
              try {
                await api.removeMedia(m.id);
                onChanged();
              } catch (e) {
                toast(errorText(e), "error");
              } finally {
                setRemoving(false);
              }
            }}
          >
            Remove
          </button>
        ) : (
          !progress && (
            <button className="btn primary small" disabled={busy} title={busy ? "One install at a time" : undefined} onClick={() => api.installMedia(m.id).catch((e) => toast(errorText(e), "error"))}>
              Install
            </button>
          )
        )}
      </div>
      {progress && <InstallBar p={progress} />}
    </article>
  );
}

function InstallBar({ p }: { p: InstallProgress }) {
  const label = p.phase === "engine" ? "Setting up the engine" : p.phase === "download" ? "Downloading" : "Checking";
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${bytes(p.received)} of ${bytes(p.total)}` : "…"}</span>
        <button className="link" onClick={() => api.cancelInstall(p.model_id)}>Cancel</button>
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
    </div>
  );
}
