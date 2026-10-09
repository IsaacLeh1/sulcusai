// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type CatalogView, type FoundModels, type InstallProgress, type Profile } from "../api";
import { APP_NAME } from "../brand";
import { bytes, percent, placementLabel, speedLabel } from "../format";
import { NewPinForm, RecoveryCode } from "../components/Security";
import type { PushToast } from "../components/Toasts";
import { starterModel } from "../starter";
import { t, tx } from "../i18n";

const STEPS = [tx("Welcome"), tx("Your PC"), tx("First model"), tx("About you"), tx("Protect"), tx("Done")] as const;

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
        <ol className="steps" aria-label={t("Setup steps")}>
          {STEPS.map((s, i) => (
            <li key={s} className={i === step ? "active" : i < step ? "done" : ""} aria-current={i === step ? "step" : undefined}>
              {t(s)}
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

function Nav({ onBack, onNext, nextLabel = t("Continue"), nextDisabled }: { onBack?: () => void; onNext: () => void; nextLabel?: string; nextDisabled?: boolean }) {
  return (
    <div className="onboarding-nav">
      {onBack ? <button className="btn ghost" onClick={onBack}>{t("Back")}</button> : <span />}
      <button className="btn primary" onClick={onNext} disabled={nextDisabled}>{nextLabel}</button>
    </div>
  );
}

function Welcome({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <>
      <img src="/logo.svg" alt="" width={64} height={64} className="onboarding-logo" />
      <h1>{t("Welcome to {app}", { app: APP_NAME })}</h1>
      <p>{t("An AI assistant that runs on your own computer.")}</p>
      <ul className="bullets">
        <li><strong>{t("Private by default.")}</strong> {t("Your chats stay on this PC, encrypted. Nothing goes online unless you turn on web or cloud features.")}</li>
        <li><strong>{t("Made for your PC.")}</strong> {t("You only see models this computer can actually run.")}</li>
        <li><strong>{t("You're in charge.")}</strong> {t("An activity log shows everything the app does, including anything that goes over the internet.")}</li>
      </ul>
      <div className="onboarding-nav">
        <button className="btn ghost" onClick={onSkip}>{t("Skip setup")}</button>
        <button className="btn primary" onClick={onNext}>{t("Get started")}</button>
      </div>
    </>
  );
}

function YourPc({ catalog, onNext, onBack }: { catalog: CatalogView | null; onNext: () => void; onBack: () => void }) {
  if (!catalog) return <p className="muted">{t("Checking this PC…")}</p>;
  const hw = catalog.hardware;
  const gpu = hw.gpus.filter((g) => !g.integrated).sort((a, b) => b.vram_bytes - a.vram_bytes)[0];
  const fits = catalog.models.length;
  return (
    <>
      <h1>{t("Here's what this PC can do")}</h1>
      <div className="hw-grid">
        <div className="spec"><span className="spec-label">{t("Graphics card")}</span><span className="spec-value">{gpu ? t("{size} video memory", { size: bytes(gpu.vram_bytes) }) : t("None usable")}</span><span className="muted small ellipsis">{gpu?.name ?? t("Models will run on the processor")}</span></div>
        <div className="spec"><span className="spec-label">{t("Memory")}</span><span className="spec-value">{bytes(hw.ram_total)}</span></div>
        <div className="spec"><span className="spec-label">{t("Disk")}</span><span className="spec-value">{t("{size} free", { size: bytes(hw.disk_free) })}</span></div>
      </div>
      <p className="callout">
        <strong>{fits === 1 ? t("1 model fits") : t("{n} models fit", { n: fits })}</strong> {t("this PC.")}
        {catalog.hints.hidden > 0 &&
          ` ${catalog.hints.hidden === 1 ? t("1 larger one is hidden because it needs more memory than this PC has.") : t("{n} larger ones are hidden because they need more memory than this PC has.", { n: catalog.hints.hidden })}`}
      </p>
      <Nav onBack={onBack} onNext={onNext} />
    </>
  );
}

function FirstModel({ catalog, progress, onNext, onBack, toast }: { catalog: CatalogView | null; progress: Record<string, InstallProgress>; onNext: () => void; onBack: () => void; toast: PushToast }) {
  // Models from an earlier install or another app (LM Studio, Ollama and so on).
  const [looking, setLooking] = useState(true);
  const [found, setFound] = useState<FoundModels | null>(null);
  useEffect(() => {
    api.findModels().then(setFound).catch(() => {}).finally(() => setLooking(false));
  }, []);
  if (!catalog || looking) return <p className="muted">{t("Looking for models already on this PC…")}</p>;
  const installed = catalog.models.filter((m) => m.installed);
  const pick = starterModel(catalog.models);
  const p = pick ? progress[pick.id] : undefined;
  const quant = pick?.on_disk ?? pick?.fit.recommended;
  const fit = pick?.fit.variants.find((v) => v.quant === quant);
  const variant = pick?.variants.find((v) => v.quant === quant);

  return (
    <>
      <h1>{t("Pick your first model")}</h1>
      {installed.length > 0 && (
        <p className="callout">
          {t("You already have {models}. You can add another or continue.", {
            models: installed.map((m) => (m.local ? t("{name} (from {app})", { name: m.name, app: m.local.app }) : m.name)).join(", "),
          })}
        </p>
      )}
      {installed.length === 0 && found && (
        <p className="muted small">
          {found.looked_in.length > 0
            ? t("Looked for models in {places} and found none this PC can use{skipped}.", {
                places: found.looked_in.join(", "),
                skipped: found.skipped.length ? ` (${found.skipped.map((s) => `${s.name} ${s.reason}`).join("; ")})` : "",
              })
            : t("No models from other apps (Ollama, LM Studio and others) are on this PC.")}
        </p>
      )}
      {pick && fit && variant ? (
        <div className="card model starter">
          <div className="model-head">
            <div>
              <h3>{pick.name}</h3>
              <span className="muted small">{pick.publisher} · {t("{quality} quality", { quality: variant.quality })} · {bytes(variant.size)}</span>
            </div>
            <span className="badge">{pick.on_disk ? t("Already on this PC") : t("Suggested for this PC")}</span>
          </div>
          <p className="desc">{pick.description}</p>
          <p className="muted small">{placementLabel(fit.placement)} · {speedLabel(fit.est_tps)} · {pick.license.name}</p>
          {p ? (
            <div className="install-progress">
              <div className="progress-label">
                <span>{p.phase === "download" && p.total ? t("Downloading · {received} of {total}", { received: bytes(p.received), total: bytes(p.total) }) : t("Setting up…")}</span>
              </div>
              <div className={`progress ${p.total ? "" : "indeterminate"}`}><span style={{ width: p.total ? `${percent(p.received, p.total)}%` : undefined }} /></div>
              <p className="muted small">{t("You can keep going while it downloads.")}</p>
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
              {pick.on_disk ? t("Set up {name} (no download)", { name: pick.name }) : t("Install {name}", { name: pick.name })}
            </button>
          )}
        </div>
      ) : (
        installed.length === 0 && <p className="muted">{t("No model fits this PC's free disk space and memory right now. You can come back to this from the Models page.")}</p>
      )}
      <p className="muted small">{t("You can install more models, or remove this one, from the Models page at any time.")}</p>
      <Nav onBack={onBack} onNext={onNext} nextLabel={p || installed.length > 0 ? t("Continue") : t("Choose later")} />
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
      <h1>{t("Tell it about you")} <span className="muted small">{t("(optional)")}</span></h1>
      <p className="muted">{t("Every chat includes this, so you don't have to repeat yourself. It's encrypted on this PC.")}</p>
      <div className="form">
        <label>
          {t("Name")}
          <input className="input" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} placeholder={t("What should it call you?")} maxLength={80} />
        </label>
        <label>
          {t("About you")}
          <textarea className="input" rows={3} value={profile.about} onChange={(e) => setProfile({ ...profile, about: e.target.value })} placeholder={t("Your work, studies or interests.")} maxLength={4000} />
        </label>
        <label>
          {t("How it should respond")}
          <textarea className="input" rows={2} value={profile.preferences} onChange={(e) => setProfile({ ...profile, preferences: e.target.value })} placeholder={t("For example: short answers, plain language.")} maxLength={4000} />
        </label>
      </div>
      <Nav onBack={onBack} onNext={filled ? save : onNext} nextLabel={filled ? t("Save and continue") : t("Skip")} />
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
        <h1>{t("Your recovery code")}</h1>
        <RecoveryCode code={code} onDone={onNext} />
      </>
    );
  }
  return (
    <>
      <h1>{t("Protect your chats")}</h1>
      <p>
        {t("Your chats are already")} <strong>{t("encrypted on this PC")}</strong>
        {t(", and only your Windows account can open them. Add a PIN if other people use this PC with your account, so SulcusAI asks for it before showing anything.")}
      </p>
      {alreadyOn ? (
        <p className="callout">{t("App lock is already on.")}</p>
      ) : mode === "pin" ? (
        <NewPinForm
          submitLabel={t("Turn on app lock")}
          onSubmit={async (pin) => {
            setCode(await api.enableLock(pin));
            setMode("code");
          }}
        />
      ) : (
        <button className="btn" onClick={() => setMode("pin")}>{t("Set a PIN")}</button>
      )}
      <Nav onBack={onBack} onNext={onNext} nextLabel={alreadyOn ? t("Continue") : t("Not now")} />
    </>
  );
}

function Finished({ onFinish, installing }: { onFinish: () => void; installing: boolean }) {
  return (
    <>
      <h1>{t("You're all set")}</h1>
      <ul className="bullets">
        <li>{t("Start a chat with")} <strong>{t("+ New chat")}</strong>.</li>
        <li>{t("The")} <strong>{t("🔒 Offline")}</strong> {t("switch at the bottom controls what may go online. 🌐 in a chat turns on web for that chat only.")}</li>
        <li>{t("Settings has your profile, app lock and connectivity. Activity shows what the app has done.")}</li>
        <li>{t("It starts simple. Add dictation, voice chat, meeting notes, files and more from")} <strong>{t("✨ Features")}</strong> {t("whenever you want them.")}</li>
        {installing && <li>{t("Your model is still downloading. Its progress shows on the Models page.")}</li>}
      </ul>
      <div className="onboarding-nav">
        <span />
        <button className="btn primary" onClick={onFinish}>{t("Start using {app}", { app: APP_NAME })}</button>
      </div>
    </>
  );
}
