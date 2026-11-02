// SPDX-License-Identifier: AGPL-3.0-only
// Pieces the Studio's forms share: picking a model (and the default), and
// writing a better description.
import { useState } from "react";
import { api, errorText, type MediaKind, type CloudMediaChoice, type MediaModelCard, type MediaRequest } from "../api";
import { t } from "../i18n";

export type Shape = NonNullable<MediaRequest["shape"]>;

/** Cloud models shown alongside this PC's in the model menus. */
export function cloudCards(cloud: CloudMediaChoice[], kind: MediaKind): MediaModelCard[] {
  const want = kind === "image" ? "image" : kind === "video" ? "video" : null;
  return cloud
    .filter((c) => c.kind === want)
    .map((c) => {
      const edits = /:(gpt-image|dall-e-2)/.test(c.id);
      return {
        id: c.id,
        name: `${c.name} (${c.provider})`,
        publisher: c.provider,
        source: "",
        description: "",
        license: { name: t("{provider}'s terms", { provider: c.provider }), url: "", commercial: true, note: null },
        kind,
        can: kind === "image" ? (edits ? ["generate", "edit", "fill", "restyle"] : ["generate"]) : ["text", "image"],
        files: [],
        quality: 0,
        defaults: { steps: 0, width: 1024, height: 1024, fps: 24, seconds: 4 },
        fit: { runnable: true, on_gpu: false, est_secs: kind === "video" ? 180 : 20, reason: null, disk_ok: true, need_bytes: 0 } as unknown as MediaModelCard["fit"],
        installed: true,
      };
    });
}

/** The model a composer uses: always a menu, even with one model, so there's
 * a way to add another (its last entry opens this kind's models). */
export function ModelPick({ models, value, onChange, onMore }: { models: MediaModelCard[]; value: string; onChange: (id: string) => void; onMore?: () => void }) {
  // Models on this PC are marked as such; cloud ones carry their provider in their names.
  const label = (m: MediaModelCard) => (m.id.startsWith("cloud:") ? m.name : t("{name} (this PC)", { name: m.name }));
  if (models.length === 0) return null;
  if (models.length === 1 && !onMore) return <span className="small muted" title={t("Model")}>{label(models[0])}</span>;
  return (
    <select
      value={value}
      onChange={(e) => (e.target.value === MORE ? onMore?.() : onChange(e.target.value))}
      title={t("Model")}
      aria-label={t("Model")}
    >
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {label(m)}
        </option>
      ))}
      {onMore && <option value={MORE}>{t("More models…")}</option>}
    </select>
  );
}

/** ModelPick's entry that opens the models list. */
export const MORE = "__more_models__";

export function best(models: MediaModelCard[], can?: string): string {
  return [...models].filter((m) => !can || m.can.includes(can)).sort((a, b) => b.quality - a.quality)[0]?.id ?? "";
}

/** The model a composer uses: the one last picked on this tab (remembered,
 * so leaving the tab doesn't undo it), else the default set for this kind,
 * else the best installed one. Worked out on every render, so it's right
 * once the model list arrives too. */
export function useModelPick(kind: string, models: MediaModelCard[], defaultId: string | null): [string, (id: string) => void] {
  const key = `sulcus.studio.model.${kind}`;
  const [chosen, setChosen] = useState<string | null>(() => {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  });
  const resolved = [chosen, defaultId].find((id) => id && models.some((m) => m.id === id)) ?? best(models);
  const pick = (id: string) => {
    setChosen(id);
    try {
      localStorage.setItem(key, id);
    } catch {
      /* not kept */
    }
  };
  return [resolved, pick];
}

/** "Make this the default" beside the model picker (models on this PC only). */
export function DefaultLink({ id, defaultId, onSetDefault }: { id: string | undefined; defaultId: string | null; onSetDefault: (id: string) => void }) {
  if (!id || id.startsWith("cloud:")) return null;
  if (id === defaultId) return <span className="badge subtle" title={t("Chats use this model too, unless you ask for another")}>★ {t("Default")}</span>;
  return (
    <button className="link small" title={t("Chats use the default too, unless you ask for another")} onClick={() => onSetDefault(id)}>
      {t("Make default")}
    </button>
  );
}

/** "Improve": the chat model turns a short idea into a fuller description. */
export function ImproveButton({ prompt, kind, onDone }: { prompt: string; kind: string; onDone: (p: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [why, setWhy] = useState<string | null>(null);
  return (
    <>
      <button
        className="btn ghost small"
        disabled={!prompt.trim() || busy}
        title={t("Your chat model writes a fuller description from your idea. You can edit it before making anything.")}
        onClick={async () => {
          setBusy(true);
          setWhy(null);
          try {
            onDone(await api.improveMediaPrompt(prompt, kind));
          } catch (e) {
            setWhy(errorText(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? t("Improving…") : t("Improve")}
      </button>
      {why && <span className="small warn-text">{why}</span>}
    </>
  );
}
