// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";
import { RemoteView } from "./RemoteView";
import {
  modelCardFetch,
  setSharing,
  shareFetch,
  shareList,
  shareMake,
  shareRead,
  shareStop,
  sharingSettings,
  sizeText,
  vlanConnect,
  vlanCreate,
  vlanDisconnect,
  vlanList,
  vlanStatus,
  vpnList,
  type Hosted,
  type Network,
  type SharingSettings as S,
  type Vpn,
} from "../api/sharing";

interface Friend {
  peer: string;
  name: string;
}

/** Settings: file sharing, community models, virtual LANs and remote control. Each part off until turned on, and only friends are served. */
export function SharingSettings({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<S | null>(null);
  const [hosted, setHosted] = useState<Hosted[]>([]);
  const [nets, setNets] = useState<Network[]>([]);
  const [vpns, setVpns] = useState<Vpn[]>([]);
  const [friends, setFriends] = useState<Friend[]>([]);
  const [lanOn, setLanOn] = useState<string | null>(null);
  const [path, setPath] = useState("");
  const [pasted, setPasted] = useState("");
  const [saveDir, setSaveDir] = useState("");
  const [progress, setProgress] = useState("");
  const [netName, setNetName] = useState("");
  const [controlling, setControlling] = useState<string | null>(null);
  const fail = useCallback((e: unknown) => toast(errorText(e), "error"), [toast]);

  const refresh = () => {
    sharingSettings().then(setS).catch(() => {});
    shareList().then(setHosted).catch(() => {});
    vlanList().then(setNets).catch(() => {});
    vlanStatus().then(setLanOn).catch(() => {});
    vpnList().then(setVpns).catch(() => {});
    invoke<Friend[]>("p2p_friends").then(setFriends).catch(() => {});
  };
  useEffect(refresh, []);
  useEffect(() => {
    const un = listen<{ done: number; total: number }>("share:progress", (e) => setProgress(`${e.payload.done} / ${e.payload.total}`));
    return () => {
      un.then((f) => f());
    };
  }, []);
  const stopControlling = useCallback(() => setControlling(null), []);
  if (!s) return null;

  const flip = (k: keyof S, v: boolean) =>
    setSharing({ ...s, [k]: v })
      .then(setS)
      .catch(fail);
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
  const friendName = (peer: string) => friends.find((f) => f.peer === peer)?.name ?? `${peer.slice(0, 10)}...`;

  return (
    <div className="sharing-settings">
      <h3>{t("Sharing and private networks")}</h3>

      {row("fileSharing", t("File sharing"), t("Share a file by link or .sulcus file. Pieces are encrypted with a key only the link carries; this PC hosts its own shares, for friends only."))}
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
                .catch(fail)
            }
          >
            {t("Share")}
          </button>
          {hosted.map((h) => (
            <p key={h.id} className="small">
              {h.name} ({sizeText(h.size)}){" "}
              <button className="link" onClick={() => navigator.clipboard?.writeText(h.link).then(() => toast(t("Share link copied."), "success"))}>
                {t("Copy link")}
              </button>{" "}
              <button className="link" onClick={() => shareStop(h.id).then(refresh).catch(fail)}>
                {t("Stop sharing")}
              </button>
            </p>
          ))}
          <textarea value={pasted} onChange={(e) => setPasted(e.target.value)} rows={2} placeholder={t("Paste a share link or .sulcus file")} aria-label={t("Paste a share link or .sulcus file")} />
          <input value={saveDir} onChange={(e) => setSaveDir(e.target.value)} placeholder={t("Folder to save into")} aria-label={t("Folder to save into")} />
          <button
            disabled={!pasted.trim()}
            onClick={() =>
              shareRead(pasted)
                .then((i) => toast(t("{name}, {size}", { name: i.name, size: sizeText(i.size) }), "info"))
                .catch(fail)
            }
          >
            {t("Check link")}
          </button>
          <button
            disabled={!pasted.trim() || !saveDir.trim()}
            onClick={() => {
              setProgress("...");
              shareFetch(pasted, saveDir.trim())
                .then((p) => {
                  toast(t("Saved to {path}", { path: p }), "success");
                  setPasted("");
                })
                .catch(fail)
                .finally(() => setProgress(""));
            }}
          >
            {t("Download")}
          </button>
          {progress && <span className="small muted">{progress}</span>}
        </div>
      )}

      {row("modelCards", t("Community models"), t("Pass signed model cards between friends and groups. No list is kept by anyone else."))}
      {s.modelCards && friends.length > 0 && (
        <div className="network-options">
          {friends.map((f) => (
            <button key={f.peer} className="secondary" onClick={() => modelCardFetch(f.peer).then(() => toast(t("Cards from {name} added.", { name: f.name }), "success")).catch(fail)}>
              {t("Get cards from {name}", { name: f.name })}
            </button>
          ))}
        </div>
      )}

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
                .catch(fail)
            }
          >
            {t("Create network")}
          </button>
          {nets.map((n) => (
            <p key={n.id} className="small">
              {n.name}: {Object.entries(n.members).map(([p, a]) => `${friendName(p)} ${a}`).join(", ")}{" "}
              {lanOn === n.id ? (
                <button className="link" onClick={() => vlanDisconnect().then(refresh).catch(fail)}>
                  {t("Disconnect")}
                </button>
              ) : (
                <button className="link" onClick={() => vlanConnect(n.id).then((a) => toast(t("Connected as {addr}.", { addr: a }), "success")).then(refresh).catch(fail)}>
                  {t("Connect")}
                </button>
              )}
            </p>
          ))}
          {vpns.length > 0 && (
            <>
              <strong className="small">{t("VPNs on this PC")}</strong>
              {vpns.map((v) => (
                <p key={v.adapter} className="small muted">
                  {v.kind}: {v.adapter} ({v.up ? t("connected") : t("off")})
                </p>
              ))}
            </>
          )}
        </div>
      )}

      {row("remoteControl", t("Remote control"), t("Let a friend control this PC after you accept within 10 seconds and the codes match. A border and a bar with Stop show while it runs; Ctrl+Alt+Backspace ends it."))}
      {friends.length > 0 && !controlling && (
        <div className="network-options">
          {friends.map((f) => (
            <button key={f.peer} className="secondary" onClick={() => setControlling(f.peer)}>
              {t("Control {name}'s PC", { name: f.name })}
            </button>
          ))}
        </div>
      )}
      {controlling && <RemoteView peer={controlling} onClose={stopControlling} />}
    </div>
  );
}
