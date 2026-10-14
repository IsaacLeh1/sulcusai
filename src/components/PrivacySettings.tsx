// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, on, type PrivacySettings as Settings, type PrivacyTest, type PrivacyView } from "../api";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

/** Each kind of traffic that can go the private way, with what turning it on can cause. */
const ROUTES: { key: "web" | "updates" | "mail" | "models"; label: string; detail: string; warning?: string }[] = [
  { key: "web", label: tx("Web search, pages and weather"), detail: tx("Sites you search and read see the other address and place.") },
  { key: "updates", label: tx("Update checks"), detail: tx("GitHub doesn't see your address when SulcusAI checks for a new version.") },
  {
    key: "mail",
    label: tx("Email and calendar"),
    detail: tx("Syncing, sending, and renewing sign-ins."),
    warning: tx("Google and Microsoft may see a sign-in from far away and ask you to confirm it's you, or lock the account for a while. Syncing gets slower, and some mail servers refuse Tor."),
  },
  {
    key: "models",
    label: tx("Model downloads"),
    detail: tx("Downloading models and the AI engine."),
    warning: tx("Models are several GB. Through Tor they can take hours and load a network run by volunteers; your own VPN or proxy is better for these."),
  },
];

/** Settings → Private connection: SulcusAI's own traffic through Tor or the user's proxy. */
export function PrivacySettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<PrivacyView | null>(null);
  const [proxy, setProxy] = useState("");
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<PrivacyTest | string | null>(null);

  useEffect(() => {
    const load = () => api.privacyView().then((view) => {
      setV(view);
      setProxy((p) => p || view.settings.proxy);
    }).catch(() => {});
    load();
    const sub = on("privacy:status", load);
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!v) return null;
  const s = v.settings;

  const save = async (next: Settings) => {
    setV({ ...v, settings: next });
    setTest(null);
    try {
      await api.setPrivacy(next);
      setV(await api.privacyView());
    } catch (e) {
      toast(errorText(e), "error");
      setV(await api.privacyView());
    }
  };

  const status = !s.enabled
    ? t("Off. SulcusAI connects directly, like most apps.")
    : v.state === "stuck"
      ? v.problem
      : v.state === "starting"
        ? t("Starting Tor… {progress}%", { progress: v.progress })
        : v.state === "waiting"
          ? t("On. Tor connects the first time something goes through it.")
          : s.via === "tor"
          ? t("On, through Tor.")
          : t("On, through your proxy.");

  return (
    <section className="card privacy-card">
      <h2>{t("Private connection")}</h2>
      <p className="muted small">
        {t("Sends SulcusAI's own internet traffic through Tor or a proxy/VPN you already have, so the sites it contacts see another address, not yours or where you are. Nothing is hosted by SulcusAI.")}{" "}
        {t("It covers SulcusAI only, not your browser or the rest of this PC.")}
      </p>
      <div className="toggle-row">
        <div>
          <strong>{t("Private connection")}</strong>
          <span className={`small block ${v.state === "stuck" && s.enabled ? "warn-text" : "muted"}`}>{status}</span>
        </div>
        <label className="switch" title={s.enabled ? t("On") : t("Off")}>
          <input type="checkbox" checked={s.enabled} onChange={(e) => save({ ...s, enabled: e.target.checked, proxy: proxy.trim() })} aria-label={t("Private connection")} />
          <span />
        </label>
      </div>
      {s.enabled && v.state === "starting" && (
        <div className="progress" aria-hidden>
          <span style={{ width: `${Math.max(v.progress, 3)}%` }} />
        </div>
      )}

      {s.enabled && (
        <>
          <h3>{t("Go through")}</h3>
          <div className="levels" role="radiogroup" aria-label={t("Go through")}>
            <button role="radio" aria-checked={s.via === "tor"} className={`level ${s.via === "tor" ? "active" : ""}`} onClick={() => s.via !== "tor" && save({ ...s, via: "tor" })}>
              <span className="conn-icon" aria-hidden>🧅</span>
              <span>
                <strong>{t("Tor (built in)")}</strong>
                <span className="small muted block">{t("Free, nothing to set up, run by volunteers worldwide. Slower, and some sites show extra checks to Tor users. Some school and work networks block it.")}</span>
              </span>
            </button>
            <button role="radio" aria-checked={s.via === "proxy"} className={`level ${s.via === "proxy" ? "active" : ""}`} onClick={() => {
                if (s.via === "proxy") return;
                if (proxy.trim()) save({ ...s, via: "proxy", proxy: proxy.trim() });
                else toast(t("Type your proxy's address below first."));
              }}>
              <span className="conn-icon" aria-hidden>🛡️</span>
              <span>
                <strong>{t("Your own proxy or VPN")}</strong>
                <span className="small muted block">{t("A SOCKS5 or HTTP proxy you already use. Many VPN apps offer one in their settings.")}</span>
              </span>
            </button>
          </div>
          <div className="row wrap">
            <input
              className="input"
              value={proxy}
              placeholder={t("e.g. 127.0.0.1:1080 or socks5://user:password@host:1080")}
              aria-label={t("Proxy address")}
              onChange={(e) => setProxy(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && proxy.trim() && save({ ...s, via: "proxy", proxy: proxy.trim() })}
              style={{ flex: 1, minWidth: 220 }}
            />
            <button className="btn small" disabled={!proxy.trim() || (s.via === "proxy" && proxy.trim() === s.proxy)} onClick={() => save({ ...s, via: "proxy", proxy: proxy.trim() })}>
              {s.via === "proxy" ? t("Save") : t("Use this proxy")}
            </button>
          </div>

          <h3>{t("What goes this way")}</h3>
          {ROUTES.map((r) => (
            <label key={r.key} className="check privacy-route">
              <input type="checkbox" checked={s[r.key]} onChange={(e) => save({ ...s, [r.key]: e.target.checked })} />
              <span>
                {t(r.label)}
                <span className="muted small block">{t(r.detail)}</span>
                {r.warning && <span className="warn-text small block">⚠ {t(r.warning)}</span>}
              </span>
            </label>
          ))}
          <p className="muted small">{t("Local AI never leaves this PC either way. Cloud AI and syncing through your own relay go directly: those services know your account anyway.")}</p>

          <div className="row wrap">
            <button
              className="btn small"
              disabled={testing}
              title={t("Asks ipinfo.io which address and place it sees, going the way web traffic goes now.")}
              onClick={async () => {
                setTesting(true);
                setTest(null);
                try {
                  setTest(await api.testPrivacy());
                } catch (e) {
                  setTest(errorText(e));
                } finally {
                  setTesting(false);
                }
              }}
            >
              {testing ? t("Checking…") : t("See what sites see")}
            </button>
            {test && (typeof test === "string"
              ? <span className="warn-text small">{test}</span>
              : <span className="small">{test.place ? t("Sites see {ip}, in {place}.", { ip: test.ip, place: test.place }) : t("Sites see {ip}.", { ip: test.ip })}</span>)}
          </div>
          {!s.web && <p className="muted small">{t("The test goes the way web traffic goes, which is directly now.")}</p>}
        </>
      )}
    </section>
  );
}

/** Status bar: shown while the private connection is on, with Tor's progress or problem. */
export function PrivacyPill({ onOpen }: { onOpen: () => void }) {
  const [v, setV] = useState<PrivacyView | null>(null);
  useEffect(() => {
    const load = () => api.privacyView().then(setV).catch(() => {});
    load();
    const sub = on("privacy:status", load);
    return () => {
      sub.then((un) => un());
    };
  }, []);
  if (!v?.settings.enabled) return null;
  const via = v.settings.via === "tor" ? t("through Tor") : t("through your proxy");
  const [label, title] =
    v.state === "stuck"
      ? [t("Private: stuck"), v.problem]
      : v.state === "starting"
        ? [t("Private: {progress}%", { progress: v.progress }), t("Tor is starting. Until it's ready, traffic that goes the private way waits; it never goes out directly.")]
        : [t("Private"), t("SulcusAI's chosen traffic goes {via}.", { via })];
  return (
    <button className={`status-item link-btn privacy-pill ${v.state === "stuck" ? "warn-text" : ""}`} onClick={onOpen} title={title}>
      <span aria-hidden>{v.state === "stuck" ? "⚠" : "🧅"}</span> {label}
    </button>
  );
}
