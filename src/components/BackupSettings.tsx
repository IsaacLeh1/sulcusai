// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type BackupInfo } from "../api";
import { bytes } from "../format";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const today = () => new Date().toISOString().slice(0, 10);

export function BackupSettings({ toast }: { toast: PushToast }) {
  const [making, setMaking] = useState<{ password: string; again: string; busy: boolean } | null>(null);
  const [restoring, setRestoring] = useState<{ path: string; info: BackupInfo; password: string; busy: boolean } | null>(null);
  const [exporting, setExporting] = useState(false);

  const make = async () => {
    if (!making) return;
    const path = await save({ defaultPath: `SulcusAI backup ${today()}.sulcusbackup`, filters: [{ name: "SulcusAI backup", extensions: ["sulcusbackup"] }] });
    if (!path) return;
    setMaking({ ...making, busy: true });
    try {
      const r = await api.makeBackup(path, making.password);
      setMaking(null);
      toast(`Backup saved: ${r.files} files, ${bytes(r.bytes)}. Keep the password somewhere safe.`, "success");
    } catch (e) {
      setMaking({ ...making, busy: false });
      toast(errorText(e), "error");
    }
  };

  const pickBackup = async () => {
    const path = await openFile({ multiple: false, filters: [{ name: "SulcusAI backup", extensions: ["sulcusbackup"] }] });
    if (typeof path !== "string") return;
    try {
      setRestoring({ path, info: await api.backupInfo(path), password: "", busy: false });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const restore = async () => {
    if (!restoring) return;
    setRestoring({ ...restoring, busy: true });
    try {
      await api.restoreBackup(restoring.path, restoring.password);
      toast("Backup unpacked. Restarting to finish…", "success");
    } catch (e) {
      setRestoring({ ...restoring, busy: false });
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>Backup and restore</h2>
      <p className="muted small">
        One file with your chats, notes, memory, tasks, settings, pictures and meeting recordings, locked with a password. Restore it on this PC or a new one. Models aren't included; install them again or let the app find them.
      </p>
      <div className="row wrap">
        <button className="btn" onClick={() => setMaking({ password: "", again: "", busy: false })}>Make a backup…</button>
        <button className="btn" onClick={pickBackup}>Restore from a backup…</button>
        <button
          className="btn ghost"
          disabled={exporting}
          onClick={async () => {
            const dir = await openFile({ directory: true, multiple: false });
            if (typeof dir !== "string") return;
            setExporting(true);
            try {
              const out = await api.exportMarkdown(dir);
              toast(`Exported your chats and notes as Markdown to ${out}`, "success");
            } catch (e) {
              toast(errorText(e), "error");
            } finally {
              setExporting(false);
            }
          }}
        >
          {exporting ? "Exporting…" : "Export chats and notes as Markdown…"}
        </button>
      </div>
      <p className="muted small">The Markdown export is readable by any app and isn't encrypted, so keep it somewhere private.</p>

      {making && (
        <Modal title="Make a backup" onClose={() => !making.busy && setMaking(null)}>
          <p className="small">Choose a password for this backup. You'll need it to restore, and it can't be recovered, so write it down somewhere safe.</p>
          <div className="form">
            <label>
              Password
              <input className="input" type="password" value={making.password} onChange={(e) => setMaking({ ...making, password: e.target.value })} autoFocus />
            </label>
            <label>
              Same password again
              <input className="input" type="password" value={making.again} onChange={(e) => setMaking({ ...making, again: e.target.value })} />
            </label>
            {making.password.length > 0 && making.password.length < 8 && <span className="muted small">At least 8 characters.</span>}
            {making.again.length > 0 && making.again !== making.password && <span className="warn-text small">The passwords don't match.</span>}
          </div>
          <div className="modal-actions">
            <button className="btn" onClick={() => setMaking(null)} disabled={making.busy}>Cancel</button>
            <button className="btn primary" onClick={make} disabled={making.busy || making.password.length < 8 || making.password !== making.again}>
              {making.busy ? "Making the backup…" : "Choose where to save…"}
            </button>
          </div>
        </Modal>
      )}

      {restoring && (
        <Modal title="Restore from a backup" onClose={() => !restoring.busy && setRestoring(null)}>
          <p className="small">
            Made {new Date(restoring.info.created).toLocaleString()} with version {restoring.info.app_version}: {restoring.info.files} files, {bytes(restoring.info.bytes)}.
          </p>
          <p className="small">
            Your current chats, notes and settings are replaced with the backup's. They aren't deleted: they're moved to a <code>before-restore</code> folder in the app's data folder. The app restarts to finish, and app lock is off afterwards; turn it on again in Settings.
          </p>
          <div className="form">
            <label>
              The backup's password
              <input className="input" type="password" value={restoring.password} onChange={(e) => setRestoring({ ...restoring, password: e.target.value })} autoFocus />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn" onClick={() => setRestoring(null)} disabled={restoring.busy}>Cancel</button>
            <button className="btn danger-fill" onClick={restore} disabled={restoring.busy || !restoring.password}>
              {restoring.busy ? "Unpacking…" : "Restore and restart"}
            </button>
          </div>
        </Modal>
      )}
    </section>
  );
}
