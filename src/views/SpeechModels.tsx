// SPDX-License-Identifier: AGPL-3.0-only
// Models › Speech: speech-recognition models for dictation, voice chats and meetings.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type InstallProgress, type SpeakerModel, type SpeechCard, type SpeechView, type VoicePack } from "../api";
import { bytes, percent } from "../format";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

function speedText(speed: number): string {
  return `${speed >= 10 ? Math.round(speed) : speed.toFixed(1)}× faster than real time`;
}

export function SpeechModels({ progress, toast }: { progress: Record<string, InstallProgress>; toast: PushToast }) {
  const [view, setView] = useState<SpeechView | null>(null);
  const [packs, setPacks] = useState<VoicePack[]>([]);
  const [speakers, setSpeakers] = useState<SpeakerModel | null>(null);
  const refresh = useCallback(() => {
    api.speechView().then(setView).catch((e) => toast(errorText(e), "error"));
    api.voicePacks().then(setPacks).catch(() => {});
    api.speakerModel().then(setSpeakers).catch(() => {});
  }, [toast]);
  useEffect(() => {
    refresh();
    const subs = [on("install:finished", () => refresh()), on("models:found", () => refresh())];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [refresh]);

  if (!view) return <p className="muted">Checking this PC…</p>;
  const installed = view.models.filter((m) => m.installed);
  const available = view.models.filter((m) => !m.installed);
  const busy = Object.keys(progress).length > 0;

  return (
    <>
      <p className="muted">
        Speech recognition turns what you say into text, on this PC, for dictation, voice chats and meetings. It runs on the
        processor ({view.threads} threads) so your graphics card stays free for the chat model.
      </p>
      {installed.length > 0 && (
        <>
          <h2>Installed</h2>
          <div className="model-grid">
            {installed.map((m) => (
              <InstalledSpeech key={m.id} m={m} view={view} onChanged={refresh} toast={toast} />
            ))}
          </div>
        </>
      )}
      <h2>Available for this PC</h2>
      {available.length === 0 && <p className="muted">Every speech model that fits this PC is installed.</p>}
      {installed.length === 0 && view.recommended_live && (
        <p className="callout small">
          Tip: install two. The most accurate model writes meeting transcripts; a quicker one makes dictation and voice chats
          feel instant. {SPEECH_HINT}
        </p>
      )}
      <div className="model-grid">
        {available.map((m) => (
          <AvailableSpeech key={m.id} m={m} live={m.id === view.recommended_live} p={progress[m.id]} otherBusy={busy && !progress[m.id]} toast={toast} />
        ))}
      </div>
      <h2>Voices</h2>
      <p className="muted small">
        Voices read replies aloud and talk in voice chats. Windows' built-in voices work already; natural voices sound far
        more human.
      </p>
      <div className="model-grid">
        {packs.map((p) => (
          <VoicePackCard key={p.id} p={p} progress={progress[p.id]} otherBusy={busy && !progress[p.id]} onChanged={refresh} toast={toast} />
        ))}
      </div>
      <h2>Meetings</h2>
      <div className="model-grid">
        {speakers && <SpeakerCard m={speakers} progress={progress[speakers.id]} otherBusy={busy && !progress[speakers.id]} onChanged={refresh} toast={toast} />}
      </div>
      {view.hidden > 0 && (
        <section className="card hint">
          <strong>
            {view.hidden} {view.hidden === 1 ? "speech model is" : "speech models are"} hidden because this PC's processor
            couldn't keep up with live speech.
          </strong>
        </section>
      )}
    </>
  );
}

const SPEECH_HINT = "SulcusAI picks the right one for each job automatically; you can change that in Settings › Voice.";

function Badges({ m, live }: { m: SpeechCard; live: boolean }) {
  return (
    <span className="row">
      {m.recommended && <span className="badge">Best for meetings</span>}
      {live && <span className="badge">Best for dictation</span>}
    </span>
  );
}

function AvailableSpeech({ m, live, p, otherBusy, toast }: { m: SpeechCard; live: boolean; p?: InstallProgress; otherBusy: boolean; toast: PushToast }) {
  return (
    <article className="card model">
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">{m.publisher} · {m.languages} languages</span>
        </div>
        <Badges m={m} live={live} />
      </div>
      <p className="desc">{m.description}</p>
      <div className="tags">
        <span className="tag license" title={m.license.url}>{m.license.name}</span>
      </div>
      {p ? (
        <SpeechProgress id={m.id} p={p} />
      ) : (
        <div className="install-row">
          <span className="muted small">{bytes(m.size)} download</span>
          <span className="spacer" />
          <button
            className="btn primary"
            disabled={otherBusy}
            title={otherBusy ? "One install at a time" : undefined}
            onClick={async () => {
              try {
                await api.installSpeech(m.id);
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            Install
          </button>
        </div>
      )}
      {!p && <p className="muted small fit-line">About {speedText(m.fit.speed)} on this PC (estimated)</p>}
    </article>
  );
}

function SpeechProgress({ id, p }: { id: string; p: InstallProgress }) {
  const label = { engine: "Setting up the speech engine", verify: "Checking the file", download: "Downloading", benchmark: "Testing it", vision: "Downloading" }[p.phase];
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${bytes(p.received)} of ${bytes(p.total)}` : "…"}</span>
        {p.phase !== "benchmark" && <button className="link" onClick={() => api.cancelInstall(id)}>Cancel</button>}
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
    </div>
  );
}

function InstalledSpeech({ m, view, onChanged, toast }: { m: SpeechCard; view: SpeechView; onChanged: () => void; toast: PushToast }) {
  const [confirm, setConfirm] = useState(false);
  const inst = m.installed!;
  const uses = [view.active === m.id && "meetings", view.live === m.id && "dictation and voice chats"].filter(Boolean).join(" and ");
  return (
    <article className="card model installed">
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">{bytes(inst.size)} · {m.languages} languages</span>
        </div>
        {uses && <span className="badge">In use</span>}
      </div>
      <p className="desc">{m.description}</p>
      <p className="small fit-line">
        {inst.speed !== null ? `Measured on this PC: ${speedText(inst.speed)} for a short sentence` : "Speed not measured yet"}
        {uses && <span className="muted"> · Used for {uses}</span>}
      </p>
      <div className="install-row">
        <span className="spacer" />
        <button className="btn ghost danger" onClick={() => setConfirm(true)}>Remove</button>
      </div>
      {confirm && (
        <Modal title={`Remove ${m.name}?`} onClose={() => setConfirm(false)}>
          <p>This deletes the model ({bytes(inst.size)}) from this PC. Meetings you recorded keep their transcripts.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>Cancel</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                try {
                  await api.removeSpeech(m.id);
                  setConfirm(false);
                  onChanged();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Remove
            </button>
          </div>
        </Modal>
      )}
    </article>
  );
}

function VoicePackCard({ p, progress, otherBusy, onChanged, toast }: { p: VoicePack; progress?: InstallProgress; otherBusy: boolean; onChanged: () => void; toast: PushToast }) {
  const [confirm, setConfirm] = useState(false);
  return (
    <article className={`card model ${p.installed ? "installed" : ""}`}>
      <div className="model-head">
        <div>
          <h3>{p.name}</h3>
          <span className="muted small">{p.publisher} · {p.styles.length} voices · {p.languages.length} languages</span>
        </div>
        {p.installed && <span className="badge">Installed</span>}
      </div>
      <p className="desc">{p.description}</p>
      <div className="tags">
        <span className="tag license" title={`${p.license.note ?? p.license.name}\n${p.license.url}`}>{p.license.name}</span>
      </div>
      {progress ? (
        <SpeechProgress id={p.id} p={progress} />
      ) : p.installed ? (
        <div className="install-row">
          <span className="muted small">Choose a voice in Settings › Voice and meetings.</span>
          <span className="spacer" />
          <button className="btn ghost danger" onClick={() => setConfirm(true)}>Remove</button>
        </div>
      ) : (
        <div className="install-row">
          <span className="muted small">{bytes(p.size)} download</span>
          <span className="spacer" />
          <button
            className="btn primary"
            disabled={otherBusy}
            onClick={async () => {
              try {
                await api.installVoicePack(p.id);
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            Install
          </button>
        </div>
      )}
      {confirm && (
        <Modal title={`Remove ${p.name}?`} onClose={() => setConfirm(false)}>
          <p>This deletes the voices ({bytes(p.size)}) from this PC. Replies will be read by Windows' voices.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>Cancel</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                try {
                  await api.removeVoicePack(p.id);
                  setConfirm(false);
                  onChanged();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Remove
            </button>
          </div>
        </Modal>
      )}
    </article>
  );
}

function SpeakerCard({ m, progress, otherBusy, onChanged, toast }: { m: SpeakerModel; progress?: InstallProgress; otherBusy: boolean; onChanged: () => void; toast: PushToast }) {
  return (
    <article className={`card model ${m.installed ? "installed" : ""}`}>
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">{m.publisher}</span>
        </div>
        {m.installed && <span className="badge">Installed</span>}
      </div>
      <p className="desc">{m.description}</p>
      <div className="tags">
        <span className="tag license" title={`${m.license.note ?? m.license.name}\n${m.license.url}`}>{m.license.name}</span>
      </div>
      {progress ? (
        <SpeechProgress id={m.id} p={progress} />
      ) : m.installed ? (
        <div className="install-row">
          <span className="muted small">New meetings label each person on the call.</span>
          <span className="spacer" />
          <button
            className="btn ghost danger"
            onClick={() => api.removeSpeakerModel().then(onChanged).catch((e) => toast(errorText(e), "error"))}
          >
            Remove
          </button>
        </div>
      ) : (
        <div className="install-row">
          <span className="muted small">{bytes(m.download)} download</span>
          <span className="spacer" />
          <button className="btn primary" disabled={otherBusy} onClick={() => api.installSpeakerModel().catch((e) => toast(errorText(e), "error"))}>
            Install
          </button>
        </div>
      )}
    </article>
  );
}
