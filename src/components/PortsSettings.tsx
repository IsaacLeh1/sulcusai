// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { api, errorText, type PortsView, type PortRow } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Settings → Servers and ports: what's listening on this PC, the servers
 * the assistant started (stop, start again), and previews in SulcusAI's browser. */
export function PortsSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PortsView | null>(null);
  const [all, setAll] = useState(false);
  const [stopping, setStopping] = useState<PortRow | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(() => {
    api.portsView().then(setV).catch(() => {});
  }, []);
  useEffect(() => {
    load();
    const id = window.setInterval(() => document.visibilityState === "visible" && load(), 10_000);
    return () => window.clearInterval(id);
  }, [load]);

  const run = async (key: string, f: () => Promise<PortsView | void>) => {
    setBusy(key);
    try {
      const next = await f();
      if (next) setV(next);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const preview = (port: number) => run(`view${port}`, () => api.previewPort(port));
  const portOf = (url: string | null) => {
    const m = url && /:(\d+)/.exec(url.replace(/^\w+:\/\//, "x"));
    return m ? Number(m[1]) : null;
  };

  if (!v) return null;
  const shown = v.ports.filter((p) => all || !p.system);
  return (
    <section className="card ports">
      <div className="adv-head">
        <h2>{t("Servers and ports")}</h2>
        <button className="btn ghost small" onClick={load}>{t("Refresh")}</button>
      </div>
      <p className="muted small">{t("Programs on this PC that are waiting for connections, and the port each one uses. View opens one in SulcusAI's browser.")}</p>

      <h3>{t("Servers the assistant started")}</h3>
      {v.servers.length === 0 ? (
        <p className="muted small">{t("None yet. When the assistant runs a website or app in a shared folder, it shows here.")}</p>
      ) : (
        <ul className="teach-list small ports-servers">
          {v.servers.map((s) => {
            const port = portOf(s.url) ?? v.ports.find((p) => p.server === s.id)?.port ?? null;
            return (
              <li key={s.id}>
                <span className="ellipsis">
                  <code>{s.command.split("\n")[0]}</code>
                  <span className="muted block ellipsis" title={s.folder}>
                    {s.running ? (port ? t("Running on port {port}", { port }) : t("Running")) : t("Stopped")} · {s.folder}
                  </span>
                </span>
                <span className="row">
                  {s.running && port && <button className="btn ghost small" disabled={!!busy} onClick={() => preview(port)}>{t("View")}</button>}
                  {s.running ? (
                    <button className="btn ghost small" disabled={!!busy} onClick={() => run(s.id, () => api.serverStop(s.id))}>{t("Stop")}</button>
                  ) : (
                    <button className="btn small" disabled={!!busy} onClick={() => run(s.id, () => api.serverStart(s.id))}>{busy === s.id ? t("Starting…") : t("Start")}</button>
                  )}
                </span>
              </li>
            );
          })}
        </ul>
      )}

      <div className="adv-head">
        <h3>{t("Ports in use")}</h3>
        <label className="check small">
          <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} />
          <span>{t("Show Windows' own")}</span>
        </label>
      </div>
      {!v.supported ? (
        <p className="muted small">{t("Listing ports works on Windows only for now.")}</p>
      ) : shown.length === 0 ? (
        <p className="muted small">{t("Nothing is listening right now.")}</p>
      ) : (
        <table className="ports-table small">
          <thead>
            <tr>
              <th>{t("Port")}</th>
              <th>{t("Program")}</th>
              <th>{t("Reachable from")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {shown.map((p) => (
              <tr key={`${p.port}-${p.pid}`}>
                <td><strong>{p.port}</strong></td>
                <td className="ellipsis" title={`${p.program} (${p.pid})`}>
                  {p.program} <span className="muted">({p.pid})</span>
                  {p.server && <span className="badge subtle">{t("started by the assistant")}</span>}
                </td>
                <td>{p.local_only ? t("This PC only") : t("Your network too")}</td>
                <td className="row">
                  <button className="btn ghost small" disabled={!!busy} onClick={() => preview(p.port)}>{t("View")}</button>
                  {!p.system && <button className="btn ghost small danger" disabled={!!busy} onClick={() => setStopping(p)}>{t("Stop")}</button>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {stopping && (
        <Modal title={t("Stop {program}?", { program: stopping.program })} onClose={() => setStopping(null)}>
          <p>{t("This ends {program} (process {pid}) and anything it started, freeing port {port}. Anything unsaved in it is lost.", { program: stopping.program, pid: stopping.pid, port: stopping.port })}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setStopping(null)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={() => {
                const p = stopping;
                setStopping(null);
                run(`stop${p.pid}`, () => api.stopPortProgram(p.pid));
              }}
            >
              {t("Stop it")}
            </button>
          </div>
        </Modal>
      )}
    </section>
  );
}
