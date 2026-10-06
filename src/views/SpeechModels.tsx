// SPDX-License-Identifier: AGPL-3.0-only
// Models › Speech: speech-recognition models for dictation, voice chats and meetings.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type InstallProgress, type SpeechCard, type SpeechView } from "../api";
import { bytes, percent } from "../format";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

function speedText(speed: number): string {
  return `${speed >= 10 ? Math.round(speed) : speed.toFixed(1)}× faster than real time`;
}

export function SpeechModels({ progress, toast }: { progress: Record<string, InstallProgress>; toast: PushToast }) {
  const [view, setView] = useState<SpeechView | null>(null);
  const refresh = useCallback(() => api.speechView().then(setView).catch((e) => toast(errorText(e), "error")), [toast]);
  useEffect(() => {
    refresh();
    const sub = on("install:finished", () => refresh());
    return () => {
      sub.then((un) => un());
    };
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
  const label = { engine: "Setting up speech recognition", verify: "Checking the file", download: "Downloading", benchmark: "Testing it with a spoken sentence" }[p.phase];
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
