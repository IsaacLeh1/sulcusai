// SPDX-License-Identifier: AGPL-3.0-only
// Picture, video and music models: what fits this PC, install and remove.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type HubMediaFile, type HubMediaModel, type HubMediaResults, type InstallProgress, type MediaKind, type MediaModelCard, type MediaView } from "../api";
import { bytes, percent } from "../format";
import { t, tx } from "../i18n";
import { waitLabel } from "../media";
import { CancelInstall } from "./CancelInstall";
import { HubLinkBox } from "./HubLink";
import { LockIcon } from "./Icons";
import type { PushToast } from "./Toasts";

const KINDS: { kind: MediaKind; title: string; per: string }[] = [
  { kind: "image", title: tx("Pictures"), per: tx("a picture") },
  { kind: "upscale", title: tx("Upscaling"), per: tx("a picture") },
  { kind: "background", title: tx("Background removal"), per: tx("a picture") },
  { kind: "video", title: tx("Video"), per: tx("a 3-second clip") },
  { kind: "music", title: tx("Music and sound"), per: tx("a 30-second song") },
];

export function useMediaView(all = false): [MediaView | null, () => void] {
  const [view, setView] = useState<MediaView | null>(null);
  const load = useCallback(() => {
    api.mediaView(all).then(setView).catch(() => {});
  }, [all]);
  useEffect(() => {
    load();
    const subs = [on("install:finished", load), on("features:changed", load), on("media:done", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [load]);
  return [view, load];
}

export function MediaModels({ progress, toast, only }: { progress: Record<string, InstallProgress>; toast: PushToast; only?: MediaKind[] }) {
  // Every model, also those too big for this PC (they say why).
  const [showAll, setShowAll] = useState(false);
  const [view, load] = useMediaView(showAll);
  // Models locked against the automatic cleanup.
  const [kept, setKept] = useState<string[]>([]);
  useEffect(() => {
    api.keepLocks().then((l) => setKept(l.models)).catch(() => {});
  }, []);
  if (!view) return <p className="muted">{t("Loading…")}</p>;
  const busy = Object.keys(progress).length > 0;
  const kinds = KINDS.filter((k) => !only || only.includes(k.kind));
  const installed = new Set(view.models.filter((m) => m.installed).map((m) => m.id));
  const first: HubKind = only?.includes("video") ? "video" : only?.includes("music") ? "music" : "image";
  const hubKinds = (["image", "video", "music"] as HubKind[]).filter((k) => !only || only.includes(k));
  return (
    <div className="media-models">
      <section className="media-link">
        <HubLinkBox
          progress={progress}
          isInstalled={(id) => installed.has(id)}
          toast={toast}
          onInstalled={load}
          placeholder={hubKinds.length === 1 ? t("Paste a Hugging Face link to a model, e.g. {link}", { link: LINK_EXAMPLE[hubKinds[0]] }) : t("Paste a Hugging Face link to a picture, video or music model")}
        />
      </section>
      <label className="small row">
        <input type="checkbox" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} />
        {t("Show models this PC can't run")}
      </label>
      {kinds.map(({ kind, title, per }) => {
        const list = view.models.filter((m) => m.kind === kind).sort((a, b) => b.quality - a.quality);
        const left = (view.hidden_by_kind ?? []).filter((h) => h.kind === kind);
        if (list.length === 0 && left.length === 0 && kind !== "video") return null;
        return (
          <section key={kind}>
            <h3>{t(title)}</h3>
            {kind === "video" && view.video_note && <p className="small warn-text">{view.video_note}</p>}
            {list.map((m) => (
              <MediaModelRow key={m.id} m={m} per={per} progress={progress[m.id]} busy={busy} toast={toast} onChanged={load} isDefault={defaultOf(view, m.kind) === m.id} kept={kept.includes(m.id)} onKept={setKept} />
            ))}
            {!showAll && left.map((h) => (
              <p key={h.why} className="small muted">{hiddenText(h.why, h.count)}</p>
            ))}
          </section>
        );
      })}
      <MediaHubBrowser first={first} installed={installed} progress={progress} toast={toast} onChanged={load} />
    </div>
  );
}

/** A Hugging Face link of each kind, as an example. */
const LINK_EXAMPLE: Record<HubKind, string> = {
  image: "https://huggingface.co/RunDiffusion/Juggernaut-XL-v9",
  video: "https://huggingface.co/QuantStack/Wan2.2-TI2V-5B-GGUF",
  music: "https://huggingface.co/Serveurperso/ACE-Step-1.5-GGUF",
};

/** Why some models of a kind aren't listed. */
function hiddenText(why: "memory" | "card" | "disk", n: number): string {
  if (why === "disk") {
    return n === 1
      ? t("1 more model needs more free disk space than this PC has. Free some space to see it.")
      : t("{n} more models need more free disk space than this PC has. Free some space to see them.", { n });
  }
  if (why === "card") {
    return n === 1
      ? t("1 more model needs a bigger graphics card than this PC has, so it's not shown.")
      : t("{n} more models need a bigger graphics card than this PC has, so they're not shown.", { n });
  }
  return n === 1
    ? t("1 more model needs more memory than this PC has, so it's not shown.")
    : t("{n} more models need more memory than this PC has, so they're not shown.", { n });
}

/** The default model for a kind that has one (pictures, video, music). */
function defaultOf(view: MediaView, kind: MediaKind): string | null {
  return kind === "image" ? view.defaults?.image ?? null : kind === "video" ? view.defaults?.video ?? null : kind === "music" ? view.defaults?.music ?? null : null;
}

/** Asks before installing a model whose license limits its use. */
function licenseOk(m: MediaModelCard): boolean {
  if (m.license.commercial) return true;
  return window.confirm(
    t("{name}'s license ({license}) allows personal, non-commercial use only. {note}Read it on the model's page before using what it makes. Install it?", {
      name: m.name,
      license: m.license.name,
      note: m.license.note ? `${m.license.note} ` : "",
    }),
  );
}

type HubKind = "image" | "video" | "music";
const HUB_KINDS: { kind: HubKind; title: string; per: string; hint: string }[] = [
  { kind: "image", title: tx("Pictures"), per: tx("a picture"), hint: tx("Search, e.g. realistic, juggernaut, anime") },
  { kind: "video", title: tx("Video"), per: tx("a 3-second clip"), hint: tx("Search, e.g. wan, gguf") },
  { kind: "music", title: tx("Music"), per: tx("a 30-second song"), hint: tx("Search, e.g. ace-step, xl") },
];
const compact = new Intl.NumberFormat(undefined, { notation: "compact" });
const baseName = (path: string) => path.split("/").pop() ?? path;

/** Picture, video and music models people share on Hugging Face, built on a
 * kind SulcusAI runs (SDXL, Z-Image, FLUX.2 klein, Qwen Image, Wan,
 * ACE-Step). Nothing is asked of huggingface.co until the person searches. */
function MediaHubBrowser({ first, installed, progress, toast, onChanged }: { first: HubKind; installed: Set<string>; progress: Record<string, InstallProgress>; toast: PushToast; onChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const [kind, setKind] = useState<HubKind>(first);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<"downloads" | "trending" | "newest">("downloads");
  const [results, setResults] = useState<HubMediaResults | null>(null);
  const [searching, setSearching] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [allOf, setAllOf] = useState<string | null>(null);
  const busy = Object.keys(progress).length > 0;
  const per = HUB_KINDS.find((k) => k.kind === kind)!;

  const search = async (k = kind, by = sort) => {
    setSearching(true);
    setProblem(null);
    try {
      setResults(await api.mediaHubSearch(k, query, by));
    } catch (e) {
      setProblem(errorText(e));
    } finally {
      setSearching(false);
    }
  };
  const install = async (m: HubMediaModel, f: HubMediaFile) => {
    const ok = window.confirm(
      t("{name} is shared on Hugging Face by {author}{license}. Check what its license allows on the model's page before using what it makes for work. Install it?", {
        name: m.name,
        author: m.author,
        license: m.license ? t(" under the license “{license}”", { license: m.license }) : "",
      }),
    );
    if (!ok) return;
    try {
      await api.mediaHubInstall(m.id, f.file);
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  if (!open) {
    return (
      <section className="media-hub closed">
        <button className="btn ghost small" onClick={() => setOpen(true)}>
          {t("More models from Hugging Face…")}
        </button>
      </section>
    );
  }

  const row = (m: HubMediaModel, f: HubMediaFile) => {
    const p = progress[f.model_id];
    return (
      <div key={f.file} className="hub-file">
        <div className="hub-file-what">
          <strong>{f.quant}</strong>
          {f.file === m.recommended && <span className="badge" title={t("The best fit for this PC")}>★ {t("Best fit")}</span>}
          <span className="muted small ellipsis" title={f.file}>{baseName(f.file)}</span>
          {f.runnable ? (
            <span className="muted small block">
              {f.on_gpu ? t("Runs on your graphics card") : t("Runs on your processor")} · {t("{time} for {what}", { time: waitLabel(f.est_secs), what: t(per.per) })} ·{" "}
              {f.download < f.size ? t("{size} to download (shares files you have)", { size: bytes(f.download) }) : t("{size} download", { size: bytes(f.download) })}
            </span>
          ) : (
            <span className="warn-text small block">{f.reason ? t("This PC can't run it: it {reason}.", { reason: f.reason }) : t("too much for this PC")}</span>
          )}
        </div>
        {p ? (
          <InstallBar p={p} />
        ) : installed.has(f.model_id) ? (
          <span className="badge subtle">{t("Installed")}</span>
        ) : (
          <button className="btn small" onClick={() => install(m, f)} disabled={busy || !f.runnable} title={busy ? t("One install at a time") : undefined}>
            {t("Install")}
          </button>
        )}
      </div>
    );
  };

  return (
    <section className="card hub media-hub">
      <div className="look-row">
        <div>
          <strong>{t("Hugging Face")}</strong>
          <p className="muted small">
            {t("Picture, video and music models people share, built on the kinds SulcusAI runs (SDXL, Z-Image, FLUX.2 klein, Qwen Image, Wan and ACE-Step). Only versions this PC can run are shown. Searching asks huggingface.co, which sees this PC's address like any website; downloads come straight from there.")}
          </p>
        </div>
      </div>
      <div className="mode-switch small" role="tablist">
        {HUB_KINDS.map((k) => (
          <button
            key={k.kind}
            role="tab"
            aria-selected={kind === k.kind}
            className={`mode ${kind === k.kind ? "active" : ""}`}
            onClick={() => {
              setKind(k.kind);
              setResults(null);
              setAllOf(null);
            }}
          >
            {t(k.title)}
          </button>
        ))}
      </div>
      <form
        className="row wrap hub-search"
        onSubmit={(e) => {
          e.preventDefault();
          search();
        }}
      >
        <input className="input" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t(per.hint)} aria-label={t("Search Hugging Face")} />
        <select
          className="input"
          value={sort}
          aria-label={t("Order")}
          onChange={(e) => {
            const by = e.target.value as typeof sort;
            setSort(by);
            if (results) search(kind, by);
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
            {results.hidden > 0 && t("{n} more were left out: too big for this PC, add-ons, or in formats SulcusAI can't use.", { n: results.hidden })}
          </p>
          <div className="hub-list">
            {results.models.map((m) => {
              const best = m.files.find((f) => f.file === m.recommended) ?? m.files[m.files.length - 1];
              const shown = allOf === m.id ? m.files : [best];
              return (
                <article key={m.id} className="hub-model">
                  <div className="hub-head">
                    <div>
                      <a href={m.url} target="_blank" rel="noreferrer"><strong>{m.name}</strong></a> <span className="muted small">{t("by {author}", { author: m.author })}</span>{" "}
                      <span className="badge subtle" title={t("Built on {model}", { model: m.base_model })}>{m.family}</span>
                    </div>
                    <span className="muted small">
                      {t("{n} downloads", { n: compact.format(m.downloads) })} · {t("{n} likes", { n: compact.format(m.likes) })}
                      {m.license && ` · ${m.license}`}
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

/** What a model can make, as badges: pictures, edits, video, music… */
function abilities(m: MediaModelCard): string[] {
  const can = (op: string) => m.can.includes(op);
  switch (m.kind) {
    case "image":
      return [
        can("generate") ? tx("Makes pictures") : null,
        can("edit") ? tx("Edits pictures") : null,
        can("restyle") ? tx("Restyles pictures") : null,
        can("fill") || can("extend") ? tx("Fills in and extends") : null,
      ].filter((x): x is string => !!x);
    case "video":
      return [tx("Makes video"), ...(can("image") ? [tx("Brings a picture to life")] : [])];
    case "music":
      return [tx("Makes music and sound")];
    case "upscale":
      return [tx("Makes pictures bigger and sharper")];
    case "background":
      return [tx("Removes backgrounds")];
    default:
      return [];
  }
}

function MediaModelRow({
  m,
  per,
  progress,
  busy,
  toast,
  onChanged,
  isDefault,
  kept,
  onKept,
}: {
  m: MediaModelCard;
  per: string;
  progress?: InstallProgress;
  busy: boolean;
  toast: PushToast;
  onChanged: () => void;
  isDefault: boolean;
  kept: boolean;
  onKept: (models: string[]) => void;
}) {
  const defaultable = m.installed && (m.kind === "image" || m.kind === "video" || m.kind === "music");
  const [removing, setRemoving] = useState(false);
  const size = m.files.reduce((s, f) => s + f.size, 0);
  return (
    <article className="card media-model">
      <div className="row">
        <div className="grow">
          <strong>{m.name}</strong> <span className="muted small">· {m.publisher}</span>
          {m.from_hub && (
            <a className="badge subtle" href={m.source} target="_blank" rel="noreferrer" title={m.from_hub}>
              {t("From Hugging Face")}
            </a>
          )}
          {!m.license.commercial && <span className="badge subtle" title={m.license.note ?? undefined}>{t("Non-commercial")}</span>}
          {isDefault && <span className="badge" title={t("Chats and the Studio use this model unless you pick another")}>★ {t("Default")}</span>}
          {m.installed && (
            <button
              className={`icon-btn keep-btn ${kept ? "on" : ""}`}
              aria-pressed={kept}
              title={kept ? t("Locked: the automatic cleanup never removes it. Click to unlock.") : t("Lock it so the automatic cleanup never removes it")}
              onClick={() => api.setKept("model", m.id, !kept).then((l) => onKept(l.models)).catch((e) => toast(errorText(e), "error"))}
            >
              <LockIcon open={!kept} />
            </button>
          )}
          <span className="cap-badges">
            {abilities(m).map((a) => (
              <span key={a} className="badge subtle">{t(a)}</span>
            ))}
          </span>
          <span className="small block">{m.description}</span>
          {!!m.strengths?.length && (
            <span className="small block">
              <strong>{t("Good at:")}</strong> {m.strengths.join(" · ")}
            </span>
          )}
          {!!m.weaknesses?.length && (
            <span className="small block muted">
              <strong>{t("Less good at:")}</strong> {m.weaknesses.join(" · ")}
            </span>
          )}
          <span className="small muted block">
            {m.fit.on_gpu ? t("Runs on your graphics card") : t("Runs on your processor")} · {t("{time} for {what}", { time: waitLabel(m.fit.est_secs), what: t(per) })} ·{" "}
            {m.installed
              ? t("{size} on disk", { size: bytes(size) })
              : m.fit.needed_bytes < size
                ? t("{size} to download (shares files you have)", { size: bytes(m.fit.needed_bytes) })
                : t("{size} download", { size: bytes(size) })}{" "}
            ·{" "}
            <a href={m.license.url} target="_blank" rel="noreferrer">{m.license.name}</a>
          </span>
        </div>
        {defaultable && !isDefault && (
          <button
            className="btn ghost small"
            title={t("Chats and the Studio use this model unless you pick another")}
            onClick={() => {
              // The Studio's remembered pick for this kind gives way to the new default.
              try {
                localStorage.removeItem(`sulcus.studio.model.${m.kind}`);
              } catch {
                /* nothing kept */
              }
              api.mediaSetDefault(m.kind as "image" | "video" | "music", m.id).then(onChanged).catch((e) => toast(errorText(e), "error"));
            }}
          >
            {t("Make default")}
          </button>
        )}
        {m.installed ? (
          <button
            className="btn ghost danger small"
            disabled={removing}
            onClick={async () => {
              if (!window.confirm(t("Delete {name} from this PC? Its files ({size}) are deleted, except ones another installed model shares. You can install it again any time.", { name: m.name, size: bytes(size) }))) return;
              setRemoving(true);
              try {
                const freed = await api.removeMedia(m.id);
                toast(t("Deleted {name} from this PC ({size} freed).", { name: m.name, size: bytes(freed) }), "success");
                onChanged();
              } catch (e) {
                toast(errorText(e), "error");
              } finally {
                setRemoving(false);
              }
            }}
          >
            {t("Delete from this PC")}
          </button>
        ) : !m.fit.runnable || !m.fit.disk_ok ? (
          <span className="small warn-text">{m.fit.reason ? t("This PC can't run it: it {why}.", { why: m.fit.reason }) : t("Not enough free disk space for it.")}</span>
        ) : (
          !progress && (
            <button className="btn primary small" disabled={busy} title={busy ? t("One install at a time") : undefined} onClick={() => licenseOk(m) && api.installMedia(m.id).catch((e) => toast(errorText(e), "error"))}>
              {t("Install")}
            </button>
          )
        )}
      </div>
      {progress && <InstallBar p={progress} />}
    </article>
  );
}

function InstallBar({ p }: { p: InstallProgress }) {
  const label = p.phase === "engine" ? t("Setting up the engine") : p.phase === "download" ? t("Downloading") : t("Checking");
  const pct = p.total > 0 ? percent(p.received, p.total) : null;
  return (
    <div className="install-progress">
      <div className="progress-label">
        <span>{label}{pct !== null ? ` · ${t("{received} of {total}", { received: bytes(p.received), total: bytes(p.total) })}` : "…"}</span>
        <CancelInstall p={p} />
      </div>
      <div className={`progress ${pct === null ? "indeterminate" : ""}`}>
        <span style={{ width: pct === null ? undefined : `${pct}%` }} />
      </div>
    </div>
  );
}
