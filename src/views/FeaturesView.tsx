// SPDX-License-Identifier: AGPL-3.0-only
// Features: the app starts bare-bones; turn on or install what you need.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type BridgeStatus, type FeatureId, type FeatureView, type InstallProgress, type SpeakerModel, type VoicePack } from "../api";
import { bytes, percent } from "../format";
import type { PushToast } from "../components/Toasts";
import { TESTING, TestingNote, useTestStart } from "../components/Testing";
import { t, tx } from "../i18n";

interface Info {
  name: string;
  detail: string;
}

export const FEATURES: Record<FeatureId, Info> = {
  dictation: { name: tx("Dictation"), detail: tx("Type by talking: the microphone button in a chat, or Ctrl+Shift+Space.") },
  voice_chat: { name: tx("Voice chat"), detail: tx("Talk with the assistant and hear its answers. Talk over it to interrupt.") },
  read_aloud: { name: tx("Read aloud"), detail: tx("A speaker button on every reply, read by Windows' voices or natural voices.") },
  meetings: { name: tx("Meeting notes"), detail: tx("Records your mic and your computer's sound during calls, then writes a transcript, notes and action items.") },
  translate: { name: tx("Translation"), detail: tx("Translate text and documents with your chat model.") },
  notes: { name: tx("Notes"), detail: tx("Markdown notes with folders and tags. The assistant can save and find notes for you.") },
  tasks: { name: tx("Tasks"), detail: tx("A to-do list with due dates, reminders and subtasks. Meeting action items can become tasks.") },
  web_search: {
    name: tx("Web search"),
    detail: tx("Search the web and read pages in every chat, even while Offline. Only the searches and the pages it opens go online; your messages and the AI stay on this PC. The browser, email and cloud still follow the connectivity level."),
  },
  browser: {
    name: tx("Browser"),
    detail: tx("A built-in browser with its own sign-ins that the assistant can use while you watch. It asks before submitting forms or buying anything, and never types passwords or card numbers. Needs Local AI + Web (or web on for the chat)."),
  },
  email: {
    name: tx("Email"),
    detail: tx("Connect Gmail, Outlook or any other email account so you and the assistant can read, search and summarize it, and draft replies. A copy is kept encrypted on this PC; sending always asks you first."),
  },
  calendar: {
    name: tx("Calendar"),
    detail: tx("Events on this PC, plus iCloud, Fastmail or Nextcloud calendars. The assistant can check your schedule, find free time and add events; inviting people always asks you first."),
  },
  documents: {
    name: tx("Documents"),
    detail: tx("The assistant can make Word, PDF, Excel (with working formulas and charts) and PowerPoint files, and read them, in folders you share. No Office needed. Needs Files and coding for the folders."),
  },
  quick_ask: {
    name: tx("Quick ask"),
    detail: tx("Press Ctrl+Alt+Space anywhere to ask the assistant, or to summarize, explain, fix or translate what you copied. Adds a tray icon; closing the window keeps SulcusAI in the tray so the shortcut keeps working."),
  },
  browser_control: {
    name: tx("Browser control"),
    detail: tx("Lets the assistant use tabs in your own Chrome or Edge, with your sign-ins, through the SulcusAI extension. It works in one tab you can watch, asks before submitting, buying or sending anything, and never types passwords or card numbers. Needs Local AI + Web (or web on for the chat)."),
  },
  images: {
    name: tx("Pictures"),
    detail: tx("Make pictures from a description and edit them: change things by instruction, paint over a spot to fill it in, extend the edges, restyle, make them bigger and sharper, or cut out the background. The assistant can make pictures in chats too."),
  },
  video: {
    name: tx("Video"),
    detail: tx("Short clips (a few seconds) from a description, or bring a picture to life. The heaviest feature: it needs a graphics card with 6 GB or more, and a clip takes a few minutes."),
  },
  music: {
    name: tx("Music and audio"),
    detail: tx("Songs from a description, with lyrics and vocals or instrumental, rough sound effects, and narration read by your voices. A player with trim, loop and save."),
  },
  files: { name: tx("Files and coding"), detail: tx("Share folders so the assistant can read and change files and run commands. It asks before deleting, or before sending anything off this PC.") },
  memory: { name: tx("Memory"), detail: tx("Remembers facts across chats. You can see and edit everything it remembers.") },
  projects: { name: tx("Projects"), detail: tx("Group chats with their own instructions, folders and memories.") },
  scheduled: { name: tx("Scheduled tasks"), detail: tx("Run prompts on a schedule, such as a morning summary.") },
  connectors: { name: tx("Connectors and plugins"), detail: tx("Add tools from MCP servers and plugins on this PC.") },
  chat_team: {
    name: tx("Chats working together"),
    detail: tx("The assistant can look through your other chats, read them, and hand another chat a question or a piece of work: it works on it there, with the same model, and the answer comes back. Incognito chats are never included."),
  },
  autocomplete: {
    name: tx("Autocomplete"),
    detail: tx("While you type in a chat, shows the rest of the word or of something you've asked before in light grey. Press Tab to take it. It learns from your own chats, on this PC."),
  },
  autocorrect: {
    name: tx("Autocorrect"),
    detail: tx("Fixes a misspelled word when you finish it, with Windows' own spell checker, only when it's sure. Press Backspace right after to undo a fix. Windows only."),
  },
  next_prompt: {
    name: tx("Suggested next message"),
    detail: tx("When a reply is done, the message box shows what you're likely to ask next. Press Tab to use it, or just type to make it go away. Guessed by the model on this PC; never with an online model."),
  },
  people: {
    name: tx("People"),
    detail: tx("Connect with someone else's SulcusAI by a code and chat with them directly: messages and files go straight between your PCs, encrypted, and never to a model. Works on the same network, or across the internet through a virtual network like Radmin VPN or Tailscale."),
  },
};

const GROUPS: { title: string; ids: FeatureId[] }[] = [
  { title: tx("Voice"), ids: ["dictation", "voice_chat", "read_aloud"] },
  { title: tx("Meetings and language"), ids: ["meetings", "translate"] },
  { title: tx("Notes, tasks, mail and calendar"), ids: ["notes", "tasks", "email", "calendar"] },
  { title: tx("Create"), ids: ["images", "video", "music"] },
  { title: tx("Assistant"), ids: ["quick_ask", "web_search", "browser", "browser_control", "files", "documents", "memory", "projects", "scheduled", "connectors", "chat_team"] },
  { title: tx("Typing"), ids: ["autocomplete", "autocorrect", "next_prompt"] },
  { title: tx("With other people"), ids: ["people"] },
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

  if (!list) return <div className="page"><p className="muted">{t("Loading…")}</p></div>;
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
            <h1>{t("Features")}</h1>
            <p className="muted">
              {t("Turn on what you need. Some features download a model first; everything still runs on this PC. Turning a feature off hides it and stops it, and keeps your data.")}
            </p>
          </div>
        </header>
        {GROUPS.map((g) => (
          <section key={g.title}>
            <h2>{t(g.title)}</h2>
            <div className="feature-list">
              {g.ids.map((id) => {
                const f = byId.get(id);
                if (!f) return null;
                const info = FEATURES[id];
                const p = f.need ? progress[f.need.model_id] : undefined;
                const needsInstall = !!f.need && !f.need.met && !f.enabled;
                return (
                  <article key={id} className={`card feature ${f.enabled ? "on" : ""}`}>
                    <div className="feature-body">
                      <strong>{t(info.name)}</strong>
                      <span className="small muted block">{t(info.detail)}</span>
                      {needsInstall && !p && (
                        <span className="small block">
                          {["dictation", "voice_chat", "meetings"].includes(id)
                            ? t("Installs {model} ({size}) for speech recognition.", { model: f.need!.model_name, size: bytes(f.need!.size) })
                            : t("Installs {model} ({size}).", { model: f.need!.model_name, size: bytes(f.need!.size) })}
                        </span>
                      )}
                      {p && <Progress p={p} />}
                      {f.enabled && (id === "voice_chat" || id === "read_aloud") && voices && !voices.installed && (
                        <AddOn
                          label={t("Add natural voices ({size})", { size: bytes(voices.size) })}
                          note={t("Far more human-sounding, in 31 languages.")}
                          progress={progress[voices.id]}
                          disabled={busy}
                          onInstall={() => api.installVoicePack(voices.id)}
                          toast={toast}
                        />
                      )}
                      {f.enabled && id === "browser_control" && <ExtensionSetup toast={toast} />}
                      {f.enabled && id === "meetings" && speakers && !speakers.installed && (
                        <AddOn
                          label={t("Add speaker labels ({size})", { size: bytes(speakers.download) })}
                          note={t("Tells the other people on a call apart.")}
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
                          title={busy ? t("One install at a time") : undefined}
                          onClick={() => api.installFeature(id).catch((e) => toast(errorText(e), "error"))}
                        >
                          {t("Install")}
                        </button>
                      )
                    ) : (
                      <label className="switch" title={f.enabled ? t("On") : t("Off")}>
                        <input type="checkbox" checked={f.enabled} onChange={(e) => toggle(f, e.target.checked)} aria-label={t("{name} on or off", { name: t(info.name) })} />
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
  const label = { engine: t("Setting up"), verify: t("Checking the file"), download: t("Downloading"), loading: t("Loading it to test it"), benchmark: t("Testing it"), vision: t("Adding the picture reader"), optimizing: t("Finding the fastest setup"), waiting: t("Waiting for a reply that uses this model to finish") }[p.phase];
  const testing = TESTING.has(p.phase);
  const since = useTestStart(testing);
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${t("{received} of {total}", { received: bytes(p.received), total: bytes(p.total) })}` : "…"}</span>
        {!testing && <button className="link" onClick={() => api.cancelInstall(p.model_id)}>{t("Cancel")}</button>}
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
      {testing && <TestingNote since={since} short />}
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

/** The steps to add the SulcusAI extension, and whether it's connected. */
function ExtensionSetup({ toast }: { toast: PushToast }) {
  const [st, setSt] = useState<BridgeStatus | null>(null);
  useEffect(() => {
    const load = () => api.bridgeStatus().then(setSt).catch(() => {});
    load();
    const sub = on("bridge:status", () => load());
    const timer = setInterval(load, 5000);
    return () => {
      clearInterval(timer);
      sub.then((u) => u());
    };
  }, []);
  if (!st) return null;
  if (st.connected) return <span className="small block ok-text">{t("✓ The extension is connected (version {version}).", { version: st.version ?? "" })}</span>;
  return (
    <div className="ext-setup small">
      <strong>{t("Add the extension (once):")}</strong>
      <ol>
        <li>{t("In Chrome open")} <code>chrome://extensions</code>{t(", or in Edge")} <code>edge://extensions</code>.</li>
        <li>{t("Turn on")} <b>{t("Developer mode")}</b>.</li>
        <li>{t("Click")} <b>{t("Load unpacked")}</b> {t("and choose the SulcusAI extension folder.")}</li>
      </ol>
      <div className="row">
        <button className="btn small" onClick={() => api.showExtensionFolder().catch((e) => toast(errorText(e), "error"))}>{t("Show the folder")}</button>
        <button className="btn ghost small" onClick={() => navigator.clipboard.writeText(st.extension_dir).then(() => toast(t("Folder path copied."), "success"))}>{t("Copy its path")}</button>
      </div>
      <span className="muted block">{t("Waiting for the extension… It connects by itself once added, while SulcusAI is running.")}</span>
    </div>
  );
}
