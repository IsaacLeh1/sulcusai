// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type ModelPreset } from "../api";
import { t } from "../i18n";
import { Num } from "./AdvancedSettings";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const EMPTY: ModelPreset = {
  instructions: "",
  sampling: { temperature: null, top_p: null, top_k: null, min_p: null, repeat_penalty: null, presence_penalty: null, frequency_penalty: null, seed: null },
  ctx: null,
  kv_cache: null,
  flash_attn: null,
};

/** How the engine keeps a model's cache, and flash attention: empty is automatic. */
export function CacheChoices({ kv, flash, onChange }: { kv: string | null | undefined; flash: boolean | null | undefined; onChange: (kv: string | null, flash: boolean | null) => void }) {
  return (
    <>
      <label className="adv-field">
        <span>{t("Cache type")}</span>
        <select className="input" value={kv ?? ""} onChange={(e) => onChange(e.target.value || null, flash ?? null)}>
          <option value="">{t("automatic")}</option>
          <option value="f16">{t("Whole (f16)")}</option>
          <option value="q8_0">{t("Compressed (q8_0)")}</option>
          <option value="q4_0">{t("Small (q4_0)")}</option>
        </select>
        <span className="muted small">{t("Smaller fits a bigger window on the card, a little less exact")}</span>
      </label>
      <label className="adv-field">
        <span>{t("Flash attention")}</span>
        <select className="input" value={flash == null ? "" : flash ? "on" : "off"} onChange={(e) => onChange(kv ?? null, e.target.value === "" ? null : e.target.value === "on")}>
          <option value="">{t("automatic")}</option>
          <option value="on">{t("On")}</option>
          <option value="off">{t("Off")}</option>
        </select>
        <span className="muted small">{t("Off keeps the cache whole")}</span>
      </label>
    </>
  );
}

/** One model's own settings: instructions, how it picks words, its window and cache. */
export function ModelSettings({ modelId, name, onClose, toast }: { modelId: string; name: string; onClose: () => void; toast: PushToast }) {
  const [p, setP] = useState<ModelPreset | null>(null);
  useEffect(() => {
    api.modelPreset(modelId).then((x) => setP({ ...EMPTY, ...x, sampling: { ...EMPTY.sampling, ...x.sampling } })).catch(() => setP(EMPTY));
  }, [modelId]);
  if (!p) return null;
  const s = p.sampling;
  const save = async (next: ModelPreset) => {
    try {
      await api.setModelPreset(modelId, next);
      toast(t("Saved. They apply from its next reply."), "success");
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <Modal title={t("Settings for {name}", { name })} onClose={onClose}>
      <p className="muted small">{t("These apply whenever this model answers: in chats, the local API and programs made for Ollama. Empty fields keep the app's choice (or Advanced's).")}</p>
      <label className="adv-field wide">
        <span>{t("Instructions for this model")}</span>
        <textarea
          className="input"
          rows={3}
          value={p.instructions}
          onChange={(e) => setP({ ...p, instructions: e.target.value })}
          placeholder={t("Added to the usual instructions, e.g. “Answer in British English.”")}
          aria-label={t("Instructions for this model")}
        />
      </label>
      <div className="adv-grid">
        <Num label={t("Temperature")} value={s.temperature} onChange={(v) => setP({ ...p, sampling: { ...s, temperature: v } })} placeholder={t("model default")} step={0.05} min={0} max={2} />
        <Num label="Top-p" value={s.top_p} onChange={(v) => setP({ ...p, sampling: { ...s, top_p: v } })} placeholder={t("model default")} step={0.01} min={0} max={1} />
        <Num label="Top-k" value={s.top_k} onChange={(v) => setP({ ...p, sampling: { ...s, top_k: v } })} placeholder={t("model default")} min={0} />
        <Num label="Min-p" value={s.min_p} onChange={(v) => setP({ ...p, sampling: { ...s, min_p: v } })} placeholder={t("model default")} step={0.01} min={0} max={1} />
        <Num label={t("Repeat penalty")} value={s.repeat_penalty} onChange={(v) => setP({ ...p, sampling: { ...s, repeat_penalty: v } })} placeholder="1.0" step={0.05} min={0.5} max={2} />
        <Num label={t("Context length")} value={p.ctx} onChange={(v) => setP({ ...p, ctx: v })} placeholder={t("automatic")} step={1024} min={512} hint={t("Tokens the model can see")} />
        <CacheChoices kv={p.kv_cache} flash={p.flash_attn} onChange={(kv_cache, flash_attn) => setP({ ...p, kv_cache, flash_attn })} />
      </div>
      <p className="muted small">{t("A different window, cache or flash attention restarts the model on its next message.")}</p>
      <div className="modal-actions">
        <button className="btn ghost" onClick={() => save(EMPTY)}>{t("Reset")}</button>
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={() => save(p)}>{t("Save")}</button>
      </div>
    </Modal>
  );
}
