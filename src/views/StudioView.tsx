// SPDX-License-Identifier: AGPL-3.0-only
// The studio: make pictures, video, music and narration on this PC, and
// keep everything in an encrypted gallery.
import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorText,
  on,
  type FeatureId,
  type InstallProgress,
  type MediaItem,
  type MediaKind,
  type CloudMediaChoice,
  type MediaModelCard,
  type MediaRequest,
  type Voice,
} from "../api";
import { clock } from "../format";
import { t, tx } from "../i18n";
import { forget, OP_LABEL, useMediaUrl, waitLabel } from "../media";
import { AudioPlayer, JobCard, MadeStatsLine, MediaTile, saveAs, useMediaJobs } from "../components/MediaBits";
import { MediaModels, useMediaView } from "../components/MediaModels";
import { MaskPainter, type MaskHandle } from "../components/MaskPainter";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { asksFirst, ExamplePicker, PictureQuestions, PromptHelpPanel, SetWallpaperButton, useAskFirst, usePromptHelp, WallpaperComposer, type ExampleUse } from "../components/StudioExtras";

type Tab = "pictures" | "video" | "wallpaper" | "music" | "narrate";
type Shape = NonNullable<MediaRequest["shape"]>;

const TAB_FEATURE: Record<Tab, FeatureId> = { pictures: "images", video: "video", wallpaper: "images", music: "music", narrate: "music" };
const TAB_KIND: Partial<Record<Tab, MediaKind>> = { pictures: "image", video: "video", wallpaper: "image", music: "music" };

const STYLES = [tx("Watercolor"), tx("Oil painting"), tx("Pencil sketch"), tx("Anime"), tx("3D render"), tx("Pixel art"), tx("Vintage photo"), tx("Comic book")];
const LANGUAGES: [string, string][] = [
  ["auto", tx("Any language")],
  ["en", tx("English")],
  ["es", tx("Spanish")],
  ["fr", tx("French")],
  ["de", tx("German")],
  ["it", tx("Italian")],
  ["pt", tx("Portuguese")],
  ["ja", tx("Japanese")],
  ["ko", tx("Korean")],
  ["zh", tx("Chinese")],
];

interface Props {
  features: Set<FeatureId>;
  progress: Record<string, InstallProgress>;
  toast: PushToast;
  onGoFeatures: () => void;
}

export function StudioView({ features, progress, toast, onGoFeatures }: Props) {
  const tabs: Tab[] = (["pictures", "video", "wallpaper", "music", "narrate"] as Tab[]).filter((tb) => features.has(TAB_FEATURE[tb]));
  const [tab, setTab] = useState<Tab>(tabs[0] ?? "pictures");
  const [view, reloadView] = useMediaView();
  const jobs = useMediaJobs();
  const [items, setItems] = useState<MediaItem[]>([]);
  const [more, setMore] = useState(false);
  const [filter, setFilter] = useState<"all" | "image" | "video" | "audio" | "favorites">("all");
  const [viewing, setViewing] = useState<MediaItem | null>(null);
  const [models, setModels] = useState<MediaKind[] | null>(null);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  const [source, setSource] = useState<MediaItem | null>(null);

  const load = useCallback(async () => {
    const kind = filter === "all" || filter === "favorites" ? null : filter;
    const list = await api.mediaList(kind, null, 120);
    setItems(filter === "favorites" ? list.filter((i) => i.favorite) : list);
    setMore(list.length === 120);
  }, [filter]);

  useEffect(() => {
    load().catch((e) => toast(errorText(e), "error"));
  }, [load, toast]);

  useEffect(() => {
    const sub = on("media:done", (d) => {
      if (d.ok && d.items?.length) {
        setFresh((s) => new Set([...s, ...d.items!.map((i) => i.id)]));
        load();
      } else if (d.error) toast(errorText(d.error), "error");
    });
    const tuned = on("media:tuned", (r) =>
      toast(t("The prompt writer learned this model: {before}% → {after}% of what was asked for showed up.", { before: Math.round(r.baseline * 100), after: Math.round(r.score * 100) }), "success"),
    );
    return () => {
      sub.then((un) => un());
      tuned.then((un) => un());
    };
  }, [load, toast]);

  useEffect(() => {
    if (!tabs.includes(tab) && tabs[0]) setTab(tabs[0]);
  }, [tabs, tab]);

  const start = async (req: MediaRequest) => {
    try {
      await api.mediaStart(req);
      return true;
    } catch (e) {
      toast(errorText(e), "error");
      return false;
    }
  };

  const loadMore = async () => {
    const last = items[items.length - 1];
    if (!last) return;
    const kind = filter === "all" || filter === "favorites" ? null : filter;
    const next = await api.mediaList(kind, last.created_at, 120);
    setItems((all) => [...all, ...(filter === "favorites" ? next.filter((i) => i.favorite) : next)]);
    setMore(next.length === 120);
  };

  const importFile = async () => {
    const picked = await openFile({
      multiple: false,
      filters: [{ name: t("Pictures, clips and sounds"), extensions: ["png", "jpg", "jpeg", "webp", "webm", "mp4", "mp3", "wav", "ogg"] }],
    });
    if (typeof picked !== "string") return;
    try {
      const item = await api.mediaImport(picked);
      await load();
      setViewing(item);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  if (tabs.length === 0) {
    return (
      <div className="page">
        <div className="narrow">
          <h1>{t("Studio")}</h1>
          <p className="muted">{t("Turn on Pictures, Video or Music and audio in Features to start making things here.")}</p>
          <button className="btn primary" onClick={onGoFeatures}>{t("Open Features")}</button>
        </div>
      </div>
    );
  }

  const installedOf = (kind: MediaKind) => [...(view?.models ?? []).filter((m) => m.kind === kind && m.installed), ...cloudCards(view?.cloud ?? [], kind)];
  const kind = TAB_KIND[tab];
  const needsModel = kind && view && installedOf(kind).length === 0;

  return (
    <div className="page studio">
      <header className="page-head">
        <div>
          <h1>{t("Studio")}</h1>
          <p className="muted">{t("Everything is made on this PC and kept encrypted in your gallery.")}</p>
        </div>
        <div className="row">
          <button className="btn" onClick={importFile}>{t("Import…")}</button>
          <button className="btn" onClick={() => setModels(null === models ? (kind ? [kind, ...(kind === "image" ? (["upscale", "background"] as MediaKind[]) : [])] : ["image", "video", "music"]) : null)}>
            {t("Models")}
          </button>
        </div>
      </header>

      <div className="mode-switch studio-tabs" role="tablist">
        {tabs.map((tb) => (
          <button key={tb} role="tab" aria-selected={tab === tb} className={`mode ${tab === tb ? "active" : ""}`} onClick={() => setTab(tb)}>
            {{ pictures: t("Pictures"), video: t("Video"), wallpaper: t("Wallpapers"), music: t("Music and sound"), narrate: t("Narration") }[tb]}
          </button>
        ))}
      </div>

      <section className="card studio-composer">
        {needsModel ? (
          <>
            <p>
              {tab === "pictures"
                ? t("Pick a model to install for pictures. It downloads once and runs on this PC.")
                : tab === "video"
                  ? t("Pick a model to install for video. It downloads once and runs on this PC.")
                  : t("Pick a model to install for music. It downloads once and runs on this PC.")}
            </p>
            <MediaModels progress={progress} toast={toast} only={[kind!]} />
          </>
        ) : tab === "pictures" ? (
          <PictureComposer
            key={view?.defaults?.image ?? "no-default"}
            models={installedOf("image")}
            defaultId={view?.defaults?.image ?? null}
            onSetDefault={(id) => api.mediaSetDefault("image", id).then(reloadView).catch(() => {})}
            onStart={start}
            onEditPhoto={importFile}
            lifelike={view?.models.find((m) => m.id === "z-image-turbo" && !m.installed && m.fit.runnable) ?? null}
            onModels={() => setModels(["image"])}
            gallery={items}
            toast={toast}
          />
        ) : tab === "video" ? (
          <VideoComposer
            key={view?.defaults?.video ?? "no-default"}
            models={installedOf("video")}
            defaultId={view?.defaults?.video ?? null}
            onSetDefault={(id) => api.mediaSetDefault("video", id).then(reloadView).catch(() => {})}
            source={source}
            onPickSource={setSource}
            gallery={items}
            onStart={start}
            toast={toast}
          />
        ) : tab === "wallpaper" ? (
          <WallpaperComposer imageModels={installedOf("image")} videoModels={features.has("video") ? installedOf("video") : []} gallery={items} onStart={start} toast={toast} />
        ) : tab === "music" ? (
          <MusicComposer key={view?.defaults?.music ?? "no-default"} models={installedOf("music")} defaultId={view?.defaults?.music ?? null} onSetDefault={(id) => api.mediaSetDefault("music", id).then(reloadView).catch(() => {})} onStart={start} />
        ) : (
          <NarrateComposer cloud={(view?.cloud ?? []).filter((c) => c.kind === "speech")} onStart={start} />
        )}
      </section>

      {jobs.length > 0 && (
        <section className="studio-jobs">
          {jobs.map((j) => (
            <JobCard key={j.id} job={j} />
          ))}
        </section>
      )}

      <section>
        <div className="filters">
          {(["all", "image", "video", "audio", "favorites"] as const).map((f) => (
            <button key={f} className={`chip ${filter === f ? "active" : ""}`} onClick={() => setFilter(f)}>
              {{ all: t("Everything"), image: t("Pictures"), video: t("Video"), audio: t("Sound"), favorites: t("★ Favorites") }[f]}
            </button>
          ))}
          <span className="spacer" />
          {view && <span className="small muted">{t("{n} in the gallery", { n: view.gallery })}</span>}
        </div>
        {items.length === 0 ? (
          <p className="muted small">{t("Nothing here yet. What you make appears here.")}</p>
        ) : (
          <div className="media-grid">
            {items.map((i) => (
              <MediaTile key={i.id} item={i} selected={fresh.has(i.id)} onOpen={() => setViewing(i)} />
            ))}
          </div>
        )}
        {more && (
          <div className="center pad">
            <button className="btn" onClick={loadMore}>{t("Show more")}</button>
          </div>
        )}
      </section>

      {viewing && (
        <Viewer
          item={viewing}
          features={features}
          imageModels={installedOf("image")}
          toast={toast}
          onClose={() => setViewing(null)}
          onOpen={setViewing}
          onStart={async (req) => {
            if (await start(req)) {
              setViewing(null);
              toast(t("Started. It appears in the gallery when it's done."), "success");
            }
          }}
          onAnimate={(i) => {
            setSource(i);
            setTab("video");
            setViewing(null);
          }}
          onChanged={(i) => {
            setItems((all) => all.map((x) => (x.id === i.id ? i : x)));
            setViewing(i);
          }}
          onDeleted={(id) => {
            forget(id);
            setItems((all) => all.filter((x) => x.id !== id));
            setViewing(null);
            reloadView();
          }}
          onSaved={() => load()}
        />
      )}

      {models && (
        <Modal
          title={t("Models for the studio")}
          onClose={() => {
            setModels(null);
            reloadView();
          }}
        >
          <div className="studio-models">
            <MediaModels progress={progress} toast={toast} />
          </div>
          <div className="modal-actions">
            <button
              className="btn"
              onClick={() => {
                setModels(null);
                reloadView();
              }}
            >
              {t("Close")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/** Cloud models shown alongside this PC's in the model menus. */
function cloudCards(cloud: CloudMediaChoice[], kind: MediaKind): MediaModelCard[] {
  const want = kind === "image" ? "image" : kind === "video" ? "video" : null;
  return cloud
    .filter((c) => c.kind === want)
    .map((c) => {
      const edits = /:(gpt-image|dall-e-2)/.test(c.id);
      return {
        id: c.id,
        name: `${c.name} (${c.provider})`,
        publisher: c.provider,
        source: "",
        description: "",
        license: { name: t("{provider}'s terms", { provider: c.provider }), url: "", commercial: true, note: null },
        kind,
        can: kind === "image" ? (edits ? ["generate", "edit", "fill", "restyle"] : ["generate"]) : ["text", "image"],
        files: [],
        quality: 0,
        defaults: { steps: 0, width: 1024, height: 1024, fps: 24, seconds: 4 },
        fit: { runnable: true, on_gpu: false, est_secs: kind === "video" ? 180 : 20, reason: null, disk_ok: true, need_bytes: 0 } as unknown as MediaModelCard["fit"],
        installed: true,
      };
    });
}

function ModelPick({ models, value, onChange }: { models: MediaModelCard[]; value: string; onChange: (id: string) => void }) {
  // Models on this PC are marked as such; cloud ones carry their provider in their names.
  const label = (m: MediaModelCard) => (m.id.startsWith("cloud:") ? m.name : t("{name} (this PC)", { name: m.name }));
  if (models.length === 1) return <span className="small muted" title={t("Model")}>{label(models[0])}</span>;
  if (models.length < 2) return null;
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} title={t("Model")}>
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {label(m)}
        </option>
      ))}
    </select>
  );
}

function best(models: MediaModelCard[], can?: string): string {
  return [...models].filter((m) => !can || m.can.includes(can)).sort((a, b) => b.quality - a.quality)[0]?.id ?? "";
}

/** The model a composer uses: the one last picked on this tab (remembered,
 * so leaving the tab doesn't undo it), else the default set for this kind,
 * else the best installed one. Worked out on every render, so it's right
 * once the model list arrives too. */
function useModelPick(kind: string, models: MediaModelCard[], defaultId: string | null): [string, (id: string) => void] {
  const key = `sulcus.studio.model.${kind}`;
  const [chosen, setChosen] = useState<string | null>(() => {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  });
  const resolved = [chosen, defaultId].find((id) => id && models.some((m) => m.id === id)) ?? best(models);
  const pick = (id: string) => {
    setChosen(id);
    try {
      localStorage.setItem(key, id);
    } catch {
      /* not kept */
    }
  };
  return [resolved, pick];
}

/** "Make this the default" beside the model picker (models on this PC only). */
function DefaultLink({ id, defaultId, onSetDefault }: { id: string | undefined; defaultId: string | null; onSetDefault: (id: string) => void }) {
  if (!id || id.startsWith("cloud:")) return null;
  if (id === defaultId) return <span className="badge subtle" title={t("Chats use this model too, unless you ask for another")}>★ {t("Default")}</span>;
  return (
    <button className="link small" title={t("Chats use the default too, unless you ask for another")} onClick={() => onSetDefault(id)}>
      {t("Make default")}
    </button>
  );
}

/** Looks for new pictures (ids match the core's STYLES). Auto: a realistic
 * photo unless the description names another look. */
const LOOKS: { id: string; label: string }[] = [
  { id: "auto", label: tx("Auto (realistic unless you say)") },
  { id: "photo", label: tx("Realistic photo") },
  { id: "phone_photo", label: tx("Phone photo (most lifelike)") },
  { id: "cinematic", label: tx("Cinematic") },
  { id: "cartoon", label: tx("Cartoon") },
  { id: "anime", label: tx("Anime") },
  { id: "digital_art", label: tx("Digital art") },
  { id: "oil_painting", label: tx("Oil painting") },
  { id: "watercolor", label: tx("Watercolor") },
  { id: "sketch", label: tx("Pencil sketch") },
  { id: "3d", label: tx("3D render") },
  { id: "pixel_art", label: tx("Pixel art") },
];

/** "Improve": the chat model turns a short idea into a fuller description. */
function ImproveButton({ prompt, kind, onDone }: { prompt: string; kind: string; onDone: (p: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [why, setWhy] = useState<string | null>(null);
  return (
    <>
      <button
        className="btn ghost small"
        disabled={!prompt.trim() || busy}
        title={t("Your chat model writes a fuller description from your idea. You can edit it before making anything.")}
        onClick={async () => {
          setBusy(true);
          setWhy(null);
          try {
            onDone(await api.improveMediaPrompt(prompt, kind));
          } catch (e) {
            setWhy(errorText(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? t("Improving…") : t("Improve")}
      </button>
      {why && <span className="small warn-text">{why}</span>}
    </>
  );
}

function PictureComposer({
  models,
  onStart,
  onEditPhoto,
  lifelike,
  onModels,
  defaultId,
  onSetDefault,
  gallery,
  toast,
}: {
  models: MediaModelCard[];
  onStart: (r: MediaRequest) => Promise<boolean>;
  onEditPhoto: () => void;
  /** A model that makes more lifelike photos, not installed yet. */
  lifelike: MediaModelCard | null;
  onModels: () => void;
  defaultId: string | null;
  onSetDefault: (id: string) => void;
  gallery: MediaItem[];
  toast: PushToast;
}) {
  const [prompt, setPrompt] = useState("");
  const [shape, setShape] = useState<Shape>("square");
  const [count, setCount] = useState(1);
  const [style, setStyle] = useState("auto");
  const [model, setModel] = useModelPick("image", models, defaultId);
  const m = models.find((x) => x.id === model) ?? models[0];
  const canEdit = models.some((x) => x.can.includes("edit"));
  const [examples, setExamples] = useState<MediaItem[]>([]);
  const [use, setUse] = useState<ExampleUse>("style");
  const help = usePromptHelp(m?.id);
  const ask = useAskFirst();
  const local = !!m && !m.id.startsWith("cloud:");
  const make = async (description: string) => {
    const extra: Partial<MediaRequest> = local ? { examples: examples.map((e) => e.id), example_use: use, write_prompt: help.write, check: help.check } : {};
    if (await onStart({ op: "generate", prompt: description, shape, count, style, model_id: m?.id, ...extra })) {
      setPrompt("");
      setExamples([]);
      ask.clear();
    }
  };
  const go = async () => {
    if (ask.asking) return;
    if (help.ask && !(await ask.check("generate", prompt, null, m?.id ?? null, local ? examples.length : 0))) return;
    await make(prompt);
  };
  return (
    <div className="form">
      {ask.questions && <PictureQuestions key={ask.questions.join("\n")} questions={ask.questions} prompt={prompt} onMake={make} />}
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder={t("Describe the picture: the subject, the setting, the light and mood. Pick a look below.")}
        onChange={(e) => {
          setPrompt(e.target.value);
          // Rewording it asks afresh on Create.
          ask.clear();
        }}
        onKeyDown={(e) => e.key === "Enter" && (e.ctrlKey || e.metaKey) && prompt.trim() && go()}
        autoFocus
      />
      <div className="row wrap">
        <div className="filters tight">
          {(["square", "portrait", "landscape", "wide", "tall"] as Shape[]).map((s) => (
            <button key={s} className={`chip ${shape === s ? "active" : ""}`} onClick={() => setShape(s)}>
              {{ square: t("◻ Square"), portrait: t("▯ Portrait"), landscape: t("▭ Landscape"), wide: t("▬ Wide"), tall: t("▮ Tall") }[s]}
            </button>
          ))}
        </div>
        <select value={style} onChange={(e) => setStyle(e.target.value)} title={t("Look")} aria-label={t("Look")}>
          {LOOKS.map((l) => (
            <option key={l.id} value={l.id}>{t(l.label)}</option>
          ))}
        </select>
        <select value={count} onChange={(e) => setCount(+e.target.value)} title={t("How many")}>
          {[1, 2, 3, 4].map((n) => (
            <option key={n} value={n}>{n === 1 ? t("1 picture") : t("{n} versions", { n })}</option>
          ))}
        </select>
        <ModelPick models={models} value={m?.id ?? ""} onChange={setModel} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        <ImproveButton prompt={prompt} kind="image" onDone={setPrompt} />
        {m && <span className="small muted">{waitLabel(m.fit.est_secs * count)}</span>}
        <button className="btn primary" disabled={!prompt.trim() || ask.asking} onClick={go}>{ask.asking ? t("Checking…") : t("Create")}</button>
      </div>
      {local && <ExamplePicker value={examples} onChange={setExamples} use={use} onUse={setUse} gallery={gallery} toast={toast} canRead={!!help.status?.reader} />}
      <PromptHelpPanel model={m} help={help} onStart={onStart} toast={toast} canAsk />
      {lifelike && (style === "auto" || style === "photo" || style === "phone_photo") && (
        <p className="small callout">
          {t("For the most lifelike photos, add {name}: it makes pictures that look like real photographs.", { name: lifelike.name })}{" "}
          <button className="link" onClick={onModels}>{t("Open Models")}</button>
        </p>
      )}
      <p className="small muted">
        {canEdit ? (
          <>
            {t("Have a photo to change? ")}
            <button className="link" onClick={onEditPhoto}>{t("Edit a photo…")}</button>{" "}
            {t("Open it, then describe the change, like “add a top hat” or “put us at a party”.")}
          </>
        ) : (
          t("To edit your own photos (add a hat, change the background), install a picture model that edits, such as FLUX.2 klein, from Models.")
        )}
      </p>
    </div>
  );
}

function VideoComposer({
  models,
  defaultId,
  onSetDefault,
  source,
  onPickSource,
  gallery,
  onStart,
  toast,
}: {
  models: MediaModelCard[];
  defaultId: string | null;
  onSetDefault: (id: string) => void;
  source: MediaItem | null;
  onPickSource: (i: MediaItem | null) => void;
  gallery: MediaItem[];
  onStart: (r: MediaRequest) => Promise<boolean>;
  toast: PushToast;
}) {
  const [prompt, setPrompt] = useState("");
  const [seconds, setSeconds] = useState(3);
  const [customLength, setCustomLength] = useState(false);
  const [shape, setShape] = useState<Shape>("landscape");
  const [model, setModel] = useModelPick("video", models, defaultId);
  const [picking, setPicking] = useState(false);
  const usable = source ? models.filter((m) => m.can.includes("image")) : models;
  const m = usable.find((x) => x.id === model) ?? usable[0];
  const thumb = useMediaUrl(source, true);
  const help = usePromptHelp(m?.id);
  // What the model makes at once; longer clips are made in parts, each going
  // on from the last frame of the one before (models that start from a picture).
  const once = m ? Math.max(1, Math.floor(((m.defaults.max_frames || 81) - 1) / Math.max(8, m.defaults.fps || 16))) : 5;
  const continues = !!m && m.can.includes("image") && !m.id.startsWith("cloud:");
  const most = continues ? 600 : once;
  const length = Math.min(Math.max(1, seconds), most);
  const parts = Math.ceil(length / once);
  const importPicture = async () => {
    const picked = await openFile({ multiple: false, filters: [{ name: t("Pictures"), extensions: ["png", "jpg", "jpeg", "webp"] }] });
    if (typeof picked !== "string") return;
    try {
      onPickSource(await api.mediaImport(picked));
      setPicking(false);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const go = async () => {
    const write = m && !m.id.startsWith("cloud:") && prompt.trim() ? { write_prompt: help.write } : {};
    if (await onStart({ op: "video", prompt, seconds: length, shape, source: source?.id ?? null, model_id: m?.id, ...write })) {
      setPrompt("");
      onPickSource(null);
    }
  };
  return (
    <div className="form">
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder={source ? t("How should the picture move? E.g. \"the camera slowly pushes in, leaves drift in the wind\"") : t("Describe the clip: the subject, what happens, the camera and the style.")}
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />
      <div className="row wrap">
        {source ? (
          <span className="chip active row">
            {thumb && <img src={thumb} alt="" className="chip-thumb" />} {t("Starts from this picture")}
            <button className="link" onClick={() => onPickSource(null)}>✕</button>
          </span>
        ) : (
          <button className="btn small" onClick={() => setPicking(true)} title={t("Turn a picture into a video: it starts on your picture and comes to life")}>{t("Start from a picture…")}</button>
        )}
        {!source && (
          <div className="filters tight">
            {(["landscape", "portrait", "square"] as Shape[]).map((s) => (
              <button key={s} className={`chip ${shape === s ? "active" : ""}`} onClick={() => setShape(s)}>
                {({ landscape: t("▭ Landscape"), portrait: t("▯ Portrait"), square: t("◻ Square") } as Record<string, string>)[s]}
              </button>
            ))}
          </div>
        )}
        <select
          value={customLength ? "custom" : String(length)}
          onChange={(e) => {
            if (e.target.value === "custom") return setCustomLength(true);
            setCustomLength(false);
            setSeconds(+e.target.value);
          }}
          title={t("Length")}
        >
          {[2, 3, 5, 10, 20, 30, 60, 120].filter((s) => s <= most).map((s) => (
            <option key={s} value={s}>{s < 60 ? t("{s} seconds", { s }) : t("{m} min", { m: s / 60 })}</option>
          ))}
          {![2, 3, 5, 10, 20, 30, 60, 120].includes(length) && !customLength && <option value={length}>{t("{s} seconds", { s: length })}</option>}
          {continues && <option value="custom">{t("Any length…")}</option>}
        </select>
        {customLength && (
          <label className="check small">
            <input className="input tiny" type="number" min={1} max={most} value={seconds} onChange={(e) => setSeconds(Math.max(1, Math.min(most, +e.target.value || 1)))} aria-label={t("Seconds")} />
            {t("seconds")}
          </label>
        )}
        <ModelPick models={usable} value={m?.id ?? ""} onChange={setModel} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel((m.fit.est_secs * length) / Math.max(1, m.defaults.seconds || 3))}</span>}
        <button className="btn primary" disabled={(!prompt.trim() && !source) || !m} onClick={go}>{t("Make video")}</button>
      </div>
      {source && !m && <p className="small warn-text">{t("Bringing a picture to life needs Wan 2.2. Install it from Models.")}</p>}
      {parts > 1 && (
        <p className="small muted">
          {t("{model} makes about {n} seconds at a time, so this is made in {parts} parts, each going on from the last frame of the one before, then joined into one video.", { model: m?.name ?? "", n: once, parts })}
        </p>
      )}
      {!continues && m && seconds > once && <p className="small muted">{t("{model} makes clips up to {n} seconds. For longer ones, use Wan 2.2.", { model: m.name, n: once })}</p>}
      <PromptHelpPanel model={m} help={help} onStart={onStart} toast={toast} canCheck={false} />
      {picking && (
        <Modal title={t("Start from a picture")} onClose={() => setPicking(false)}>
          <div className="row">
            <button className="btn small" onClick={importPicture}>{t("From a file…")}</button>
            <span className="small muted">{t("Or pick one from the gallery:")}</span>
          </div>
          <div className="media-grid small-grid">
            {gallery.filter((i) => i.kind === "image").map((i) => (
              <MediaTile
                key={i.id}
                item={i}
                onOpen={() => {
                  onPickSource(i);
                  setPicking(false);
                }}
              />
            ))}
          </div>
          {!gallery.some((i) => i.kind === "image") && <p className="muted small">{t("No pictures in the gallery yet.")}</p>}
        </Modal>
      )}
    </div>
  );
}

function MusicComposer({ models, defaultId, onSetDefault, onStart }: { models: MediaModelCard[]; defaultId: string | null; onSetDefault: (id: string) => void; onStart: (r: MediaRequest) => Promise<boolean> }) {
  const [sound, setSound] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [vocals, setVocals] = useState<"write" | "none" | "mine">("write");
  const [lyrics, setLyrics] = useState("");
  const [seconds, setSeconds] = useState(30);
  const [language, setLanguage] = useState("auto");
  const [model, setModel] = useModelPick("music", models, defaultId);
  const m = models.find((x) => x.id === model) ?? models[0];
  const secs = sound ? Math.min(Math.max(seconds, 10), 30) : seconds;
  const go = async () => {
    const ok = await onStart({
      op: sound ? "sound" : "music",
      prompt,
      seconds: secs,
      instrumental: !sound && vocals === "none",
      lyrics: !sound && vocals === "mine" ? lyrics : null,
      language: language === "auto" ? null : language,
      model_id: m?.id,
    });
    if (ok) setPrompt("");
  };
  return (
    <div className="form">
      <div className="mode-switch">
        <button className={`mode ${!sound ? "active" : ""}`} onClick={() => setSound(false)}>{t("Song")}</button>
        <button className={`mode ${sound ? "active" : ""}`} onClick={() => setSound(true)}>{t("Sound effect")}</button>
      </div>
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder={sound ? t("Describe the sound, e.g. \"rain on a tin roof with distant thunder\"") : t("Describe the song: genre, mood, instruments, tempo, voice. E.g. \"upbeat acoustic folk about a road trip, warm male vocals\"")}
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />
      {!sound && (
        <div className="row wrap">
          <div className="filters tight">
            {(["write", "none", "mine"] as const).map((v) => (
              <button key={v} className={`chip ${vocals === v ? "active" : ""}`} onClick={() => setVocals(v)}>
                {{ write: t("Vocals, lyrics written for me"), none: t("Instrumental"), mine: t("My own lyrics") }[v]}
              </button>
            ))}
          </div>
          {vocals !== "none" && (
            <select value={language} onChange={(e) => setLanguage(e.target.value)} title={t("Language of the vocals")}>
              {LANGUAGES.map(([c, n]) => (
                <option key={c} value={c}>{t(n)}</option>
              ))}
            </select>
          )}
        </div>
      )}
      {!sound && vocals === "mine" && (
        <textarea className="input" rows={6} value={lyrics} placeholder={"[Verse]\nYour lines here\n\n[Chorus]\n…"} onChange={(e) => setLyrics(e.target.value)} />
      )}
      <div className="row wrap">
        <select value={secs} onChange={(e) => setSeconds(+e.target.value)} title={t("Length")}>
          {(sound ? [10, 15, 20, 30] : [15, 30, 60, 90, 120, 180]).map((s) => (
            <option key={s} value={s}>{s < 60 ? t("{s} seconds", { s }) : s % 60 ? t("{m} min {s} s", { m: s / 60, s: s % 60 }) : t("{m} min", { m: s / 60 })}</option>
          ))}
        </select>
        <ModelPick models={models} value={m?.id ?? ""} onChange={setModel} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel((m.fit.est_secs * secs) / 30)}</span>}
        <button className="btn primary" disabled={!prompt.trim() || (vocals === "mine" && !sound && !lyrics.trim())} onClick={go}>
          {sound ? t("Make the sound") : t("Make the song")}
        </button>
      </div>
      {sound && <p className="small muted">{t("Sound effects come from the music model, so they're rough sketches rather than library-quality recordings.")}</p>}
    </div>
  );
}

const CLOUD_VOICES = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer"];

function NarrateComposer({ cloud, onStart }: { cloud: CloudMediaChoice[]; onStart: (r: MediaRequest) => Promise<boolean> }) {
  const [text, setText] = useState("");
  const [voices, setVoices] = useState<Voice[]>([]);
  const [voice, setVoice] = useState("");
  const [engine, setEngine] = useState("");
  const [cloudVoice, setCloudVoice] = useState("alloy");
  useEffect(() => {
    api.voices().then((v) => {
      setVoices(v);
      if (v[0]) setVoice(v[0].id);
    }).catch(() => {});
  }, []);
  const go = async () => {
    const req: MediaRequest = engine ? { op: "narrate", prompt: text, voice: cloudVoice, model_id: engine } : { op: "narrate", prompt: text, voice: voice || null };
    if (await onStart(req)) setText("");
  };
  return (
    <div className="form">
      <textarea className="input" rows={6} value={text} placeholder={t("The text to read aloud. It's saved as a sound you can play, trim and save.")} onChange={(e) => setText(e.target.value)} autoFocus />
      <div className="row wrap">
        {cloud.length > 0 && (
          <select value={engine} onChange={(e) => setEngine(e.target.value)} title={t("Voices from")}>
            <option value="">{t("This PC's voices")}</option>
            {cloud.map((c) => (
              <option key={c.id} value={c.id}>{c.name} ({c.provider})</option>
            ))}
          </select>
        )}
        {engine ? (
          <select value={cloudVoice} onChange={(e) => setCloudVoice(e.target.value)} title={t("Voice")}>
            {CLOUD_VOICES.map((v) => (
              <option key={v} value={v}>{v[0].toUpperCase() + v.slice(1)}</option>
            ))}
          </select>
        ) : (
          <select value={voice} onChange={(e) => setVoice(e.target.value)} title={t("Voice")}>
            {voices.map((v) => (
              <option key={v.id} value={v.id}>{v.name}</option>
            ))}
          </select>
        )}
        <span className="spacer" />
        <button className="btn primary" disabled={!text.trim()} onClick={go}>{t("Read aloud")}</button>
      </div>
    </div>
  );
}

type Tool = "edit" | "fill" | "extend" | "restyle" | null;

function Viewer({
  item,
  features,
  imageModels,
  toast,
  onClose,
  onOpen,
  onStart,
  onAnimate,
  onChanged,
  onDeleted,
  onSaved,
}: {
  item: MediaItem;
  features: Set<FeatureId>;
  imageModels: MediaModelCard[];
  toast: PushToast;
  onClose: () => void;
  onOpen: (i: MediaItem) => void;
  onStart: (r: MediaRequest) => void;
  onAnimate: (i: MediaItem) => void;
  onChanged: (i: MediaItem) => void;
  onDeleted: (id: string) => void;
  onSaved: () => void;
}) {
  const src = useMediaUrl(item);
  const [tool, setTool] = useState<Tool>(null);
  const [text, setText] = useState("");
  const [brush, setBrush] = useState(40);
  const [painted, setPainted] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const painter = useRef<MaskHandle>(null);
  const can = (op: string) => imageModels.some((m) => m.can.includes(op));
  const ask = useAskFirst();

  useEffect(() => {
    setTool(null);
    setText("");
    ask.clear();
  }, [item.id]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && !confirm && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose, confirm]);

  const run = (req: Omit<MediaRequest, "source">) => onStart({ ...req, source: item.id });
  const change = async () => {
    if (!text.trim() || ask.asking) return;
    if (asksFirst() && !(await ask.check("edit", text, item.id, null))) return;
    run({ op: "edit", prompt: text });
  };
  const fill = async () => {
    const mask = await painter.current?.mask();
    if (!mask) return toast(t("Paint over the part to change first."), "error");
    run({ op: "fill", prompt: text, mask });
  };
  const extend = (sides: "wide" | "tall" | "all") => {
    const [w, h] = [item.width, item.height];
    const x = Math.round(w / 4);
    const y = Math.round(h / 4);
    const amounts: [number, number, number, number] = sides === "wide" ? [x, 0, x, 0] : sides === "tall" ? [0, y, 0, y] : [x, y, x, y];
    run({ op: "extend", prompt: text, extend: amounts });
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal viewer" role="dialog" aria-modal="true" aria-label={t("Gallery item")}>
        <div className="viewer-stage">
          {item.kind === "image" && src && (tool === "fill" ? (
            <MaskPainter ref={painter} src={src} width={item.width} height={item.height} brush={brush} onPainted={setPainted} />
          ) : (
            <img src={src} alt={item.prompt || t("Picture")} className={item.mime === "image/png" ? "checker" : ""} />
          ))}
          {item.kind === "video" && src && <video src={src} controls autoPlay loop playsInline />}
          {item.kind === "audio" && <AudioPlayer item={item} toast={toast} onSaved={(i) => { onSaved(); onOpen(i); }} />}
        </div>
        <aside className="viewer-side">
          <div className="row">
            <strong>{OP_LABEL[item.op] ? t(OP_LABEL[item.op]) : t("Made")}</strong>
            <span className="small muted">{new Date(item.created_at).toLocaleString()}</span>
            <span className="spacer" />
            <button className="icon-btn" aria-label={t("Close")} onClick={onClose}>✕</button>
          </div>
          {item.prompt && <p className="viewer-prompt small">{item.prompt}</p>}
          <p className="small muted">
            {item.kind === "image" ? `${item.width} × ${item.height}` : clock(item.seconds)}
            {item.kind === "video" && ` · ${item.width} × ${item.height}`}
            {item.seed != null && ` · ${t("seed {seed}", { seed: item.seed })}`}
          </p>
          <MadeStatsLine item={item} />
          {item.parent && (
            <button className="link small" onClick={() => api.mediaGet(item.parent!).then(onOpen).catch(() => toast(t("The original is no longer in the gallery."), "error"))}>
              {t("← The original")}
            </button>
          )}
          {item.lyrics && (
            <details className="small">
              <summary>{t("Lyrics")}</summary>
              <pre className="lyrics">{item.lyrics}</pre>
            </details>
          )}

          {item.kind === "image" && features.has("images") && (
            <div className="viewer-tools">
              <div className="filters tight">
                {can("edit") && <button className={`chip ${tool === "edit" ? "active" : ""}`} onClick={() => setTool(tool === "edit" ? null : "edit")}>{t("Change it")}</button>}
                {can("fill") && <button className={`chip ${tool === "fill" ? "active" : ""}`} onClick={() => setTool(tool === "fill" ? null : "fill")}>{t("Paint to fill in")}</button>}
                {can("extend") && <button className={`chip ${tool === "extend" ? "active" : ""}`} onClick={() => setTool(tool === "extend" ? null : "extend")}>{t("Extend")}</button>}
                {can("restyle") && <button className={`chip ${tool === "restyle" ? "active" : ""}`} onClick={() => setTool(tool === "restyle" ? null : "restyle")}>{t("Restyle")}</button>}
              </div>
              {tool === "edit" && (
                <div className="form">
                  {ask.questions && <PictureQuestions key={ask.questions.join("\n")} questions={ask.questions} prompt={text} onMake={(p) => run({ op: "edit", prompt: p })} />}
                  <input
                    className="input"
                    value={text}
                    onChange={(e) => {
                      setText(e.target.value);
                      ask.clear();
                    }}
                    placeholder={t("E.g. make it night, add a red scarf, remove the car")}
                    autoFocus
                    onKeyDown={(e) => e.key === "Enter" && change()}
                  />
                  <button className="btn primary small" disabled={!text.trim() || ask.asking} onClick={change}>{ask.asking ? t("Checking…") : t("Change it")}</button>
                </div>
              )}
              {tool === "fill" && (
                <div className="form">
                  <span className="small muted">{t("Paint over the part to change. Describe what goes there, or leave it empty to remove what's there.")}</span>
                  <label className="small row">
                    {t("Brush")} <input type="range" min={10} max={120} value={brush} onChange={(e) => setBrush(+e.target.value)} />
                  </label>
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder={t("What goes there (optional)")} />
                  <div className="row">
                    <button className="btn small" disabled={!painted} onClick={() => painter.current?.clear()}>{t("Clear")}</button>
                    <button className="btn primary small" disabled={!painted} onClick={fill}>{t("Fill in")}</button>
                  </div>
                </div>
              )}
              {tool === "extend" && (
                <div className="form">
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder={t("What's beyond the edges (optional)")} />
                  <div className="row">
                    <button className="btn small" onClick={() => extend("wide")}>{t("Wider")}</button>
                    <button className="btn small" onClick={() => extend("tall")}>{t("Taller")}</button>
                    <button className="btn small" onClick={() => extend("all")}>{t("All around")}</button>
                  </div>
                </div>
              )}
              {tool === "restyle" && (
                <div className="form">
                  <div className="filters tight">
                    {STYLES.map((s) => (
                      <button key={s} className="chip" onClick={() => run({ op: "restyle", prompt: s })}>{t(s)}</button>
                    ))}
                  </div>
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder={t("Or describe a style")} onKeyDown={(e) => e.key === "Enter" && text.trim() && run({ op: "restyle", prompt: text })} />
                </div>
              )}
              <div className="row wrap">
                <button
                  className="btn small"
                  disabled={Math.max(item.width, item.height) > 2048}
                  onClick={() => run({ op: "upscale", prompt: "" })}
                  title={Math.max(item.width, item.height) > 2048 ? t("It's already large. Upscaling is for pictures up to 2048 pixels across.") : t("Four times larger and sharper")}
                >
                  {t("Upscale 4×")}
                </button>
                <button className="btn small" onClick={() => run({ op: "remove_background", prompt: "" })}>{t("Remove background")}</button>
                {features.has("video") && <button className="btn small" onClick={() => onAnimate(item)} title={t("Turn a picture into a video: it starts on your picture and comes to life")}>{t("Turn into a video")}</button>}
                <SetWallpaperButton item={item} toast={toast} />
                {item.op === "generate" && item.prompt && (
                  <button className="btn small" onClick={() => onStart({ op: "generate", prompt: item.prompt, model_id: item.model_id, shape: item.width > item.height ? "landscape" : item.width < item.height ? "portrait" : "square" })}>
                    {t("↻ More like this")}
                  </button>
                )}
              </div>
            </div>
          )}

          {item.kind === "video" && item.op === "wallpaper" && (
            <div className="row">
              <SetWallpaperButton item={item} toast={toast} />
            </div>
          )}
          <span className="spacer" />
          <div className="row">
            <button
              className="btn small"
              onClick={() => api.mediaFavorite(item.id, !item.favorite).then(onChanged).catch((e) => toast(errorText(e), "error"))}
              aria-pressed={item.favorite}
            >
              {item.favorite ? t("★ Favorite") : t("☆ Favorite")}
            </button>
            <button className="btn small" onClick={() => saveAs(item, toast)}>{t("Save as…")}</button>
            <span className="spacer" />
            <button className="btn ghost danger small" onClick={() => setConfirm(true)}>{t("Delete")}</button>
          </div>
        </aside>
      </div>
      {confirm && (
        <Modal title={t("Delete this from the gallery?")} onClose={() => setConfirm(false)}>
          <p>{t("It's deleted from this PC. Copies you saved elsewhere stay.")}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              autoFocus
              onClick={async () => {
                try {
                  await api.mediaDelete(item.id);
                  setConfirm(false);
                  onDeleted(item.id);
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Delete")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
