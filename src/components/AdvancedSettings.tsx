// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Advanced, type ModelCard } from "../api";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";
import { TeachModel } from "./TeachModel";

const DEFAULTS: Advanced = {
  enabled: false,
  sampling: { temperature: null, top_p: null, top_k: null, min_p: null, repeat_penalty: null, presence_penalty: null, frequency_penalty: null, seed: null },
  engine: { ctx: null, gpu_layers: null, threads: null, batch: null },
  guardrails: { instructions: null, extra: "", never: [], always_ask: false, max_steps: null },
  processing: { thinking: "auto", handoff_at: null, handoff_off: false, helper_steps: null },
};

/** A number field where empty means "the app decides". */
function Num({ label, value, onChange, placeholder, step, min, max, hint }: { label: string; value: number | null; onChange: (v: number | null) => void; placeholder: string; step?: number; min?: number; max?: number; hint?: string }) {
  return (
    <label className="adv-field">
      <span>{label}</span>
      <input
        className="input"
        type="number"
        value={value ?? ""}
        step={step ?? 1}
        min={min}
        max={max}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))}
      />
      {hint && <span className="muted small">{hint}</span>}
    </label>
  );
}

const NEVER: { id: string; label: string }[] = [
  { id: "write", label: tx("Change, move or delete files") },
  { id: "execute", label: tx("Run programs and commands") },
  { id: "connector", label: tx("Use connector tools (MCP servers)") },
];

export function AdvancedSettings({ toast }: { toast: PushToast }) {
  const [saved, setSaved] = useState<Advanced | null>(null);
  const [draft, setDraft] = useState<Advanced | null>(null);
  const [locked, setLocked] = useState(false);
  const [pin, setPin] = useState<string | null>(null);
  const [pinInput, setPinInput] = useState("");
  const [newPin, setNewPin] = useState("");
  const [models, setModels] = useState<ModelCard[]>([]);
  const [speedModel, setSpeedModel] = useState("");
  const [measuring, setMeasuring] = useState(false);

  const load = () =>
    api.advancedView().then((v) => {
      setSaved(v.settings);
      setDraft(v.settings);
      setLocked(v.locked);
    });
  useEffect(() => {
    load();
    api.catalog().then((c) => {
      const list = c.models.filter((m) => m.installed);
      setModels(list);
      setSpeedModel((id) => id || list[0]?.id || "");
    });
  }, []);

  if (!draft || !saved) return null;
  const unlocked = !locked || pin !== null;
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const set = <K extends keyof Advanced>(k: K, v: Advanced[K]) => setDraft({ ...draft, [k]: v });

  const save = async (next = draft) => {
    try {
      await api.setAdvanced(next, pin);
      setSaved(next);
      setDraft(next);
      toast(next.enabled ? t("Advanced settings saved. They apply from the next message.") : t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  if (!unlocked) {
    return (
      <section className="card">
        <h2>{t("Advanced")}</h2>
        <p className="muted small">{t("Advanced settings are locked with a PIN.")}</p>
        <form
          className="row"
          onSubmit={async (e) => {
            e.preventDefault();
            try {
              await api.checkAdvancedPin(pinInput);
              setPin(pinInput);
              setPinInput("");
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        >
          <input className="input" type="password" value={pinInput} onChange={(e) => setPinInput(e.target.value)} placeholder={t("PIN")} aria-label={t("Advanced settings PIN")} />
          <button className="btn" disabled={!pinInput}>{t("Unlock")}</button>
        </form>
      </section>
    );
  }

  const s = draft.sampling;
  const en = draft.engine;
  const g = draft.guardrails;
  const p = draft.processing;

  return (
    <section className="card advanced">
      <h2>{t("Advanced")}</h2>
      <label className="check">
        <input type="checkbox" checked={draft.enabled} onChange={(e) => save({ ...draft, enabled: e.target.checked })} />
        <span>
          <strong>{t("Enable advanced mode")}</strong>
          <span className="muted small block">{t("These settings can make answers worse, slower or less safe. Nothing below applies while this is off.")}</span>
        </span>
      </label>

      {draft.enabled && (
        <>
          <div className="adv-group">
            <div className="adv-head">
              <h3>{t("Model tuning")}</h3>
              <button className="btn ghost small" onClick={() => setDraft({ ...draft, sampling: DEFAULTS.sampling, engine: DEFAULTS.engine })}>{t("Reset")}</button>
            </div>
            <p className="muted small">{t("Empty fields use the app's choice for this PC and model.")}</p>
            <div className="adv-grid">
              <Num label={t("Temperature")} value={s.temperature} onChange={(v) => set("sampling", { ...s, temperature: v })} placeholder="0.6" step={0.05} min={0} max={2} hint={t("Higher is more varied")} />
              <Num label="Top-p" value={s.top_p} onChange={(v) => set("sampling", { ...s, top_p: v })} placeholder={t("model default")} step={0.01} min={0} max={1} />
              <Num label="Top-k" value={s.top_k} onChange={(v) => set("sampling", { ...s, top_k: v })} placeholder={t("model default")} min={0} />
              <Num label="Min-p" value={s.min_p} onChange={(v) => set("sampling", { ...s, min_p: v })} placeholder={t("model default")} step={0.01} min={0} max={1} />
              <Num label={t("Repeat penalty")} value={s.repeat_penalty} onChange={(v) => set("sampling", { ...s, repeat_penalty: v })} placeholder="1.0" step={0.05} min={0.5} max={2} />
              <Num label={t("Presence penalty")} value={s.presence_penalty} onChange={(v) => set("sampling", { ...s, presence_penalty: v })} placeholder="0" step={0.1} min={-2} max={2} />
              <Num label={t("Frequency penalty")} value={s.frequency_penalty} onChange={(v) => set("sampling", { ...s, frequency_penalty: v })} placeholder="0" step={0.1} min={-2} max={2} />
              <Num label={t("Seed")} value={s.seed} onChange={(v) => set("sampling", { ...s, seed: v })} placeholder={t("random")} hint={t("Same seed, same answer")} />
            </div>
            <div className="adv-grid">
              <Num label={t("Context length")} value={en.ctx} onChange={(v) => set("engine", { ...en, ctx: v })} placeholder={t("automatic")} step={1024} min={512} hint={t("Tokens the model can see")} />
              <Num label={t("GPU layers")} value={en.gpu_layers} onChange={(v) => set("engine", { ...en, gpu_layers: v })} placeholder={t("automatic")} min={0} hint={t("0 runs on the processor")} />
              <Num label={t("Threads")} value={en.threads} onChange={(v) => set("engine", { ...en, threads: v })} placeholder={t("automatic")} min={1} />
              <Num label={t("Batch size")} value={en.batch} onChange={(v) => set("engine", { ...en, batch: v })} placeholder={t("automatic")} step={64} min={32} />
            </div>
            <p className="muted small">{t("A different context length, layers, threads or batch size restarts the model on its next message. Too much context or too many GPU layers can fail to load.")}</p>
          </div>

          <div className="adv-group">
            <div className="adv-head">
              <h3>{t("Guardrails")}</h3>
              <button className="btn ghost small" onClick={() => set("guardrails", DEFAULTS.guardrails)}>{t("Reset")}</button>
            </div>
            <label className="adv-field wide">
              <span>{t("Instructions")}</span>
              <textarea
                className="input"
                rows={4}
                value={g.instructions ?? ""}
                onChange={(e) => set("guardrails", { ...g, instructions: e.target.value || null })}
                placeholder={t("Leave empty to use the app's own instructions (its role, honesty and how it uses tools). Anything here replaces them.")}
              />
            </label>
            <label className="adv-field wide">
              <span>{t("Extra instructions")}</span>
              <textarea className="input" rows={2} value={g.extra} onChange={(e) => set("guardrails", { ...g, extra: e.target.value })} placeholder={t("Added after the instructions, e.g. “Never give medical advice.”")} />
            </label>
            <span className="small">{t("Never let the assistant:")}</span>
            {NEVER.map((n) => (
              <label key={n.id} className="check">
                <input
                  type="checkbox"
                  checked={g.never.includes(n.id)}
                  onChange={(e) => set("guardrails", { ...g, never: e.target.checked ? [...g.never, n.id] : g.never.filter((x) => x !== n.id) })}
                />
                <span>{t(n.label)}</span>
              </label>
            ))}
            <label className="check">
              <input type="checkbox" checked={g.always_ask} onChange={(e) => set("guardrails", { ...g, always_ask: e.target.checked })} />
              <span>{t("Ask me before every change or command, even in Auto and Bypass")}</span>
            </label>
            <div className="adv-grid">
              <Num label={t("Most steps per reply")} value={g.max_steps} onChange={(v) => set("guardrails", { ...g, max_steps: v })} placeholder="30" min={1} max={200} />
            </div>
            <p className="muted small">{t("These change the app's own layer only. Behavior trained into a model can't be switched off here, so how much can be loosened depends on the model.")}</p>
          </div>

          <div className="adv-group">
            <div className="adv-head">
              <h3>{t("How it processes")}</h3>
              <button className="btn ghost small" onClick={() => set("processing", DEFAULTS.processing)}>{t("Reset")}</button>
            </div>
            <label className="adv-field">
              <span>{t("Thinking before answering")}</span>
              <select className="input" value={p.thinking} onChange={(e) => set("processing", { ...p, thinking: e.target.value as Advanced["processing"]["thinking"] })}>
                <option value="auto">{t("The model's default")}</option>
                <option value="on">{t("On")}</option>
                <option value="off">{t("Off (faster)")}</option>
              </select>
              <span className="muted small">{t("Only for models that can switch it, such as Qwen3.")}</span>
            </label>
            <span className="small">{t("When a chat gets long:")}</span>
            <label className="check">
              <input type="radio" name="long" checked={!p.handoff_off} onChange={() => set("processing", { ...p, handoff_off: false })} />
              <span>
                {t("Continue in a new chat with a summary when it's")}{" "}
                <input
                  className="input inline-num"
                  type="number"
                  min={50}
                  max={95}
                  value={Math.round((p.handoff_at ?? 0.8) * 100)}
                  onChange={(e) => set("processing", { ...p, handoff_at: Number(e.target.value) / 100 })}
                  aria-label={t("Percent full")}
                />
                {t("% full")}
              </span>
            </label>
            <label className="check">
              <input type="radio" name="long" checked={p.handoff_off} onChange={() => set("processing", { ...p, handoff_off: true })} />
              <span>{t("Stay in the same chat and leave out the oldest messages")}</span>
            </label>
            <div className="adv-grid">
              <Num label={t("Steps for each helper agent")} value={p.helper_steps} onChange={(v) => set("processing", { ...p, helper_steps: v })} placeholder="12" min={1} max={100} />
            </div>
          </div>

          <div className="adv-actions">
            <button className="btn primary" onClick={() => save()} disabled={!dirty}>{t("Save")}</button>
            <button className="btn ghost" onClick={() => setDraft({ ...DEFAULTS, enabled: true })}>{t("Reset everything")}</button>
          </div>

          <div className="adv-group">
            <h3>{t("Model work")}</h3>
            <div className="row wrap">
              <button
                className="btn"
                onClick={async () => {
                  const path = await openFile({ multiple: false, filters: [{ name: t("GGUF model"), extensions: ["gguf"] }] });
                  if (typeof path !== "string") return;
                  try {
                    const name = await api.importModelFile(path);
                    toast(t("Added {name}. It's used where it is, not copied.", { name }), "success");
                  } catch (e) {
                    toast(errorText(e), "error");
                  }
                }}
              >
                {t("Import a model file (.gguf)…")}
              </button>
            </div>
            {models.length > 0 && (
              <div className="row wrap">
                <select className="input" value={speedModel} onChange={(e) => setSpeedModel(e.target.value)} aria-label={t("Model to measure")}>
                  {models.map((m) => (
                    <option key={m.id} value={m.id}>{m.name}</option>
                  ))}
                </select>
                <button
                  className="btn"
                  disabled={measuring || !speedModel}
                  onClick={async () => {
                    setMeasuring(true);
                    try {
                      const tps = await api.measureModelSpeed(speedModel);
                      toast(t("{model}: {speed} tokens/sec on this PC.", { model: String(models.find((m) => m.id === speedModel)?.name), speed: tps.toFixed(1) }), "success");
                    } catch (e) {
                      toast(errorText(e), "error");
                    } finally {
                      setMeasuring(false);
                    }
                  }}
                >
                  {measuring ? t("Measuring…") : t("Measure speed")}
                </button>
              </div>
            )}
            <TeachModel toast={toast} />
          </div>
        </>
      )}

      <div className="adv-group">
        <h3>{t("Lock with a PIN")}</h3>
        <p className="muted small">
          {locked
            ? t("Advanced settings need a PIN to change. This PIN is separate from the app lock, so you can share the app without sharing it.")
            : t("For parental controls: a PIN, separate from the app lock, that's needed to change advanced settings and guardrails.")}
        </p>
        <div className="row wrap">
          <input className="input" type="password" value={newPin} onChange={(e) => setNewPin(e.target.value)} placeholder={locked ? t("New PIN") : t("PIN (4+ characters)")} aria-label={t("New advanced settings PIN")} />
          <button
            className="btn"
            disabled={newPin.length < 4}
            onClick={async () => {
              try {
                await api.setAdvancedPin(pin, newPin);
                setPin(newPin);
                setLocked(true);
                setNewPin("");
                toast(t("Advanced settings are locked with your PIN."), "success");
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            {locked ? t("Change PIN") : t("Set PIN")}
          </button>
          {locked && (
            <button
              className="btn ghost"
              onClick={async () => {
                try {
                  await api.setAdvancedPin(pin, "");
                  setLocked(false);
                  setPin(null);
                  toast(t("Removed the PIN."), "success");
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Remove PIN")}
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
