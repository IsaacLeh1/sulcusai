// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, on, type MemoryWarning } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** One kind of memory: what other apps use now, and what the model would add. */
function Bar({ label, total, free, need, short }: { label: string; total: number; free: number; need: number; short: boolean }) {
  const used = Math.max(0, total - free);
  const pct = (n: number) => `${Math.min(100, (n / Math.max(total, 1)) * 100)}%`;
  return (
    <div className={`memory-bar ${short ? "short" : ""}`}>
      <div className="label">
        <span>{label}</span>
        <span className="muted">{t("{need} needed, {free} free", { need: bytes(need), free: bytes(free) })}</span>
      </div>
      <div className="track" aria-hidden="true">
        <span className="used" style={{ width: pct(used) }} />
        <span className="model" style={{ width: pct(need) }} />
      </div>
    </div>
  );
}

/** Before a model loads: the PC is short of memory right now (a game or
 * other apps hold it). Loading anyway can make everything stutter, so the
 * person decides. Closing it counts as no. */
export function MemoryWarningPopup() {
  const [w, setW] = useState<MemoryWarning | null>(null);
  useEffect(() => {
    const subs = [on("engine:memory", (p) => setW(p)), on("engine:memory_done", (p) => setW((cur) => (cur?.id === p.id ? null : cur)))];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []);
  if (!w) return null;
  const answer = (go: boolean) => {
    setW(null);
    api.answerMemoryWarning(w.id, go).catch(() => {});
  };
  return (
    <Modal title={t("Not much memory free for {model}", { model: w.model })} onClose={() => answer(false)}>
      <p>
        {w.short_ram && w.short_vram
          ? t("Other apps are using most of this PC's memory and graphics memory right now, so {model} would leave too little room.", { model: w.model })
          : w.short_vram
            ? t("Other apps are using most of the graphics card's memory right now, so {model} would leave too little room on it.", { model: w.model })
            : t("Other apps are using most of this PC's memory right now, so {model} would leave too little room.", { model: w.model })}
      </p>
      <div className="memory-bars">
        <Bar label={t("Memory")} total={w.ram_total} free={w.ram_free} need={w.ram_need} short={w.short_ram} />
        {w.vram_total !== null && w.vram_free !== null && w.vram_need > 0 && (
          <Bar label={t("Graphics memory")} total={w.vram_total} free={w.vram_free} need={w.vram_need} short={w.short_vram} />
        )}
      </div>
      {w.apps.length > 0 && (
        <>
          <p className="small">{t("Using the most memory now:")}</p>
          <ul className="memory-apps">
            {w.apps.map((a) => (
              <li key={a.name}>
                {a.name} <span className="muted">· {bytes(a.bytes)}</span>
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="muted small">
        {w.short_vram && !w.short_ram
          ? t("Loading it anyway can make the PC and a running game stutter. Closing the game, a smaller context window or a smaller model avoids that. A lower GPU limit at the top of the chat does too, but runs part of the model on the processor: slower and hotter.")
          : t("Loading it anyway can make the PC (and a running game) stutter or freeze while memory runs out. Closing apps you aren't using, or a smaller model, avoids that.")}
      </p>
      <div className="modal-actions">
        <span className="spacer" />
        <button className="btn" onClick={() => answer(true)}>
          {t("Load anyway")}
        </button>
        <button className="btn primary" onClick={() => answer(false)} autoFocus>
          {t("Don't load it")}
        </button>
      </div>
    </Modal>
  );
}
