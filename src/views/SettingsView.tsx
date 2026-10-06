// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type AppInfo, type Connectivity, type Profile, type SecurityStatus } from "../api";
import { APP_NAME } from "../brand";
import { CloudConfirm, LEVELS } from "../components/ConnectivityMenu";
import { SecuritySection } from "../components/Security";
import { FoldersSection } from "../components/AgentCards";
import { VoiceSection } from "../components/VoiceSettings";
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

interface Props {
  connectivity: Connectivity;
  onConnectivity: (l: Connectivity) => void;
  security: SecurityStatus;
  onSecurityChanged: () => void;
  toast: PushToast;
}

export function SettingsView({ connectivity, onConnectivity, security, onSecurityChanged, toast }: Props) {
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

      <FoldersSection toast={toast} />

      <VoiceSection toast={toast} />

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
