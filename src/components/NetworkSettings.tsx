// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

interface P2pStatus {
  settings: { network: boolean };
  running: boolean;
  nodeId: string | null;
  listening: string[];
  connected: string[];
}

interface Friend {
  peer: string;
  name: string;
  kind: string;
  added: number;
}

/** Settings: SulcusAI's own network between people's PCs (no server in between). Off by default. */
export function NetworkSettings({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<P2pStatus | null>(null);
  const [friends, setFriends] = useState<Friend[]>([]);
  const [made, setMade] = useState<{ text: string; warning: string } | null>(null);
  const [pasted, setPasted] = useState("");
  const [busy, setBusy] = useState(false);
  const refresh = () => {
    invoke<P2pStatus>("p2p_status").then(setS).catch(() => {});
    invoke<Friend[]>("p2p_friends").then(setFriends).catch(() => {});
  };
  useEffect(refresh, []);
  if (!s) return null;

  const setNetwork = async (on: boolean) => {
    try {
      setS(await invoke<P2pStatus>("p2p_set_network", { on }));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const make = async () => {
    try {
      setMade(await invoke<{ text: string; warning: string }>("p2p_make_invite", { kind: "friend", days: 7, uses: 1 }));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const redeem = async () => {
    setBusy(true);
    try {
      const f = await invoke<Friend>("p2p_use_invite", { text: pasted.trim() });
      toast(t("Connected with {name}.", { name: f.name }), "success");
      setPasted("");
      refresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card">
      <h2>{t("Network")}</h2>
      <div className="toggle-row">
        <div>
          <strong>{t("Connect with other people's PCs directly")}</strong>
          <span className="small muted block">
            {t("SulcusAI's own network: PCs reach each other with no server in between. For now this works on the same network, or where a router lets the connection in; relays come later. Off: nothing listens.")}
          </span>
        </div>
        <label className="switch">
          <input type="checkbox" checked={s.settings.network} onChange={(e) => setNetwork(e.target.checked)} aria-label={t("Connect with other people's PCs directly")} />
          <span />
        </label>
      </div>
      {s.running && (
        <>
          <p className="small muted">
            {t("This PC's network id:")} <code>{s.nodeId}</code> · {t("{n} connected", { n: s.connected.length })}
          </p>
          <div className="row wrap">
            <button className="btn" onClick={make}>{t("Make an invite")}</button>
          </div>
          {made && (
            <div className="invite-made">
              <p className="small warn-box">{t(made.warning)}</p>
              <textarea className="input" readOnly rows={3} value={made.text} onFocus={(e) => e.target.select()} aria-label={t("Invite")} />
              <button className="btn small" onClick={() => navigator.clipboard.writeText(made.text).then(() => toast(t("Copied."), "success")).catch(() => {})}>{t("Copy")}</button>
              <p className="small muted">{t("Send it to one person. It works once and for 7 days.")}</p>
            </div>
          )}
          <div className="row">
            <input className="input" style={{ flex: 1 }} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("Paste an invite (sulcus:i1…)")} aria-label={t("Paste an invite")} />
            <button className="btn primary" disabled={busy || !pasted.trim()} onClick={redeem}>{busy ? t("Connecting…") : t("Use invite")}</button>
          </div>
          {friends.length > 0 && (
            <ul className="small">
              {friends.map((f) => (
                <li key={f.peer}>{f.name}</li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}
