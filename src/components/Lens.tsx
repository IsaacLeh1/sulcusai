// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useState } from "react";
import { api, errorText, type LensEdit, type LensLayers, type LensWords, type ModelCard } from "../api";
import { t, tx } from "../i18n";
import { InfoTip } from "./InfoTip";
import type { PushToast } from "./Toasts";

const PART_NAMES: Record<string, string> = {
  attention_in: tx("Attention in"),
  attention_out: tx("Attention out"),
  ffn_in: tx("Feed-forward in"),
  ffn_inside: tx("Feed-forward inside"),
};

const SCALES: [number, string][] = [
  [1, tx("Leave as is")],
  [0, tx("Turn off")],
  [0.5, tx("Half")],
  [2, tx("Double")],
  [3, tx("Triple")],
];

const pct = (p: number) => (p > 0 && p < 0.005 ? "<1%" : `${Math.round(p * 100)}%`);
/** How many times everyday text, kept short when it's huge. */
const times = (x: number) => (x >= 100 ? "100+" : x.toFixed(1));
/** How a piece shows: the less sure the model was, the warmer. */
const doubt = (p: number) => `color-mix(in srgb, var(--warn, #d97706) ${Math.round(Math.min(1, Math.max(0, 1 - p)) * 70)}%, transparent)`;
const shown = (s: string) => s.replace(/\n/g, "⏎\n");

/** The Lens: a model on this PC seen from inside (lens.rs). Word by word,
 * layer by layer, and an edited copy to compare. Each part works on its
 * own; one that can't run says why. */
export function Lens({ models, toast }: { models: ModelCard[]; toast: PushToast }) {
  const [model, setModel] = useState(models[0]?.id ?? "");
  const [prompt, setPrompt] = useState("");
  const [words, setWords] = useState<LensWords | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [busy, setBusy] = useState<"words" | "layers" | "edit" | "compare" | null>(null);
  const [wordsError, setWordsError] = useState<string | null>(null);
  const [map, setMap] = useState<LensLayers | null>(null);
  const [mapError, setMapError] = useState<string | null>(null);
  const [vsUsual, setVsUsual] = useState(true);
  const [edits, setEdits] = useState<Record<string, number>>({});
  const [copy, setCopy] = useState<string | null>(null);
  const [compare, setCompare] = useState<{ original: string; edited: string } | null>(null);

  const ask = async (given = "", forModel = model) => {
    setBusy("words");
    setWordsError(null);
    setOpen(null);
    try {
      setWords(await api.lensWords(forModel, prompt, given));
    } catch (e) {
      setWordsError(errorText(e));
    } finally {
      setBusy(null);
    }
  };
  const look = async () => {
    setMap(null);
    setCopy(null);
    setCompare(null);
    setEdits({});
    await ask();
  };
  const mapLayers = async () => {
    setBusy("layers");
    setMapError(null);
    try {
      const reply = words ? words.given + words.pieces.map((p) => p.text).join("") : "";
      setMap(await api.lensLayers(model, `${prompt}\n\n${reply}`.trim()));
    } catch (e) {
      setMapError(errorText(e));
    } finally {
      setBusy(null);
    }
  };
  const makeCopy = async () => {
    const list: LensEdit[] = Object.entries(edits)
      .filter(([, s]) => s !== 1)
      .map(([k, scale]) => {
        const [layer, index] = k.split(":").map(Number);
        return { layer, index, scale };
      });
    setBusy("edit");
    try {
      const id = await api.lensEdit(model, list);
      setCopy(id);
      toast(t("Made an edited copy. It's in your models list too."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const compareCopy = async () => {
    if (!copy) return;
    setBusy("compare");
    try {
      const [a, b] = [await api.lensWords(model, prompt, ""), await api.lensWords(copy, prompt, "")];
      setCompare({ original: a.pieces.map((p) => p.text).join(""), edited: b.pieces.map((p) => p.text).join("") });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  if (models.length === 0) return <p className="muted">{t("Install a chat model first: the Lens looks inside models on this PC.")}</p>;
  const changed = Object.values(edits).filter((s) => s !== 1).length;
  const cell = (v: number, column: number[], usual: number) => {
    if (vsUsual && map) {
      const r = usual > 0 ? v / usual : 1;
      // Above everyday text: the accent; below: grey.
      const k = Math.min(1, Math.abs(Math.log2(Math.max(r, 1e-6))));
      return r >= 1 ? `color-mix(in srgb, var(--accent) ${Math.round(k * 90)}%, transparent)` : `color-mix(in srgb, var(--muted, #888) ${Math.round(k * 70)}%, transparent)`;
    }
    const top = Math.max(...column, 1e-9);
    return `color-mix(in srgb, var(--accent) ${Math.round(Math.sqrt(v / top) * 90)}%, transparent)`;
  };
  return (
    <div className="lens">
      <p className="small muted">
        {t("See what happens inside a model on this PC: how sure it was of each word and what else it weighed, which of its layers your prompt used most, and which of its units (neurons) it leaned on. Then turn chosen units up, down or off in a copy of the model and compare the replies. The original model is never changed.")}
      </p>
      <div className="row wrap">
        <select value={model} onChange={(e) => setModel(e.target.value)} aria-label={t("Model")}>
          {models.map((m) => (
            <option key={m.id} value={m.id}>{m.name}</option>
          ))}
        </select>
      </div>
      <textarea className="input" rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder={t("A prompt to look at, e.g. Write one sentence about the sea.")} aria-label={t("Prompt")} />
      <div className="row">
        <span className="spacer" />
        <button className="btn primary" disabled={!prompt.trim() || !!busy} onClick={look}>{busy === "words" ? t("Looking…") : t("Look inside")}</button>
      </div>

      {wordsError && <p className="callout warn small">{wordsError}</p>}
      {words && (
        <section className="card lens-words">
          <h3>
            {t("Word by word")}
            <InfoTip label={t("Word by word")} text={t("Each piece the model wrote, shaded by how unsure it was (darker: less sure). Click one to see what else it weighed there; pick another to carry the reply on from that point. It always takes its most likely choice, so a change comes from your pick, not chance.")} />
          </h3>
          <p className="lens-text">
            {words.given && <span className="lens-given">{shown(words.given)}</span>}
            {words.pieces.map((p, i) => (
              <span key={i} className={`lens-piece ${open === i ? "open" : ""}`}>
                <button className="lens-token" style={{ background: doubt(p.prob) }} onClick={() => setOpen(open === i ? null : i)} title={t("{p} sure", { p: pct(p.prob) })}>
                  {shown(p.text) || "·"}
                </button>
                {open === i && p.alts.length > 0 && (
                  <span className="lens-alts" role="dialog" aria-label={t("What else it weighed")}>
                    {p.alts.map((a, k) => (
                      <span key={k} className="lens-alt">
                        <span className="lens-bar" style={{ width: pct(a.prob) }} />
                        <code>{JSON.stringify(a.text)}</code> {pct(a.prob)}
                        {a.text !== p.text && (
                          <button className="link small" disabled={!!busy} onClick={() => ask(words.given + words.pieces.slice(0, i).map((x) => x.text).join("") + a.text)}>
                            {t("Use this")}
                          </button>
                        )}
                      </span>
                    ))}
                  </span>
                )}
              </span>
            ))}
          </p>
          {words.given && (
            <button className="link small" onClick={() => ask()}>{t("Back to its own reply")}</button>
          )}
        </section>
      )}

      {words && (
        <section className="card lens-layers">
          <h3>
            {t("Layer by layer")}
            <InfoTip label={t("Layer by layer")} text={t("How strongly each layer's attention and feed-forward parts were used by your prompt and the reply, measured with llama.cpp's own statistics tool on the processor. Compared with everyday text, the accent shows what your prompt used more and grey what it used less.")} />
          </h3>
          {!map && (
            <div className="row">
              <span className="small muted">{t("Takes a minute or so on the processor; the card stays free.")}</span>
              <span className="spacer" />
              <button className="btn small" disabled={!!busy} onClick={mapLayers}>{busy === "layers" ? t("Measuring…") : t("Map the layers")}</button>
            </div>
          )}
          {mapError && <p className="callout warn small">{mapError}</p>}
          {map && (
            <>
              <label className="check small">
                <input type="checkbox" checked={vsUsual} onChange={(e) => setVsUsual(e.target.checked)} />
                {t("Compared with everyday text")}
              </label>
              <p className="small muted">{vsUsual ? t("Darker accent: used more than for everyday text. Grey: used less.") : t("Darker: used more, within each column.")}</p>
              <div className="lens-map" style={{ gridTemplateColumns: `3.5em repeat(${map.parts.length}, minmax(0, 1fr))` }}>
                <span />
                {map.parts.map((p) => (
                  <span key={p} className="lens-map-head small">{t(PART_NAMES[p] ?? p)}</span>
                ))}
                {map.energy.map((row, l) => (
                  <Fragment key={l}>
                    <span className="lens-map-layer small muted">{l + 1}</span>
                    {row.map((v, c) => (
                      <span
                        key={`${l}-${c}`}
                        className="lens-map-cell"
                        style={{ background: cell(v, map.energy.map((r) => r[c]), map.usual[l]?.[c] ?? 0) }}
                        title={t("Layer {l}, {part}: {x}× everyday text", { l: l + 1, part: t(PART_NAMES[map.parts[c]] ?? map.parts[c]), x: (map.usual[l]?.[c] ? v / map.usual[l][c] : 0).toFixed(2) })}
                      />
                    ))}
                  </Fragment>
                ))}
              </div>

              <h3>
                {t("Units it leaned on")}
                <InfoTip label={t("Units it leaned on")} text={t("Feed-forward units (often called neurons) that fired much more for your prompt than for everyday text. Each one adds its own small push to what comes next. Turn some off, down or up in a copy of the model and compare the replies.")} />
              </h3>
              {map.neurons.length === 0 && <p className="small muted">{t("None stood out for this prompt.")}</p>}
              <div className="lens-neurons">
                {map.neurons.map((n) => {
                  const key = `${n.layer}:${n.index}`;
                  return (
                    <div key={key} className="lens-neuron small">
                      <span>{t("Layer {l} · unit {i}", { l: n.layer + 1, i: n.index })}</span>
                      <span className="muted">{t("{x}× everyday", { x: times(n.ratio) })}</span>
                      <select value={edits[key] ?? 1} disabled={!!map.editable} onChange={(e) => setEdits((d) => ({ ...d, [key]: Number(e.target.value) }))} aria-label={t("Change this unit")}>
                        {SCALES.map(([s, label]) => (
                          <option key={s} value={s}>{t(label)}</option>
                        ))}
                      </select>
                    </div>
                  );
                })}
              </div>
              {map.editable ? (
                <p className="small muted">{t("Units in this model can't be changed: {why}.", { why: map.editable })}</p>
              ) : (
                <div className="row wrap">
                  <span className="small muted">{t("A copy takes as much disk space as the model.")}</span>
                  <span className="spacer" />
                  <button className="btn small" disabled={!changed || !!busy} onClick={makeCopy}>
                    {busy === "edit" ? t("Making the copy…") : t("Make an edited copy ({n} changed)", { n: changed })}
                  </button>
                  {copy && (
                    <button className="btn primary small" disabled={!!busy} onClick={compareCopy}>{busy === "compare" ? t("Asking both…") : t("Compare the replies")}</button>
                  )}
                </div>
              )}
              {compare && (
                <div className="lens-compare">
                  <div>
                    <h4>{t("Original")}</h4>
                    <p className="lens-text">{compare.original}</p>
                  </div>
                  <div>
                    <h4>{t("Edited copy")}</h4>
                    <p className="lens-text">{compare.edited}</p>
                  </div>
                </div>
              )}
            </>
          )}
        </section>
      )}
    </div>
  );
}
