// SPDX-License-Identifier: AGPL-3.0-only
// Features: the app starts bare-bones; turn on or install what you need.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type FeatureId, type FeatureView, type InstallProgress, type SpeakerModel, type VoicePack } from "../api";
import { bytes, percent } from "../format";
import type { PushToast } from "../components/Toasts";

interface Info {
  icon: string;
  name: string;
  detail: string;
}

export const FEATURES: Record<FeatureId, Info> = {
  dictation: { icon: "🎤", name: "Dictation", detail: "Type by talking: the 🎤 button in a chat, or Ctrl+Shift+Space." },
  voice_chat: { icon: "🗣", name: "Voice chat", detail: "Talk with the assistant and hear its answers. Talk over it to interrupt." },
  read_aloud: { icon: "🔊", name: "Read aloud", detail: "A 🔊 button on every reply, read by Windows' voices or natural voices." },
  meetings: { icon: "●", name: "Meeting notes", detail: "Records your mic and your computer's sound during calls, then writes a transcript, notes and action items." },
  translate: { icon: "🌍", name: "Translation", detail: "Translate text and documents with your chat model." },
  files: { icon: "📁", name: "Files and coding", detail: "Share folders so the assistant can read and change files and run commands, with your approval." },
  memory: { icon: "🧠", name: "Memory", detail: "Remembers facts across chats. You can see and edit everything it remembers." },
  projects: { icon: "📚", name: "Projects", detail: "Group chats with their own instructions, folders and memories." },
  scheduled: { icon: "⏰", name: "Scheduled tasks", detail: "Run prompts on a schedule, such as a morning summary." },
  connectors: { icon: "🔌", name: "Connectors and plugins", detail: "Add tools from MCP servers and plugins on this PC." },
};

const GROUPS: { title: string; ids: FeatureId[] }[] = [
  { title: "Voice", ids: ["dictation", "voice_chat", "read_aloud"] },
  { title: "Meetings and language", ids: ["meetings", "translate"] },
  { title: "Assistant", ids: ["files", "memory", "projects", "scheduled", "connectors"] },
];

export function FeaturesView({ progress, toast }: { progress: Record<string, InstallProgress>; toast: PushToast }) {
  const [list, setList] = useState<FeatureView[] | null>(null);
  const [voices, setVoices] = useState<VoicePack | null>(null);
  const [speakers, setSpeakers] = useState<SpeakerModel | null>(null);
  const refresh = useCallback(() => {
    api.features().then(setList).catch((e) => toast(errorText(e), "error"));
    api.voicePacks().then((p) => setVoices(p[0] ?? null)).catch(() => {});
    api.speakerModel().then(setSpeakers).catch(() => {});
  }, [toast]);
  useEffect(() => {
    refresh();
    const subs = [on("features:changed", refresh), on("install:finished", refresh)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [refresh]);

  if (!list) return <div className="page"><p className="muted">Loading…</p></div>;
  const busy = Object.keys(progress).length > 0;
  const byId = new Map(list.map((f) => [f.id, f]));

  const toggle = async (f: FeatureView, enabled: boolean) => {
    try {
      await api.setFeature(f.id, enabled);
      refresh();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>Features</h1>
            <p className="muted">
              Turn on what you need. Some features download a model first; everything still runs on this PC. Turning a
              feature off hides it and stops it, and keeps your data.
            </p>
          </div>
        </header>
        {GROUPS.map((g) => (
          <section key={g.title}>
            <h2>{g.title}</h2>
            <div className="feature-list">
              {g.ids.map((id) => {
                const f = byId.get(id);
                if (!f) return null;
                const info = FEATURES[id];
                const p = f.need ? progress[f.need.model_id] : undefined;
                const needsInstall = !!f.need && !f.need.met && !f.enabled;
                return (
                  <article key={id} className={`card feature ${f.enabled ? "on" : ""}`}>
                    <span className="feature-icon" aria-hidden>{info.icon}</span>
                    <div className="feature-body">
                      <strong>{info.name}</strong>
                      <span className="small muted block">{info.detail}</span>
                      {needsInstall && !p && (
                        <span className="small block">Installs {f.need!.model_name} ({bytes(f.need!.size)}) for speech recognition.</span>
                      )}
                      {p && <Progress p={p} />}
                      {f.enabled && (id === "voice_chat" || id === "read_aloud") && voices && !voices.installed && (
                        <AddOn
                          label={`Add natural voices (${bytes(voices.size)})`}
                          note="Far more human-sounding, in 31 languages."
                          progress={progress[voices.id]}
                          disabled={busy}
                          onInstall={() => api.installVoicePack(voices.id)}
                          toast={toast}
                        />
                      )}
                      {f.enabled && id === "meetings" && speakers && !speakers.installed && (
                        <AddOn
                          label={`Add speaker labels (${bytes(speakers.download)})`}
                          note="Tells the other people on a call apart."
                          progress={progress[speakers.id]}
                          disabled={busy}
                          onInstall={() => api.installSpeakerModel()}
                          toast={toast}
                        />
                      )}
                    </div>
                    {needsInstall ? (
                      !p && (
                        <button
                          className="btn primary small"
                          disabled={busy}
                          title={busy ? "One install at a time" : undefined}
                          onClick={() => api.installFeature(id).catch((e) => toast(errorText(e), "error"))}
                        >
                          Install
                        </button>
                      )
                    ) : (
                      <label className="switch" title={f.enabled ? "On" : "Off"}>
                        <input type="checkbox" checked={f.enabled} onChange={(e) => toggle(f, e.target.checked)} aria-label={`${info.name} on or off`} />
                        <span />
                      </label>
                    )}
                  </article>
                );
              })}
            </div>
          </section>
        ))}
      </div>
    </div>
  );
}

function Progress({ p }: { p: InstallProgress }) {
  const label = { engine: "Setting up", verify: "Checking the file", download: "Downloading", benchmark: "Testing it" }[p.phase];
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${bytes(p.received)} of ${bytes(p.total)}` : "…"}</span>
        {p.phase !== "benchmark" && <button className="link" onClick={() => api.cancelInstall(p.model_id)}>Cancel</button>}
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
    </div>
  );
}

function AddOn({ label, note, progress, disabled, onInstall, toast }: { label: string; note: string; progress?: InstallProgress; disabled: boolean; onInstall: () => Promise<void>; toast: PushToast }) {
  if (progress) return <Progress p={progress} />;
  return (
    <div className="row addon">
      <button className="btn small" disabled={disabled} onClick={() => onInstall().catch((e) => toast(errorText(e), "error"))}>
        + {label}
      </button>
      <span className="muted small">{note}</span>
    </div>
  );
}
