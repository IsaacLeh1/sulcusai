// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";
import { setSharing, shareList, shareMake, shareRead, shareStop, sharingSettings, sizeText, vlanCreate, vlanList, remoteStop, type Hosted, type Network, type SharingSettings as S } from "../api/sharing";

/** Settings: file sharing, community models, virtual LANs and remote control. Each part off until turned on. */
export function SharingSettings({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<S | null>(null);
  const [hosted, setHosted] = useState<Hosted[]>([]);
  const [nets, setNets] = useState<Network[]>([]);
  const [path, setPath] = useState("");
  const [pasted, setPasted] = useState("");
  const [netName, setNetName] = useState("");
  const refresh = () => {
    sharingSettings().then(setS).catch(() => {});
    shareList().then(setHosted).catch(() => {});
    vlanList().then(setNets).catch(() => {});
  };
  useEffect(refresh, []);
  if (!s) return null;
  const flip = (k: keyof S, v: boolean) =>
    setSharing({ ...s, [k]: v })
      .then((n) => {
        setS(n);
        if (k === "remoteControl" && !v) remoteStop().catch(() => {});
      })
      .catch((e) => toast(errorText(e), "error"));
  const row = (k: keyof S, title: string, help: string) => (
    <div className="toggle-row" key={k}>
      <div>
        <strong>{title}</strong>
        <span className="small muted block">{help}</span>
      </div>
      <label className="switch">
        <input type="checkbox" checked={s[k]} onChange={(e) => flip(k, e.target.checked)} aria-label={title} />
        <span />
      </label>
    </div>
  );
  return (
    <div className="sharing-settings">
      <h3>{t("Sharing and private networks")}</h3>
      {row("fileSharing", t("File sharing"), t("Share a file by link or .sulcus file. Pieces are encrypted with a key only the link carries, and this PC hosts its own shares."))}
      {s.fileSharing && (
        <div className="network-options">
          <input value={path} onChange={(e) => setPath(e.target.value)} placeholder={t("Path of a file to share")} aria-label={t("Path of a file to share")} />
          <button
            disabled={!path.trim()}
            onClick={() =>
              shareMake(path.trim())
                .then((m) => {
                  navigator.clipboard?.writeText(m.link).catch(() => {});
                  toast(t("Share link copied."), "success");
                  setPath("");
                  refresh();
                })
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {t("Share")}
          </button>
          {hosted.map((h) => (
            <p key={h.id} className="small">
              {h.name} ({sizeText(h.size)}){" "}
              <button className="link" onClick={() => shareStop(h.id).then(refresh).catch((e) => toast(errorText(e), "error"))}>
                {t("Stop sharing")}
              </button>
            </p>
          ))}
          <input value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("Paste a share link")} aria-label={t("Paste a share link")} />
          <button
            disabled={!pasted.trim()}
            onClick={() =>
              shareRead(pasted)
                .then((i) => toast(t("{name}, {size}", { name: i.name, size: sizeText(i.size) }), "info"))
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {t("Check link")}
          </button>
        </div>
      )}
      {row("modelCards", t("Community models"), t("Pass signed model cards between people and groups. There is no list kept by anyone else."))}
      {row("virtualLan", t("Virtual LAN"), t("A private network run by the people in it. LAN games see each other as if on the same network."))}
      {s.virtualLan && (
        <div className="network-options">
          <input value={netName} onChange={(e) => setNetName(e.target.value)} placeholder={t("Network name")} aria-label={t("Network name")} />
          <button
            disabled={!netName.trim()}
            onClick={() =>
              vlanCreate(netName)
                .then(() => {
                  setNetName("");
                  refresh();
                })
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {t("Create network")}
          </button>
          {nets.map((n) => (
            <p key={n.id} className="small">
              {n.name}: {t("{count} members", { count: Object.keys(n.members).length })}
            </p>
          ))}
        </div>
      )}
      {row("remoteControl", t("Remote control"), t("Let a friend control this PC after you accept within 10 seconds and the codes match. A border and a bar with Stop show while it runs."))}
    </div>
  );
}
