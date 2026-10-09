// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText, type CatalogView, type InstallProgress, type ModelCard, type Settings } from "../api";
import { bytes, percent, placementLabel, speedLabel } from "../format";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { SpeechModels } from "./SpeechModels";
import { MediaModels } from "../components/MediaModels";
import { AREA_MIN, FILTERS, arrange, speedOf, type Area, type Filter } from "../ranking";

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
  if (!catalog) return <div className="page"><p className="muted">Checking this PC…</p></div>;

  const { hardware: hw, hints } = catalog;
  const gpu = hw.gpus.filter((g) => !g.integrated).sort((a, b) => b.vram_bytes - a.vram_bytes)[0];
  // Rank among the models this PC can run, least capable = 1.
  const byCapability = [...catalog.models].sort((a, b) => a.ratings.overall - b.ratings.overall);
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
      // Also picks up models downloaded by other apps since the app started.
      await api.findModels().catch(() => {});
      await onRefresh();
    } finally {
      setChecking(false);
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>Models</h1>
          <p className="muted">Only models this PC can run are shown. Click Install and they're ready in a few minutes.</p>
        </div>
        <div className="mode-switch" role="tablist" aria-label="Kind of model">
          <button role="tab" aria-selected={tab === "chat"} className={`mode ${tab === "chat" ? "active" : ""}`} onClick={() => setTab("chat")}>Chat</button>
          <button role="tab" aria-selected={tab === "speech"} className={`mode ${tab === "speech" ? "active" : ""}`} onClick={() => setTab("speech")}>Speech</button>
          <button role="tab" aria-selected={tab === "create"} className={`mode ${tab === "create" ? "active" : ""}`} onClick={() => setTab("create")}>Pictures, video, music</button>
        </div>
      </header>

      <section className="card hw">
        <div className="hw-grid">
          <Spec label="Graphics card" value={gpu ? `${gpu.name}` : "None usable"} sub={gpu ? `${bytes(gpu.vram_bytes)} video memory` : "Models run on the processor"} />
          <Spec label="Memory" value={bytes(hw.ram_total)} sub={`${bytes(hw.ram_available)} free now`} />
          <Spec label="Processor" value={hw.cpu_name || "Unknown"} sub={`${hw.cpu_cores ?? "?"} cores · ${hw.cpu_threads} threads`} />
          <Spec label="Disk" value={`${bytes(hw.disk_free)} free`} sub={`on ${hw.models_drive || "the models drive"}`} />
        </div>
        <div className="hw-foot">
          <span className="muted small">
            Engine: llama.cpp ({catalog.backend === "vulkan-x64" ? "graphics card" : catalog.backend === "cpu-x64" ? "processor" : "unavailable on this system"})
            {hw.on_battery ? " · On battery: models run slower and drain power faster." : ""}
          </span>
          <button className="btn ghost small" onClick={recheck} disabled={checking} title="Checks this PC again and looks for models other apps (LM Studio, Ollama and others) already downloaded">
            {checking ? "Checking…" : "Check again"}
          </button>
        </div>
      </section>

      {tab === "speech" && <SpeechModels progress={progress} toast={toast} />}
      {tab === "create" && <MediaModels progress={progress} toast={toast} />}

      {tab === "chat" && (
        <div className="model-filters">
          <div className="chips" role="tablist" aria-label="Show models">
            {FILTERS.map((f) => (
              <button key={f.id} role="tab" aria-selected={filter === f.id} className={`chip ${filter === f.id ? "active" : ""}`} onClick={() => setFilter(f.id)} title={f.hint}>
                {f.label}
              </button>
            ))}
          </div>
          {filter !== "fastest" && (
            <div className="mode-switch small-switch" role="radiogroup" aria-label="Order">
              <button role="radio" aria-checked={!mostFirst} className={`mode ${!mostFirst ? "active" : ""}`} onClick={() => setMostFirst(false)}>Least → most capable</button>
              <button role="radio" aria-checked={mostFirst} className={`mode ${mostFirst ? "active" : ""}`} onClick={() => setMostFirst(true)}>Most → least</button>
            </div>
          )}
          <p className="muted small">
            {FILTERS.find((f) => f.id === filter)!.hint}
            {filter === "fastest" ? " Measured once installed; estimated before." : " Scores are rough guides from public benchmarks; bigger models also need more of the PC."}
          </p>
          {arranged.length === 0 && <p className="muted">None of the models this PC can run stand out here yet. Try All.</p>}
        </div>
      )}

      {tab === "chat" && installed.length > 0 && (
        <>
          <h2>Installed</h2>
          <div className="model-grid">
            {installed.map((m) => (
              <InstalledCard key={m.id} m={m} rank={cardRank} isDefault={m.id === defaultModel} onRefresh={onRefresh} onSettings={onSettings} toast={toast} />
            ))}
          </div>
        </>
      )}

      {tab === "chat" && (
        <>
          <h2>Available for this PC</h2>
          {available.length === 0 && filter === "all" && <p className="muted">Every model that fits this PC is installed.</p>}
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
            {hints.hidden} {hints.hidden === 1 ? "model is" : "models are"} hidden because this PC can't run {hints.hidden === 1 ? "it" : "them"}.
          </strong>
          <ul>
            {hints.ram.map((r) => (
              <li key={r.extra_ram_gb}>
                Adding {r.extra_ram_gb} GB of memory would unlock {r.unlocks} more {r.unlocks === 1 ? "model" : "models"}.
              </li>
            ))}
            {hints.disk_blocked > 0 && (
              <li>
                Freeing up {bytes(hints.disk_needed_bytes)} of disk space would unlock {hints.disk_blocked} more {hints.disk_blocked === 1 ? "model" : "models"}.
              </li>
            )}
            {hints.ram.length === 0 && hints.disk_blocked === 0 && <li>They need a graphics card with more video memory.</li>}
          </ul>
        </section>
      )}
    </div>
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

const AREA_LABEL: Record<Area, string> = { coding: "Coding", writing: "Writing", research: "Research", agents: "Agents", languages: "Languages" };

/** Where the model sits from least to most capable, and its strengths. */
function Capability({ m, rank }: { m: ModelCard; rank: RankInfo }) {
  const r = m.ratings;
  const strengths = (Object.keys(AREA_LABEL) as Area[]).filter((a) => r[a] >= AREA_MIN).sort((a, b) => r[b] - r[a]);
  return (
    <div className="capability">
      <div className="cap-head small">
        <span>
          Capability <strong>{rank.rankOf(m)}</strong> of {rank.total}
          <span className="muted"> (1 = lightest)</span>
        </span>
        {rank.best === m.id && rank.filter !== "all" && <span className="badge">{rank.filter === "fastest" ? "Fastest here" : `Best for ${AREA_LABEL[rank.filter as Area].toLowerCase()}`}</span>}
      </div>
      <div className="cap-bar" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={r.overall} aria-label="Overall capability">
        <span style={{ width: `${r.overall}%` }} />
      </div>
      {strengths.length > 0 && <span className="muted small">Strong at: {strengths.map((a) => AREA_LABEL[a].toLowerCase()).join(", ")}</span>}
    </div>
  );
}

function Tags({ m }: { m: ModelCard }) {
  return (
    <div className="tags">
      {m.tags.filter((t) => t !== "recommended").map((t) => (
        <span key={t} className="tag">{t}</span>
      ))}
      <span className="tag license" title={`${m.license.note ?? m.license.name}\n${m.license.url}`}>
        {m.license.name}
      </span>
      {!m.license.commercial && <span className="tag warn">Non-commercial</span>}
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
          <span className="muted small">{m.publisher} · {m.params_b}B parameters</span>
        </div>
        {onDisk ? <span className="badge">Already on this PC</span> : m.tags.includes("recommended") && <span className="badge">Recommended</span>}
      </div>
      <p className="desc">{m.description}</p>
      <Capability m={m} rank={rank} />
      <Tags m={m} />
      {progress ? (
        <InstallBar m={m} p={progress} />
      ) : (
        <div className="install-row">
          <select value={quant} onChange={(e) => setQuant(e.target.value)} aria-label={`Quality for ${m.name}`} title="Quality and download size. ★ is the best fit for this PC.">

            {options.map((v) => {
              const spec = m.variants.find((x) => x.quant === v.quant)!;
              return (
                <option key={v.quant} value={v.quant}>
                  {spec.quality} · {bytes(spec.size)}{v.quant === m.fit.recommended ? " ★" : ""}
                </option>
              );
            })}
          </select>
          <button className="btn primary" onClick={install} disabled={otherBusy || !quant} title={otherBusy ? "One install at a time" : undefined}>
            {quant === onDisk ? "Set up (no download)" : "Install"}
          </button>
        </div>
      )}
      {fit && variant && !progress && (
        <p className="muted small fit-line">
          {placementLabel(fit.placement)} · {speedLabel(fit.est_tps)} (about {Math.round(fit.est_tps)} tokens/sec, estimated)
        </p>
      )}
    </article>
  );
}

function InstallBar({ m, p }: { m: ModelCard; p: InstallProgress }) {
  const label = {
    engine: "Setting up the engine",
    verify: "Checking the downloaded file",
    download: "Downloading",
    benchmark: "Testing speed on this PC",
    vision: "Downloading the picture reader",
  }[p.phase];
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${bytes(p.received)} of ${bytes(p.total)}` : "…"}</span>
        {p.phase !== "benchmark" && (
          <button className="link" onClick={() => api.cancelInstall(m.id)}>
            Cancel
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
            {variant?.quality ?? inst.quant} quality · {bytes(inst.size)}
          </span>
        </div>
        {isDefault ? <span className="badge">Default</span> : null}
      </div>
      <p className="desc">{m.description}</p>
      <Capability m={m} rank={rank} />
      <Tags m={m} />
      <p className="small fit-line">
        {inst.tps !== null
          ? `Measured on this PC: ${inst.tps} tokens/sec (${speedLabel(inst.tps).toLowerCase()})`
          : "Speed not measured yet"}
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
            Make default
          </button>
        )}
        <span className="spacer" />
        <button className="btn ghost danger" onClick={() => setConfirm(true)}>
          Remove
        </button>
      </div>
      {confirm && (
        <Modal title={`Remove ${m.name}?`} onClose={() => setConfirm(false)}>
          <p>This deletes the model file ({bytes(inst.size)}) from this PC. Your chats stay, and you can reinstall it any time.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>Cancel</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                try {
                  await api.removeModel(m.id);
                  setConfirm(false);
                  await onRefresh();
                  onSettings(await api.settings());
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Remove
            </button>
          </div>
        </Modal>
      )}
    </article>
  );
}
