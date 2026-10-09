// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type UpdateAvailable, type UpdateView } from "../api";
import { bytes } from "../format";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Confirms, downloads and installs an update (the app restarts after). */
function InstallModal({ update, onClose, toast }: { update: UpdateAvailable; onClose: () => void; toast: PushToast }) {
  const [progress, setProgress] = useState<{ received: number; total: number | null } | null>(null);
  useEffect(() => {
    const sub = on("update:progress", (p) => setProgress(p));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  return (
    <Modal title={`Update to SulcusAI ${update.version}`} onClose={() => !progress && onClose()}>
      {update.notes && <p className="small update-notes">{update.notes}</p>}
      <p className="muted small">The update is checked against SulcusAI's signing key before it installs. SulcusAI closes and opens again when it's done; your chats and settings stay.</p>
      {progress && (
        <div className="install-progress">
          <span className="small">{progress.total ? `Downloading · ${bytes(progress.received)} of ${bytes(progress.total)}` : "Downloading…"}</span>
          <div className={`progress ${progress.total ? "" : "indeterminate"}`}>
            <span style={{ width: progress.total ? `${(progress.received / progress.total) * 100}%` : undefined }} />
          </div>
        </div>
      )}
      <div className="modal-actions">
        <button className="btn" onClick={onClose} disabled={!!progress}>Later</button>
        <button
          className="btn primary"
          disabled={!!progress}
          onClick={async () => {
            setProgress({ received: 0, total: null });
            try {
              await api.installUpdate();
            } catch (e) {
              setProgress(null);
              toast(errorText(e), "error");
            }
          }}
        >
          Install and restart
        </button>
      </div>
    </Modal>
  );
}

/** Settings → About: the version, checking now, and automatic checks. */
export function UpdateSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<UpdateView | null>(null);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [installing, setInstalling] = useState<UpdateAvailable | null>(null);
  useEffect(() => {
    api.updateView().then(setV);
  }, []);
  if (!v) return null;
  return (
    <div className="update-settings">
      <div className="row wrap">
        <span className="small">Version {v.current}</span>
        <button
          className="btn small"
          disabled={checking}
          onClick={async () => {
            setChecking(true);
            setResult(null);
            try {
              const found = await api.checkForUpdate();
              if (found) setInstalling(found);
              else setResult("You have the latest version.");
              setV(await api.updateView());
            } catch (e) {
              setResult(errorText(e));
            } finally {
              setChecking(false);
            }
          }}
        >
          {checking ? "Checking…" : "Check for updates"}
        </button>
        {v.available && <button className="btn primary small" onClick={() => setInstalling(v.available)}>Install {v.available.version}</button>}
        {result && <span className="muted small">{result}</span>}
      </div>
      <label className="check">
        <input
          type="checkbox"
          checked={v.auto}
          onChange={async (e) => {
            const auto = e.target.checked;
            await api.setUpdateAuto(auto);
            setV({ ...v, auto });
          }}
        />
        <span>
          Check for updates once a day
          <span className="muted small block">Only when SulcusAI may go online (Web or Cloud). Updates come from SulcusAI's GitHub releases and only install when you say so.</span>
        </span>
      </label>
      {installing && <InstallModal update={installing} onClose={() => setInstalling(null)} toast={toast} />}
    </div>
  );
}

/** Status-bar button that appears when a background check found an update. */
export function UpdatePill({ toast }: { toast: PushToast }) {
  const [found, setFound] = useState<UpdateAvailable | null>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    api.updateView().then((v) => setFound(v.available)).catch(() => {});
    const sub = on("update:available", (u) => setFound(u));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!found) return null;
  return (
    <>
      <button className="status-item update-pill" onClick={() => setOpen(true)} title="A newer version of SulcusAI is ready">
        ⬆ Update to {found.version}
      </button>
      {open && <InstallModal update={found} onClose={() => setOpen(false)} toast={toast} />}
    </>
  );
}
