// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type AppInfo, type Connectivity, type FeatureId, type Profile, type SearchProvider, type SecurityStatus, type WebSettings } from "../api";
import { APP_NAME } from "../brand";
import { CloudConfirm, LEVELS } from "../components/ConnectivityMenu";
import { SecuritySection } from "../components/Security";
import { FoldersSection } from "../components/AgentCards";
import { VoiceSection } from "../components/VoiceSettings";
import { PerformanceSection } from "../components/Performance";
import type { PushToast } from "../components/Toasts";

const THIRD_PARTY = [
  { name: "Tauri", license: "MIT / Apache-2.0", url: "https://github.com/tauri-apps/tauri" },
  { name: "llama.cpp", license: "MIT", url: "https://github.com/ggml-org/llama.cpp" },
  { name: "whisper.cpp (downloaded with a speech model)", license: "MIT", url: "https://github.com/ggml-org/whisper.cpp" },
  { name: "Silero VAD (voice detection model)", license: "MIT", url: "https://github.com/snakers4/silero-vad" },
  { name: "ONNX Runtime (downloaded with natural voices)", license: "MIT", url: "https://github.com/microsoft/onnxruntime" },
  { name: "SQLite", license: "Public domain", url: "https://sqlite.org/copyright.html" },
  { name: "React", license: "MIT", url: "https://github.com/facebook/react" },
  { name: "react-markdown / remark-gfm", license: "MIT", url: "https://github.com/remarkjs/react-markdown" },
  { name: "RustCrypto (aes-gcm, argon2, sha2)", license: "MIT / Apache-2.0", url: "https://github.com/RustCrypto" },
  { name: "Rust crates (serde, tokio, reqwest, rusqlite, sysinfo, zip, windows…)", license: "MIT / Apache-2.0", url: "" },
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
      toast("Saved.", "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>Web search</h3>
      <label>
        Search with
        <select value={s.provider} onChange={(e) => save({ ...s, provider: e.target.value as SearchProvider })}>
          <option value="duckduckgo">DuckDuckGo (no account needed)</option>
          <option value="brave">Brave Search (your API key)</option>
          <option value="searxng">SearXNG (your own server)</option>
        </select>
      </label>
      {s.provider === "duckduckgo" && <p className="muted small">Reads DuckDuckGo's plain results page. If searches get limited, switch to Brave or SearXNG.</p>}
      {s.provider === "brave" && (
        <label>
          Brave Search API key {s.has_brave_key && <span className="muted small">(saved, encrypted)</span>}
          <span className="row">
            <input className="input" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={s.has_brave_key ? "Enter a new key to replace it" : "Paste your key"} autoComplete="off" />
            <button className="btn small" disabled={!key.trim()} onClick={() => save(s, key.trim())}>Save key</button>
            {s.has_brave_key && <button className="btn ghost small danger" onClick={() => save(s, "")}>Remove</button>}
          </span>
          <span className="muted small">Free tier at brave.com/search/api.</span>
        </label>
      )}
      {s.provider === "searxng" && (
        <label>
          SearXNG address
          <span className="row">
            <input className="input" value={s.searxng_url} onChange={(e) => setS({ ...s, searxng_url: e.target.value })} placeholder="https://search.example.com" />
            <button className="btn small" onClick={() => save(s)}>Save</button>
          </span>
          <span className="muted small">The server must allow JSON output (search.formats: json).</span>
        </label>
      )}
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
}

export function SettingsView({ connectivity, onConnectivity, security, onSecurityChanged, features, toast }: Props) {
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
      toast("Saved. New messages will use your profile.", "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const pick = (l: Connectivity) => {
    if (l === "cloud" && connectivity !== "cloud") setConfirmCloud(true);
    else onConnectivity(l);
  };

  return (
    <div className="page">
      <div className="narrow">
      <header className="page-head">
        <h1>Settings</h1>
      </header>

      <section className="card">
        <h2>About you</h2>
        <p className="muted small">Every chat includes this, so you don't have to repeat yourself. It's encrypted on this PC.</p>
        {profile && (
          <div className="form">
            <label>
              Name
              <input className="input" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} placeholder="What should it call you?" maxLength={80} />
            </label>
            <label>
              About you
              <textarea className="input" rows={4} value={profile.about} onChange={(e) => setProfile({ ...profile, about: e.target.value })} placeholder="Your work, interests, or anything that helps it understand you." maxLength={4000} />
            </label>
            <label>
              How it should respond
              <textarea className="input" rows={3} value={profile.preferences} onChange={(e) => setProfile({ ...profile, preferences: e.target.value })} placeholder="For example: keep answers short, use plain language, give examples." maxLength={4000} />
            </label>
            <div>
              <button className="btn primary" onClick={save} disabled={!dirty}>
                Save
              </button>
            </div>
          </div>
        )}
      </section>

      <SecuritySection status={security} onChanged={onSecurityChanged} toast={toast} />

      {features.has("files") && <FoldersSection toast={toast} />}

      {(["dictation", "voice_chat", "read_aloud", "meetings"] as FeatureId[]).some((f) => features.has(f)) && <VoiceSection toast={toast} />}

      <PerformanceSection toast={toast} />

      <section className="card">
        <h2>Connectivity</h2>
        <p className="muted small">Controls what may leave this PC. You can also switch it from the status bar at the bottom.</p>
        <div className="levels" role="radiogroup" aria-label="Connectivity level">
          {LEVELS.map((l) => (
            <button key={l.id} role="radio" aria-checked={connectivity === l.id} className={`level ${connectivity === l.id ? "active" : ""}`} onClick={() => pick(l.id)}>
              <span className="conn-icon" aria-hidden>{l.icon}</span>
              <span>
                <strong>{l.label}</strong>
                {l.id === "offline" && <span className="badge subtle">Default</span>}
                <span className="small muted block">{l.detail}</span>
              </span>
            </button>
          ))}
        </div>
        <p className="muted small">Tip: while Offline, the 🌐 button in a chat (or Ctrl+Shift+W) turns web on for just that chat.</p>
        <WebSearchSettings toast={toast} />
      </section>

      <section className="card">
        <h2>About {APP_NAME}</h2>
        {info && (
          <p className="small">
            Version {info.version}
            <br />
            <span className="muted">Data folder: {info.data_dir}</span>
          </p>
        )}
        <p className="small">
          {APP_NAME} is free software under the GNU Affero General Public License v3.0. The name and logo are not covered by that license.
        </p>
        <h3>Open-source components</h3>
        <ul className="licenses small">
          {THIRD_PARTY.map((t) => (
            <li key={t.name}>
              <strong>{t.name}</strong> <span className="muted">— {t.license}</span>
            </li>
          ))}
        </ul>
        <p className="muted small">AI models have their own licenses, shown on each model card. Full notices: THIRD_PARTY_NOTICES.md.</p>
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
