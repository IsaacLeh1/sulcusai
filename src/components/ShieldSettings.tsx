// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { errorText, on } from "../api";
import { proxyLines, shieldApi, type ShieldSettings as Settings, type ShieldView } from "../api/shield";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

/** Shown with the switch, always: where the shield stops. */
const LIMITS = [
  tx("It reduces tracking. It doesn't make anyone untraceable: what you type into a site, accounts you sign in to and files you share can still say who you are."),
  tx("Your internet provider (or the Wi-Fi's owner) can still see that you use Tor or a VPN, just not which sites you visit through it."),
  tx("People you connect to directly (People, your own VPN, file sharing) still see this PC's real address unless \"Don't show my address to peers\" is on."),
  tx("Cloud AI providers you use with your own key are reached directly: the key already tells them who you are."),
  tx("Some countries block or restrict Tor, and some networks (schools, workplaces) block it. Where using Tor could get you in trouble, don't turn it on."),
  tx("Tor is slower than a direct connection, and some sites show extra checks or refuse Tor addresses."),
];

type Flag = "per_site" | "rotate" | "peers_relays_only" | "random_ports" | "browser_common_fingerprint" | "browser_block_third_party_cookies" | "browser_clear_on_close";

/** The "Address shield" block under Settings → Private connection. */
export function ShieldSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<ShieldView | null>(null);
  const [list, setList] = useState("");
  const [doh, setDoh] = useState("");
  const [renewing, setRenewing] = useState(false);

  useEffect(() => {
    const load = () => shieldApi.view().then((view) => {
      setV(view);
      setList((l) => l || view.settings.proxies.join("\n"));
      setDoh((d) => d || view.settings.doh_custom);
    }).catch(() => {});
    load();
    const sub = on("shield:changed", load);
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!v) return null;
  const s = v.settings;

  const save = async (next: Settings) => {
    setV({ ...v, settings: next });
    try {
      setV(await shieldApi.set(next));
    } catch (e) {
      toast(errorText(e), "error");
      setV(await shieldApi.view());
    }
  };

  const check = (key: Flag, label: string, detail: string, extra?: string) => (
    <label className="check privacy-route">
      <input type="checkbox" checked={s[key]} onChange={(e) => save({ ...s, [key]: e.target.checked })} />
      <span>
        {t(label)}
        <span className="muted small block">{t(detail)}</span>
        {extra && <span className="muted small block">{t(extra)}</span>}
      </span>
    </label>
  );

  const number = (key: "rotate_minutes" | "port_change_hours", label: string, min: number, max: number) => (
    <label className="row small" style={{ marginLeft: 28 }}>
      {t(label)}
      <input
        className="input"
        type="number"
        min={min}
        max={max}
        defaultValue={s[key]}
        key={s[key]}
        style={{ width: 90 }}
        onBlur={(e) => {
          const n = Math.round(Number(e.target.value));
          if (Number.isFinite(n) && n !== s[key]) save({ ...s, [key]: Math.min(max, Math.max(min, n)) });
        }}
      />
    </label>
  );

  return (
    <>
      <h3>{t("Address shield")}</h3>
      <p className="muted small">{t("Hides this PC's internet address from the websites, services and people SulcusAI connects to, and makes SulcusAI harder to follow from one site to the next. It covers SulcusAI only, not other programs on this PC.")}</p>
      <ul className="muted small">
        {LIMITS.map((l) => <li key={l}>{t(l)}</li>)}
      </ul>
      <div className="toggle-row">
        <div>
          <strong>{t("Address shield")}</strong>
          <span className="small block muted">{s.enabled ? t("On. All of SulcusAI's internet traffic goes the shield's way, whatever is chosen above.") : t("Off.")}</span>
        </div>
        <label className="switch" title={s.enabled ? t("On") : t("Off")}>
          <input type="checkbox" checked={s.enabled} onChange={(e) => save({ ...s, enabled: e.target.checked, proxies: proxyLines(list) })} aria-label={t("Address shield")} />
          <span />
        </label>
      </div>
      {v.problems.map((p) => <p key={p} className="warn-text small">{t(p)}</p>)}

      {s.enabled && (
        <>
          <div className="levels" role="radiogroup" aria-label={t("Address shield")}>
            <button role="radio" aria-checked={s.via === "tor"} className={`level ${s.via === "tor" ? "active" : ""}`} onClick={() => s.via !== "tor" && save({ ...s, via: "tor" })}>
              <span>
                <strong>{t("Tor")}</strong>
                <span className="small muted block">{t("Through Tor: traffic goes through three volunteer-run relays, so sites see the last relay's address, not yours. Built in; nothing to install.")}</span>
              </span>
            </button>
            <button role="radio" aria-checked={s.via === "own_list"} className={`level ${s.via === "own_list" ? "active" : ""}`} onClick={() => s.via !== "own_list" && save({ ...s, via: "own_list", proxies: proxyLines(list) })}>
              <span>
                <strong>{t("Your own proxies or VPNs")}</strong>
                <span className="small muted block">{t("Through your own proxies or VPNs: sites see their address. They can see everything SulcusAI sends through them that isn't encrypted, so use ones you trust.")}</span>
              </span>
            </button>
          </div>
          {s.via === "own_list" && (
            <div className="row wrap">
              <textarea
                className="input"
                rows={3}
                value={list}
                placeholder={t("One per line, e.g. 127.0.0.1:1080 or socks5://user:password@host:1080")}
                aria-label={t("Proxy addresses")}
                onChange={(e) => setList(e.target.value)}
                style={{ flex: 1, minWidth: 220 }}
              />
              <button className="btn small" onClick={() => save({ ...s, proxies: proxyLines(list) })}>{t("Save")}</button>
            </div>
          )}
          {s.via === "tor" && v.tor_isolates === false && (
            <p className="warn-text small">{t("This copy of Tor is from before the address shield: it hides this PC's address, but every site shares one route and new addresses wait for SulcusAI's next Tor release.")}</p>
          )}

          {check("per_site", tx("A different address per website"), tx("Each site gets its own Tor route, or its own proxy from your list, so two sites can't tell they're talking to the same PC by its address. Now and then Tor picks the same exit for two sites by chance."), tx("In the built-in browser, the route is chosen per server rather than per page, so an advertiser embedded on many sites can still see one address across them until the next change."))}
          {check("rotate", tx("New address every few minutes"), tx("New connections get a new address. Connections already open (mail waiting for messages, a download in progress) keep theirs until they end."))}
          {s.rotate && number("rotate_minutes", tx("Minutes between changes"), 1, 1440)}
          <div className="row wrap">
            <button
              className="btn small"
              disabled={renewing}
              onClick={async () => {
                setRenewing(true);
                try {
                  await shieldApi.newAddress();
                  toast(t("New address: open connections reconnect."));
                } catch (e) {
                  toast(errorText(e), "error");
                } finally {
                  setRenewing(false);
                }
              }}
            >
              {t("New address now")}
            </button>
            <span className="muted small">{t("Every connection gets a new address, and open ones reconnect. Downloads in progress finish first.")}</span>
          </div>

          {check("peers_relays_only", tx("Don't show my address to peers"), tx("People, VPN and file-sharing connections go only through a relay, so others see the relay's address. Slower, and uses the relay's data."))}
          {check("random_ports", tx("Random ports"), tx("The ports other devices reach SulcusAI on change at each start and now and then (never while someone is connected), so a scan doesn't find SulcusAI by its usual numbers. A phone using the local address scans the new code after a change; one linked through the relay notices nothing. SulcusAI opens no ports on your router."))}
          {s.random_ports && number("port_change_hours", tx("Hours between port changes"), 1, 168)}

          <h3>{t("Looking up site names")}</h3>
          <p className="muted small">{t("Traffic through Tor or your proxies has its names looked up at the far end. This is for what still goes directly.")}</p>
          <div className="levels" role="radiogroup" aria-label={t("Looking up site names")}>
            {([
              ["system", tx("Windows"), tx("Your network's DNS server, as without the shield.")],
              ["tor", tx("Through Tor"), tx("Your network's DNS server doesn't see which sites SulcusAI asks for.")],
              ["encrypted", tx("Encrypted DNS"), tx("Site names are looked up over HTTPS with the provider you choose. Your network doesn't see them; that provider does.")],
            ] as const).map(([key, label, detail]) => (
              <button key={key} role="radio" aria-checked={s.dns === key} className={`level ${s.dns === key ? "active" : ""}`} onClick={() => s.dns !== key && save({ ...s, dns: key })}>
                <span>
                  <strong>{t(label)}</strong>
                  <span className="small muted block">{t(detail)}</span>
                </span>
              </button>
            ))}
          </div>
          {s.dns === "encrypted" && (
            <div className="row wrap">
              <select className="input" value={s.doh_provider} aria-label={t("Encrypted DNS provider")} onChange={(e) => save({ ...s, doh_provider: e.target.value as Settings["doh_provider"], doh_custom: doh.trim() })}>
                <option value="cloudflare">Cloudflare (1.1.1.1)</option>
                <option value="quad9">Quad9 (9.9.9.9)</option>
                <option value="custom">{t("Another provider")}</option>
              </select>
              {s.doh_provider === "custom" && (
                <>
                  <input className="input" value={doh} placeholder="https://dns.example/dns-query" aria-label={t("Encrypted DNS address")} onChange={(e) => setDoh(e.target.value)} style={{ flex: 1, minWidth: 220 }} />
                  <button className="btn small" disabled={doh.trim() === s.doh_custom} onClick={() => save({ ...s, doh_custom: doh.trim() })}>{t("Save")}</button>
                </>
              )}
            </div>
          )}

          <h3>{t("The built-in browser")}</h3>
          {check("browser_common_fingerprint", tx("Common fingerprint"), tx("The browser reports the same values as most Windows PCs instead of this PC's own. It makes this PC blend in more; it doesn't make it identical to everyone else's."))}
          {check("browser_block_third_party_cookies", tx("Block trackers' cookies"), tx("Known advertisers and trackers embedded in pages can't keep cookies or storage that follow you between sites. A few sign-ins and embedded videos may stop working."))}
          {check("browser_clear_on_close", tx("Clear history when the browser closes"), tx("History, cookies and saved site data are deleted, so every session starts signed out."))}
          <p className="muted small">{t("The browser window closes when these change and takes them when it opens again.")}</p>

          <h3>{t("Wi-Fi hardware address")}</h3>
          <p className="muted small">{t("Your Wi-Fi hardware address is seen only by the Wi-Fi network you join, never by websites. Windows can randomize it: Settings > Network & internet > Wi-Fi > Random hardware addresses (for every network), or open a network's properties and set Random hardware address to On or Change daily. SulcusAI never changes Windows settings itself; this button only opens that page.")}</p>
          <button className="btn small" onClick={() => shieldApi.wifiSettings().catch((e) => toast(errorText(e), "error"))}>{t("Open Wi-Fi settings")}</button>
        </>
      )}
    </>
  );
}
