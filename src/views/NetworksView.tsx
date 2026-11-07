// SPDX-License-Identifier: AGPL-3.0-only
// Networks as a tab of its own: the network list with members on the left
// of the page, and what this PC brings to them (games, firewall, shares,
// waking) beside it.
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import { t } from "../i18n";
import type { PushToast } from "../components/Toasts";
import { Networks, nets, type NetExtras } from "./groups/Networks";
import "./groups/social.css";

export function NetworkIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="9" y="2" width="6" height="5" rx="1" />
      <rect x="2" y="17" width="6" height="5" rx="1" />
      <rect x="16" y="17" width="6" height="5" rx="1" />
      <path d="M12 7v5M5 17v-2.5h14V17M12 12v2.5" />
    </svg>
  );
}

export function NetworksView({ toast }: { toast: PushToast }) {
  useEffect(() => {
    const un = listen<{ from: string; path: string }>("social:net_file", (e) => toast(t("A file came from {from}: {path}", { from: e.payload.from, path: e.payload.path })));
    return () => {
      un.then((f) => f());
    };
  }, [toast]);
  return (
    <div className="page networks-page">
      <h2>{t("Networks")}</h2>
      <p className="small muted">{t("Private networks between PCs, as if everyone were on the same local network: LAN games, shared folders, remote desktop.")}</p>
      <div className="networks-layout" style={{ display: "flex", flexWrap: "wrap", gap: 16, alignItems: "flex-start" }}>
        <div className="networks-main" style={{ flex: "2 1 520px", minWidth: 0 }}>
          <Networks toast={toast} />
        </div>
        <NetSide toast={toast} />
      </div>
    </div>
  );
}

export default NetworksView;

function NetSide({ toast }: { toast: PushToast }) {
  const [x, setX] = useState<NetExtras | null>(null);
  const load = useCallback(() => {
    nets.extras().then(setX).catch((e) => toast(errorText(e)));
  }, [toast]);
  useEffect(() => {
    load();
    const un = listen("social:networks", load);
    // Games start and stop on their own.
    const id = window.setInterval(load, 15000);
    return () => {
      window.clearInterval(id);
      un.then((f) => f());
    };
  }, [load]);
  if (!x) return null;
  const running = x.games.filter((g) => g.running);
  const copy = (s: string) => navigator.clipboard.writeText(s).then(() => toast(t("Copied {s}", { s })));
  return (
    <aside className="networks-side" style={{ flex: "1 1 260px", minWidth: 0, display: "flex", flexDirection: "column", gap: 12 }}>
      <section>
        <h3>{t("Games")}</h3>
        {running.length > 0 ? (
          <div className="note small">{t("{games} is running. Turn a network on so the others can join.", { games: running.map((g) => g.name).join(", ") })}</div>
        ) : (
          <p className="small muted">{t("No LAN game running right now.")}</p>
        )}
        <details>
          <summary className="small">{t("How to host and join ({n} games)", { n: x.games.length })}</summary>
          {x.games.map((g) => (
            <div key={g.exe + g.name} className="small" style={{ marginTop: 6 }}>
              <strong>{g.name}</strong>
              {g.running && <span className="radmin-status-badge online" style={{ marginLeft: 6 }}>{t("Running")}</span>}
              <div className="muted">{g.notes}</div>
            </div>
          ))}
        </details>
      </section>

      <section>
        <h3>{t("Firewall")}</h3>
        <label className="row small">
          <input
            type="checkbox"
            checked={x.firewall}
            onChange={(e) =>
              nets
                .setFirewall(e.target.checked)
                .then(load)
                .catch((err) => toast(errorText(err)))
            }
          />
          {t("Let network members through the Windows firewall")}
        </label>
        <p className="small muted">{t("When on, joining or turning on a network adds a rule that lets only its members (10.77.x.x) reach games on this PC. Leaving removes it. Windows asks for admin rights for each change.")}</p>
        {Object.entries(x.rules).some(([, on]) => on) && <p className="small">{t("Rules in Windows now: {n}", { n: Object.values(x.rules).filter(Boolean).length })}</p>}
      </section>

      <section>
        <h3>{t("Shared folders and printers")}</h3>
        {x.shared.length === 0 ? (
          <p className="small muted">{t("This PC shares no folders or printers.")}</p>
        ) : (
          <>
            {x.shared.map((s) => (
              <div key={s.name} className="small">
                {s.printer ? t("Printer") : t("Folder")}: <strong>{s.name}</strong> {s.path && <span className="muted">{s.path}</span>}
              </div>
            ))}
            <p className="small" style={{ marginTop: 6 }}>
              {x.can_see.length === 0
                ? t("No network is on, so no members can see them.")
                : x.sharing_open
                  ? t("Can see them now: {names}", { names: x.can_see.join(", ") })
                  : t("Members of the network that is on ({names}) could see them if File and Printer Sharing were allowed in the firewall; right now Windows blocks it.", { names: x.can_see.join(", ") })}
            </p>
          </>
        )}
      </section>

      <section>
        <h3>{t("This PC")}</h3>
        <p className="small muted">{t("Give a card address to people you allow to wake this PC. Wake-on-LAN also has to be on in this PC's setup (BIOS) and network card settings.")}</p>
        {x.my_macs.map((m) => (
          <div key={m} className="row small">
            <code>{m}</code>
            <button className="link small" onClick={() => copy(m)}>
              {t("Copy")}
            </button>
          </div>
        ))}
        <button className="link small" onClick={() => nets.receivedFolder().catch((e) => toast(errorText(e)))}>
          {t("Open received files")}
        </button>
      </section>
    </aside>
  );
}
