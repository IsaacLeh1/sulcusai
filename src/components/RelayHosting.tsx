// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { confirmed } from "../confirm";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

interface LocalRelay {
  running: boolean;
  port: number;
  id: string | null;
  folder: string;
}

interface Guide {
  steps: string[];
  cloudInit: string;
  commands: string;
}

/** Running a relay on this PC, and the guide for hosting one in the person's own cloud account. */
export function RelayHosting({ toast, ownHosting }: { toast: PushToast; ownHosting: boolean }) {
  const [local, setLocal] = useState<LocalRelay | null>(null);
  const [port, setPort] = useState("4001");
  const [hosting, setHosting] = useState(ownHosting);
  const [provider, setProvider] = useState<"oracle" | "cloudflare">("oracle");
  const [capGb, setCapGb] = useState("8000");
  const [guide, setGuide] = useState<Guide | null>(null);
  useEffect(() => {
    invoke<LocalRelay>("p2p_local_relay").then((r) => (setLocal(r), setPort(String(r.port)))).catch(() => {});
  }, []);
  const fail = (e: unknown) => toast(errorText(e), "error");
  const start = async () => {
    if (!(await confirmed(t("Run a relay on this PC? It keeps running after SulcusAI closes, listens on port {port} on every network, and Windows may ask to let it through the firewall. Only people you approve can use it.", { port })))) return;
    invoke<LocalRelay>("p2p_local_relay_start", { port: Number(port) }).then(setLocal).catch(fail);
  };
  const setOwn = async (on: boolean) => {
    if (on && !(await confirmed(t("This writes setup steps for your own cloud account. It's your account and your bill if you change its limits; the provider may reclaim idle free machines; and you are the relay's operator. SulcusAI never signs in or creates anything there."))))
      return;
    invoke<boolean>("p2p_set_own_hosting", { on }).then((v) => (setHosting(v), v || setGuide(null))).catch(fail);
  };
  const write = () =>
    invoke<Guide>("p2p_hosting_guide", { provider, port: Number(port), capGb: Number(capGb) }).then(setGuide).catch(fail);
  const address = local?.id ? `/ip4/<${t("this PC's public address")}>/udp/${local.port}/quic-v1/p2p/${local.id}` : "";
  return (
    <details className="small relay-hosting">
      <summary>{t("Run your own relay")}</summary>
      <h4>{t("Run a relay on this PC")}</h4>
      <p className="muted">{t("Starts the Sulcus Relay in the background with this PC as its owner, so you can manage it under Your relay.")}</p>
      <div className="row">
        <input className="input" style={{ width: "6em" }} value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} aria-label={t("Port")} />
        {local?.running ? (
          <button className="btn small" onClick={() => invoke<LocalRelay>("p2p_local_relay_stop").then(setLocal).catch(fail)}>{t("Stop the relay")}</button>
        ) : (
          <button className="btn small" disabled={!port} onClick={start}>{t("Start the relay")}</button>
        )}
        <span className="muted">{local?.running ? t("Running") : t("Not running")}</span>
      </div>
      {address && <p className="muted">{t("Its address: {address}", { address })}</p>}
      <div className="row">
        <button className="btn small ghost" onClick={() => invoke("p2p_local_relay_at_boot", { on: true }).then(() => toast(t("The relay now starts when this PC starts."), "success")).catch(fail)}>{t("Start at boot")}</button>
        <button className="btn small ghost" onClick={() => invoke("p2p_local_relay_at_boot", { on: false }).then(() => toast(t("The relay no longer starts at boot."), "success")).catch(fail)}>{t("Don't start at boot")}</button>
      </div>
      <h4>{t("Host on your own cloud account")}</h4>
      <label className="row">
        <input type="checkbox" checked={hosting} onChange={(e) => setOwn(e.target.checked)} />
        <span>{t("Show the setup guide for your own Oracle Always Free or Cloudflare account")}</span>
      </label>
      {hosting && (
        <>
          <div className="row">
            <select className="input" value={provider} onChange={(e) => setProvider(e.target.value as "oracle" | "cloudflare")} aria-label={t("Provider")}>
              <option value="oracle">{t("Oracle Always Free")}</option>
              <option value="cloudflare">{t("Cloudflare (name for the server)")}</option>
            </select>
            <input className="input" style={{ width: "6em" }} value={capGb} onChange={(e) => setCapGb(e.target.value.replace(/\D/g, ""))} aria-label={t("Monthly traffic cap (GB)")} title={t("Monthly traffic cap (GB)")} />
            <button className="btn small" onClick={write}>{t("Write the steps")}</button>
          </div>
          {guide && (
            <>
              <ol>
                {guide.steps.map((s) => (
                  <li key={s}>{s}</li>
                ))}
              </ol>
              <p>{t("Cloud-init (paste into the server's user data):")}</p>
              <textarea className="input mono" readOnly rows={10} value={guide.cloudInit} aria-label={t("Cloud-init")} />
              <p>{t("Commands to run yourself:")}</p>
              <textarea className="input mono" readOnly rows={6} value={guide.commands} aria-label={t("Commands")} />
            </>
          )}
        </>
      )}
    </details>
  );
}
