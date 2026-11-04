// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { api, errorText, type BackupInfo } from "../api";
import { invoke } from "@tauri-apps/api/core";
import { bytes } from "../format";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const today = () => new Date().toISOString().slice(0, 10);

export function BackupSettings({ toast }: { toast: PushToast }) {
  const [making, setMaking] = useState<{ password: string; again: string; busy: boolean } | null>(null);
  const [restoring, setRestoring] = useState<{ path: string; info: BackupInfo; password: string; busy: boolean } | null>(null);
  const [exporting, setExporting] = useState(false);

  const make = async () => {
    if (!making) return;
    const path = await save({ defaultPath: `SulcusAI backup ${today()}.sulcusbackup`, filters: [{ name: t("SulcusAI backup"), extensions: ["sulcusbackup"] }] });
    if (!path) return;
    setMaking({ ...making, busy: true });
    try {
      const r = await api.makeBackup(path, making.password);
      setMaking(null);
      toast(t("Backup saved: {n} files, {size}. Keep the password somewhere safe.", { n: r.files, size: bytes(r.bytes) }), "success");
    } catch (e) {
      setMaking({ ...making, busy: false });
      toast(errorText(e), "error");
    }
  };

  const pickBackup = async () => {
    const path = await openFile({ multiple: false, filters: [{ name: t("SulcusAI backup"), extensions: ["sulcusbackup"] }] });
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
      toast(t("Backup unpacked. Restarting to finish…"), "success");
    } catch (e) {
      setRestoring({ ...restoring, busy: false });
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>{t("Backup and restore")}</h2>
      <p className="muted small">
        {t("One file with your chats, notes, memory, tasks, settings, pictures and meeting recordings, locked with a password. Restore it on this PC or a new one. Models aren't included; install them again or let the app find them.")}
      </p>
      <div className="row wrap">
        <button className="btn" onClick={() => setMaking({ password: "", again: "", busy: false })}>{t("Make a backup…")}</button>
        <button className="btn" onClick={pickBackup}>{t("Restore from a backup…")}</button>
        <button
          className="btn ghost"
          disabled={exporting}
          onClick={async () => {
            const dir = await openFile({ directory: true, multiple: false });
            if (typeof dir !== "string") return;
            setExporting(true);
            try {
              const out = await api.exportMarkdown(dir);
              toast(t("Exported your chats and notes as Markdown to {path}", { path: out }), "success");
            } catch (e) {
              toast(errorText(e), "error");
            } finally {
              setExporting(false);
            }
          }}
        >
          {exporting ? t("Exporting…") : t("Export chats and notes as Markdown…")}
        </button>
      </div>
      <p className="muted small">{t("The Markdown export is readable by any app and isn't encrypted, so keep it somewhere private.")}</p>

      {making && (
        <Modal title={t("Make a backup")} onClose={() => !making.busy && setMaking(null)}>
          <p className="small">{t("Choose a password for this backup. You'll need it to restore, and it can't be recovered, so write it down somewhere safe.")}</p>
          <div className="form">
            <label>
              {t("Password")}
              <input className="input" type="password" value={making.password} onChange={(e) => setMaking({ ...making, password: e.target.value })} autoFocus />
            </label>
            <label>
              {t("Same password again")}
              <input className="input" type="password" value={making.again} onChange={(e) => setMaking({ ...making, again: e.target.value })} />
            </label>
            {making.password.length > 0 && making.password.length < 8 && <span className="muted small">{t("At least 8 characters.")}</span>}
            {making.again.length > 0 && making.again !== making.password && <span className="warn-text small">{t("The passwords don't match.")}</span>}
          </div>
          <div className="modal-actions">
            <button className="btn" onClick={() => setMaking(null)} disabled={making.busy}>{t("Cancel")}</button>
            <button className="btn primary" onClick={make} disabled={making.busy || making.password.length < 8 || making.password !== making.again}>
              {making.busy ? t("Making the backup…") : t("Choose where to save…")}
            </button>
          </div>
        </Modal>
      )}

      {restoring && (
        <Modal title={t("Restore from a backup")} onClose={() => !restoring.busy && setRestoring(null)}>
          <p className="small">
            {t("Made {date} with version {version}: {n} files, {size}.", { date: new Date(restoring.info.created).toLocaleString(), version: restoring.info.app_version, n: restoring.info.files, size: bytes(restoring.info.bytes) })}
          </p>
          <p className="small">
            {t("Your current chats, notes and settings are replaced with the backup's.")} {t("They aren't deleted: they're moved to a")} <code>before-restore</code>{" "}
            {t("folder in the app's data folder.")} {t("The app restarts to finish, and app lock is off afterwards; turn it on again in Settings.")}
          </p>
          <div className="form">
            <label>
              {t("The backup's password")}
              <input className="input" type="password" value={restoring.password} onChange={(e) => setRestoring({ ...restoring, password: e.target.value })} autoFocus />
            </label>
          </div>
          <div className="modal-actions">
            <button className="btn" onClick={() => setRestoring(null)} disabled={restoring.busy}>{t("Cancel")}</button>
            <button className="btn danger-fill" onClick={restore} disabled={restoring.busy || !restoring.password}>
              {restoring.busy ? t("Unpacking…") : t("Restore and restart")}
            </button>
          </div>
        </Modal>
      )}
      <AutoBackup toast={toast} />
    </section>
  );
}

interface AutoView {
  on: boolean;
  folder: string;
  days: number;
  keep: number;
  has_password: boolean;
  last: number | null;
  last_error: string | null;
}

/** Backups on their own, into a folder you pick. */
function AutoBackup({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<AutoView | null>(null);
  const [password, setPassword] = useState("");
  useEffect(() => {
    invoke<AutoView>("auto_backup_view").then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const set = async (next: Partial<AutoView>, pw?: string) => {
    const merged = { ...v, ...next };
    try {
      setV(await invoke<AutoView>("set_auto_backup", { on: merged.on, folder: merged.folder, days: merged.days, keep: merged.keep, password: pw ?? null }));
      if (pw) setPassword("");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="auto-backup">
      <div className="toggle-row">
        <div>
          <strong>{t("Back up on its own")}</strong>
          <span className="small muted block">{t("A locked backup file in a folder you pick (another drive or a synced folder), keeping the last few. It runs while SulcusAI is open.")}</span>
        </div>
        <label className="switch">
          <input type="checkbox" checked={v.on} onChange={(e) => set({ on: e.target.checked })} aria-label={t("Back up on its own")} />
          <span />
        </label>
      </div>
      <div className="row wrap small">
        <button
          className="btn small"
          onClick={async () => {
            const dir = await openFile({ directory: true, multiple: false });
            if (typeof dir === "string") set({ folder: dir });
          }}
        >
          {v.folder ? t("Folder: {folder}", { folder: v.folder }) : t("Pick a folder")}
        </button>
        <label>
          {t("Every")}{" "}
          <select value={v.days} onChange={(e) => set({ days: Number(e.target.value) })}>
            <option value={1}>{t("day")}</option>
            <option value={7}>{t("week")}</option>
          </select>
        </label>
        <label>
          {t("Keep")}{" "}
          <select value={v.keep} onChange={(e) => set({ keep: Number(e.target.value) })}>
            {[3, 5, 10].map((n) => (
              <option key={n} value={n}>{n}</option>
            ))}
          </select>
        </label>
      </div>
      <div className="row small">
        <input className="input" type="password" autoComplete="new-password" placeholder={v.has_password ? t("Password saved (type to change)") : t("Password for the backups")} value={password} onChange={(e) => setPassword(e.target.value)} />
        <button className="btn small" disabled={password.length < 8} onClick={() => set({}, password)}>{t("Save password")}</button>
      </div>
      {v.last && <p className="small muted">{t("Last backup: {when}", { when: new Date(v.last).toLocaleString() })}</p>}
      {v.last_error && <p className="small error-text">{v.last_error}</p>}
    </div>
  );
}
