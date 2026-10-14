// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type UpdateAvailable, type UpdateView } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
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
    <Modal title={t("Update to SulcusAI {version}", { version: update.version })} onClose={() => !progress && onClose()}>
      {update.notes && <p className="small update-notes">{update.notes}</p>}
      <p className="muted small">{t("The update is checked against SulcusAI's signing key before it installs. SulcusAI closes and opens again when it's done; your chats and settings stay.")}</p>
      {progress && (
        <div className="install-progress">
          <span className="small">{progress.total ? t("Downloading · {received} of {total}", { received: bytes(progress.received), total: bytes(progress.total) }) : t("Downloading…")}</span>
          <div className={`progress ${progress.total ? "" : "indeterminate"}`}>
            <span style={{ width: progress.total ? `${(progress.received / progress.total) * 100}%` : undefined }} />
          </div>
        </div>
      )}
      <div className="modal-actions">
        <button className="btn" onClick={onClose} disabled={!!progress}>{t("Later")}</button>
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
          {t("Install and restart")}
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
        <span className="small">{t("Version {version}", { version: v.current })}</span>
        <button
          className="btn small"
          disabled={checking}
          onClick={async () => {
            setChecking(true);
            setResult(null);
            try {
              const found = await api.checkForUpdate();
              if (found) setInstalling(found);
              else setResult(t("You have the latest version."));
              setV(await api.updateView());
            } catch (e) {
              setResult(errorText(e));
            } finally {
              setChecking(false);
            }
          }}
        >
          {checking ? t("Checking…") : t("Check for updates")}
        </button>
        {v.available && <button className="btn primary small" onClick={() => setInstalling(v.available)}>{t("Install {version}", { version: v.available.version })}</button>}
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
          {t("Check for updates automatically")}
          <span className="muted small block">{t("A few times a day, even Offline: it only asks SulcusAI's GitHub releases for the latest version number. Updates install only when you say so.")}</span>
        </span>
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={v.install}
          onChange={async (e) => {
            const install = e.target.checked;
            try {
              await api.setUpdateInstall(install);
              setV({ ...v, install, auto: install ? true : v.auto });
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        />
        <span>
          {t("Install updates automatically")}
          <span className="muted small block">{t("New versions install on their own, checked against SulcusAI's signing key. It waits until nothing is running (a reply, a meeting, a download), then SulcusAI restarts.")}</span>
        </span>
      </label>
      {installing && <InstallModal update={installing} onClose={() => setInstalling(null)} toast={toast} />}
    </div>
  );
}

/** Status bar: "Update to x" when a check found one, else "Check for
 * updates", which first says what the check sends. */
export function UpdatePill({ toast }: { toast: PushToast }) {
  const [found, setFound] = useState<UpdateAvailable | null>(null);
  const [open, setOpen] = useState(false);
  const [asking, setAsking] = useState(false);
  const [checking, setChecking] = useState(false);
  const [current, setCurrent] = useState("");
  useEffect(() => {
    api.updateView().then((v) => {
      setFound(v.available);
      setCurrent(v.current);
    }).catch(() => {});
    const subs = [
      on("update:available", (u) => setFound(u)),
      on("update:installing", () => toast(t("Installing the update. SulcusAI will restart in a moment."), "info")),
    ];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, [toast]);
  const check = async () => {
    setAsking(false);
    setChecking(true);
    try {
      const u = await api.checkForUpdate();
      if (u) {
        setFound(u);
        setOpen(true);
      } else {
        toast(t("You have the latest version ({version}).", { version: current }), "success");
      }
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setChecking(false);
    }
  };
  if (found) {
    return (
      <>
        <button className="status-item update-pill" onClick={() => setOpen(true)} title={t("A newer version of SulcusAI is ready")}>
          ⬆ {t("Update to {version}", { version: found.version })}
        </button>
        {open && <InstallModal update={found} onClose={() => setOpen(false)} toast={toast} />}
      </>
    );
  }
  return (
    <>
      <button className="status-item link-btn" onClick={() => setAsking(true)} disabled={checking} title={t("See if a newer version of SulcusAI is out")}>
        {checking ? t("Checking…") : `⟳ ${t("Check for updates")}`}
      </button>
      {asking && (
        <Modal title={t("Check for updates?")} onClose={() => setAsking(false)}>
          <p>{t("This contacts GitHub, where SulcusAI's releases are published, to see if a version newer than {version} is out.", { version: current })}</p>
          <p className="small"><strong>{t("What GitHub receives:")}</strong></p>
          <ul className="small privacy-list">
            <li>{t("A request for SulcusAI's public release file (latest.json)")}</li>
            <li>{t("Your IP address and the time, as with any website visit")}</li>
            <li>{t("The name and version of the update tool (tauri-plugin-updater)")}</li>
          </ul>
          <p className="small">{t("Nothing else: no chats, files, accounts, settings or anything about you or this PC. Installing a new version is still up to you.")}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setAsking(false)}>{t("Cancel")}</button>
            <button className="btn primary" onClick={check}>{t("Check now")}</button>
          </div>
        </Modal>
      )}
    </>
  );
}
