// SPDX-License-Identifier: AGPL-3.0-only
// Models › Speech: speech-recognition models for dictation, voice chats and meetings.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type InstallProgress, type SpeakerModel, type SpeechCard, type SpeechView, type VoicePack } from "../api";
import { bytes, percent } from "../format";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

function speedText(speed: number): string {
  return t("{speed}× faster than real time", { speed: speed >= 10 ? Math.round(speed) : speed.toFixed(1) });
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

  if (!view) return <p className="muted">{t("Checking this PC…")}</p>;
  const installed = view.models.filter((m) => m.installed);
  const available = view.models.filter((m) => !m.installed);
  const busy = Object.keys(progress).length > 0;

  return (
    <>
      <p className="muted">
        {t("Speech recognition turns what you say into text, on this PC, for dictation, voice chats and meetings. It runs on the processor ({threads} threads) so your graphics card stays free for the chat model.", { threads: view.threads })}
      </p>
      {installed.length > 0 && (
        <>
          <h2>{t("Installed")}</h2>
          <div className="model-grid">
            {installed.map((m) => (
              <InstalledSpeech key={m.id} m={m} view={view} onChanged={refresh} toast={toast} />
            ))}
          </div>
        </>
      )}
      <h2>{t("Available for this PC")}</h2>
      {available.length === 0 && <p className="muted">{t("Every speech model that fits this PC is installed.")}</p>}
      {installed.length === 0 && view.recommended_live && (
        <p className="callout small">
          {t("Tip: install two. The most accurate model writes meeting transcripts; a quicker one makes dictation and voice chats feel instant.")} {t(SPEECH_HINT)}
        </p>
      )}
      <div className="model-grid">
        {available.map((m) => (
          <AvailableSpeech key={m.id} m={m} live={m.id === view.recommended_live} p={progress[m.id]} otherBusy={busy && !progress[m.id]} toast={toast} />
        ))}
      </div>
      <h2>{t("Voices")}</h2>
      <p className="muted small">
        {t("Voices read replies aloud and talk in voice chats. Windows' built-in voices work already; natural voices sound far more human.")}
      </p>
      <div className="model-grid">
        {packs.map((p) => (
          <VoicePackCard key={p.id} p={p} progress={progress[p.id]} otherBusy={busy && !progress[p.id]} onChanged={refresh} toast={toast} />
        ))}
      </div>
      <h2>{t("Meetings")}</h2>
      <div className="model-grid">
        {speakers && <SpeakerCard m={speakers} progress={progress[speakers.id]} otherBusy={busy && !progress[speakers.id]} onChanged={refresh} toast={toast} />}
      </div>
      {view.hidden > 0 && (
        <section className="card hint">
          <strong>
            {view.hidden === 1
              ? t("1 speech model is hidden because this PC's processor couldn't keep up with live speech.")
              : t("{n} speech models are hidden because this PC's processor couldn't keep up with live speech.", { n: view.hidden })}
          </strong>
        </section>
      )}
    </>
  );
}

const SPEECH_HINT = tx("SulcusAI picks the right one for each job automatically; you can change that in Settings › Voice.");

function Badges({ m, live }: { m: SpeechCard; live: boolean }) {
  return (
    <span className="row">
      {m.recommended && <span className="badge">{t("Best for meetings")}</span>}
      {live && <span className="badge">{t("Best for dictation")}</span>}
    </span>
  );
}

function AvailableSpeech({ m, live, p, otherBusy, toast }: { m: SpeechCard; live: boolean; p?: InstallProgress; otherBusy: boolean; toast: PushToast }) {
  return (
    <article className="card model">
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">{m.publisher} · {t("{n} languages", { n: m.languages })}</span>
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
          <span className="muted small">{t("{size} download", { size: bytes(m.size) })}</span>
          <span className="spacer" />
          <button
            className="btn primary"
            disabled={otherBusy}
            title={otherBusy ? t("One install at a time") : undefined}
            onClick={async () => {
              try {
                await api.installSpeech(m.id);
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            {t("Install")}
          </button>
        </div>
      )}
      {!p && <p className="muted small fit-line">{t("About {speed} on this PC (estimated)", { speed: speedText(m.fit.speed) })}</p>}
    </article>
  );
}

function SpeechProgress({ id, p }: { id: string; p: InstallProgress }) {
  const label = { engine: t("Setting up the speech engine"), verify: t("Checking the file"), download: t("Downloading"), benchmark: t("Testing it"), vision: t("Downloading") }[p.phase];
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${t("{received} of {total}", { received: bytes(p.received), total: bytes(p.total) })}` : "…"}</span>
        {p.phase !== "benchmark" && <button className="link" onClick={() => api.cancelInstall(id)}>{t("Cancel")}</button>}
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
  const forMeetings = view.active === m.id;
  const forLive = view.live === m.id;
  const uses =
    forMeetings && forLive
      ? t("Used for meetings and dictation and voice chats")
      : forMeetings
        ? t("Used for meetings")
        : forLive
          ? t("Used for dictation and voice chats")
          : "";
  return (
    <article className="card model installed">
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">{bytes(inst.size)} · {t("{n} languages", { n: m.languages })}</span>
        </div>
        {uses && <span className="badge">{t("In use")}</span>}
      </div>
      <p className="desc">{m.description}</p>
      <p className="small fit-line">
        {inst.speed !== null ? t("Measured on this PC: {speed} for a short sentence", { speed: speedText(inst.speed) }) : t("Speed not measured yet")}
        {uses && <span className="muted"> · {uses}</span>}
      </p>
      <div className="install-row">
        <span className="spacer" />
        <button className="btn ghost danger" onClick={() => setConfirm(true)}>{t("Remove")}</button>
      </div>
      {confirm && (
        <Modal title={t("Remove {name}?", { name: m.name })} onClose={() => setConfirm(false)}>
          <p>{t("This deletes the model ({size}) from this PC. Meetings you recorded keep their transcripts.", { size: bytes(inst.size) })}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
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
              {t("Remove")}
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
          <span className="muted small">{p.publisher} · {t("{n} voices", { n: p.styles.length })} · {t("{n} languages", { n: p.languages.length })}</span>
        </div>
        {p.installed && <span className="badge">{t("Installed")}</span>}
      </div>
      <p className="desc">{p.description}</p>
      <div className="tags">
        <span className="tag license" title={`${p.license.note ?? p.license.name}\n${p.license.url}`}>{p.license.name}</span>
      </div>
      {progress ? (
        <SpeechProgress id={p.id} p={progress} />
      ) : p.installed ? (
        <div className="install-row">
          <span className="muted small">{t("Choose a voice in Settings › Voice and meetings.")}</span>
          <span className="spacer" />
          <button className="btn ghost danger" onClick={() => setConfirm(true)}>{t("Remove")}</button>
        </div>
      ) : (
        <div className="install-row">
          <span className="muted small">{t("{size} download", { size: bytes(p.size) })}</span>
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
            {t("Install")}
          </button>
        </div>
      )}
      {confirm && (
        <Modal title={t("Remove {name}?", { name: p.name })} onClose={() => setConfirm(false)}>
          <p>{t("This deletes the voices ({size}) from this PC. Replies will be read by Windows' voices.", { size: bytes(p.size) })}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
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
              {t("Remove")}
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
        {m.installed && <span className="badge">{t("Installed")}</span>}
      </div>
      <p className="desc">{m.description}</p>
      <div className="tags">
        <span className="tag license" title={`${m.license.note ?? m.license.name}\n${m.license.url}`}>{m.license.name}</span>
      </div>
      {progress ? (
        <SpeechProgress id={m.id} p={progress} />
      ) : m.installed ? (
        <div className="install-row">
          <span className="muted small">{t("New meetings label each person on the call.")}</span>
          <span className="spacer" />
          <button
            className="btn ghost danger"
            onClick={() => api.removeSpeakerModel().then(onChanged).catch((e) => toast(errorText(e), "error"))}
          >
            {t("Remove")}
          </button>
        </div>
      ) : (
        <div className="install-row">
          <span className="muted small">{t("{size} download", { size: bytes(m.download) })}</span>
          <span className="spacer" />
          <button className="btn primary" disabled={otherBusy} onClick={() => api.installSpeakerModel().catch((e) => toast(errorText(e), "error"))}>
            {t("Install")}
          </button>
        </div>
      )}
    </article>
  );
}
