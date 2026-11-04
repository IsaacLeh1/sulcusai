// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import { t } from "../i18n";
import { remoteAccept, remoteStatus, remoteStop, secondsLeft, type RemoteStatus } from "../api/sharing";

/** A short tone so a request, a start and an end are heard as well as seen. */
function tone(freq: number) {
  try {
    const ctx = new AudioContext();
    const o = ctx.createOscillator();
    const g = ctx.createGain();
    o.frequency.value = freq;
    g.gain.setValueAtTime(0.15, ctx.currentTime);
    g.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.35);
    o.connect(g).connect(ctx.destination);
    o.start();
    o.stop(ctx.currentTime + 0.4);
    o.onended = () => ctx.close();
  } catch {
    // No audio device: the prompt and border still show.
  }
}

/** Remote control on this PC: the 10-second Accept prompt, then a border and a bar with Stop while it runs. */
export function RemoteOverlay() {
  const [ask, setAsk] = useState<{ peer: string; code: string; at: number } | null>(null);
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [now, setNow] = useState(Date.now());
  const [err, setErr] = useState("");
  const live = useRef(false);

  useEffect(() => {
    const un = [
      listen<{ peer: string; code: string }>("remote:asked", (e) => {
        setAsk({ ...e.payload, at: Date.now() });
        setErr("");
        tone(660);
      }),
      listen("remote:changed", () => remoteStatus().then(setStatus).catch(() => {})),
    ];
    return () => un.forEach((p) => p.then((f) => f()));
  }, []);

  useEffect(() => {
    if (!ask && !status?.show.bar) return;
    const id = setInterval(() => {
      setNow(Date.now());
      remoteStatus().then(setStatus).catch(() => {});
    }, 500);
    return () => clearInterval(id);
  }, [ask, status?.show.bar]);

  const isLive = !!status?.show.bar;
  useEffect(() => {
    if (isLive !== live.current) tone(isLive ? 880 : 440);
    live.current = isLive;
  }, [isLive]);

  const left = ask ? secondsLeft(ask.at, now) : 0;
  useEffect(() => {
    if (ask && left === 0) setAsk(null);
  }, [ask, left]);

  return (
    <>
      {ask && !isLive && (
        <div className="remote-ask" role="alertdialog" aria-label={t("Remote control request")}>
          <strong>{t("A friend asks to control this PC")}</strong>
          <p className="small">{t("Accept only if they read you this same code:")}</p>
          <p className="remote-code">{ask.code}</p>
          <p className="small muted">{t("{n} seconds left", { n: String(left) })}</p>
          {err && <p className="small">{err}</p>}
          <div className="row">
            <button
              onClick={() =>
                remoteAccept(ask.code)
                  .then((s) => {
                    setStatus(s);
                    setAsk(null);
                  })
                  .catch((e) => setErr(errorText(e)))
              }
            >
              {t("Accept")}
            </button>
            <button
              className="secondary"
              onClick={() => {
                remoteStop().then(setStatus).catch(() => {});
                setAsk(null);
              }}
            >
              {t("Decline")}
            </button>
          </div>
        </div>
      )}
      {isLive && (
        <>
          <div className="remote-border" aria-hidden="true" />
          <div className="remote-bar" role="status">
            <span>{t("This PC is being controlled remotely. Ctrl+Alt+Backspace stops it.")}</span>
            <button onClick={() => remoteStop().then(setStatus).catch(() => {})}>{t("Stop")}</button>
          </div>
        </>
      )}
    </>
  );
}
