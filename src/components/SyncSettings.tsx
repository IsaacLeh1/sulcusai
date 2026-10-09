// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type SyncView } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

function when(ms: number | null): string {
  if (!ms) return t("not yet");
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return t("just now");
  if (s < 3600) return t("{n} min ago", { n: Math.round(s / 60) });
  if (s < 86400) return t("{n} h ago", { n: Math.round(s / 3600) });
  return new Date(ms).toLocaleDateString();
}

/** Settings → Your devices: sync with the user's other PCs on this network. */
export function SyncSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<SyncView | null>(null);
  const [entering, setEntering] = useState<{ code: string; target: string; busy: boolean } | null>(null);
  const [name, setName] = useState<string | null>(null);

  const load = useCallback(() => {
    api.syncView().then(setV).catch(() => {});
  }, []);
  useEffect(() => {
    load();
    const subs = [on("sync:status", load), on("sync:changed", load)];
    // Keeps "last synced" and the code's countdown current.
    const t = window.setInterval(load, 5000);
    return () => {
      window.clearInterval(t);
      subs.forEach((s) => s.then((un) => un()));
    };
  }, [load]);
  if (!v) return null;

  const run = async (f: () => Promise<SyncView>) => {
    try {
      setV(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const showing = v.nearby.filter((n) => n.pairing);
  const pair = async () => {
    if (!entering) return;
    setEntering({ ...entering, busy: true });
    try {
      const target = entering.target || (showing.length === 1 ? showing[0].id : "");
      setV(await api.pairDevice(entering.code, target || null));
      setEntering(null);
      toast(t("Paired. Your chats, notes and tasks start syncing now."), "success");
    } catch (e) {
      setEntering({ ...entering, busy: false });
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      <h2>{t("Your devices")}</h2>
      <p className="muted small">
        {t("Keep your chats, projects, memory, notes, tasks and “About you” the same on your PCs. They sync directly over your home or office network, encrypted, with no cloud in between, so this works on Offline too. Models, pictures and video, meetings, mail and calendar accounts, connectors, plugins, schedules and other settings stay on each PC. Incognito chats never sync.")}
      </p>
      <div className="toggle-row">
        <div>
          <strong>{t("Sync with my other PCs")}</strong>
          <span className="small muted block">
            {v.enabled ? (v.listening ? t("On. SulcusAI needs to be open on both PCs.") : t("Starting…")) : t("Off. Nothing is shared.")}
          </span>
        </div>
        <label className="switch" title={v.enabled ? t("On") : t("Off")}>
          <input type="checkbox" checked={v.enabled} onChange={(e) => run(() => api.setSync(e.target.checked, null))} aria-label={t("Sync with my other PCs")} />
          <span />
        </label>
      </div>

      <label className="form small">
        {t("This PC's name")}
        <input
          className="input"
          value={name ?? v.name}
          maxLength={60}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            if (name !== null && name.trim() && name !== v.name) run(() => api.setSync(v.enabled, name));
            setName(null);
          }}
        />
      </label>

      {v.peers.length > 0 && (
        <ul className="teach-list small">
          {v.peers.map((p) => (
            <li key={p.id}>
              <span>
                <strong>{p.name}</strong>{" "}
                <span className="muted">
                  · {p.syncing ? t("syncing…") : t("synced {when}", { when: when(p.last_sync) })}
                  {v.enabled && (p.online ? " · " + t("on this network") : " · " + t("not seen on this network"))}
                </span>
                {p.error && <span className="warn-text block">{p.error}</span>}
              </span>
              <button
                className="btn ghost small"
                onClick={() => {
                  if (window.confirm(t("Stop syncing with {name}? What's already on each PC stays there.", { name: p.name }))) run(() => api.removeDevice(p.id));
                }}
              >
                {t("Unpair")}
              </button>
            </li>
          ))}
        </ul>
      )}

      {v.code ? (
        <div className="pair-code" role="status">
          <span className="small">{t("On your other PC, open Settings → Your devices, choose “Enter a code” and type:")}</span>
          <strong className="code">{v.code}</strong>
          <span className="muted small">
            {t("Good for {time} more.", { time: `${Math.floor(v.code_left / 60)}:${String(v.code_left % 60).padStart(2, "0")}` })}
            {v.address && " " + t("If the other PC doesn't find this one by itself, type this address there too: {address}", { address: v.address })}
          </span>
          <button className="btn ghost small" onClick={() => run(() => api.cancelPairing())}>
            {t("Cancel")}
          </button>
        </div>
      ) : entering ? (
        <div className="form">
          <label>
            {t("Code shown on the other PC")}
            <input
              className="input code-input"
              value={entering.code}
              autoFocus
              placeholder="K7Q2-9XMB"
              maxLength={12}
              onChange={(e) => setEntering({ ...entering, code: e.target.value.toUpperCase() })}
              onKeyDown={(e) => e.key === "Enter" && pair()}
            />
          </label>
          {showing.length > 1 && (
            <label>
              {t("Which PC")}
              <select className="input" value={entering.target} onChange={(e) => setEntering({ ...entering, target: e.target.value })}>
                <option value="">{t("Choose…")}</option>
                {showing.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.name}
                  </option>
                ))}
              </select>
            </label>
          )}
          {showing.length === 0 && (
            <label>
              {t("The other PC's address (only if it isn't found by itself)")}
              <input className="input" value={entering.target} placeholder="192.168.1.20" onChange={(e) => setEntering({ ...entering, target: e.target.value })} />
            </label>
          )}
          {showing.length === 1 && <p className="muted small">{t("Pairing with {name}.", { name: showing[0].name })}</p>}
          <div className="adv-actions">
            <button className="btn primary" disabled={entering.busy || entering.code.replace(/[^0-9A-Za-z]/g, "").length !== 8} onClick={pair}>
              {entering.busy ? t("Pairing…") : t("Pair")}
            </button>
            <button className="btn ghost" onClick={() => setEntering(null)}>
              {t("Cancel")}
            </button>
          </div>
        </div>
      ) : (
        <div className="row wrap">
          <button className="btn" onClick={() => run(() => api.startPairing())}>
            {t("Pair another PC: show a code")}
          </button>
          <button className="btn" onClick={() => setEntering({ code: "", target: "", busy: false })}>
            {t("Enter a code")}
          </button>
          {v.peers.length > 0 && v.enabled && (
            <button className="btn ghost" onClick={() => run(() => api.syncNow())}>
              {t("Sync now")}
            </button>
          )}
        </div>
      )}
      {v.enabled && (
        <p className="muted small">
          {t("The first time, Windows may ask whether SulcusAI may use private networks: allow it, or the other PC can't reach this one. Both PCs need SulcusAI open (and unlocked) to sync.")}
        </p>
      )}
    </section>
  );
}
