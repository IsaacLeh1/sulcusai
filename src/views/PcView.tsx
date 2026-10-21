// SPDX-License-Identifier: AGPL-3.0-only
// The phone's PC tab: how the PC is doing (temperatures, processor, memory,
// graphics card), what SulcusAI is working on there, and Stop all.
import { useEffect, useState } from "react";
import { api, errorText, type PcStatus } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import { phoneInfo } from "../phone/env";
import type { PushToast } from "../components/Toasts";

/** Under 70 °C is fine, to 83 warm, above that hot (most chips slow down near 85-95). */
function heat(temp: number | null): string {
  return temp == null ? "" : temp >= 83 ? "hot" : temp >= 70 ? "warm" : "cool";
}

const pct = (used: number | null, total: number | null) => (used != null && total ? (used / total) * 100 : null);

function Meter({ label, value, text }: { label: string; value: number | null; text: string }) {
  return (
    <div className="pc-meter">
      <div className="pc-meter-head">
        <span className="ellipsis">{label}</span>
        <span className="small muted">{text}</span>
      </div>
      <div className={`progress ${value != null && value >= 90 ? "warn" : ""}`} role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(value ?? 0)}>
        <span style={{ width: `${Math.min(100, Math.max(1, value ?? 0))}%` }} />
      </div>
    </div>
  );
}

function Temp({ label, value }: { label: string; value: number | null }) {
  return (
    <div className={`pc-temp ${heat(value)}`}>
      <span className="pc-temp-value">{value != null ? `${Math.round(value)} °C` : "—"}</span>
      <span className="small muted">{label}</span>
    </div>
  );
}

function kindName(kind: PcStatus["jobs"][number]["kind"]): string {
  return { chat: t("Chat"), media: t("Studio"), install: t("Download"), training: t("Teaching") }[kind];
}

export function PcView({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<PcStatus | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const info = phoneInfo();

  useEffect(() => {
    let alive = true;
    let timer = 0;
    const load = async () => {
      try {
        const v = await api.pcStatus();
        if (alive) {
          setS(v);
          setProblem(null);
        }
      } catch (e) {
        if (alive) setProblem(errorText(e));
      }
      // Every 3 seconds while it's shown; seldom with the phone's screen off.
      if (alive) timer = window.setTimeout(load, document.hidden ? 15000 : 3000);
    };
    load();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, []);

  const stopAll = async () => {
    if (!window.confirm(t("Stop every reply, picture, video, song and teaching job on the PC, and unload the model? Downloads keep going."))) return;
    setStopping(true);
    try {
      const n = await api.stopAll();
      toast(n > 0 ? t("Stopped {n} and freed the graphics card.", { n }) : t("Nothing was running. The model was unloaded."), "success");
      setS(await api.pcStatus());
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setStopping(false);
    }
  };

  return (
    <div className="page pc-page">
      <header className="page-head">
        <div>
          <h1>{s?.pc || info?.pc || t("Your PC")}</h1>
          <p className="muted small">{info?.relay ? t("Connected through SulcusAI's relay.") : t("Connected on this network.")}</p>
        </div>
      </header>
      {problem && <p className="warn-text">{problem}</p>}
      {!s ? (
        !problem && <p className="muted">{t("Loading…")}</p>
      ) : (
        <>
          <section className="card">
            <h2>{t("Temperatures")}</h2>
            <div className="pc-temps">
              <Temp label={t("Processor")} value={s.cpu_temp} />
              <Temp label={t("Graphics card")} value={s.gpu_temp} />
            </div>
            <p className="muted small">
              {s.cooling > 0 ? t("It's warm, so SulcusAI is going easier to cool down (level {n} of 3).", { n: s.cooling }) : t("Cool enough: SulcusAI isn't holding back.")}
            </p>
            {s.gpu_temp == null && <p className="muted small">{t("The graphics card's temperature shows while SulcusAI is working (reading it wakes the card).")}</p>}
          </section>
          <section className="card">
            <h2>{t("Use")}</h2>
            <Meter label={t("Processor")} value={s.cpu} text={t("{n}% busy", { n: Math.round(s.cpu) })} />
            <Meter label={t("Memory (RAM)")} value={pct(s.ram_used, s.ram_total)} text={t("{used} of {total}", { used: bytes(s.ram_used), total: bytes(s.ram_total) })} />
            <Meter label={s.gpu.name ?? t("Graphics card")} value={s.gpu.usage} text={s.gpu.usage != null ? t("{n}% busy", { n: Math.round(s.gpu.usage) }) : t("not read while idle")} />
            {s.gpu.mem_total ? (
              <Meter
                label={t("Graphics card memory")}
                value={pct(s.gpu.mem_used, s.gpu.mem_total)}
                text={s.gpu.mem_used != null ? t("{used} of {total}", { used: bytes(s.gpu.mem_used), total: bytes(s.gpu.mem_total) }) : t("not read while idle")}
              />
            ) : null}
            <p className="small">{s.model ? t("{name} is loaded.", { name: s.model }) : t("No chat model is loaded.")}</p>
          </section>
          <section className="card">
            <h2>{t("Working on")}</h2>
            {s.jobs.length === 0 ? (
              <p className="muted small">{t("Nothing right now.")}</p>
            ) : (
              <ul className="pc-jobs">
                {s.jobs.map((j, i) => (
                  <li key={i}>
                    <strong className="ellipsis">{j.title || kindName(j.kind)}</strong>
                    <span className="muted small">
                      {kindName(j.kind)} · {j.detail}
                      {j.fraction != null ? ` · ${Math.round(j.fraction * 100)}%` : ""}
                    </span>
                    {j.fraction != null && (
                      <div className="progress">
                        <span style={{ width: `${Math.round(j.fraction * 100)}%` }} />
                      </div>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </section>
          <button className="btn danger-fill block pc-stop" disabled={stopping} onClick={stopAll}>
            {stopping ? t("Stopping…") : t("Stop all")}
          </button>
          <p className="muted small">{t("Stops replies, pictures, videos, music and teaching, then unloads the model to free the graphics card. Downloads keep going.")}</p>
        </>
      )}
    </div>
  );
}
