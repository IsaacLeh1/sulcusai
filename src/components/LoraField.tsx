// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Lora } from "../api";
import { t } from "../i18n";
import { InfoTip } from "./InfoTip";
import type { PushToast } from "./Toasts";

/** A picture model's family: which LoRAs fit it (media/loras.rs `family_of`). */
export function loraFamily(modelId: string): string | null {
  const id = modelId.toLowerCase();
  if (id.includes("klein-4b")) return "flux2-klein-4b";
  if (id.includes("klein-9b")) return "flux2-klein-9b";
  if (id.startsWith("flux1") || id.includes("flux.1")) return "flux1";
  if (id.includes("xl")) return "sdxl";
  if (id.includes("realistic-vision") || id.includes("sd15") || id.includes("sd-1.5") || id.includes("v1-5")) return "sd15";
  if (id.includes("z-image")) return "z-image";
  if (id.includes("qwen-image")) return "qwen-image";
  return null;
}

/** The picture form's LoRAs: the person's own add-ons for the picked model's
 * family, each on or off with how strongly it's used; added from a file. */
export function LoraField({ modelId, value, onChange, toast }: { modelId: string; value: [string, number][]; onChange: (v: [string, number][]) => void; toast: PushToast }) {
  const [all, setAll] = useState<Lora[]>([]);
  const [families, setFamilies] = useState<[string, string][]>([]);
  const [editing, setEditing] = useState<Lora | null>(null);
  const load = useCallback(() => api.loras().then((v) => (setAll(v.loras ?? []), setFamilies(v.families ?? []))).catch(() => {}), []);
  useEffect(() => {
    load();
  }, [load]);
  const family = loraFamily(modelId);
  const fits = all.filter((l) => l.family === family);
  const others = all.length - fits.length;
  const picked = (id: string) => value.find(([x]) => x === id);
  const add = async () => {
    try {
      const p = await openFile({ multiple: false, filters: [{ name: t("LoRA"), extensions: ["safetensors"] }] });
      if (typeof p !== "string") return;
      const l = await api.loraAdd(p);
      await load();
      // Its family to confirm (or pick, when it couldn't be told).
      setEditing(l);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const save = async (l: Lora) => {
    try {
      setAll(await api.loraSave(l));
      setEditing(null);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="music-field">
      <span className="music-label">
        {t("Your LoRAs")}
        <InfoTip label={t("Your LoRAs")} text={t("Small add-on files that teach a picture model a character, a style or an object. Each works with the models it was made for (its family). Add one from a file, then turn it on here and set how strongly it's used.")} />
      </span>
      <div className="row wrap">
        {fits.map((l) => {
          const on = picked(l.id);
          return (
            <span key={l.id} className="row lora-pick">
              <div className="filters tight">
                <button className={`chip ${on ? "active" : ""}`} onClick={() => onChange(on ? value.filter(([x]) => x !== l.id) : [...value, [l.id, l.weight]])} title={l.trigger ? t("Adds: {words}", { words: l.trigger }) : undefined}>
                  {l.name}
                </button>
              </div>
              {on && (
                <label className="small row music-length" title={t("How strongly it's used")}>
                  <input type="range" min={0.1} max={1.5} step={0.05} value={on[1]} onChange={(e) => onChange(value.map(([x, w]) => (x === l.id ? [x, +e.target.value] : [x, w])))} aria-label={t("How strongly {name} is used", { name: l.name })} />
                  {on[1].toFixed(2)}
                </label>
              )}
              <button className="link small" onClick={() => setEditing(l)}>{t("Edit")}</button>
            </span>
          );
        })}
        <button className="btn small ghost" onClick={add}>{t("Add a LoRA…")}</button>
      </div>
      {fits.length === 0 && (
        <span className="small muted">
          {family ? t("None for this model's family yet.") : t("LoRAs don't work with this model.")}
          {others > 0 ? ` ${t("{n} more are for other models.", { n: others })}` : ""}
        </span>
      )}
      {editing && (
        <div className="lora-edit form">
          <label className="small">
            {t("Name")}
            <input className="input" value={editing.name} onChange={(e) => setEditing({ ...editing, name: e.target.value })} />
          </label>
          <label className="small">
            {t("Made for")}
            <select value={editing.family} onChange={(e) => setEditing({ ...editing, family: e.target.value })}>
              <option value="">{t("Not known: pick the model it was made for")}</option>
              {families.map(([id, name]) => (
                <option key={id} value={id}>{name}</option>
              ))}
            </select>
          </label>
          <label className="small">
            {t("Words that call it up (from where you got it, optional)")}
            <input className="input" value={editing.trigger} onChange={(e) => setEditing({ ...editing, trigger: e.target.value })} placeholder={t("e.g. ohwx woman")} />
          </label>
          <div className="row">
            <button className="btn small danger ghost" onClick={async () => { setAll(await api.loraDelete(editing.id)); onChange(value.filter(([x]) => x !== editing.id)); setEditing(null); }}>{t("Remove")}</button>
            <span className="spacer" />
            <button className="btn small" onClick={() => setEditing(null)}>{t("Cancel")}</button>
            <button className="btn small primary" onClick={() => save(editing)}>{t("Save")}</button>
          </div>
        </div>
      )}
    </div>
  );
}
