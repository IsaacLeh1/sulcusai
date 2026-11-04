// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFolder } from "@tauri-apps/plugin-dialog";
import { errorText } from "../api";
import { t } from "../i18n";
import { consentApi, ERASE_PHRASE, type EraseRequest } from "./ConsentApi";
import { ConsentReport } from "./ConsentReport";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Taking everything out, deleting everything, and where to report crimes. */
export function YourDataSettings({ toast }: { toast: PushToast }) {
  const [exporting, setExporting] = useState<{ password: string; busy: boolean } | null>(null);
  const [erasing, setErasing] = useState<{ confirm: string; keepModels: boolean; busy: boolean } | null>(null);
  const [pending, setPending] = useState<EraseRequest | null>(null);

  useEffect(() => {
    consentApi.eraseStatus().then(setPending).catch(() => setPending(null));
  }, []);

  const doExport = async () => {
    if (!exporting) return;
    const dir = await openFolder({ directory: true, multiple: false });
    if (typeof dir !== "string") return;
    setExporting({ ...exporting, busy: true });
    try {
      const r = await consentApi.exportEverything(dir, exporting.password);
      setExporting(null);
      toast(t("Everything was saved to {folder}.", { folder: r.folder }), "success");
    } catch (e) {
      setExporting({ ...exporting, busy: false });
      toast(errorText(e), "error");
    }
  };

  const doErase = async () => {
    if (!erasing) return;
    setErasing({ ...erasing, busy: true });
    try {
      await consentApi.eraseEverything(erasing.keepModels, erasing.confirm);
      setErasing(null);
      setPending(await consentApi.eraseStatus());
    } catch (e) {
      setErasing({ ...erasing, busy: false });
      toast(errorText(e), "error");
    }
  };

  const cancel = async () => {
    try {
      await consentApi.cancelErase();
      setPending(null);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>{t("Your data")}</h2>
      <p className="muted small">
        {t("Take a copy of everything SulcusAI keeps about you, or delete all of it from this PC.")}
      </p>
      <div className="row wrap">
        <button className="btn" onClick={() => setExporting({ password: "", busy: false })}>{t("Export everything")}</button>
        {!pending && (
          <button className="btn danger" onClick={() => setErasing({ confirm: "", keepModels: true, busy: false })}>
            {t("Delete everything")}
          </button>
        )}
      </div>
      {pending && (
        <div className="row wrap">
          <p className="small">
            {pending.keep_models
              ? t("Everything except your models will be deleted the next time SulcusAI starts.")
              : t("Everything, models included, will be deleted the next time SulcusAI starts.")}
          </p>
          <button className="btn" onClick={cancel}>{t("Don't delete")}</button>
        </div>
      )}

      <h3>{t("Report a crime")}</h3>
      <ConsentReport />

      {exporting && (
        <Modal title={t("Export everything")} onClose={() => !exporting.busy && setExporting(null)}>
          <p className="small">
            {t("Saves a readable copy and a backup file, locked with this password, into a folder you pick.")}
          </p>
          <input
            type="password"
            autoFocus
            placeholder={t("Password for the backup file")}
            value={exporting.password}
            onChange={(e) => setExporting({ ...exporting, password: e.target.value })}
          />
          <div className="row">
            <button className="btn" onClick={() => setExporting(null)} disabled={exporting.busy}>{t("Cancel")}</button>
            <button className="btn primary" onClick={doExport} disabled={exporting.busy || exporting.password.length < 8}>
              {exporting.busy ? t("Saving…") : t("Pick a folder and save")}
            </button>
          </div>
        </Modal>
      )}

      {erasing && (
        <Modal title={t("Delete everything")} onClose={() => !erasing.busy && setErasing(null)}>
          <p className="small">
            {t("Chats, notes, memory, settings, keys and connected accounts are deleted from this PC when SulcusAI next starts. Until then you can call it off here.")}
          </p>
          <label className="row">
            <input type="checkbox" checked={erasing.keepModels} onChange={(e) => setErasing({ ...erasing, keepModels: e.target.checked })} />
            {t("Keep downloaded models")}
          </label>
          <p className="small">{t("Type \"{phrase}\" to confirm.", { phrase: ERASE_PHRASE })}</p>
          <input value={erasing.confirm} onChange={(e) => setErasing({ ...erasing, confirm: e.target.value })} />
          <div className="row">
            <button className="btn" onClick={() => setErasing(null)} disabled={erasing.busy}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={doErase}
              disabled={erasing.busy || erasing.confirm.trim().toLowerCase() !== ERASE_PHRASE}
            >
              {t("Delete everything")}
            </button>
          </div>
        </Modal>
      )}
    </section>
  );
}
