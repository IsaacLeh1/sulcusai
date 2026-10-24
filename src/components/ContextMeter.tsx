// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type ContextInfo, type ContextOptions, type WindowOption } from "../api";
import { bytes, duration, percent, placementLabel, tokens, words } from "../format";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** "32K" for 32,768 tokens, "1M" for 1,048,576. */
const short = (n: number) => (n >= 1 << 20 ? `${+(n / (1 << 20)).toFixed(1)}M` : n >= 1024 ? `${Math.round(n / 1024)}K` : String(n));

/** How fast a window size runs: at the start, once full, and how long a
 * full one takes to read (when this PC's reading speed was measured). */
function speedOf(o: WindowOption): string {
  const parts = [placementLabel(o.placement!), t("about {n} words a second", { n: words(o.est_tps) })];
  if (o.full_tps > 0 && o.full_tps < o.est_tps * 0.9) parts.push(t("about {n} when the window is full", { n: words(o.full_tps) }));
  if (o.read_full_secs && o.read_full_secs >= 60) parts.push(t("a full window takes {time} to read", { time: duration(o.read_full_secs * 1000) }));
  return parts.join(" · ");
}

/** How much of the model's context window the chat uses. Clicking opens
 * the details: what fills it, Compact now, and the window size. */
export function ContextMeter({ info, fallbackCtx, modelId, canCompact, onCompact }: {
  info: ContextInfo | null;
  fallbackCtx: number | null;
  modelId: string | null;
  /** Something to compact and nothing running. */
  canCompact: boolean;
  onCompact: () => void;
}) {
  const [open, setOpen] = useState(false);
  // After a reply, the engine's own count (prompt + reply) is the accurate one.
  const used = info ? info.last_total ?? info.system_tokens + info.profile_tokens + info.history_tokens : 0;
  const ctx = info?.ctx ?? fallbackCtx ?? 0;
  const share = ctx > 0 ? used / ctx : 0;
  const level = share >= 0.9 ? "full" : share >= 0.7 ? "high" : "";
  const parts = info
    ? [
        { key: "sys", label: t("Instructions"), value: info.system_tokens },
        { key: "profile", label: t("Your profile"), value: info.profile_tokens },
        { key: "history", label: t("Conversation"), value: info.history_tokens },
        { key: "reserve", label: t("Room for the reply"), value: info.reply_reserve },
      ]
    : [];

  return (
    <>
      <button
        className={`ctx ${level}`}
        onClick={() => setOpen(true)}
        title={info ? t("Context window: {used} of {total} tokens used. Click for details.", { used: tokens(used), total: tokens(ctx) }) : t("Fills in after your first message")}
        aria-label={t("Context window: {used} of {total} tokens used", { used: tokens(used), total: tokens(ctx) })}
      >
        <div className="ctx-bar">
          {parts.map((p) => (
            <span key={p.key} className={`ctx-seg ${p.key}`} style={{ width: `${percent(p.value, info!.ctx)}%` }} />
          ))}
        </div>
        <span className={`ctx-label ${info && !info.estimated ? "" : "muted"}`}>
          {ctx > 0 ? `${info?.estimated ? "~" : ""}${tokens(used)} / ${tokens(ctx)}` : t("Context")}
          {info && info.dropped_messages > 0 && <span className="warn"> · {t("{n} left out", { n: info.dropped_messages })}</span>}
        </span>
      </button>
      {open && <ContextPanel info={info} used={used} ctx={ctx} parts={parts} modelId={modelId} canCompact={canCompact} onCompact={onCompact} onClose={() => setOpen(false)} />}
    </>
  );
}

function ContextPanel({ info, used, ctx, parts, modelId, canCompact, onCompact, onClose }: {
  info: ContextInfo | null;
  used: number;
  ctx: number;
  parts: { key: string; label: string; value: number }[];
  modelId: string | null;
  canCompact: boolean;
  onCompact: () => void;
  onClose: () => void;
}) {
  const [opts, setOpts] = useState<ContextOptions | null>(null);
  const [saved, setSaved] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    api.contextOptions(modelId).then(setOpts).catch((e) => setProblem(errorText(e)));
  }, [modelId]);
  const choose = async (size: number | null) => {
    try {
      await api.setContextWindow(size);
      setOpts((o) => (o ? { ...o, chosen: size } : o));
      setSaved(true);
    } catch (e) {
      setProblem(errorText(e));
    }
  };
  const pct = ctx > 0 ? Math.round((used / ctx) * 100) : 0;

  return (
    <Modal title={t("Context window")} onClose={onClose}>
      <p>
        <strong>{t("{used} of {total} tokens used ({pct}%)", { used: tokens(used), total: tokens(ctx), pct })}</strong>
      </p>
      <p className="muted small">{t("This is how much of the conversation the model can keep in mind at once. When it gets full, the chat compacts itself: the model summarizes the earlier part and keeps going from that summary.")}</p>
      {info?.estimated && <p className="muted small">{t("Worked out from the saved messages, so it's close but not exact: the model counts it at your next message.")}</p>}
      {info && (
        <ul className="ctx-legend small">
          {parts.map((p) => (
            <li key={p.key}>
              <span className={`ctx-dot ${p.key}`} /> {p.label}: {tokens(p.value)}
            </li>
          ))}
        </ul>
      )}
      {info && info.dropped_messages > 0 && <p className="warn-text small">{t("{n} older message(s) no longer fit and were left out. Compacting keeps what they said.", { n: info.dropped_messages })}</p>}
      <div className="row wrap">
        <button
          className="btn primary"
          disabled={!canCompact}
          onClick={() => {
            onClose();
            onCompact();
          }}
          title={canCompact ? undefined : t("Nothing to compact yet, or a reply is still running")}
        >
          {t("Compact now")}
        </button>
        <span className="muted small">{t("Or type /compact in the chat.")}</span>
      </div>

      <h3>{t("Window size")}</h3>
      <p className="muted small">{t("A bigger window keeps more of a long chat in mind, but needs more memory, so the model may run slower. Applies from your next message (the model reloads).")}</p>
      {problem && <p className="warn-text small">{problem}</p>}
      {opts?.advanced && <p className="warn-text small">{t("Advanced settings set this model's window to {size}, which wins over the sizes here. Change it in Settings, Advanced.", { size: short(opts.advanced) })}</p>}
      {opts && (
        <div className="ctx-sizes" role="radiogroup" aria-label={t("Window size")}>
          <label className="check">
            <input type="radio" name="ctx-size" checked={opts.chosen === null} disabled={!!opts.advanced} onChange={() => choose(null)} />
            <span>
              {t("Automatic ({size})", { size: short(opts.auto) })}
              <span className="muted small block">{t("What fits this PC comfortably")}</span>
            </span>
          </label>
          {opts.options.map((o) => (
            <label key={o.ctx} className={`check ${o.placement ? "" : "muted"}`}>
              <input type="radio" name="ctx-size" checked={opts.chosen === o.ctx} disabled={!o.placement || !!opts.advanced} onChange={() => choose(o.ctx)} />
              <span>
                {t("{size} tokens", { size: short(o.ctx) })}
                <span className="muted small block">
                  {o.placement ? speedOf(o) : t("Too much for this PC (needs about {needed})", { needed: bytes(o.needed) })}
                  {opts.tools && o.ctx < 16_384 && <span className="warn-text"> · {t("small for tasks with tools: a few steps fill it")}</span>}
                </span>
              </span>
            </label>
          ))}
          {opts.max > 0 && <p className="muted small">{t("This model supports up to {size} tokens.", { size: short(opts.max) })}</p>}
          <p className="muted small">{opts.measured ? t("Speeds are from this PC's speed test of this model.") : t("Speeds are estimates: test this model's speed in Models for this PC's own figures.")}</p>
        </div>
      )}
      {saved && <p className="small" role="status">{t("Saved. Your next message reloads the model with the new size.")}</p>}
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Close")}</button>
      </div>
    </Modal>
  );
}
