// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type LineupEntry, type StartNow } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const since = (ms: number | null) => {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? t("{n} s", { n: s }) : t("{n} min", { n: Math.round(s / 60) });
};

function where(e: LineupEntry): string {
  if (e.card === "main") return t("on the graphics card");
  if (e.card === "second") return t("on the second graphics card");
  return t("on the processor");
}

/** The bottom bar's line for the graphics cards: chat replies and Studio
 * jobs take turns on the card rather than slowing down on the processor.
 * Opened, it lists what's running and what waits; what waits can be moved,
 * started now (on the processor, or by stopping what's in its way) or taken
 * out of the line. */
export function LineupBar({ toast }: { toast: PushToast }) {
  const [line, setLine] = useState<LineupEntry[]>([]);
  const [open, setOpen] = useState(false);
  const [asking, setAsking] = useState<{ e: LineupEntry; opts: StartNow } | null>(null);
  const [, tick] = useState(0);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.lineupList().then((l) => setLine(l ?? [])).catch(() => {});
    const sub = on("lineup:changed", (l) => setLine(l ?? []));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  // The times shown while it's open.
  useEffect(() => {
    if (!open) return;
    const timer = setInterval(() => tick((n) => n + 1), 5000);
    const away = (ev: MouseEvent) => {
      if (box.current && !box.current.contains(ev.target as Node) && !asking) setOpen(false);
    };
    document.addEventListener("mousedown", away);
    return () => {
      clearInterval(timer);
      document.removeEventListener("mousedown", away);
    };
  }, [open, asking]);

  const running = line.filter((e) => e.running && !e.inside);
  const waiting = line.filter((e) => !e.running);
  const label = waiting.length === 0 ? (running.length ? t("{n} running · queue empty", { n: running.length }) : t("Queue empty")) : t("{n} in queue", { n: waiting.length });

  const act = async (f: () => Promise<unknown>) => {
    try {
      await f();
    } catch (err) {
      toast(errorText(err), "error");
    }
  };
  const startNow = async (e: LineupEntry) => {
    try {
      setAsking({ e, opts: await api.lineupStartNowOptions(e.id) });
      setOpen(false);
    } catch (err) {
      toast(errorText(err), "error");
    }
  };

  return (
    <div className="lineup" ref={box}>
      <button className={`status-item link-btn lineup-pill ${waiting.length ? "busy" : ""}`} onClick={() => setOpen((o) => !o)} aria-expanded={open} title={t("Chats and Studio jobs take turns on the graphics card")}>
        {label}
      </button>
      {open && (
        <div className="lineup-panel" role="dialog" aria-label={t("The line for the graphics card")}>
          <div className="row">
            <strong>{t("The line for the graphics card")}</strong>
            <span className="spacer" />
            <button className="btn ghost small" onClick={() => setOpen(false)} aria-label={t("Close")}>✕</button>
          </div>
          <p className="small muted">{t("Replies and Studio jobs take turns, so each runs on the graphics card instead of slowing down on the processor. Replies from the same model share it, and small jobs run side by side when the card has room.")}</p>
          {line.length === 0 && <p className="small muted">{t("Nothing is running or waiting.")}</p>}
          {running.length > 0 && <h4 className="small">{t("Running")}</h4>}
          {running.map((e) => (
            <div key={e.id} className="lineup-row running">
              <span className="led busy" />
              <div className="grow">
                <strong className="ellipsis">{e.title}</strong>
                <span className="small muted block">
                  {t(e.what)} · {where(e)} · {since(e.started_at)}
                  {line.filter((x) => x.inside === e.id).map((x) => ` · ${t(x.what)}`).join("")}
                </span>
              </div>
            </div>
          ))}
          {waiting.length > 0 && <h4 className="small">{t("Waiting")}</h4>}
          {waiting.map((e, i) => (
            <div key={e.id} className="lineup-row">
              <span className="lineup-place">{i + 1}</span>
              <div className="grow">
                <strong className="ellipsis">{e.title}</strong>
                <span className="small muted block">
                  {t(e.what)} · {t("waiting {time}", { time: since(e.queued_at) })}
                </span>
              </div>
              <div className="seg small">
                <button disabled={i === 0} onClick={() => act(() => api.lineupMove(e.id, i - 1))} aria-label={t("Move up")} title={t("Move up")}>↑</button>
                <button disabled={i === waiting.length - 1} onClick={() => act(() => api.lineupMove(e.id, i + 1))} aria-label={t("Move down")} title={t("Move down")}>↓</button>
              </div>
              <button className="btn small" onClick={() => startNow(e)}>{t("Start now")}</button>
              <button className="btn ghost small" onClick={() => act(() => api.lineupRemove(e.id))}>{t("Remove")}</button>
            </div>
          ))}
        </div>
      )}
      {asking && (
        <Modal title={t("Start “{title}” now?", { title: asking.e.title })} onClose={() => setAsking(null)}>
          {asking.opts.in_the_way.length > 0 ? (
            <p>
              {t("The graphics card is busy with: {list}.", { list: asking.opts.in_the_way.map((x) => `${x.title} (${t(x.what)})`).join(", ") })}
            </p>
          ) : (
            <p>{t("It's next in line for the graphics card.")}</p>
          )}
          <ul className="small lineup-choices">
            {asking.opts.processor && <li>{t("Run it now on the processor and system memory: slower (often several times) and warmer, while the card keeps going.")}</li>}
            {asking.opts.in_the_way.length > 0 && <li>{t("Or stop what's on the card first: this starts on the card next.")}</li>}
            <li>{t("Or wait: it starts by itself when its turn comes.")}</li>
          </ul>
          <div className="modal-actions wrap">
            <button className="btn ghost" onClick={() => setAsking(null)}>{t("Wait")}</button>
            <span className="spacer" />
            {asking.opts.in_the_way.length > 0 && (
              <button className="btn danger" onClick={() => act(() => api.lineupStartNow(asking.e.id, "stop")).then(() => setAsking(null))}>
                {t("Stop that and start this")}
              </button>
            )}
            {asking.opts.processor && (
              <button className="btn primary" onClick={() => act(() => api.lineupStartNow(asking.e.id, "processor")).then(() => setAsking(null))}>
                {t("Run on the processor")}
              </button>
            )}
          </div>
        </Modal>
      )}
    </div>
  );
}
