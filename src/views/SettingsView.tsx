// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { ThemePicker } from "../components/ThemePicker";
import { api, errorText, type AppInfo, type Connectivity, type FeatureId, type Profile, type SearchProvider, type SecurityStatus, type WebSettings, type BrowsersView, type MainBrowser, type SavedSiteKey } from "../api";
import { APP_NAME } from "../brand";
import { chosenLanguage, LANGUAGES, setLanguage, t, tx } from "../i18n";
import { CloudConfirm, LEVELS } from "../components/ConnectivityMenu";
import { SecuritySection } from "../components/Security";
import { FoldersSection } from "../components/AgentCards";
import { SignInApps } from "../components/SignIn";
import { VoiceSection } from "../components/VoiceSettings";
import { PerformanceSection } from "../components/Performance";
import { ModelStorage } from "../components/ModelStorage";
import { CleanupSettings } from "../components/CleanupSettings";
import type { PushToast } from "../components/Toasts";
import { AdvancedSettings } from "../components/AdvancedSettings";
import { ApiServerSettings } from "../components/ApiServerSettings";
import { CloudSettings } from "../components/CloudSettings";
import { BackupSettings } from "../components/BackupSettings";
import { SyncSettings } from "../components/SyncSettings";
import { PhoneSettings } from "../components/PhoneSettings";
import { PortsSettings } from "../components/PortsSettings";
import { HandoffSettings } from "../components/HandoffSettings";
import { LogSettings } from "../components/LogSettings";
import { DevModeSetting } from "../components/DevMode";
import { PrivacySettings } from "../components/PrivacySettings";
import { PrivacyModeSettings, SentLog } from "../components/SharePermission";
import { UpdateNotes, UpdateSettings } from "../components/Updates";
import { CrashReportsSetting } from "../components/Feedback";
import { VOICE_FEATURES } from "../components/SettingsNav";
import { SettingsSearch } from "../components/SettingsSearch";

const THIRD_PARTY = [
  { name: "Tauri", license: "MIT / Apache-2.0", url: "https://github.com/tauri-apps/tauri" },
  { name: tx("axum (local API server)"), license: "MIT", url: "https://github.com/tokio-rs/axum" },
  { name: "llama.cpp", license: "MIT", url: "https://github.com/ggml-org/llama.cpp" },
  { name: tx("whisper.cpp (downloaded with a speech model)"), license: "MIT", url: "https://github.com/ggml-org/whisper.cpp" },
  { name: tx("Silero VAD (voice detection model)"), license: "MIT", url: "https://github.com/snakers4/silero-vad" },
  { name: tx("ONNX Runtime (downloaded with natural voices)"), license: "MIT", url: "https://github.com/microsoft/onnxruntime" },
  { name: "SQLite", license: tx("Public domain"), url: "https://sqlite.org/copyright.html" },
  { name: "React", license: "MIT", url: "https://github.com/facebook/react" },
  { name: "react-markdown / remark-gfm", license: "MIT", url: "https://github.com/remarkjs/react-markdown" },
  { name: "KaTeX / remark-math / rehype-katex", license: "MIT", url: "https://github.com/KaTeX/KaTeX" },
  { name: "RustCrypto (aes-gcm, argon2, sha2)", license: "MIT / Apache-2.0", url: "https://github.com/RustCrypto" },
  { name: tx("Email and calendar (async-imap, mail-parser, lettre, quick-xml, chrono-tz)"), license: "MIT / Apache-2.0", url: "" },
  { name: tx("Documents (docx-rs, rust_xlsxwriter, calamine, pdf-extract)"), license: "MIT / Apache-2.0", url: "" },
  { name: tx("PowerPoint template (from python-pptx)"), license: "MIT", url: "https://github.com/scanny/python-pptx" },
  { name: tx("stable-diffusion.cpp (downloaded with a picture or video model)"), license: "MIT", url: "https://github.com/leejet/stable-diffusion.cpp" },
  { name: tx("acestep.cpp (downloaded with a music model)"), license: "MIT", url: "https://github.com/ServeurpersoCom/acestep.cpp" },
  { name: tx("image (pictures)"), license: "MIT / Apache-2.0", url: "https://github.com/image-rs/image" },
  { name: tx("Arti (Tor, for the private connection)"), license: "MIT / Apache-2.0", url: "https://gitlab.torproject.org/tpo/core/arti" },
  { name: tx("Rust crates (serde, tokio, reqwest, rusqlite, sysinfo, zip, windows…)"), license: "MIT / Apache-2.0", url: "" },
];

function WebSearchSettings({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<WebSettings | null>(null);
  const [key, setKey] = useState("");
  useEffect(() => {
    api.webSettings().then(setS).catch(() => {});
  }, []);
  if (!s) return null;
  const save = async (next: WebSettings, braveKey: string | null = null) => {
    setS(next);
    try {
      await api.setWebSettings({ provider: next.provider, searxng_url: next.searxng_url }, braveKey);
      if (braveKey !== null) {
        setKey("");
        setS({ ...next, has_brave_key: braveKey !== "" });
      }
      toast(t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>{t("Web search")}</h3>
      <label>
        {t("Search with")}
        <select value={s.provider} onChange={(e) => save({ ...s, provider: e.target.value as SearchProvider })}>
          <option value="duckduckgo">{t("DuckDuckGo (no account needed)")}</option>
          <option value="brave">{t("Brave Search (your API key)")}</option>
          <option value="searxng">{t("SearXNG (your own server)")}</option>
        </select>
      </label>
      {s.provider === "duckduckgo" && <p className="muted small">{t("Reads DuckDuckGo's plain results page. If searches get limited, switch to Brave or SearXNG.")}</p>}
      {s.provider === "brave" && (
        <label>
          {t("Brave Search API key")} {s.has_brave_key && <span className="muted small">{t("(saved, encrypted)")}</span>}
          <span className="row">
            <input className="input" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={s.has_brave_key ? t("Enter a new key to replace it") : t("Paste your key")} autoComplete="off" />
            <button className="btn small" disabled={!key.trim()} onClick={() => save(s, key.trim())}>{t("Save key")}</button>
            {s.has_brave_key && <button className="btn ghost small danger" onClick={() => save(s, "")}>{t("Remove")}</button>}
          </span>
          <span className="muted small">{t("Free tier at brave.com/search/api.")}</span>
        </label>
      )}
      {s.provider === "searxng" && (
        <label>
          {t("SearXNG address")}
          <span className="row">
            <input className="input" value={s.searxng_url} onChange={(e) => setS({ ...s, searxng_url: e.target.value })} placeholder="https://search.example.com" />
            <button className="btn small" onClick={() => save(s)}>{t("Save")}</button>
          </span>
          <span className="muted small">{t("The server must allow JSON output (search.formats: json).")}</span>
        </label>
      )}
    </div>
  );
}

/** Keys the user gave for sites the assistant built (live prices and the like). */
function SiteKeys({ toast }: { toast: PushToast }) {
  const [keys, setKeys] = useState<SavedSiteKey[]>([]);
  useEffect(() => {
    api.siteKeys().then(setKeys).catch(() => {});
  }, []);
  if (keys.length === 0) return null;
  const forget = async (host: string) => {
    try {
      await api.siteKeyForget(host);
      setKeys((all) => all.filter((k) => k.host !== host));
      toast(t("Removed."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>{t("Keys for sites the assistant built")}</h3>
      <p className="muted small">{t("Saved encrypted on this PC. Each is sent only to its own service, from sites in the chats you allowed it for.")}</p>
      {keys.map((k) => (
        <div key={k.host} className="row">
          <span>
            <strong>{k.platform}</strong> <span className="muted small">{k.host}</span>
          </span>
          <span className="spacer" />
          <button className="btn ghost small danger" onClick={() => forget(k.host)}>{t("Remove")}</button>
        </div>
      ))}
    </div>
  );
}

function BrowserChoice({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<BrowsersView | null>(null);
  useEffect(() => {
    api.browsersView().then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const value = v.current.kind === "app" ? `app:${v.current.name}` : v.current.kind;
  const pick = async (val: string) => {
    const choice: MainBrowser = val.startsWith("app:") ? { kind: "app", name: val.slice(4) } : val === "system" ? { kind: "system" } : { kind: "builtin" };
    try {
      await api.setMainBrowser(choice);
      setV({ ...v, current: choice });
      window.dispatchEvent(new Event("browser-changed"));
      toast(t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>{t("Main browser")}</h3>
      <label>
        {t("Links and the globe button open in")}
        <select value={value} onChange={(e) => pick(e.target.value)}>
          <option value="builtin">{t("SulcusAI's web viewer (inside this window)")}</option>
          <option value="system">{t("Windows' default browser")}</option>
          {v.installed.map((b) => (
            <option key={b.name} value={`app:${b.name}`}>{b.name}</option>
          ))}
        </select>
      </label>
      <p className="muted small">{t("Unless you pick one, it's Chrome (or Windows' default browser without Chrome). The web viewer has its own button next to the globe either way, and the assistant browses in a browser of its own. For DuckDuckGo or another browser not listed, make it your Windows default and pick “Windows' default browser”.")}</p>
    </div>
  );
}

interface Props {
  connectivity: Connectivity;
  onConnectivity: (l: Connectivity) => void;
  security: SecurityStatus;
  onSecurityChanged: () => void;
  features: Set<FeatureId>;
  toast: PushToast;
  /** A quick link was picked: scroll to that section (`at` makes a repeat count). */
  jump: { id: string; at: number } | null;
  /** The section being read, for the sidebar's highlight. */
  onSection: (id: string | null) => void;
}

export function SettingsView({ connectivity, onConnectivity, security, onSecurityChanged, features, toast, jump, onSection }: Props) {
  const [locating, setLocating] = useState(false);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [saved, setSaved] = useState<Profile | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [confirmCloud, setConfirmCloud] = useState(false);

  useEffect(() => {
    api.profile().then((p) => {
      setProfile(p);
      setSaved(p);
    });
    api.appInfo().then(setInfo);
  }, []);

  const dirty = JSON.stringify(profile) !== JSON.stringify(saved);
  const save = async () => {
    if (!profile) return;
    try {
      await api.setProfile(profile);
      setSaved(profile);
      toast(t("Saved. New messages will use your profile."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const pick = (l: Connectivity) => {
    if (l === "cloud" && connectivity !== "cloud") setConfirmCloud(true);
    else onConnectivity(l);
  };

  // The section at the top of the page (the last anchor scrolled past).
  // After a quick link, the picked section stays marked until the person
  // scrolls themselves (a section near the end can't reach the top).
  const pageRef = useRef<HTMLDivElement>(null);
  const held = useRef(false);
  const release = () => {
    held.current = false;
  };
  const spy = () => {
    const page = pageRef.current;
    if (!page || held.current) return;
    const anchors = [...page.querySelectorAll<HTMLElement>(".settings-anchor")];
    const top = page.getBoundingClientRect().top + 80;
    let current = anchors[0];
    for (const a of anchors) if (a.getBoundingClientRect().top <= top) current = a;
    if (page.scrollTop + page.clientHeight >= page.scrollHeight - 4) current = anchors[anchors.length - 1];
    onSection(current ? current.id.replace("settings-", "") : null);
  };
  useEffect(() => {
    spy();
    return () => onSection(null);
  }, []);

  // Quick links: smooth while the page is settled; on a fresh open, cards
  // load and grow above the target, so it lands again a moment later.
  const opened = useRef(Date.now());
  useEffect(() => {
    if (!jump) return;
    held.current = true;
    onSection(jump.id);
    const go = (behavior: ScrollBehavior) => document.getElementById(`settings-${jump.id}`)?.scrollIntoView({ behavior, block: "start" });
    if (Date.now() - opened.current > 1500) {
      go("smooth");
      // Lands anyway if the animation didn't run (a window not being drawn).
      const id = setTimeout(() => {
        const page = pageRef.current;
        const el = document.getElementById(`settings-${jump.id}`);
        if (page && el && Math.abs(el.getBoundingClientRect().top - page.getBoundingClientRect().top) > 40) go("auto");
      }, 900);
      return () => clearTimeout(id);
    }
    go("auto");
    const timers = [250, 700].map((ms) => setTimeout(() => go("auto"), ms));
    return () => timers.forEach(clearTimeout);
  }, [jump]);

  return (
    <div className="page" ref={pageRef} onScroll={spy} onWheel={release} onPointerDown={release} onKeyDown={release} onTouchStart={release}>
      <div className="narrow">
      <header className="page-head settings-head">
        <h1>{t("Settings")}</h1>
        <SettingsSearch pageRef={pageRef} />
      </header>

      <span id="settings-appearance" className="settings-anchor" />
      <section className="card">
        <h2>{t("Appearance")}</h2>
        <ThemePicker />
      </section>

      <span id="settings-language" className="settings-anchor" />
      <section className="card">
        <h2>{t("Language")}</h2>
        <label className="form small">
          {t("The app's language")}
          <select className="input" defaultValue={chosenLanguage()} onChange={(e) => setLanguage(e.target.value)}>
            <option value="system">{t("Same as Windows")}</option>
            {LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.name}
              </option>
            ))}
          </select>
        </label>
        <p className="muted small">{t("The assistant answers in the language you write in, whatever this is set to.")}</p>
      </section>

      <span id="settings-about-you" className="settings-anchor" />
      <section className="card">
        <h2>{t("About you")}</h2>
        <p className="muted small">{t("Every chat includes this, so you don't have to repeat yourself. It's encrypted on this PC.")}</p>
        {profile && (
          <div className="form">
            <label>
              {t("Name")}
              <input className="input" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} placeholder={t("What should it call you?")} aria-label={t("What should it call you?")} maxLength={80} />
            </label>
            <label>
              {t("About you")}
              <textarea className="input" rows={4} value={profile.about} onChange={(e) => setProfile({ ...profile, about: e.target.value })} placeholder={t("Your work, interests, or anything that helps it understand you.")} aria-label={t("Your work, interests, or anything that helps it understand you.")} maxLength={4000} />
            </label>
            <label>
              {t("Where you are")}
              <span className="row">
                <input
                  className="input"
                  value={profile.location ?? ""}
                  onChange={(e) => setProfile({ ...profile, location: e.target.value, lat: null, lon: null })}
                  placeholder={t("Your city, e.g. Orem, Utah")} aria-label={t("Your city, e.g. Orem, Utah")}
                  maxLength={120}
                />
                <button
                  className="btn small"
                  disabled={locating}
                  onClick={async () => {
                    setLocating(true);
                    try {
                      const p = await api.detectLocation();
                      setProfile({ ...profile, location: p.label, lat: p.lat, lon: p.lon });
                    } catch (e) {
                      toast(errorText(e), "error");
                    } finally {
                      setLocating(false);
                    }
                  }}
                >
                  {locating ? t("Finding…") : t("Use this PC's location")}
                </button>
              </span>
              <span className="muted small">{t("For the weather and local questions. Kept on this PC; it goes out only with a weather lookup. Leave it empty and the assistant asks Windows when it needs to.")}</span>
            </label>
            <label>
              {t("How it should respond")}
              <textarea className="input" rows={3} value={profile.preferences} onChange={(e) => setProfile({ ...profile, preferences: e.target.value })} placeholder={t("For example: keep answers short, use plain language, give examples.")} aria-label={t("For example: keep answers short, use plain language, give examples.")} maxLength={4000} />
            </label>
            <div>
              <button className="btn primary" onClick={save} disabled={!dirty}>
                {t("Save")}
              </button>
            </div>
          </div>
        )}
      </section>

      <span id="settings-security" className="settings-anchor" />
      <SecuritySection status={security} onChanged={onSecurityChanged} toast={toast} />

      <span id="settings-folders" className="settings-anchor" />
      {features.has("files") && <FoldersSection toast={toast} />}

      <span id="settings-handoffs" className="settings-anchor" />
      {features.has("files") && <HandoffSettings toast={toast} />}

      <span id="settings-voice" className="settings-anchor" />
      {VOICE_FEATURES.some((f) => features.has(f)) && <VoiceSection toast={toast} />}

      <span id="settings-performance" className="settings-anchor" />
      <PerformanceSection toast={toast} />

      <span id="settings-servers" className="settings-anchor" />
      <PortsSettings toast={toast} />

      <span id="settings-storage" className="settings-anchor" />
      <ModelStorage toast={toast} />
      <span id="settings-cleanup" className="settings-anchor" />
      <CleanupSettings toast={toast} />

      <span id="settings-sign-in" className="settings-anchor" />
      {(features.has("email") || features.has("calendar")) && <SignInApps toast={toast} />}

      <span id="settings-connectivity" className="settings-anchor" />
      <section className="card">
        <h2>{t("Connectivity")}</h2>
        <p className="muted small">{t("Controls what may leave this PC. You can also switch it from the status bar at the bottom.")}</p>
        <div className="levels" role="radiogroup" aria-label={t("Connectivity level")}>
          {LEVELS.map((l) => (
            <button key={l.id} role="radio" aria-checked={connectivity === l.id} className={`level ${connectivity === l.id ? "active" : ""}`} onClick={() => pick(l.id)}>
              <span>
                <strong>{t(l.label)}</strong>
                {l.id === "offline" && <span className="badge subtle">{t("Default")}</span>}
                <span className="small muted block">{t(l.detail)}</span>
              </span>
            </button>
          ))}
        </div>
        <p className="muted small">{t("Tip: while Offline, the Web button in a chat (or Ctrl+Shift+W) turns web on for just that chat.")}</p>
        <WebSearchSettings toast={toast} />
        <SiteKeys toast={toast} />
        <BrowserChoice toast={toast} />
      </section>

      <span id="settings-private" className="settings-anchor" />
      <PrivacySettings toast={toast} />

      <span id="settings-privacy-mode" className="settings-anchor" />
      <PrivacyModeSettings toast={toast} />

      <span id="settings-sent" className="settings-anchor" />
      <SentLog toast={toast} />

      <span id="settings-devices" className="settings-anchor" />
      <SyncSettings toast={toast} />

      <span id="settings-phone" className="settings-anchor" />
      <PhoneSettings toast={toast} />

      <span id="settings-backup" className="settings-anchor" />
      <BackupSettings toast={toast} />

      <span id="settings-cloud" className="settings-anchor" />
      <CloudSettings toast={toast} cloudOn={connectivity === "cloud"} />

      <span id="settings-api" className="settings-anchor" />
      <ApiServerSettings toast={toast} />

      <span id="settings-advanced" className="settings-anchor" />
      <AdvancedSettings toast={toast} />

      <span id="settings-logs" className="settings-anchor" />
      <LogSettings toast={toast} />

      <span id="settings-about" className="settings-anchor" />
      <section className="card">
        <h2>{t("About {app}", { app: APP_NAME })}</h2>
        <UpdateSettings toast={toast} />
        <CrashReportsSetting toast={toast} />
        <DevModeSetting toast={toast} />
        {info && (
          <p className="small muted row wrap">
            <span>{t("Data folder: {path}", { path: info.data_dir })}</span>
            <button className="btn ghost small" onClick={() => api.openAppFolder(true).catch((e) => toast(errorText(e), "error"))}>{t("Open")}</button>
            <button className="btn ghost small" onClick={() => api.openAppFolder(false).catch((e) => toast(errorText(e), "error"))}>{t("Open logs folder")}</button>
          </p>
        )}
        <p className="small">
          {t("{app} is free software under the GNU Affero General Public License v3.0. The name and logo are not covered by that license.", { app: APP_NAME })}
        </p>
        <h3>{t("Open-source components")}</h3>
        <ul className="licenses small">
          {THIRD_PARTY.map((c) => (
            <li key={c.name}>
              <strong>{t(c.name)}</strong> <span className="muted">— {t(c.license)}</span>
            </li>
          ))}
        </ul>
        <p className="muted small">{t("AI models have their own licenses, shown on each model card. Full notices: THIRD_PARTY_NOTICES.md.")}</p>
      </section>

      <span id="settings-update-notes" className="settings-anchor" />
      <section className="card">
        <h2>{t("Update notes")}</h2>
        <p className="muted small">{t("What each version of {app} changed, newest first. Click a version to open or close it.", { app: APP_NAME })}</p>
        <UpdateNotes />
      </section>

      {confirmCloud && (
        <CloudConfirm
          onCancel={() => setConfirmCloud(false)}
          onConfirm={() => {
            setConfirmCloud(false);
            onConnectivity("cloud");
          }}
        />
      )}
      </div>
    </div>
  );
}
