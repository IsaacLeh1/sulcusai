// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { ask, open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, mainGpu, on, type Budget, type FolderImport, type FoundModels, type HubFile, type HubModel, type HubPurpose, type HubResults, type CatalogView, type InstallProgress, type ModelCard, type ModelTuning, type OptimizeReport, type Settings, type VariantFit } from "../api";
import { bytes, percent, placementLabel, speedLabel, words } from "../format";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { SpeechModels } from "./SpeechModels";
import { MediaModels } from "../components/MediaModels";
import { Lens } from "../components/Lens";
import { HubLinkBox } from "../components/HubLink";
import { ModelStorage } from "../components/ModelStorage";
import { SpeedChart } from "../components/SpeedChart";
import { TESTING, TestingNote, useTestStart } from "../components/Testing";
import { CancelInstall } from "../components/CancelInstall";
import { Downloads } from "../components/Downloads";
import { LockIcon } from "../components/Icons";
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
  const tunings = useTunings();
  const [tab, setTab] = useState<"chat" | "speech" | "create" | "lens">("chat");
  const [filter, setFilter] = useState<Filter>("all");
  const [mostFirst, setMostFirst] = useState(false);
  // "Show models this PC can't run": every model, fetched when switched on
  // (and again when the list changes, e.g. after an install).
  const [showAll, setShowAll] = useState(false);
  // Cards in a grid, or one row per model (kept for next time).
  const [layout, setLayoutState] = useState<"grid" | "list">(() => {
    try {
      return localStorage.getItem("sulcus.models.layout") === "list" ? "list" : "grid";
    } catch {
      return "grid";
    }
  });
  const setLayout = (l: "grid" | "list") => {
    setLayoutState(l);
    try {
      localStorage.setItem("sulcus.models.layout", l);
    } catch {
      /* not kept */
    }
  };
  const [everything, setEverything] = useState<ModelCard[] | null>(null);
  // Models locked against the automatic cleanup.
  const [keptModels, setKeptModels] = useState<string[]>([]);
  useEffect(() => {
    api.keepLocks().then((l) => setKeptModels(l.models)).catch(() => {});
  }, []);
  useEffect(() => {
    if (!showAll) return;
    api.catalog(true).then((v) => setEverything(v.models)).catch(() => {});
  }, [showAll, catalog]);
  if (!catalog) return <div className="page"><p className="muted">{t("Checking this PC…")}</p></div>;

  const { hardware: hw, hints } = catalog;
  // The card models use: the one picked in the status bar, else the one with the most memory.
  const gpu = mainGpu(hw);
  // Rank among the models this PC can run, least capable = 1.
  // Models from other apps have no ratings, so they aren't ranked.
  const byCapability = catalog.models.filter((m) => !m.local).sort((a, b) => a.ratings.overall - b.ratings.overall);
  const rankOf = (m: ModelCard) => byCapability.indexOf(m) + 1;
  const arranged = arrange(catalog.models, filter, mostFirst);
  const pool = showAll && everything ? everything : catalog.models;
  const arrangedPool = showAll ? arrange(pool, filter, mostFirst) : arranged;
  const best = filter !== "all" ? arranged.reduce<ModelCard | null>((top, m) => (!top || (filter === "fastest" ? speedOf(m) > speedOf(top) : m.ratings[filter] > top.ratings[filter]) ? m : top), null) : null;
  const installed = arranged.filter((m) => m.installed);
  const available = arrangedPool.filter((m) => !m.installed);
  // Chat model downloads (speech and picture models have their own pages).
  const chatProgress = Object.fromEntries(Object.entries(progress).filter(([id]) => pool.some((m) => m.id === id)));
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
          <button role="tab" aria-selected={tab === "lens"} className={`mode ${tab === "lens" ? "active" : ""}`} onClick={() => setTab("lens")}>{t("Look inside")}</button>
        </div>
      </header>

      {tab === "chat" && (
        <p className="small callout">
          {t("Chat models write, answer and use tools. Pictures, video and music are made by separate models, which chats use too:")}{" "}
          <button className="link" onClick={() => setTab("create")}>{t("Pictures, video, music")}</button>
        </p>
      )}
      {tab === "chat" && <Downloads progress={chatProgress} models={pool} onRefresh={onRefresh} toast={toast} />}
      {tab === "chat" && <Upgrade models={catalog.models} progress={chatProgress} toast={toast} />}

      <section className="card hw">
        <div className="hw-grid">
          <Spec label={t("Graphics card")} value={gpu ? `${gpu.name}` : t("None usable")} sub={gpu ? t("{size} video memory", { size: bytes(gpu.vram_bytes) }) : hw.broken_cards?.length ? t("A graphics card was found without a working driver. Update its driver, then click Check again.") : t("Models run on the processor")} />
          <Spec label={t("Memory")} value={bytes(hw.ram_total)} sub={t("{size} free now", { size: bytes(hw.ram_available) })} />
          <Spec label={t("Processor")} value={hw.cpu_name || t("Unknown")} sub={t("{cores} cores · {threads} threads", { cores: hw.cpu_cores ?? "?", threads: hw.cpu_threads })} />
          <Spec label={t("Disk")} value={t("{size} free", { size: bytes(hw.disk_free) })} sub={hw.models_drive ? t("on {drive}", { drive: hw.models_drive }) : t("on the models drive")} />
        </div>
        <div className="hw-foot">
          <span className="muted small">
            {t("Engine: llama.cpp ({engine})", { engine: catalog.backend === "vulkan-x64" ? t("graphics card") : catalog.backend === "cpu-x64" ? t("processor") : t("unavailable on this system") })}
            {hw.on_battery ? ` · ${t("On battery: models run slower and drain power faster.")}` : ""}
          </span>
          <span className="row">
            <button className="btn ghost small" onClick={() => document.querySelector(".storage-card")?.scrollIntoView({ behavior: "smooth" })}>
              {t("Change where models are saved")}
            </button>
            <button className="btn ghost small" onClick={recheck} disabled={checking}>
              {checking ? t("Checking…") : t("Check again")}
            </button>
          </span>
        </div>
      </section>

      {tab === "chat" && <SpeedChart models={catalog.models} onRefresh={onRefresh} toast={toast} />}

      {tab === "speech" && <SpeechModels progress={progress} toast={toast} />}
      {tab === "create" && <MediaModels progress={progress} toast={toast} />}
      {tab === "lens" && <Lens models={installed} toast={toast} />}

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
          <div className="row wrap">
            <label className="check small">
              <input type="checkbox" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} />
              <span>{t("Show models this PC can't run")}</span>
            </label>
            <span className="spacer" />
            <div className="mode-switch small-switch" role="radiogroup" aria-label={t("View")}>
              <button role="radio" aria-checked={layout === "grid"} className={`mode ${layout === "grid" ? "active" : ""}`} onClick={() => setLayout("grid")}>▦ {t("Grid")}</button>
              <button role="radio" aria-checked={layout === "list"} className={`mode ${layout === "list" ? "active" : ""}`} onClick={() => setLayout("list")}>☰ {t("List")}</button>
            </div>
          </div>
          {arranged.length === 0 && <p className="muted">{t("None of the models this PC can run stand out here yet. Try All.")}</p>}
        </div>
      )}

      {tab === "chat" && installed.length > 0 && (
        <>
          <h2>{t("Installed")}</h2>
          <div className={`model-grid ${layout}`}>
            {installed.map((m) => (
              <InstalledCard key={m.id} m={m} rank={cardRank} isDefault={m.id === defaultModel} kept={keptModels.includes(m.id)} tuning={tunings[m.id] ?? null} onKept={setKeptModels} onRefresh={onRefresh} onSettings={onSettings} toast={toast} />
            ))}
          </div>
        </>
      )}

      {tab === "chat" && (
        <>
          <h2>{showAll ? t("Available") : t("Available for this PC")}</h2>
          {available.length === 0 && filter === "all" && <p className="muted">{t("Every model that fits this PC is installed.")}</p>}
          <div className={`model-grid ${layout}`}>
            {available.map((m) => (
              <AvailableCard key={m.id} m={m} rank={cardRank} budget={catalog.budget} progress={progress[m.id]} otherBusy={busy && !progress[m.id]} toast={toast} />
            ))}
          </div>
        </>
      )}

      {tab === "chat" && <LookForModels onRefresh={onRefresh} toast={toast} />}

      {tab === "chat" && <HubBrowser models={pool} budget={catalog.budget} showAll={showAll} progress={progress} onRefresh={onRefresh} toast={toast} />}

      <ModelStorage toast={toast} />

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
  const [adding, setAdding] = useState(false);
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
        <div className="row wrap look-buttons">
          <button className="btn" onClick={look} disabled={looking}>
            {looking ? t("Looking…") : t("Look for models on this PC")}
          </button>
          <button className="btn" onClick={() => setAdding(true)}>
            {t("Add from a folder…")}
          </button>
        </div>
      </div>
      {adding && <AddFromFolder onClose={() => setAdding(false)} onRefresh={onRefresh} />}
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

/** Adds the .gguf models in a folder (or a single file) by its path. */
function AddFromFolder({ onClose, onRefresh }: { onClose: () => void; onRefresh: () => Promise<void> }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<FolderImport | string | null>(null);
  const add = async () => {
    if (!path.trim()) return;
    setBusy(true);
    setResult(null);
    try {
      const r = await api.importModelFolder(path);
      setResult(r);
      if (r.added.length > 0) await onRefresh();
    } catch (e) {
      setResult(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal title={t("Add models from a folder")} onClose={onClose}>
      <p className="small">{t("Point SulcusAI at a folder with a model in it, or at a .gguf file. It finds the .gguf model files inside (subfolders too) and uses them where they are: nothing is copied or moved.")}</p>
      <div className="row wrap">
        <input
          className="input"
          style={{ flex: 1, minWidth: 240 }}
          value={path}
          autoFocus
          placeholder={t("e.g. D:\\Models\\gpt-oss-20b")}
          aria-label={t("Folder or file")}
          onChange={(e) => setPath(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !busy && add()}
        />
        <button
          className="btn"
          disabled={busy}
          onClick={async () => {
            const picked = await openDialog({ directory: true, multiple: false });
            if (typeof picked === "string") setPath(picked);
          }}
        >
          {t("Browse…")}
        </button>
      </div>
      {result && (
        <div className="small look-result" role="status">
          {typeof result === "string" ? (
            <p className="warn-text">{result}</p>
          ) : (
            <>
              {result.added.length > 0 ? <p>{t("Added {names}.", { names: result.added.join(", ") })}</p> : <p>{t("Nothing new to add.")}</p>}
              {result.skipped.length > 0 && (
                <ul className="muted">
                  {result.skipped.map((s) => (
                    <li key={s.name}>{s.name}: {t(s.reason)}</li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      )}
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{result && typeof result !== "string" && result.added.length > 0 ? t("Done") : t("Cancel")}</button>
        <button className="btn primary" onClick={add} disabled={busy || !path.trim()}>
          {busy ? t("Adding…") : t("Add")}
        </button>
      </div>
    </Modal>
  );
}

const compact = new Intl.NumberFormat(undefined, { notation: "compact" });

/** Hugging Face's GGUF chat models, showing only the versions this PC can
 * run; installing one works like a model from the list. Nothing is asked of
 * huggingface.co until the person searches. */
const PURPOSES: { id: HubPurpose; label: string }[] = [
  { id: "", label: tx("Any purpose") },
  { id: "coding", label: tx("Coding") },
  { id: "writing", label: tx("Writing") },
  { id: "reasoning", label: tx("Reasoning and maths") },
  { id: "vision", label: tx("Understands pictures") },
  { id: "translation", label: tx("Translation") },
  { id: "uncensored", label: tx("Uncensored") },
];

/** "128K" for 131,072 tokens, "1M" for 1,048,576. */
const ctxSize = (n: number) => (n >= 1 << 20 ? `${+(n / (1 << 20)).toFixed(1)}M` : `${Math.round(n / 1024)}K`);

/** How a version would run on this PC, in a few words. */
function pace(f: HubFile): string {
  const n = words(f.est_tps);
  if (f.est_tps >= 15) return t("runs well, about {n} words a second", { n });
  if (f.est_tps >= 8) return t("runs a little slowly, about {n} words a second", { n });
  return t("runs slowly, about {n} words a second", { n });
}

/** A catalog version's caveats, as the core works out a Hugging Face file's:
 * most of the memory this PC has for models, or 3 bits a weight or fewer. */
function strainOf(fit: VariantFit, budget: Budget): Pick<HubFile, "heavy" | "squeezed" | "placement" | "est_tps"> {
  return {
    heavy: budget.vram + budget.ram > 0 && fit.needed_bytes > (budget.vram + budget.ram) * 0.85,
    squeezed: /^(IQ1|IQ2|IQ3|Q2|Q3|TQ1|TQ2)/i.test(fit.quant),
    placement: fit.placement,
    est_tps: fit.est_tps,
  };
}

/** What to know before downloading a version that runs, in sentences ("" for nothing). */
function caveats(f: Pick<HubFile, "heavy" | "squeezed" | "placement" | "est_tps">): string {
  return [
    f.heavy ? t("It takes most of this PC's memory: close games and other big programs while it runs, or the PC slows down.") : "",
    f.squeezed ? t("It's squeezed hard (3 bits or fewer a weight), so its answers are noticeably worse than a bigger version's.") : "",
    f.placement && f.est_tps < 8 ? t("It writes slowly here.") : "",
  ]
    .filter(Boolean)
    .join(" ");
}

function HubBrowser({ models, budget, showAll, progress, onRefresh, toast }: { models: ModelCard[]; budget: Budget; showAll: boolean; progress: Record<string, InstallProgress>; onRefresh: () => Promise<void>; toast: PushToast }) {
  const [query, setQuery] = useState("");
  const [purpose, setPurpose] = useState<HubPurpose>("");
  const [sort, setSort] = useState<"downloads" | "trending" | "newest">("downloads");
  const [results, setResults] = useState<HubResults | null>(null);
  const [searching, setSearching] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [allOf, setAllOf] = useState<string | null>(null);
  const busy = Object.keys(progress).length > 0;

  const search = async (by = sort, all = showAll, forWhat = purpose) => {
    setSearching(true);
    setProblem(null);
    try {
      setResults(await api.hubSearch(query, by, all, forWhat));
    } catch (e) {
      setProblem(errorText(e));
    } finally {
      setSearching(false);
    }
  };
  // Switching "show what this PC can't run" redoes a search already made.
  useEffect(() => {
    if (results) search(sort, showAll);
  }, [showAll]);
  const install = async (m: HubModel, f: HubFile) => {
    try {
      const runs = f.placement !== null;
      if (!runs) {
        const fit = { quant: f.quant, placement: null, needed_bytes: f.needed, est_tps: f.est_tps, gpu_layers: 0, disk_ok: f.size <= budget.disk };
        if (!(await confirmAnyway(`${m.name} ${f.quant}`, cantRun(fit, budget)))) return;
      } else if (caveats(f)) {
        // Runs, but would strain this PC or disappoint: said first, then it's theirs to decide.
        const go = await ask(t("{caveats} Download {name} anyway?", { caveats: caveats(f), name: `${m.name} ${f.quant}` }), {
          title: t("Before you download it"),
          kind: "warning",
          okLabel: t("Download anyway"),
          cancelLabel: t("Cancel"),
        });
        if (!go) return;
      }
      await api.hubInstall(m.id, f.file, !runs);
      await onRefresh();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const row = (m: HubModel, f: HubFile) => {
    const installed = models.some((c) => c.id === f.model_id && c.installed);
    const p = progress[f.model_id];
    return (
      <div key={f.file} className="hub-file">
        <div className="hub-file-what">
          <strong>{f.quant}</strong>
          {f.file === m.recommended && <span className="badge" title={t("The best fit for this PC")}>★ {t("Best fit")}</span>}
          {f.placement ? (
            <span className="muted small">
              {t(f.quality)} · {bytes(f.size)} · {placementLabel(f.placement)} · {pace(f)}
              {caveats(f) && <span className="warn-text block">{caveats(f)}</span>}
            </span>
          ) : (
            <span className="warn-text small" title={cantRun({ quant: f.quant, placement: null, needed_bytes: f.needed, est_tps: f.est_tps, gpu_layers: 0, disk_ok: f.size <= budget.disk }, budget)}>
              {t(f.quality)} · {bytes(f.size)} · {t("too much for this PC")}
            </span>
          )}
        </div>
        {p ? (
          <InstallBar modelId={f.model_id} p={p} />
        ) : installed ? (
          <span className="badge subtle">{t("Installed")}</span>
        ) : (
          <button className="btn small" onClick={() => install(m, f)} disabled={busy} title={busy ? t("One install at a time") : undefined}>
            {f.placement ? t("Install") : t("Install anyway")}
          </button>
        )}
      </div>
    );
  };

  return (
    <section className="card hub">
      <div className="look-row">
        <div>
          <strong>{t("Hugging Face")}</strong>
          <p className="muted small">{t("Thousands of models people share. Only the versions this PC can run are shown, with how well each runs and anything to know before downloading it. Searching asks huggingface.co, which sees this PC's address like any website; downloads come straight from there.")}</p>
        </div>
      </div>
      <HubLinkBox progress={progress} isInstalled={(id) => models.some((c) => c.id === id && c.installed)} toast={toast} onInstalled={() => onRefresh()} />
      <form
        className="row wrap hub-search"
        onSubmit={(e) => {
          e.preventDefault();
          search();
        }}
      >
        <input className="input" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("Search, e.g. qwen3, gemma, coder")} aria-label={t("Search Hugging Face")} />
        <select
          className="input"
          value={purpose}
          aria-label={t("What it's for")}
          onChange={(e) => {
            const forWhat = e.target.value as HubPurpose;
            setPurpose(forWhat);
            search(sort, showAll, forWhat);
          }}
        >
          {PURPOSES.map((p) => (
            <option key={p.id} value={p.id}>{t(p.label)}</option>
          ))}
        </select>
        <select
          className="input"
          value={sort}
          aria-label={t("Order")}
          onChange={(e) => {
            const by = e.target.value as typeof sort;
            setSort(by);
            if (results) search(by);
          }}
        >
          <option value="downloads">{t("Most downloaded")}</option>
          <option value="trending">{t("Trending")}</option>
          <option value="newest">{t("Recently updated")}</option>
        </select>
        <button className="btn" type="submit" disabled={searching}>
          {searching ? t("Searching…") : query.trim() ? t("Search") : t("Show popular models")}
        </button>
      </form>
      {problem && <p className="warn-text small">{problem}</p>}
      {results && (
        <>
          <p className="muted small" role="status">
            {results.models.length === 0
              ? t("Nothing found that this PC can run. Try another search.")
              : results.models.length === 1 ? t("1 model with versions this PC can run.") : t("{n} models with versions this PC can run.", { n: results.models.length })}{" "}
            {results.hidden > 0 && t("{n} more were left out: too big for this PC, or only in formats SulcusAI can't use.", { n: results.hidden })}
          </p>
          <div className="hub-list">
            {results.models.map((m) => {
              const best = m.files.find((f) => f.file === m.recommended) ?? m.files[m.files.length - 1];
              const shown = allOf === m.id ? m.files : [best];
              return (
                <article key={m.id} className="hub-model">
                  <div className="hub-head">
                    <div>
                      <strong>{m.name}</strong> <span className="muted small">{t("by {author}", { author: m.author })}</span>
                    </div>
                    <span className="muted small">
                      {m.params_b !== null && `${t("{n}B parameters", { n: m.params_b })} · `}
                      {m.context > 0 && `${t("up to {size} tokens of context", { size: ctxSize(m.context) })} · `}
                      {t("{n} downloads", { n: compact.format(m.downloads) })} · {t("{n} likes", { n: compact.format(m.likes) })}
                      {m.vision && ` · ${t("sees pictures")}`}
                    </span>
                  </div>
                  {shown.map((f) => row(m, f))}
                  {m.files.length > 1 && (
                    <button className="link small" onClick={() => setAllOf(allOf === m.id ? null : m.id)}>
                      {allOf === m.id ? t("Show only the best fit") : t("All {n} versions", { n: m.files.length })}
                    </button>
                  )}
                </article>
              );
            })}
          </div>
        </>
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
          {rank.rankOf(m) > 0 ? (
            <>
              {t("Capability")} <strong>{rank.rankOf(m)}</strong> {t("of {total}", { total: rank.total })}
              <span className="muted"> {t("(1 = lightest)")}</span>
            </>
          ) : (
            t("Capability")
          )}
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

/** Why this PC can't run a version: too little memory, too slow, or no disk space. */
function cantRun(f: VariantFit | undefined, b: Budget): string {
  if (!f) return t("This PC can't run it.");
  if (!f.disk_ok) return t("There isn't enough free disk space for it.");
  if (f.needed_bytes > Math.max(b.vram + b.ram, b.ram)) {
    return t("It needs about {needed} of memory; this PC has {vram} on the graphics card and {ram} for models.", { needed: bytes(f.needed_bytes), vram: bytes(b.vram), ram: bytes(b.ram) });
  }
  return t("It would run too slowly here (about {n} words a second).", { n: words(Math.max(0.1, f.est_tps)) });
}

/** Installing something this PC probably can't run: say so first. */
async function confirmAnyway(name: string, reason: string): Promise<boolean> {
  return ask(t("{reason} It may not load, or be very slow. Download {name} anyway?", { reason, name }), {
    title: t("This PC probably can't run it"),
    kind: "warning",
    okLabel: t("Download anyway"),
    cancelLabel: t("Cancel"),
  });
}

function AvailableCard({ m, rank, budget, progress, otherBusy, toast }: { m: ModelCard; rank: RankInfo; budget: Budget; progress?: InstallProgress; otherBusy: boolean; toast: PushToast }) {
  // Models this PC can't run (shown on request) offer every version.
  const options = m.runnable ? m.fit.variants.filter((v) => v.placement && v.disk_ok) : m.fit.variants;
  const onDisk = options.find((v) => v.quant === m.on_disk)?.quant;
  const [quant, setQuant] = useState(onDisk ?? m.fit.recommended ?? options[0]?.quant ?? "");
  const fit = m.fit.variants.find((v) => v.quant === quant);
  const variant = m.variants.find((v) => v.quant === quant);

  const runs = !!fit?.placement && fit.disk_ok;
  const warn = fit && runs ? caveats(strainOf(fit, budget)) : "";
  const install = async () => {
    try {
      if (!runs) {
        if (!(await confirmAnyway(m.name, cantRun(fit, budget)))) return;
      } else if (warn && quant !== onDisk) {
        const go = await ask(t("{caveats} Download {name} anyway?", { caveats: warn, name: m.name }), {
          title: t("Before you download it"),
          kind: "warning",
          okLabel: t("Download anyway"),
          cancelLabel: t("Cancel"),
        });
        if (!go) return;
      }
      await api.install(m.id, quant, !runs);
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
        {onDisk ? (
          <span className="badge">{t("Already on this PC")}</span>
        ) : (
          m.best && (
            <span className="badge" title={t("The most capable model that runs comfortably on this PC")}>
              {t("Best for this PC")}
            </span>
          )
        )}
      </div>
      <p className="desc">{m.description}</p>
      <Capability m={m} rank={rank} />
      <Tags m={m} />
      {progress ? (
        <InstallBar modelId={m.id} p={progress} />
      ) : (
        <div className="install-row">
          <select value={quant} onChange={(e) => setQuant(e.target.value)} aria-label={t("Quality for {name}", { name: m.name })} title={t("Quality and download size. ★ is the best fit for this PC.")}>

            {options.map((v) => {
              // A fit for a version the model doesn't list (stale data): left
              // out, rather than the whole page failing.
              const spec = m.variants.find((x) => x.quant === v.quant);
              if (!spec) return null;
              return (
                <option key={v.quant} value={v.quant}>
                  {spec.quality} · {bytes(spec.size)}{v.quant === m.fit.recommended ? " ★" : ""}
                </option>
              );
            })}
          </select>
          <button className={`btn ${runs ? "primary" : ""}`} onClick={install} disabled={otherBusy || !quant || (fit !== undefined && !fit.disk_ok)} title={otherBusy ? t("One install at a time") : undefined}>
            {quant === onDisk ? t("Set up (no download)") : runs ? t("Install") : t("Install anyway")}
          </button>
        </div>
      )}
      {fit && variant && !progress && (runs ? (
        <p className="muted small fit-line">
          {placementLabel(fit.placement)} · {speedLabel(fit.est_tps)} {t("(about {n} words a second, estimated)", { n: words(fit.est_tps) })}
          {warn && <span className="warn-text block">{warn}</span>}
        </p>
      ) : (
        <p className="warn-text small fit-line" role="note">{t("This PC probably can't run this version.")} {cantRun(fit, budget)}</p>
      ))}
    </article>
  );
}

/** When a much more capable model than any installed one fits this PC (a
    newer generation, usually), says so once, with Install and Not now. */
function Upgrade({ models, progress, toast }: { models: ModelCard[]; progress: Record<string, InstallProgress>; toast: PushToast }) {
  const [dismissed, setDismissed] = useState<string | null>(() => {
    try {
      return localStorage.getItem("sulcus.models.upgrade-dismissed");
    } catch {
      return null;
    }
  });
  const best = models.find((m) => m.best && !m.installed);
  const have = models.filter((m) => m.installed && m.ratings.overall > 0);
  const top = have.reduce<ModelCard | null>((a, m) => (!a || m.ratings.overall > a.ratings.overall ? m : a), null);
  if (!best || !top || best.ratings.overall < top.ratings.overall + 10 || dismissed === best.id) return null;
  const p = progress[best.id];
  const later = () => {
    setDismissed(best.id);
    try {
      localStorage.setItem("sulcus.models.upgrade-dismissed", best.id);
    } catch {
      /* not kept */
    }
  };
  return (
    <section className="card callout upgrade">
      <p>
        <strong>{t("A smarter model fits this PC: {name}", { name: best.name })}</strong>
        <br />
        <span className="muted small">
          {t("It's rated {theirs} against {ours} for {current}, the best you have now, and runs comfortably here. Your chats and other models stay as they are.", {
            theirs: best.ratings.overall,
            ours: top.ratings.overall,
            current: top.name,
          })}
        </span>
      </p>
      {p ? (
        <InstallBar modelId={best.id} p={p} />
      ) : (
        <div className="row">
          <button
            className="btn primary small"
            onClick={async () => {
              try {
                await api.install(best.id, best.fit.recommended ?? best.variants[0].quant, false);
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            {t("Install {name}", { name: best.name })}
          </button>
          <button className="btn ghost small" onClick={later}>
            {t("Not now")}
          </button>
        </div>
      )}
    </section>
  );
}

function InstallBar({ modelId, p }: { modelId: string; p: InstallProgress }) {
  const label = {
    engine: t("Setting up the engine"),
    verify: t("Checking the downloaded file"),
    download: t("Downloading"),
    loading: t("Loading the model to test it"),
    benchmark: t("Testing speed on this PC"),
    optimizing: t("Trying setups to find the fastest on this PC"),
    vision: t("Downloading the picture reader"),
    waiting: t("Waiting for a reply that uses this model to finish"),
  }[p.phase];
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  const testing = TESTING.has(p.phase);
  const since = useTestStart(testing);
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${t("{received} of {total}", { received: bytes(p.received), total: bytes(p.total) })}` : "…"}</span>
        {!testing && (
          <span className="row">
            {(p.phase === "download" || p.phase === "vision" || p.phase === "engine") && (
              <button type="button" className="btn small" onClick={() => api.pauseInstall(modelId)} title={t("Keeps what's downloaded so you can carry on later")}>
                {t("Pause")}
              </button>
            )}
            <CancelInstall p={p} />
          </span>
        )}
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
      {testing && <TestingNote since={since} />}
    </div>
  );
}

/** Shows a model under the user's own name (only the name in the app changes). */
function RenameModel({ m, onClose, onRefresh }: { m: ModelCard; onClose: () => void; onRefresh: () => Promise<void> }) {
  const [name, setName] = useState(m.name);
  const [problem, setProblem] = useState<string | null>(null);
  const save = async (value: string) => {
    try {
      await api.renameModel(m.id, value);
      await onRefresh();
      onClose();
    } catch (e) {
      setProblem(errorText(e));
    }
  };
  return (
    <Modal title={t("Rename {name}", { name: m.name })} onClose={onClose}>
      <p className="small muted">{t("Only the name shown in SulcusAI changes; the model and its file stay as they are.")}</p>
      <input
        className="input"
        value={name}
        autoFocus
        maxLength={60}
        aria-label={t("Name")}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && name.trim() && save(name)}
      />
      {m.renamed_from && <p className="small muted">{t("Its own name: {name}", { name: m.renamed_from })}</p>}
      {problem && <p className="small warn-text">{problem}</p>}
      <div className="modal-actions">
        {m.renamed_from && (
          <button className="btn ghost" onClick={() => save("")}>
            {t("Use its own name")}
          </button>
        )}
        <span className="spacer" />
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={() => save(name)} disabled={!name.trim() || name.trim() === m.name}>
          {t("Save")}
        </button>
      </div>
    </Modal>
  );
}

/** Settings measured on this PC per model, kept current as runs finish. */
function useTunings(): Record<string, ModelTuning> {
  const [tunings, setTunings] = useState<Record<string, ModelTuning>>({});
  useEffect(() => {
    const load = () => api.modelTunings().then(setTunings).catch(() => {});
    load();
    const sub = on("models:optimize", (e) => e.finished && load());
    // "Use the automatic settings" on a card.
    window.addEventListener("sulcus:tunings", load);
    return () => {
      sub.then((un) => un());
      window.removeEventListener("sulcus:tunings", load);
    };
  }, []);
  return tunings;
}

/** "Optimize for this PC": the button, its progress, and what it found. */
function Optimize({ m, tuning, onRefresh, toast }: { m: ModelCard; tuning: ModelTuning | null; onRefresh: () => Promise<void>; toast: PushToast }) {
  const [step, setStep] = useState<{ step: number; of: number; label: string } | null>(null);
  const [report, setReport] = useState<OptimizeReport | null>(null);
  const since = useTestStart(step !== null);
  useEffect(() => {
    const sub = on("models:optimize", (e) => {
      if (e.model_id !== m.id) return;
      if (e.finished) {
        setStep(null);
        if (e.report) setReport(e.report);
        if (e.error) toast(errorText(e.error), "error");
        // The speed shown on the card is the one just measured.
        onRefresh().catch(() => {});
      } else if (e.of) {
        setStep({ step: e.step ?? 0, of: e.of, label: e.label ?? "" });
      }
    });
    return () => {
      sub.then((un) => un());
    };
  }, [m.id, toast, onRefresh]);
  const run = () => {
    setReport(null);
    setStep({ step: 0, of: 1, label: "" });
    api.optimizeModel(m.id).catch(() => setStep(null));
  };
  if (step) {
    return (
      <div className="optimize small">
        <span>
          {t("Optimizing: trying setup {n} of {of}", { n: step.step + 1, of: step.of })}
          {step.label && <span className="muted"> · {step.label}</span>}
        </span>
        <TestingNote since={since} short />
      </div>
    );
  }
  const offer = report?.smaller_window ?? tuning?.smaller_window ?? null;
  const measuredTps = report?.tps ?? tuning?.tps ?? 0;
  return (
    <div className="optimize small">
      {tuning ? (
        <span>
          {t("Optimized for this PC: about {tps} words a second", { tps: words(tuning.tps) })}
          {tuning.before_tps > 0 && Math.abs(tuning.tps - tuning.before_tps) >= 0.5 && <span className="muted"> {t("(automatic settings: {tps})", { tps: tuning.before_tps })}</span>}
          <span className="muted">
            {" · "}
            {tuning.gpu_layers >= tuning.all_layers ? t("all of it on the graphics card") : t("{n} of {all} layers on the graphics card", { n: tuning.gpu_layers, all: tuning.all_layers })}
            {tuning.kv_q8 ? ` · ${t("compressed memory of the chat")}` : ""}
          </span>
        </span>
      ) : null}
      <span className="row wrap">
        <button
          className="btn small"
          onClick={run}
          title={t("Tries a few ways of running it on this PC's graphics card and keeps the fastest. Takes a minute or two, and the graphics card works hard meanwhile.")}
        >
          {tuning ? t("Optimize again") : t("Optimize for this PC")}
        </button>
        {tuning && Math.abs(tuning.tps - tuning.before_tps) >= 0.5 && (
          <button className="link" onClick={() => api.forgetOptimization(m.id).then(() => window.dispatchEvent(new Event("sulcus:tunings"))).catch((e) => toast(errorText(e), "error"))}>
            {t("Use the automatic settings")}
          </button>
        )}
      </span>
      {offer && (
        <p className="callout">
          {t("Keeping {k}K of the chat in mind, the whole model fits the graphics card: about {tps} words a second instead of {now}. It remembers less of a long chat that way.", { k: Math.round(offer.ctx / 1024), tps: words(offer.tps), now: words(measuredTps) })}{" "}
          <button
            className="link"
            onClick={() =>
              api
                .setContextWindow(offer.ctx)
                .then(() => toast(t("Chats now use a {k}K window.", { k: Math.round(offer.ctx / 1024) }), "success"))
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {t("Use a {k}K window", { k: Math.round(offer.ctx / 1024) })}
          </button>
        </p>
      )}
    </div>
  );
}

function InstalledCard({
  m,
  rank,
  isDefault,
  kept,
  tuning,
  onKept,
  onRefresh,
  onSettings,
  toast,
}: {
  m: ModelCard;
  rank: RankInfo;
  isDefault: boolean;
  kept: boolean;
  tuning: ModelTuning | null;
  onKept: (models: string[]) => void;
  onRefresh: () => Promise<void>;
  onSettings: (s: Settings) => void;
  toast: PushToast;
}) {
  const [confirm, setConfirm] = useState(false);
  const [alsoFile, setAlsoFile] = useState(false);
  const ollama = m.local?.app.toLowerCase() === "ollama";
  const [renaming, setRenaming] = useState(false);
  const inst = m.installed!;
  const variant = m.variants.find((v) => v.quant === inst.quant);
  const fit = m.fit.variants.find((v) => v.quant === inst.quant);

  return (
    <article className="card model installed">
      <div className="model-head">
        <div>
          <h3 title={m.renamed_from ? t("Its own name: {name}", { name: m.renamed_from }) : undefined}>{m.name}</h3>
          <span className="muted small">
            {m.local ? `${t("From {app}", { app: m.local.app })} · ` : ""}
            {t("{quality} quality", { quality: variant?.quality ?? inst.quant })} · {bytes(inst.size)}
          </span>
        </div>
        <span className="row">
          {isDefault ? <span className="badge">{t("Default")}</span> : null}
          <button
            className={`icon-btn keep-btn ${kept ? "on" : ""}`}
            aria-pressed={kept}
            title={kept ? t("Locked: the automatic cleanup never removes it. Click to unlock.") : t("Lock it so the automatic cleanup never removes it")}
            onClick={() => api.setKept("model", m.id, !kept).then((l) => onKept(l.models)).catch((e) => toast(errorText(e), "error"))}
          >
            <LockIcon open={!kept} />
          </button>
        </span>
      </div>
      <p className="desc">{m.description}</p>
      {m.local ? <p className="muted small ellipsis" title={m.local.path}>{t("{n}B parameters", { n: m.params_b })}{m.tools ? ` · ${t("can use tools")}` : ""}</p> : <Capability m={m} rank={rank} />}
      {!m.local && <Tags m={m} />}
      <p className="small fit-line">
        {inst.tps !== null
          ? t("Measured on this PC: about {tps} words a second ({speed})", { tps: words(inst.tps), speed: speedLabel(inst.tps).toLowerCase() })
          : t("Speed not measured yet")}
        {fit && <span className="muted"> · {placementLabel(fit.placement)}</span>}
      </p>
      {fit?.placement !== "cpu" && <Optimize m={m} tuning={tuning} onRefresh={onRefresh} toast={toast} />}
      {m.damaged && (
        <div className="warn-box small">
          <p>{t("Its model file is damaged ({why}), so it won't load.", { why: m.damaged })}</p>
          {!m.local && (
            <button
              className="btn small primary"
              onClick={async () => {
                try {
                  await api.repairModel(m.id);
                  toast(t("Downloading {name} again.", { name: m.name }), "success");
                  await onRefresh();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Download again")}
            </button>
          )}
        </div>
      )}
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
        <button className="btn ghost" onClick={() => setRenaming(true)}>
          {t("Rename")}
        </button>
        <span className="spacer" />
        <button className="btn ghost danger" onClick={() => (setAlsoFile(false), setConfirm(true))}>
          {m.local ? t("Stop using") : t("Delete from this PC")}
        </button>
      </div>
      {renaming && <RenameModel m={m} onClose={() => setRenaming(false)} onRefresh={onRefresh} />}
      {confirm && (
        <Modal title={m.local && !alsoFile ? t("Stop using {name}?", { name: m.name }) : t("Delete {name} from this PC?", { name: m.name })} onClose={() => setConfirm(false)}>
          {m.local ? (
            <>
              <p>{t("SulcusAI stops listing it. The file stays in {app}, and Look for models on this PC brings it back.", { app: m.local.app })}</p>
              {ollama ? (
                <p className="small muted">{t("To delete the file too, remove the model in Ollama (ollama rm), so Ollama keeps working.")}</p>
              ) : (
                <label className="check">
                  <input type="checkbox" checked={alsoFile} onChange={(e) => setAlsoFile(e.target.checked)} />
                  <span>
                    {t("Also delete the file itself ({size}): it moves to the Recycle Bin, and {app} won't have it either.", { size: bytes(inst.size), app: m.local.app })}
                    <span className="muted small block ellipsis" title={m.local.path}>{m.local.path}</span>
                  </span>
                </label>
              )}
            </>
          ) : (
            <p>
              {t("This deletes the model file ({size}) and everything SulcusAI kept for it: half-finished downloads, models taught on top of it and the name you gave it. Your chats stay (they use your default model), and you can install it again any time.", { size: bytes(inst.size) })}
            </p>
          )}
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                try {
                  const freed = await api.removeModel(m.id, alsoFile);
                  setConfirm(false);
                  if (m.local && !alsoFile) toast(t("Stopped using {name}. Its file is still in {app}.", { name: m.name, app: m.local.app }), "success");
                  else if (m.local) toast(t("Deleted {name}. Its file is in the Recycle Bin; empty it to free {size}.", { name: m.name, size: bytes(freed) }), "success");
                  else toast(t("Deleted {name} from this PC ({size} freed).", { name: m.name, size: bytes(freed) }), "success");
                  await onRefresh();
                  onSettings(await api.settings());
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {m.local && !alsoFile ? t("Stop using") : t("Delete from this PC")}
            </button>
          </div>
        </Modal>
      )}
    </article>
  );
}
