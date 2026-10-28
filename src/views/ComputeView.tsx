// SPDX-License-Identifier: AGPL-3.0-only
// The compute PC: how the paired PC doing the AI work is doing (its
// temperatures, use, memory, drives, how long it's been up, what it's
// working on) and its controls: Stop all, pause and resume, unload the
// model, its performance mode.
import { useEffect, useState } from "react";
import { api, errorText, type ComputeStatus } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import { kindName, Meter, pct, Temp } from "./PcView";
import type { PushToast } from "../components/Toasts";
import { confirmed } from "../confirm";

function since(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return d > 0 ? t("{d} d {h} h", { d, h }) : h > 0 ? t("{h} h {m} min", { h, m }) : t("{m} min", { m });
}

export function ComputeView({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<ComputeStatus | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    let timer = 0;
    const load = async () => {
      try {
        const v = await api.computeStatus();
        if (alive) {
          setS(v);
          setProblem(null);
        }
      } catch (e) {
        if (alive) setProblem(errorText(e));
      }
      if (alive) timer = window.setTimeout(load, document.hidden ? 15000 : 3000);
    };
    load();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, []);

  const control = async (action: string, value?: string, confirm?: string) => {
    if (confirm && !await confirmed(confirm)) return;
    setBusy(action);
    try {
      toast(await api.computeControl(action, value ?? null), "success");
      setS(await api.computeStatus());
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const pc = s?.pc;
  return (
    <div className="page pc-page compute">
      <header className="page-head">
        <div>
          <h1>{s ? t("Compute PC: {name}", { name: s.name }) : t("Compute PC")}</h1>
          {s && (
            <p className="muted small">
              {t("SulcusAI {v} · on for {up} · SulcusAI running for {app}", { v: s.version, up: since(s.uptime_s), app: since(s.app_uptime_s) })}
            </p>
          )}
        </div>
        {s && (
          <div className="row wrap">
            {s.paused ? (
              <button className="btn primary" disabled={!!busy} onClick={() => control("resume")}>{t("Resume")}</button>
            ) : (
              <button className="btn" disabled={!!busy} onClick={() => control("pause")} title={t("New replies and Studio jobs wait until it's resumed; what's running finishes.")}>{t("Pause")}</button>
            )}
            <button className="btn danger" disabled={!!busy} onClick={() => control("stop_all", undefined, t("Stop every reply, picture, video, song and teaching job on the compute PC, and unload its model? Downloads keep going."))}>
              {t("Stop all")}
            </button>
          </div>
        )}
      </header>
      {problem && <p className="warn-text">{problem}</p>}
      {s?.paused && <p className="callout small">{t("Paused: new work waits until you resume it.")}</p>}
      {!s || !pc ? (
        !problem && <p className="muted">{t("Loading…")}</p>
      ) : (
        <>
          <section className="card">
            <h2>{t("Temperatures")}</h2>
            <div className="pc-temps">
              <Temp label={t("Processor")} value={pc.cpu_temp} />
              <Temp label={t("Graphics card")} value={pc.gpu_temp} />
            </div>
            <p className="muted small">
              {pc.cooling > 0 ? t("It's warm, so SulcusAI is going easier to cool down (level {n} of 3).", { n: pc.cooling }) : t("Cool enough: SulcusAI isn't holding back.")}
            </p>
          </section>
          <section className="card">
            <h2>{t("Use")}</h2>
            <Meter label={t("Processor")} value={pc.cpu} text={t("{n}% busy", { n: Math.round(pc.cpu) })} />
            <Meter label={t("Memory (RAM)")} value={pct(pc.ram_used, pc.ram_total)} text={t("{used} of {total}", { used: bytes(pc.ram_used), total: bytes(pc.ram_total) })} />
            <Meter label={pc.gpu.name ?? t("Graphics card")} value={pc.gpu.usage} text={pc.gpu.usage != null ? t("{n}% busy", { n: Math.round(pc.gpu.usage) }) : t("not read while idle")} />
            {pc.gpu.mem_total ? (
              <Meter label={t("Graphics card memory")} value={pct(pc.gpu.mem_used, pc.gpu.mem_total)} text={pc.gpu.mem_used != null ? t("{used} of {total}", { used: bytes(pc.gpu.mem_used), total: bytes(pc.gpu.mem_total) }) : t("not read while idle")} />
            ) : null}
            <div className="row wrap">
              <span className="small">{pc.model ? t("{name} is loaded.", { name: pc.model }) : t("No chat model is loaded.")}</span>
              <span className="spacer" />
              {pc.model && <button className="btn ghost small" disabled={!!busy} onClick={() => control("unload")}>{t("Unload it")}</button>}
              <label className="small row">
                {t("Performance")}
                <select value={s.perf} disabled={!!busy} onChange={(e) => control("perf", e.target.value)}>
                  <option value="cool">{t("Cool & quiet")}</option>
                  <option value="balanced">{t("Balanced")}</option>
                  <option value="turbo">{t("Turbo")}</option>
                </select>
              </label>
            </div>
          </section>
          <section className="card">
            <h2>{t("Drives")}</h2>
            {s.drives.map((d) => (
              <Meter
                key={d.mount}
                label={`${d.mount}${d.name ? ` (${d.name})` : ""}${d.models ? ` · ${t("models are here")}` : ""}`}
                value={pct(d.total - d.free, d.total)}
                text={t("{free} free of {total}", { free: bytes(d.free), total: bytes(d.total) })}
              />
            ))}
          </section>
          <section className="card">
            <h2>{t("Working on")}</h2>
            {pc.jobs.length === 0 ? (
              <p className="muted small">{t("Nothing right now.")}</p>
            ) : (
              <ul className="pc-jobs">
                {pc.jobs.map((j, i) => (
                  <li key={i}>
                    <strong className="ellipsis">{j.title || kindName(j.kind)}</strong>
                    <span className="muted small">
                      {kindName(j.kind)} · {j.detail}
                      {j.fraction != null ? ` · ${Math.round(j.fraction * 100)}%` : ""}
                    </span>
                    {j.fraction != null && (
                      <div className="progress"><span style={{ width: `${Math.round(j.fraction * 100)}%` }} /></div>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </section>
          <section className="card">
            <h2>{t("What it has")}</h2>
            <p className="small">{s.models.length ? t("Chat models: {list}", { list: s.models.map((m) => m.name).join(", ") }) : t("No chat models installed there.")}</p>
            <p className="small">{s.media.length ? t("Studio models: {list}", { list: s.media.join(", ") }) : t("No Studio models installed there.")}</p>
            <p className="small muted">{t("Pick its models in any chat's model menu, under “On {name}”. This page checks on it every few seconds.", { name: s.name })}</p>
          </section>
        </>
      )}
    </div>
  );
}
