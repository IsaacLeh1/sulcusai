// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText, type FoundModels, type CatalogView, type InstallProgress, type ModelCard, type Settings } from "../api";
import { bytes, percent, placementLabel, speedLabel } from "../format";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { SpeechModels } from "./SpeechModels";
import { MediaModels } from "../components/MediaModels";
import { AREA_MIN, FILTERS, arrange, speedOf, type Area, type Filter } from "../ranking";
import { t, tx } from "../i18n";

interface Props {
  catalog: CatalogView | null;
  progress: Record<string, InstallProgress>;
  defaultModel: string | null;
  onRefresh: () => Promise<void>;
  onSettings: (s: Settings) => void;
  toast: PushToast;
}

export function ModelsView({ catalog, progress, defaultModel, onRefresh, onSettings, toast }: Props) {
  const [checking, setChecking] = useState(false);
  const [tab, setTab] = useState<"chat" | "speech" | "create">("chat");
  const [filter, setFilter] = useState<Filter>("all");
  const [mostFirst, setMostFirst] = useState(false);
  if (!catalog) return <div className="page"><p className="muted">{t("Checking this PC…")}</p></div>;

  const { hardware: hw, hints } = catalog;
  const gpu = hw.gpus.filter((g) => !g.integrated).sort((a, b) => b.vram_bytes - a.vram_bytes)[0];
  // Rank among the models this PC can run, least capable = 1.
  // Models from other apps have no ratings, so they aren't ranked.
  const byCapability = catalog.models.filter((m) => !m.local).sort((a, b) => a.ratings.overall - b.ratings.overall);
  const rankOf = (m: ModelCard) => byCapability.indexOf(m) + 1;
  const arranged = arrange(catalog.models, filter, mostFirst);
  const best = filter !== "all" ? arranged.reduce<ModelCard | null>((top, m) => (!top || (filter === "fastest" ? speedOf(m) > speedOf(top) : m.ratings[filter] > top.ratings[filter]) ? m : top), null) : null;
  const installed = arranged.filter((m) => m.installed);
  const available = arranged.filter((m) => !m.installed);
  const cardRank = { rankOf, total: byCapability.length, filter, best: best?.id ?? null };
  const busy = Object.keys(progress).length > 0;

  const recheck = async () => {
    setChecking(true);
    try {
      await api.refreshHardware();
      await onRefresh();
    } finally {
      setChecking(false);
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>{t("Models")}</h1>
          <p className="muted">{t("Only models this PC can run are shown. Click Install and they're ready in a few minutes.")}</p>
        </div>
        <div className="mode-switch" role="tablist" aria-label={t("Kind of model")}>
          <button role="tab" aria-selected={tab === "chat"} className={`mode ${tab === "chat" ? "active" : ""}`} onClick={() => setTab("chat")}>{t("Chat")}</button>
          <button role="tab" aria-selected={tab === "speech"} className={`mode ${tab === "speech" ? "active" : ""}`} onClick={() => setTab("speech")}>{t("Speech")}</button>
          <button role="tab" aria-selected={tab === "create"} className={`mode ${tab === "create" ? "active" : ""}`} onClick={() => setTab("create")}>{t("Pictures, video, music")}</button>
        </div>
      </header>

      <section className="card hw">
        <div className="hw-grid">
          <Spec label={t("Graphics card")} value={gpu ? `${gpu.name}` : t("None usable")} sub={gpu ? t("{size} video memory", { size: bytes(gpu.vram_bytes) }) : t("Models run on the processor")} />
          <Spec label={t("Memory")} value={bytes(hw.ram_total)} sub={t("{size} free now", { size: bytes(hw.ram_available) })} />
          <Spec label={t("Processor")} value={hw.cpu_name || t("Unknown")} sub={t("{cores} cores · {threads} threads", { cores: hw.cpu_cores ?? "?", threads: hw.cpu_threads })} />
          <Spec label={t("Disk")} value={t("{size} free", { size: bytes(hw.disk_free) })} sub={hw.models_drive ? t("on {drive}", { drive: hw.models_drive }) : t("on the models drive")} />
        </div>
        <div className="hw-foot">
          <span className="muted small">
            {t("Engine: llama.cpp ({engine})", { engine: catalog.backend === "vulkan-x64" ? t("graphics card") : catalog.backend === "cpu-x64" ? t("processor") : t("unavailable on this system") })}
            {hw.on_battery ? ` · ${t("On battery: models run slower and drain power faster.")}` : ""}
          </span>
          <button className="btn ghost small" onClick={recheck} disabled={checking}>
            {checking ? t("Checking…") : t("Check again")}
          </button>
        </div>
      </section>

      {tab === "speech" && <SpeechModels progress={progress} toast={toast} />}
      {tab === "create" && <MediaModels progress={progress} toast={toast} />}

      {tab === "chat" && (
        <div className="model-filters">
          <div className="chips" role="tablist" aria-label={t("Show models")}>
            {FILTERS.map((f) => (
              <button key={f.id} role="tab" aria-selected={filter === f.id} className={`chip ${filter === f.id ? "active" : ""}`} onClick={() => setFilter(f.id)} title={t(f.hint)}>
                {t(f.label)}
              </button>
            ))}
          </div>
          {filter !== "fastest" && (
            <div className="mode-switch small-switch" role="radiogroup" aria-label={t("Order")}>
              <button role="radio" aria-checked={!mostFirst} className={`mode ${!mostFirst ? "active" : ""}`} onClick={() => setMostFirst(false)}>{t("Least → most capable")}</button>
              <button role="radio" aria-checked={mostFirst} className={`mode ${mostFirst ? "active" : ""}`} onClick={() => setMostFirst(true)}>{t("Most → least")}</button>
            </div>
          )}
          <p className="muted small">
            {t(FILTERS.find((f) => f.id === filter)!.hint)}{" "}
            {filter === "fastest" ? t("Measured once installed; estimated before.") : t("Scores are rough guides from public benchmarks; bigger models also need more of the PC.")}
          </p>
          {arranged.length === 0 && <p className="muted">{t("None of the models this PC can run stand out here yet. Try All.")}</p>}
        </div>
      )}

      {tab === "chat" && <LookForModels onRefresh={onRefresh} toast={toast} />}

      {tab === "chat" && installed.length > 0 && (
        <>
          <h2>{t("Installed")}</h2>
          <div className="model-grid">
            {installed.map((m) => (
              <InstalledCard key={m.id} m={m} rank={cardRank} isDefault={m.id === defaultModel} onRefresh={onRefresh} onSettings={onSettings} toast={toast} />
            ))}
          </div>
        </>
      )}

      {tab === "chat" && (
        <>
          <h2>{t("Available for this PC")}</h2>
          {available.length === 0 && filter === "all" && <p className="muted">{t("Every model that fits this PC is installed.")}</p>}
          <div className="model-grid">
            {available.map((m) => (
              <AvailableCard key={m.id} m={m} rank={cardRank} progress={progress[m.id]} otherBusy={busy && !progress[m.id]} toast={toast} />
            ))}
          </div>
        </>
      )}

      {tab === "chat" && hints.hidden > 0 && (
        <section className="card hint">
          <strong>
            {hints.hidden === 1 ? t("1 model is hidden because this PC can't run it.") : t("{n} models are hidden because this PC can't run them.", { n: hints.hidden })}
          </strong>
          <ul>
            {hints.ram.map((r) => (
              <li key={r.extra_ram_gb}>
                {r.unlocks === 1
                  ? t("Adding {gb} GB of memory would unlock 1 more model.", { gb: r.extra_ram_gb })
                  : t("Adding {gb} GB of memory would unlock {n} more models.", { gb: r.extra_ram_gb, n: r.unlocks })}
              </li>
            ))}
            {hints.disk_blocked > 0 && (
              <li>
                {hints.disk_blocked === 1
                  ? t("Freeing up {size} of disk space would unlock 1 more model.", { size: bytes(hints.disk_needed_bytes) })
                  : t("Freeing up {size} of disk space would unlock {n} more models.", { size: bytes(hints.disk_needed_bytes), n: hints.disk_blocked })}
              </li>
            )}
            {hints.ram.length === 0 && hints.disk_blocked === 0 && <li>{t("They need a graphics card with more video memory.")}</li>}
          </ul>
        </section>
      )}
    </div>
  );
}

/** Finds models other apps (Ollama, LM Studio and others) already downloaded. */
export function LookForModels({ onRefresh, toast, compact }: { onRefresh: () => Promise<void>; toast: PushToast; compact?: boolean }) {
  const [looking, setLooking] = useState(false);
  const [result, setResult] = useState<FoundModels | null>(null);
  const look = async () => {
    setLooking(true);
    try {
      const r = await api.findModels(true);
      setResult(r);
      await onRefresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setLooking(false);
    }
  };
  return (
    <section className={compact ? "look-models compact" : "card look-models"}>
      <div className="look-row">
        <div>
          {!compact && <strong>{t("Models from other apps")}</strong>}
          <p className="muted small">{t("Models you downloaded with Ollama, LM Studio, Jan, GPT4All or Hugging Face can be used here without downloading them again.")}</p>
        </div>
        <button className="btn" onClick={look} disabled={looking}>
          {looking ? t("Looking…") : t("Look for models on this PC")}
        </button>
      </div>
      {result && (
        <div className="small look-result" role="status">
          {result.added.length > 0 ? (
            <p>{t("Added {names}.", { names: result.added.join(", ") })}</p>
          ) : (
            <p>
              {t("Nothing new to add.")}{" "}
              {result.looked_in.length > 0 ? t("Looked in {places}.", { places: result.looked_in.join(", ") }) : t("None of those apps' model folders are on this PC.")}
            </p>
          )}
          {result.skipped.length > 0 && (
            <ul className="muted">
              {result.skipped.map((s) => (
                <li key={s.name}>{s.name} {s.reason}.</li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

function Spec({ label, value, sub }: { label: string; value: string; sub: string }) {
  return (
    <div className="spec">
      <span className="spec-label">{label}</span>
      <span className="spec-value ellipsis" title={value}>{value}</span>
      <span className="muted small">{sub}</span>
    </div>
  );
}

interface RankInfo {
  rankOf: (m: ModelCard) => number;
  total: number;
  filter: Filter;
  best: string | null;
}

const AREA_LABEL: Record<Area, string> = { coding: tx("Coding"), writing: tx("Writing"), research: tx("Research"), agents: tx("Agents"), languages: tx("Languages") };

/** Where the model sits from least to most capable, and its strengths. */
function Capability({ m, rank }: { m: ModelCard; rank: RankInfo }) {
  const r = m.ratings;
  const strengths = (Object.keys(AREA_LABEL) as Area[]).filter((a) => r[a] >= AREA_MIN).sort((a, b) => r[b] - r[a]);
  return (
    <div className="capability">
      <div className="cap-head small">
        <span>
          {t("Capability")} <strong>{rank.rankOf(m)}</strong> {t("of {total}", { total: rank.total })}
          <span className="muted"> {t("(1 = lightest)")}</span>
        </span>
        {rank.best === m.id && rank.filter !== "all" && <span className="badge">{rank.filter === "fastest" ? t("Fastest here") : t("Best for {area}", { area: t(AREA_LABEL[rank.filter as Area]).toLowerCase() })}</span>}
      </div>
      <div className="cap-bar" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={r.overall} aria-label={t("Overall capability")}>
        <span style={{ width: `${r.overall}%` }} />
      </div>
      {strengths.length > 0 && <span className="muted small">{t("Strong at: {areas}", { areas: strengths.map((a) => t(AREA_LABEL[a]).toLowerCase()).join(", ") })}</span>}
    </div>
  );
}

function Tags({ m }: { m: ModelCard }) {
  return (
    <div className="tags">
      {m.tags.filter((tag) => tag !== "recommended").map((tag) => (
        <span key={tag} className="tag">{tag}</span>
      ))}
      <span className="tag license" title={`${m.license.note ?? m.license.name}\n${m.license.url}`}>
        {m.license.name}
      </span>
      {!m.license.commercial && <span className="tag warn">{t("Non-commercial")}</span>}
    </div>
  );
}

function AvailableCard({ m, rank, progress, otherBusy, toast }: { m: ModelCard; rank: RankInfo; progress?: InstallProgress; otherBusy: boolean; toast: PushToast }) {
  const options = m.fit.variants.filter((v) => v.placement && v.disk_ok);
  const onDisk = options.find((v) => v.quant === m.on_disk)?.quant;
  const [quant, setQuant] = useState(onDisk ?? m.fit.recommended ?? options[0]?.quant ?? "");
  const fit = m.fit.variants.find((v) => v.quant === quant);
  const variant = m.variants.find((v) => v.quant === quant);

  const install = async () => {
    try {
      await api.install(m.id, quant);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <article className="card model">
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">{m.publisher} · {t("{n}B parameters", { n: m.params_b })}</span>
        </div>
        {onDisk ? <span className="badge">{t("Already on this PC")}</span> : m.tags.includes("recommended") && <span className="badge">{t("Recommended")}</span>}
      </div>
      <p className="desc">{m.description}</p>
      <Capability m={m} rank={rank} />
      <Tags m={m} />
      {progress ? (
        <InstallBar m={m} p={progress} />
      ) : (
        <div className="install-row">
          <select value={quant} onChange={(e) => setQuant(e.target.value)} aria-label={t("Quality for {name}", { name: m.name })} title={t("Quality and download size. ★ is the best fit for this PC.")}>

            {options.map((v) => {
              const spec = m.variants.find((x) => x.quant === v.quant)!;
              return (
                <option key={v.quant} value={v.quant}>
                  {spec.quality} · {bytes(spec.size)}{v.quant === m.fit.recommended ? " ★" : ""}
                </option>
              );
            })}
          </select>
          <button className="btn primary" onClick={install} disabled={otherBusy || !quant} title={otherBusy ? t("One install at a time") : undefined}>
            {quant === onDisk ? t("Set up (no download)") : t("Install")}
          </button>
        </div>
      )}
      {fit && variant && !progress && (
        <p className="muted small fit-line">
          {placementLabel(fit.placement)} · {speedLabel(fit.est_tps)} {t("(about {n} tokens/sec, estimated)", { n: Math.round(fit.est_tps) })}
        </p>
      )}
    </article>
  );
}

function InstallBar({ m, p }: { m: ModelCard; p: InstallProgress }) {
  const label = {
    engine: t("Setting up the engine"),
    verify: t("Checking the downloaded file"),
    download: t("Downloading"),
    benchmark: t("Testing speed on this PC"),
    vision: t("Downloading the picture reader"),
  }[p.phase];
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${t("{received} of {total}", { received: bytes(p.received), total: bytes(p.total) })}` : "…"}</span>
        {p.phase !== "benchmark" && (
          <button className="link" onClick={() => api.cancelInstall(m.id)}>
            {t("Cancel")}
          </button>
        )}
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
    </div>
  );
}

function InstalledCard({ m, rank, isDefault, onRefresh, onSettings, toast }: { m: ModelCard; rank: RankInfo; isDefault: boolean; onRefresh: () => Promise<void>; onSettings: (s: Settings) => void; toast: PushToast }) {
  const [confirm, setConfirm] = useState(false);
  const inst = m.installed!;
  const variant = m.variants.find((v) => v.quant === inst.quant);
  const fit = m.fit.variants.find((v) => v.quant === inst.quant);

  return (
    <article className="card model installed">
      <div className="model-head">
        <div>
          <h3>{m.name}</h3>
          <span className="muted small">
            {m.local ? `${t("From {app}", { app: m.local.app })} · ` : ""}
            {t("{quality} quality", { quality: variant?.quality ?? inst.quant })} · {bytes(inst.size)}
          </span>
        </div>
        {isDefault ? <span className="badge">{t("Default")}</span> : null}
      </div>
      <p className="desc">{m.description}</p>
      {m.local ? <p className="muted small ellipsis" title={m.local.path}>{t("{n}B parameters", { n: m.params_b })}{m.tools ? ` · ${t("can use tools")}` : ""}</p> : <Capability m={m} rank={rank} />}
      {!m.local && <Tags m={m} />}
      <p className="small fit-line">
        {inst.tps !== null
          ? t("Measured on this PC: {tps} tokens/sec ({speed})", { tps: inst.tps, speed: speedLabel(inst.tps).toLowerCase() })
          : t("Speed not measured yet")}
        {fit && <span className="muted"> · {placementLabel(fit.placement)}</span>}
      </p>
      <div className="install-row">
        {!isDefault && (
          <button
            className="btn"
            onClick={async () => {
              try {
                onSettings(await api.setDefaultModel(m.id));
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            {t("Make default")}
          </button>
        )}
        <span className="spacer" />
        <button className="btn ghost danger" onClick={() => setConfirm(true)}>
          {m.local ? t("Stop using") : t("Remove")}
        </button>
      </div>
      {confirm && (
        <Modal title={m.local ? t("Stop using {name}?", { name: m.name }) : t("Remove {name}?", { name: m.name })} onClose={() => setConfirm(false)}>
          {m.local ? (
            <p>{t("SulcusAI stops listing it. The file stays in {app}, and Look for models on this PC brings it back.", { app: m.local.app })}</p>
          ) : (
            <p>{t("This deletes the model file ({size}) from this PC. Your chats stay, and you can reinstall it any time.", { size: bytes(inst.size) })}</p>
          )}
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                try {
                  await api.removeModel(m.id);
                  setConfirm(false);
                  if (m.local) toast(t("Stopped using {name}. Its file is still in {app}.", { name: m.name, app: m.local.app }), "success");
                  await onRefresh();
                  onSettings(await api.settings());
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {m.local ? t("Stop using") : t("Remove")}
            </button>
          </div>
        </Modal>
      )}
    </article>
  );
}
