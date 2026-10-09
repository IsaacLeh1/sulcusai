// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type CatalogView, type InstallProgress, type Profile } from "../api";
import { APP_NAME } from "../brand";
import { bytes, percent, placementLabel, speedLabel } from "../format";
import { NewPinForm, RecoveryCode } from "../components/Security";
import type { PushToast } from "../components/Toasts";
import { starterModel } from "../starter";

const STEPS = ["Welcome", "Your PC", "First model", "About you", "Protect", "Done"] as const;

interface Props {
  catalog: CatalogView | null;
  progress: Record<string, InstallProgress>;
  onFinish: () => void;
  toast: PushToast;
}

export function Onboarding({ catalog, progress, onFinish, toast }: Props) {
  const [step, setStep] = useState(0);
  const next = () => setStep((s) => Math.min(STEPS.length - 1, s + 1));
  const back = () => setStep((s) => Math.max(0, s - 1));

  return (
    <div className="onboarding">
      <div className="onboarding-card">
        <ol className="steps" aria-label="Setup steps">
          {STEPS.map((s, i) => (
            <li key={s} className={i === step ? "active" : i < step ? "done" : ""} aria-current={i === step ? "step" : undefined}>
              {s}
            </li>
          ))}
        </ol>
        {step === 0 && <Welcome onNext={next} onSkip={onFinish} />}
        {step === 1 && <YourPc catalog={catalog} onNext={next} onBack={back} />}
        {step === 2 && <FirstModel catalog={catalog} progress={progress} onNext={next} onBack={back} toast={toast} />}
        {step === 3 && <AboutYou onNext={next} onBack={back} toast={toast} />}
        {step === 4 && <Protect onNext={next} onBack={back} />}
        {step === 5 && <Finished onFinish={onFinish} installing={Object.keys(progress).length > 0} />}
      </div>
    </div>
  );
}

function Nav({ onBack, onNext, nextLabel = "Continue", nextDisabled }: { onBack?: () => void; onNext: () => void; nextLabel?: string; nextDisabled?: boolean }) {
  return (
    <div className="onboarding-nav">
      {onBack ? <button className="btn ghost" onClick={onBack}>Back</button> : <span />}
      <button className="btn primary" onClick={onNext} disabled={nextDisabled}>{nextLabel}</button>
    </div>
  );
}

function Welcome({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <>
      <img src="/logo.svg" alt="" width={64} height={64} className="onboarding-logo" />
      <h1>Welcome to {APP_NAME}</h1>
      <p>An AI assistant that runs on your own computer.</p>
      <ul className="bullets">
        <li><strong>Private by default.</strong> Your chats stay on this PC, encrypted. Nothing goes online unless you turn on web or cloud features.</li>
        <li><strong>Made for your PC.</strong> You only see models this computer can actually run.</li>
        <li><strong>You're in charge.</strong> An activity log shows everything the app does, including anything that goes over the internet.</li>
      </ul>
      <div className="onboarding-nav">
        <button className="btn ghost" onClick={onSkip}>Skip setup</button>
        <button className="btn primary" onClick={onNext}>Get started</button>
      </div>
    </>
  );
}

function YourPc({ catalog, onNext, onBack }: { catalog: CatalogView | null; onNext: () => void; onBack: () => void }) {
  if (!catalog) return <p className="muted">Checking this PC…</p>;
  const hw = catalog.hardware;
  const gpu = hw.gpus.filter((g) => !g.integrated).sort((a, b) => b.vram_bytes - a.vram_bytes)[0];
  const fits = catalog.models.length;
  return (
    <>
      <h1>Here's what this PC can do</h1>
      <div className="hw-grid">
        <div className="spec"><span className="spec-label">Graphics card</span><span className="spec-value">{gpu ? `${bytes(gpu.vram_bytes)} video memory` : "None usable"}</span><span className="muted small ellipsis">{gpu?.name ?? "Models will run on the processor"}</span></div>
        <div className="spec"><span className="spec-label">Memory</span><span className="spec-value">{bytes(hw.ram_total)}</span></div>
        <div className="spec"><span className="spec-label">Disk</span><span className="spec-value">{bytes(hw.disk_free)} free</span></div>
      </div>
      <p className="callout">
        <strong>{fits} {fits === 1 ? "model fits" : "models fit"}</strong> this PC.
        {catalog.hints.hidden > 0 && ` ${catalog.hints.hidden} larger ${catalog.hints.hidden === 1 ? "one is" : "ones are"} hidden because ${catalog.hints.hidden === 1 ? "it needs" : "they need"} more memory than this PC has.`}
      </p>
      <Nav onBack={onBack} onNext={onNext} />
    </>
  );
}

function FirstModel({ catalog, progress, onNext, onBack, toast }: { catalog: CatalogView | null; progress: Record<string, InstallProgress>; onNext: () => void; onBack: () => void; toast: PushToast }) {
  // Models from an earlier install or another app (LM Studio, Ollama and so on).
  const [looking, setLooking] = useState(true);
  useEffect(() => {
    api.findModels().catch(() => {}).finally(() => setLooking(false));
  }, []);
  if (!catalog || looking) return <p className="muted">Looking for models already on this PC…</p>;
  const installed = catalog.models.filter((m) => m.installed);
  const pick = starterModel(catalog.models);
  const p = pick ? progress[pick.id] : undefined;
  const quant = pick?.on_disk ?? pick?.fit.recommended;
  const fit = pick?.fit.variants.find((v) => v.quant === quant);
  const variant = pick?.variants.find((v) => v.quant === quant);

  return (
    <>
      <h1>Pick your first model</h1>
      {installed.length > 0 && (
        <p className="callout">You already have {installed.map((m) => m.name).join(", ")} installed. You can add another or continue.</p>
      )}
      {pick && fit && variant ? (
        <div className="card model starter">
          <div className="model-head">
            <div>
              <h3>{pick.name}</h3>
              <span className="muted small">{pick.publisher} · {variant.quality} quality · {bytes(variant.size)}</span>
            </div>
            <span className="badge">{pick.on_disk ? "Already on this PC" : "Suggested for this PC"}</span>
          </div>
          <p className="desc">{pick.description}</p>
          <p className="muted small">{placementLabel(fit.placement)} · {speedLabel(fit.est_tps)} · {pick.license.name}</p>
          {p ? (
            <div className="install-progress">
              <div className="progress-label">
                <span>{p.phase === "download" && p.total ? `Downloading · ${bytes(p.received)} of ${bytes(p.total)}` : "Setting up…"}</span>
              </div>
              <div className={`progress ${p.total ? "" : "indeterminate"}`}><span style={{ width: p.total ? `${percent(p.received, p.total)}%` : undefined }} /></div>
              <p className="muted small">You can keep going while it downloads.</p>
            </div>
          ) : (
            <button
              className="btn primary"
              onClick={async () => {
                try {
                  await api.install(pick.id, variant.quant);
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
              disabled={Object.keys(progress).length > 0}
            >
              {pick.on_disk ? `Set up ${pick.name} (no download)` : `Install ${pick.name}`}
            </button>
          )}
        </div>
      ) : (
        installed.length === 0 && <p className="muted">No model fits this PC's free disk space and memory right now. You can come back to this from the Models page.</p>
      )}
      <p className="muted small">You can install more models, or remove this one, from the Models page at any time.</p>
      <Nav onBack={onBack} onNext={onNext} nextLabel={p || installed.length > 0 ? "Continue" : "Choose later"} />
    </>
  );
}

function AboutYou({ onNext, onBack, toast }: { onNext: () => void; onBack: () => void; toast: PushToast }) {
  const [profile, setProfile] = useState<Profile>({ name: "", about: "", preferences: "" });
  useEffect(() => {
    api.profile().then(setProfile).catch(() => {});
  }, []);
  // Only the fields on this step; the profile also holds location and coordinates (which can be null).
  const filled = [profile.name, profile.about, profile.preferences].some((v) => v?.trim());
  const save = async () => {
    try {
      await api.setProfile(profile);
      onNext();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <>
      <h1>Tell it about you <span className="muted small">(optional)</span></h1>
      <p className="muted">Every chat includes this, so you don't have to repeat yourself. It's encrypted on this PC.</p>
      <div className="form">
        <label>
          Name
          <input className="input" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} placeholder="What should it call you?" maxLength={80} />
        </label>
        <label>
          About you
          <textarea className="input" rows={3} value={profile.about} onChange={(e) => setProfile({ ...profile, about: e.target.value })} placeholder="Your work, studies or interests." maxLength={4000} />
        </label>
        <label>
          How it should respond
          <textarea className="input" rows={2} value={profile.preferences} onChange={(e) => setProfile({ ...profile, preferences: e.target.value })} placeholder="For example: short answers, plain language." maxLength={4000} />
        </label>
      </div>
      <Nav onBack={onBack} onNext={filled ? save : onNext} nextLabel={filled ? "Save and continue" : "Skip"} />
    </>
  );
}

function Protect({ onNext, onBack }: { onNext: () => void; onBack: () => void }) {
  const [mode, setMode] = useState<"ask" | "pin" | "code">("ask");
  const [code, setCode] = useState<string | null>(null);
  const [alreadyOn, setAlreadyOn] = useState(false);
  useEffect(() => {
    api.security().then((s) => setAlreadyOn(s.lock_enabled)).catch(() => {});
  }, []);

  if (mode === "code" && code) {
    return (
      <>
        <h1>Your recovery code</h1>
        <RecoveryCode code={code} onDone={onNext} />
      </>
    );
  }
  return (
    <>
      <h1>Protect your chats</h1>
      <p>
        Your chats are already <strong>encrypted on this PC</strong>, and only your Windows account can open them.
        Add a PIN if other people use this PC with your account, so SulcusAI asks for it before showing anything.
      </p>
      {alreadyOn ? (
        <p className="callout">App lock is already on.</p>
      ) : mode === "pin" ? (
        <NewPinForm
          submitLabel="Turn on app lock"
          onSubmit={async (pin) => {
            setCode(await api.enableLock(pin));
            setMode("code");
          }}
        />
      ) : (
        <button className="btn" onClick={() => setMode("pin")}>Set a PIN</button>
      )}
      <Nav onBack={onBack} onNext={onNext} nextLabel={alreadyOn ? "Continue" : "Not now"} />
    </>
  );
}

function Finished({ onFinish, installing }: { onFinish: () => void; installing: boolean }) {
  return (
    <>
      <h1>You're all set</h1>
      <ul className="bullets">
        <li>Start a chat with <strong>+ New chat</strong>.</li>
        <li>The <strong>🔒 Offline</strong> switch at the bottom controls what may go online. 🌐 in a chat turns on web for that chat only.</li>
        <li>Settings has your profile, app lock and connectivity. Activity shows what the app has done.</li>
        <li>It starts simple. Add dictation, voice chat, meeting notes, files and more from <strong>✨ Features</strong> whenever you want them.</li>
        {installing && <li>Your model is still downloading. Its progress shows on the Models page.</li>}
      </ul>
      <div className="onboarding-nav">
        <span />
        <button className="btn primary" onClick={onFinish}>Start using {APP_NAME}</button>
      </div>
    </>
  );
}
