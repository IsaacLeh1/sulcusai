// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api } from "../api";
import { t } from "../i18n";

export interface ModelChoice {
  id: string;
  name: string;
  /** For a cloud model: its provider, and US dollars per million tokens in and out. */
  provider?: string;
  price_in?: number | null;
  price_out?: number | null;
}

/** "$3 in · $15 out per M tokens", or "" when the prices aren't known. */
export function priceText(price_in?: number | null, price_out?: number | null): string {
  if (price_in == null && price_out == null) return "";
  const n = (x?: number | null) => (x == null ? "?" : `$${x}`);
  return t("{inPrice} in · {outPrice} out per M tokens", { inPrice: n(price_in), outPrice: n(price_out) });
}

/** The models that can be picked: on this PC, then the cloud models turned on. */
export function useModelChoices(): { local: ModelChoice[]; cloud: ModelChoice[] } {
  const [local, setLocal] = useState<ModelChoice[]>([]);
  const [cloud, setCloud] = useState<ModelChoice[]>([]);
  useEffect(() => {
    api.catalog().then((c) => setLocal(c.models.filter((m) => m.installed).map((m) => ({ id: m.id, name: m.name })))).catch(() => {});
    api.cloudChoices().then(setCloud).catch(() => {});
  }, []);
  return { local, cloud };
}

/** A model picker: the default model first, then this PC's, then the cloud's. */
export function ModelSelect({ value, onChange, label, defaultLabel }: { value: string | null; onChange: (id: string | null) => void; label: string; defaultLabel?: string }) {
  const { local, cloud } = useModelChoices();
  return (
    <select className="input" value={value ?? ""} onChange={(e) => onChange(e.target.value || null)} aria-label={label}>
      <option value="">{defaultLabel ?? t("Your default model")}</option>
      {local.length > 0 && (
        <optgroup label={t("On this PC")}>
          {local.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name}
            </option>
          ))}
        </optgroup>
      )}
      {cloud.length > 0 && (
        <optgroup label={t("Cloud (needs the Cloud level)")}>
          {cloud.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name} · {m.provider}
              {priceText(m.price_in, m.price_out) && ` · ${priceText(m.price_in, m.price_out)}`}
            </option>
          ))}
        </optgroup>
      )}
      {value && !local.some((m) => m.id === value) && !cloud.some((m) => m.id === value) && <option value={value}>{t("(removed model)")}</option>}
    </select>
  );
}
