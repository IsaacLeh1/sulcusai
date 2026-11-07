// SPDX-License-Identifier: AGPL-3.0-only
// Recovery and trust tools, as pieces a Settings page mounts where it fits:
// one-button backup, signed updates on the next start, safe mode, low-end
// mode and the diagnostics bundle. Each piece is self-contained.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { errorText } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

export type StartupView = { safe_mode: boolean; low_end_mode: boolean; suggest_low_end: boolean };

type UpdateExtras = { current: string; on_next_start: boolean; staged: string | null; signature_checked: string | null };

/** Marks the page for safe mode and low-end mode, so styles can drop custom
 * themes, animations and blur. Call once at start. */
export async function applyStartupLook(): Promise<StartupView | null> {
  try {
    const v = await invoke<StartupView>("startup_view");
    document.documentElement.toggleAttribute("data-safe-mode", v.safe_mode);
    document.documentElement.toggleAttribute("data-low-end", v.low_end_mode);
    return v;
  } catch {
    return null;
  }
}

/** One button: a backup into Documents\SulcusAI backups, locked with a passphrase. */
export function BackupNow({ toast }: { toast: PushToast }) {
  const [open, setOpen] = useState(false);
  const [pass, setPass] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const go = async () => {
    setBusy(true);
    try {
      const where = await invoke<string>("backup_now", { password: pass });
      setOpen(false);
      setPass("");
      setAgain("");
      toast(t("Backup saved to {path}. Keep the passphrase somewhere safe: nobody can open the backup without it.", { path: where }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <button className="btn" onClick={() => setOpen(true)}>{t("Back up everything now")}</button>
      {open && (
        <Modal title={t("Back up everything")} onClose={() => !busy && setOpen(false)}>
          <p className="muted small">{t("Chats, notes, memory, settings, pictures, recordings, voices, groups and their keys, in one file locked with a passphrase. Models aren't included.")}</p>
          <input type="password" placeholder={t("Passphrase (8 characters or more)")} value={pass} onChange={(e) => setPass(e.target.value)} autoFocus />
          <input type="password" placeholder={t("The passphrase again")} value={again} onChange={(e) => setAgain(e.target.value)} />
          <div className="row">
            <button className="btn primary" disabled={busy || pass.length < 8 || pass !== again} onClick={go}>
              {busy ? t("Backing up…") : t("Back up")}
            </button>
            <button className="btn ghost" disabled={busy} onClick={() => setOpen(false)}>{t("Cancel")}</button>
          </div>
        </Modal>
      )}
    </>
  );
}

/** Updates that download in the background and install when the app next
 * closes, and whether the last one's signature was checked. */
export function UpdatesOnNextStart({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<UpdateExtras | null>(null);
  const load = () => invoke<UpdateExtras>("update_view").then(setV).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  if (!v) return null;
  return (
    <div className="stack">
      <label className="row">
        <input
          type="checkbox"
          checked={v.on_next_start}
          onChange={async (e) => {
            try {
              await invoke("set_update_on_next_start", { on: e.target.checked });
              load();
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        />
        {t("Download updates in the background and install them the next time the app starts")}
      </label>
      {v.staged && <p className="muted small">{t("SulcusAI {v} is downloaded and installs when you next close the app.", { v: v.staged })}</p>}
      {v.signature_checked && <p className="small">{t("Signed and checked: SulcusAI {v} matches the signing key built into this app.", { v: v.signature_checked })}</p>}
    </div>
  );
}

/** Restarts once without extras, plugins or custom themes; in safe mode, restarts normally. */
export function SafeModeButton() {
  const [safe, setSafe] = useState(false);
  useEffect(() => {
    invoke<StartupView>("startup_view").then((v) => setSafe(v.safe_mode)).catch(() => {});
  }, []);
  return (
    <div className="stack">
      {safe && <p className="small">{t("Running in safe mode: extras, plugins and custom themes are off for this start.")}</p>}
      <button className="btn" onClick={() => invoke("restart_in_safe_mode", { on: !safe })}>
        {safe ? t("Restart normally") : t("Restart in safe mode")}
      </button>
    </div>
  );
}

/** The low-end switch (off by default; suggested on PCs with little memory). */
export function LowEndSwitch({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<StartupView | null>(null);
  useEffect(() => {
    invoke<StartupView>("startup_view").then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  return (
    <label className="row">
      <input
        type="checkbox"
        checked={v.low_end_mode}
        onChange={async (e) => {
          const on = e.target.checked;
          try {
            await invoke("set_low_end_mode", { on });
            setV({ ...v, low_end_mode: on });
            document.documentElement.toggleAttribute("data-low-end", on);
          } catch (err) {
            toast(errorText(err), "error");
          }
        }}
      />
      <span>
        {t("Low-end mode: no animations or blur, and extras start a little later")}
        {v.suggest_low_end && !v.low_end_mode && <span className="muted small"> {t("(suggested for this PC)")}</span>}
      </span>
    </label>
  );
}

/** Saves a zip of logs and crash reports with personal data taken out. */
export function DiagnosticsButton({ toast }: { toast: PushToast }) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      className="btn ghost"
      disabled={busy}
      onClick={async () => {
        const path = await save({ defaultPath: `SulcusAI diagnostics ${new Date().toISOString().slice(0, 10)}.zip`, filters: [{ name: "Zip", extensions: ["zip"] }] });
        if (!path) return;
        setBusy(true);
        try {
          const n = await invoke<number>("save_diagnostics", { path });
          toast(t("Saved {n} files with names, addresses, keys and passwords taken out. Nothing was sent.", { n }), "success");
        } catch (e) {
          toast(errorText(e), "error");
        } finally {
          setBusy(false);
        }
      }}
    >
      {t("Save a diagnostics bundle…")}
    </button>
  );
}

/** The release notes of the version just installed, shown once. Mount near the app's root. */
export function UpdateNotesOnce() {
  const [notes, setNotes] = useState<{ version: string; notes: string } | null>(null);
  useEffect(() => {
    invoke<{ version: string; notes: string } | null>("update_notes_once").then(setNotes).catch(() => {});
  }, []);
  if (!notes) return null;
  return (
    <Modal title={t("What's new in SulcusAI {v}", { v: notes.version })} onClose={() => setNotes(null)}>
      <p style={{ whiteSpace: "pre-wrap" }}>{notes.notes}</p>
      <button className="btn primary" onClick={() => setNotes(null)}>{t("OK")}</button>
    </Modal>
  );
}
