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
  type MusicSpec,
  type Voice,
} from "../api";
import { clock } from "../format";
import { t, tf, tx } from "../i18n";
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
const ALL_KINDS: MediaKind[] = ["image", "upscale", "background", "video", "music"];

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
  // A song picked in the viewer to work on in the music tab.
  const [songToWork, setSongToWork] = useState<{ item: MediaItem; task: SongTask } | null>(null);
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
      // (A failure's message is shown by the app, whatever page is open.)
      if (d.ok && d.items?.length) {
        setFresh((s) => new Set([...s, ...d.items!.map((i) => i.id)]));
        load();
      }
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
            onSetDefault={(id) => api.mediaSetDefault("image", id).then(reloadView)}
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
            onSetDefault={(id) => api.mediaSetDefault("video", id).then(reloadView)}
            onModels={() => setModels(["video"])}
            source={source}
            onPickSource={setSource}
            gallery={items}
            onStart={start}
            toast={toast}
          />
        ) : tab === "wallpaper" ? (
          <WallpaperComposer imageModels={installedOf("image")} videoModels={features.has("video") ? installedOf("video") : []} gallery={items} onStart={start} toast={toast} />
        ) : tab === "music" ? (
          <MusicComposer
            key={view?.defaults?.music ?? "no-default"}
            models={installedOf("music")}
            defaultId={view?.defaults?.music ?? null}
            onSetDefault={(id) => api.mediaSetDefault("music", id).then(reloadView)}
            onModels={() => setModels(["music"])}
            onStart={start}
            gallery={items}
            toast={toast}
            seed={songToWork}
          />
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
          onWorkOn={(i, task) => {
            setSongToWork({ item: i, task });
            setTab("music");
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
            <MediaModels progress={progress} toast={toast} only={models} />
          </div>
          <div className="modal-actions">
            {models.length < ALL_KINDS.length && (
              <button className="link small" onClick={() => setModels(ALL_KINDS)}>
                {t("Show models of every kind")}
              </button>
            )}
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

/** The model a composer uses: always shown as a menu, even with one model,
 * whose last entry opens this kind's models to install another (his
 * brother saw only the first music model's name and no way to add one). */
function ModelPick({ models, value, onChange, onMore }: { models: MediaModelCard[]; value: string; onChange: (id: string) => void; onMore?: () => void }) {
  // Models on this PC are marked as such; cloud ones carry their provider in their names.
  const label = (m: MediaModelCard) => (m.id.startsWith("cloud:") ? m.name : t("{name} (this PC)", { name: m.name }));
  if (models.length === 0) return null;
  if (models.length === 1 && !onMore) return <span className="small muted" title={t("Model")}>{label(models[0])}</span>;
  return (
    <select
      value={value}
      onChange={(e) => (e.target.value === MORE ? onMore?.() : onChange(e.target.value))}
      title={t("Model")}
      aria-label={t("Model")}
    >
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {label(m)}
        </option>
      ))}
      {onMore && <option value={MORE}>{t("More models…")}</option>}
    </select>
  );
}

/** ModelPick's entry that opens the models list. */
const MORE = "__more_models__";

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
        placeholder={t("Describe the picture: the subject, the setting, the light and mood. Pick a look below.")} aria-label={t("Describe the picture: the subject, the setting, the light and mood. Pick a look below.")}
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
        <ModelPick models={models} value={m?.id ?? ""} onChange={setModel} onMore={onModels} />
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
  onModels,
  source,
  onPickSource,
  gallery,
  onStart,
  toast,
}: {
  models: MediaModelCard[];
  defaultId: string | null;
  onSetDefault: (id: string) => void;
  onModels: () => void;
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
        <ModelPick models={usable} value={m?.id ?? ""} onChange={setModel} onMore={onModels} />
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

/** Styles, moods, instruments and beats to pick from (any can be typed too). */
const GENRES = [
  tx("Pop"), tx("Rock"), tx("Hip hop"), tx("R&B"), tx("Country"), tx("Folk"), tx("Jazz"), tx("Blues"), tx("Classical"), tx("Electronic"),
  tx("House"), tx("Techno"), tx("Lo-fi"), tx("Metal"), tx("Punk"), tx("Indie"), tx("Reggae"), tx("Latin"), tx("K-pop"), tx("Soul"),
  tx("Funk"), tx("Gospel"), tx("Ambient"), tx("Soundtrack"), tx("Synthwave"), tx("Drum and bass"),
];
const MOODS = [tx("Happy"), tx("Sad"), tx("Energetic"), tx("Calm"), tx("Dark"), tx("Romantic"), tx("Epic"), tx("Dreamy"), tx("Angry"), tx("Hopeful"), tx("Nostalgic"), tx("Playful")];
const INSTRUMENTS = [
  tx("Piano"), tx("Acoustic guitar"), tx("Electric guitar"), tx("Bass"), tx("Drums"), tx("Strings"), tx("Violin"), tx("Cello"), tx("Synth"), tx("Brass"),
  tx("Saxophone"), tx("Trumpet"), tx("Flute"), tx("Organ"), tx("Harmonica"), tx("Ukulele"), tx("Banjo"), tx("808 bass"), tx("Choir"), tx("Percussion"),
];
const BEATS = [tx("Steady"), tx("Four on the floor"), tx("Swing"), tx("Shuffle"), tx("Half-time"), tx("Trap"), tx("Boom bap"), tx("Breakbeat"), tx("Reggaeton"), tx("Waltz")];
/** Pace presets, in beats a minute. */
const PACES: [string, number][] = [[tx("Slow"), 70], [tx("Relaxed"), 90], [tx("Medium"), 105], [tx("Upbeat"), 120], [tx("Fast"), 140], [tx("Very fast"), 170]];
const NOTES = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/** The parts the music engine knows: [what it's called there, what people call it]. */
const PARTS: [string, string][] = [
  ["vocals", tx("Vocals")], ["backing_vocals", tx("Backing vocals")], ["drums", tx("Drums")], ["bass", tx("Bass")], ["guitar", tx("Guitar")], ["keyboard", tx("Keys")],
  ["percussion", tx("Percussion")], ["strings", tx("Strings")], ["synth", tx("Synth")], ["brass", tx("Brass")], ["woodwinds", tx("Woodwinds")], ["fx", tx("Effects")],
];
type SongTask = "cover" | "repaint" | "extend" | "add" | "extract" | "complete";
const TASKS: [SongTask, string, string][] = [
  ["cover", tx("Restyle it"), tx("The same song in another style: describe how it should sound.")],
  ["repaint", tx("Make part of it again"), tx("Pick the part; the rest stays as it is.")],
  ["extend", tx("Make it longer"), tx("More music after its end, in the same style.")],
  ["add", tx("Add a part"), tx("Vocals, drums, guitar… added on top of the song.")],
  ["extract", tx("Keep only one part"), tx("Takes one part out of the mix, such as the vocals or the drums.")],
  ["complete", tx("Build a full song around it"), tx("For a lone part (a vocal, a guitar riff): the rest of the band added.")],
];

/** Styles whose vocals the music model blurs or buries (synthwave's words weren't heard however it was asked). */
const SOFT_VOCALS = ["Synthwave", "Ambient", "Techno", "House", "Lo-fi", "Drum and bass"];

/** Seconds of song a sung line needs to be heard (media/lyrics.rs `SECS_PER_LINE`). */
const SECS_PER_LINE = 7.5;

/** Sung lines in lyrics: not the [Verse] tags or blank lines. */
function sungLines(lyrics: string): number {
  return lyrics.split("\n").map((l) => l.trim()).filter((l) => l && !l.startsWith("[")).length;
}

/** A length that fits the lyrics, in steps of 10 seconds, at most 10 minutes. */
function fitFor(lyrics: string): number {
  return Math.min(600, Math.ceil((sungLines(lyrics) * SECS_PER_LINE) / 10) * 10);
}

/** Seconds as "1:05". */
function mmss(s: number): string {
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return `${m}:${String(r).padStart(2, "0")}`;
}

/** A row of chips to pick one (or none) from, plus a box for anything else. */
function PickOne({ label, options, value, onChange, other }: { label: string; options: string[]; value: string; onChange: (v: string) => void; other?: string }) {
  const custom = value !== "" && !options.some((o) => t(o) === value || o === value);
  return (
    <div className="music-field">
      <span className="music-label">{label}</span>
      <div className="filters tight wrap">
        {options.map((o) => (
          <button key={o} className={`chip ${value === o || value === t(o) ? "active" : ""}`} onClick={() => onChange(value === o || value === t(o) ? "" : o)}>
            {t(o)}
          </button>
        ))}
        {other && <input className="input chip-input" value={custom ? value : ""} placeholder={other} aria-label={other} onChange={(e) => onChange(e.target.value)} />}
      </div>
    </div>
  );
}

/** Chips to pick several from, plus a box to add others. */
function PickMany({ label, options, value, onChange, other }: { label: string; options: string[]; value: string[]; onChange: (v: string[]) => void; other: string }) {
  const [typed, setTyped] = useState("");
  const toggle = (o: string) => onChange(value.includes(o) ? value.filter((x) => x !== o) : [...value, o]);
  const extra = value.filter((v) => !options.includes(v));
  return (
    <div className="music-field">
      <span className="music-label">{label}</span>
      <div className="filters tight wrap">
        {options.map((o) => (
          <button key={o} className={`chip ${value.includes(o) ? "active" : ""}`} onClick={() => toggle(o)}>
            {t(o)}
          </button>
        ))}
        {extra.map((o) => (
          <button key={o} className="chip active" onClick={() => toggle(o)} title={t("Remove")}>
            {o} ×
          </button>
        ))}
        <input
          className="input chip-input"
          value={typed}
          placeholder={other}
          aria-label={other}
          onChange={(e) => setTyped(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && typed.trim()) {
              onChange([...value, typed.trim()]);
              setTyped("");
            }
          }}
        />
      </div>
    </div>
  );
}

/** The song being worked on: picked from a file or the gallery, and heard. */
function SongSource({ song, gallery, onPick, toast }: { song: MediaItem | null; gallery: MediaItem[]; onPick: (i: MediaItem | null) => void; toast: PushToast }) {
  const src = useMediaUrl(song);
  const [picking, setPicking] = useState(false);
  const fromFile = async () => {
    const picked = await openFile({ multiple: false, filters: [{ name: t("Songs and sounds"), extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "wma"] }] });
    if (typeof picked !== "string") return;
    try {
      onPick(await api.mediaImport(picked));
      setPicking(false);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const songs = gallery.filter((i) => i.kind === "audio");
  return (
    <div className="music-field">
      <span className="music-label">{t("The song")}</span>
      {song ? (
        <div className="row wrap">
          <strong className="small">{song.prompt || t("A song")}</strong>
          {song.seconds > 0 && <span className="small muted">{mmss(song.seconds)}</span>}
          <button className="link small" onClick={() => onPick(null)}>{t("Change")}</button>
          {src && <audio controls src={src} className="music-player" />}
        </div>
      ) : (
        <div className="row wrap">
          <button className="btn small" onClick={fromFile}>{t("From a file…")}</button>
          <button className="btn small" onClick={() => setPicking((p) => !p)} disabled={!songs.length}>{t("From the gallery")}</button>
          {!songs.length && <span className="small muted">{t("No songs in the gallery yet.")}</span>}
        </div>
      )}
      {picking && !song && (
        <div className="music-song-list">
          {songs.map((s) => (
            <button key={s.id} className="btn ghost small" onClick={() => { onPick(s); setPicking(false); }}>
              {s.prompt || t("A song")} {s.seconds > 0 ? `· ${mmss(s.seconds)}` : ""}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function MusicComposer({
  models,
  defaultId,
  onSetDefault,
  onModels,
  onStart,
  gallery,
  toast,
  seed,
}: {
  models: MediaModelCard[];
  defaultId: string | null;
  onSetDefault: (id: string) => void;
  onModels: () => void;
  onStart: (r: MediaRequest) => Promise<boolean>;
  gallery: MediaItem[];
  toast: PushToast;
  /** A song picked in the viewer to work on. */
  seed?: { item: MediaItem; task: SongTask } | null;
}) {
  const [mode, setMode] = useState<"song" | "edit" | "sound">("song");
  const [prompt, setPrompt] = useState("");
  const [genre, setGenre] = useState("");
  const [mood, setMood] = useState("");
  const [instruments, setInstruments] = useState<string[]>([]);
  const [vocals, setVocals] = useState<"write" | "none" | "mine">("write");
  const [voice, setVoice] = useState("");
  const [lyrics, setLyrics] = useState("");
  const [writing, setWriting] = useState(false);
  const [language, setLanguage] = useState("auto");
  const [bpm, setBpm] = useState<number | null>(null);
  const [beat, setBeat] = useState("");
  const [meter, setMeter] = useState("");
  const [key, setKey] = useState("");
  const [seconds, setSeconds] = useState(60);
  const [more, setMore] = useState(false);
  const [reference, setReference] = useState<MediaItem | null>(null);
  const [song, setSong] = useState<MediaItem | null>(null);
  const [task, setTask] = useState<SongTask>("cover");
  const [part, setPart] = useState("vocals");
  const [from, setFrom] = useState(0);
  const [to, setTo] = useState(30);
  const [extendBy, setExtendBy] = useState(30);
  const [strength, setStrength] = useState(0.5);
  useEffect(() => {
    if (!seed) return;
    setMode("edit");
    setSong(seed.item);
    setTask(seed.task);
    if (seed.item.seconds) {
      setFrom(0);
      setTo(Math.min(30, Math.round(seed.item.seconds)));
    }
  }, [seed]);
  const parts = task === "add" || task === "extract" || task === "complete";
  const needOp = mode === "sound" ? "sound" : mode === "edit" && parts ? "parts" : "music";
  const fit = models.filter((m) => (m.can ?? ["music", "sound"]).includes(needOp));
  const [model, setModel] = useModelPick("music", fit.length ? fit : models, defaultId);
  const m = fit.find((x) => x.id === model) ?? fit[0];
  const secs = mode === "sound" ? Math.min(Math.max(seconds, 10), 30) : seconds;
  const sings = mode === "song" ? vocals !== "none" : mode === "edit" && ((task === "add" && part.includes("vocals")) || task === "cover" || task === "repaint");
  const styled = mode === "song" || (mode === "edit" && (task === "cover" || task === "complete"));
  const spec = (): MusicSpec => ({
    genre: genre || null,
    mood: mood || null,
    instruments,
    voice: voice || null,
    beat: beat && beat !== "Steady" ? beat : null,
    bpm,
    key: key || null,
    time_signature: meter || null,
    reference: reference?.id ?? null,
    ...(mode === "edit" && song
      ? {
          task,
          audio: song.id,
          track: parts ? (task === "complete" ? null : part) : null,
          region: task === "repaint" ? [from, Math.max(to, from + 1)] : null,
          extend_by: task === "extend" ? extendBy : null,
          strength: task === "cover" ? strength : null,
        }
      : {}),
  });
  const writeLyrics = async () => {
    setWriting(true);
    try {
      setLyrics(await api.musicWriteLyrics(prompt, spec(), secs, language === "auto" ? null : language));
      setVocals("mine");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWriting(false);
    }
  };
  const tag = (s: string) => setLyrics((l) => `${l}${l && !l.endsWith("\n") ? "\n\n" : ""}[${s}]\n`);
  const ready =
    mode === "sound"
      ? !!prompt.trim()
      : mode === "song"
        ? (!!prompt.trim() || !!genre || instruments.length > 0) && !(vocals === "mine" && !lyrics.trim())
        : !!song && (task !== "cover" || !!prompt.trim() || !!genre || instruments.length > 0);
  const go = async () => {
    const ok = await onStart({
      op: mode === "sound" ? "sound" : "music",
      prompt,
      seconds: mode === "edit" ? (task === "extend" ? extendBy : undefined) : secs,
      instrumental: mode === "song" ? vocals === "none" : undefined,
      lyrics: sings && (mode === "edit" || vocals === "mine") && lyrics.trim() ? lyrics : null,
      language: sings && language !== "auto" ? language : null,
      model_id: m?.id,
      music: mode === "sound" ? null : spec(),
    });
    if (ok && mode !== "edit") setPrompt("");
  };
  const est = m ? (m.fit.est_secs * (mode === "edit" ? Math.max(song?.seconds ?? 60, 30) : secs)) / 30 : 0;
  return (
    <div className="form music-composer">
      <div className="mode-switch">
        <button className={`mode ${mode === "song" ? "active" : ""}`} onClick={() => setMode("song")}>{t("New song")}</button>
        <button className={`mode ${mode === "edit" ? "active" : ""}`} onClick={() => setMode("edit")}>{t("From a song you have")}</button>
        <button className={`mode ${mode === "sound" ? "active" : ""}`} onClick={() => setMode("sound")}>{t("Sound effect")}</button>
      </div>

      {mode === "edit" && (
        <>
          <SongSource song={song} gallery={gallery} onPick={(s) => { setSong(s); if (s?.seconds) { setFrom(0); setTo(Math.min(30, Math.round(s.seconds))); } }} toast={toast} />
          <div className="music-field">
            <span className="music-label">{t("What to do with it")}</span>
            <div className="filters tight wrap">
              {TASKS.map(([id, label]) => (
                <button key={id} className={`chip ${task === id ? "active" : ""}`} onClick={() => setTask(id)}>{t(label)}</button>
              ))}
            </div>
            <p className="small muted">{t(TASKS.find(([id]) => id === task)![2])}</p>
          </div>
          {task === "repaint" && (
            <div className="row wrap">
              <label className="small">
                {t("From")} <input type="number" className="input num" min={0} max={song?.seconds || 600} value={from} onChange={(e) => setFrom(Math.max(0, +e.target.value))} /> s
              </label>
              <label className="small">
                {t("to")} <input type="number" className="input num" min={from + 1} max={song?.seconds || 600} value={to} onChange={(e) => setTo(+e.target.value)} /> s
              </label>
              {song && song.seconds > 0 && <span className="small muted">{t("of {length}", { length: mmss(song.seconds) })}</span>}
            </div>
          )}
          {task === "extend" && (
            <label className="small music-length">
              {t("Add")}
              <input type="range" min={5} max={300} step={5} value={extendBy} onChange={(e) => setExtendBy(+e.target.value)} />
              {mmss(extendBy)}
            </label>
          )}
          {(task === "add" || task === "extract") && (
            <PickOne label={task === "add" ? t("The part to add") : t("The part to keep")} options={PARTS.map(([, n]) => n)} value={PARTS.find(([id]) => id === part)?.[1] ?? ""} onChange={(v) => setPart(PARTS.find(([, n]) => n === v || t(n) === v)?.[0] ?? "vocals")} />
          )}
          {task === "cover" && (
            <label className="small music-length">
              {t("How close to the original")}
              <input type="range" min={0} max={1} step={0.05} value={strength} onChange={(e) => setStrength(+e.target.value)} />
              {strength < 0.35 ? t("loosely") : strength < 0.7 ? t("in between") : t("very close")}
            </label>
          )}
          {parts && !m && (
            <p className="callout small">
              {t("Adding, keeping or building around a part needs ACE-Step 1.5 Base (slower, and a 2.5 GB download beside ACE-Step 1.5).")}{" "}
              <button className="link" onClick={onModels}>{t("Get it")}</button>
            </p>
          )}
        </>
      )}

      <textarea
        className="input"
        rows={mode === "edit" ? 2 : 3}
        value={prompt}
        aria-label={t("Description")}
        placeholder={
          mode === "sound"
            ? t("Describe the sound, e.g. \"rain on a tin roof with distant thunder\"")
            : mode === "edit"
              ? t("Describe the result (optional for some changes), e.g. \"as a slow piano ballad\"")
              : t("What the song is about and how it sounds, e.g. \"a road trip through the mountains with old friends\"")
        }
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />

      {styled && (
        <>
          <PickOne label={t("Style")} options={GENRES} value={genre} onChange={setGenre} other={t("Other style")} />
          <PickOne label={t("Mood")} options={MOODS} value={mood} onChange={setMood} />
          <PickMany label={t("Instruments")} options={INSTRUMENTS} value={instruments} onChange={setInstruments} other={t("Add another (Enter)")} />
        </>
      )}

      {mode === "song" && vocals !== "none" && SOFT_VOCALS.includes(genre) && (
        <p className="small muted">{t("Vocals in this style usually come out soft or blurred. For words you can make out, Pop, Rock, Folk or Country work best.")}</p>
      )}
      {mode === "song" && (
        <div className="music-field">
          <span className="music-label">{t("Vocals")}</span>
          <div className="row wrap">
            <div className="filters tight">
              {(["write", "mine", "none"] as const).map((v) => (
                <button key={v} className={`chip ${vocals === v ? "active" : ""}`} onClick={() => setVocals(v)}>
                  {{ write: t("Lyrics written for me"), mine: t("My own lyrics"), none: t("Instrumental") }[v]}
                </button>
              ))}
            </div>
            {vocals !== "none" && (
              <>
                <select value={voice} onChange={(e) => setVoice(e.target.value)} aria-label={t("Who sings")}>
                  <option value="">{t("Any voice")}</option>
                  <option value="female">{t("Female voice")}</option>
                  <option value="male">{t("Male voice")}</option>
                  <option value="duet">{t("Duet")}</option>
                  <option value="choir">{t("Choir")}</option>
                  <option value="rap">{t("Rap")}</option>
                </select>
                <select value={language} onChange={(e) => setLanguage(e.target.value)} aria-label={t("Language of the vocals")}>
                  {LANGUAGES.map(([c, n]) => (
                    <option key={c} value={c}>{t(n)}</option>
                  ))}
                </select>
              </>
            )}
          </div>
        </div>
      )}

      {sings && (mode === "edit" || vocals !== "none") && (
        <div className="music-field">
          <div className="row wrap">
            <span className="music-label">{mode === "edit" ? t("Lyrics (optional)") : t("Lyrics")}</span>
            <span className="spacer" />
            {["Intro", "Verse", "Pre-Chorus", "Chorus", "Bridge", "Outro"].map((s) => (
              <button key={s} className="btn ghost small" onClick={() => { tag(s); if (mode === "song") setVocals("mine"); }}>+ {t(s)}</button>
            ))}
            <button className="btn small" onClick={writeLyrics} disabled={writing}>{writing ? t("Writing…") : t("Write lyrics for me")}</button>
          </div>
          {(vocals === "mine" || mode === "edit") && (
            <textarea className="input lyrics-box" rows={8} value={lyrics} aria-label={t("Lyrics")} placeholder={"[Verse]\n…\n\n[Chorus]\n…"} onChange={(e) => setLyrics(e.target.value)} />
          )}
          {mode === "song" && vocals === "mine" && sungLines(lyrics) > Math.round(secs / SECS_PER_LINE) * 1.25 && (
            <p className="callout small">
              {t("{n} lines need about {time} to be sung clearly; at {length} the words come out mumbled.", { n: sungLines(lyrics), time: mmss(fitFor(lyrics)), length: mmss(secs) })}{" "}
              <button className="link" onClick={() => setSeconds(fitFor(lyrics))}>{t("Make it {time}", { time: mmss(fitFor(lyrics)) })}</button>
            </p>
          )}
          {vocals === "write" && mode === "song" && <p className="small muted">{t("They're written when the song is made. To read and change them first, press Write lyrics for me.")}</p>}
        </div>
      )}

      {mode === "song" && (
        <div className="music-field">
          <span className="music-label">{t("Tempo")}</span>
          <div className="row wrap">
            <div className="filters tight">
              <button className={`chip ${bpm === null ? "active" : ""}`} onClick={() => setBpm(null)}>{t("Auto")}</button>
              {PACES.map(([label, b]) => (
                <button key={label} className={`chip ${bpm === b ? "active" : ""}`} onClick={() => setBpm(b)}>{t(label)}</button>
              ))}
            </div>
            {bpm !== null && (
              <label className="small music-length">
                <input type="range" min={50} max={200} value={bpm} onChange={(e) => setBpm(+e.target.value)} aria-label={t("Beats a minute")} />
                {t("{n} beats a minute", { n: bpm })}
              </label>
            )}
          </div>
        </div>
      )}

      {mode === "song" && <PickOne label={t("Beat")} options={BEATS} value={beat} onChange={setBeat} other={t("Other beat")} />}

      {mode !== "sound" && (
        <button className="link small music-more" onClick={() => setMore((x) => !x)} aria-expanded={more}>
          {more ? t("Fewer settings") : t("More settings: key, time signature, a song to sound like")}
        </button>
      )}
      {more && mode !== "sound" && (
        <div className="row wrap">
          <select value={key} onChange={(e) => setKey(e.target.value)} aria-label={t("Key")}>
            <option value="">{t("Any key")}</option>
            {NOTES.flatMap((n) => [`${n} major`, `${n} minor`]).map((k) => (
              <option key={k} value={k}>{k}</option>
            ))}
          </select>
          <select value={meter} onChange={(e) => setMeter(e.target.value)} aria-label={t("Time signature")}>
            <option value="">{t("Any time signature")}</option>
            {["4/4", "3/4", "6/8", "2/4"].map((x) => (
              <option key={x} value={x}>{x}</option>
            ))}
          </select>
          <span className="small">{t("Sound like:")}</span>
          <select value={reference?.id ?? ""} onChange={(e) => setReference(gallery.find((g) => g.id === e.target.value) ?? null)} aria-label={t("A song to sound like")}>
            <option value="">{t("No song")}</option>
            {gallery.filter((g) => g.kind === "audio").map((g) => (
              <option key={g.id} value={g.id}>{g.prompt || t("A song")}</option>
            ))}
          </select>
        </div>
      )}

      <div className="row wrap">
        {mode !== "edit" && (
          <label className="small music-length">
            {t("Length")}
            <input
              type="range"
              min={mode === "sound" ? 10 : 10}
              max={mode === "sound" ? 30 : 600}
              step={mode === "sound" ? 5 : 10}
              value={secs}
              onChange={(e) => setSeconds(+e.target.value)}
              aria-label={t("Length")}
            />
            {mmss(secs)}
          </label>
        )}
        <ModelPick models={fit.length ? fit : models} value={m?.id ?? ""} onChange={setModel} onMore={onModels} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel(est)}</span>}
        <button className="btn primary" disabled={!ready || !m} onClick={go}>
          {mode === "sound" ? t("Make the sound") : mode === "edit" ? t("Make it") : t("Make the song")}
        </button>
      </div>
      {mode === "sound" && <p className="small muted">{t("Sound effects come from the music model, so they're rough sketches rather than library-quality recordings.")}</p>}
      {mode === "song" && secs > 240 && <p className="small muted">{t("Long songs take a while: about {time} here.", { time: waitLabel(est) })}</p>}
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
      <textarea className="input" rows={6} value={text} placeholder={t("The text to read aloud. It's saved as a sound you can play, trim and save.")} aria-label={t("The text to read aloud. It's saved as a sound you can play, trim and save.")} onChange={(e) => setText(e.target.value)} autoFocus />
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
  onWorkOn,
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
  onWorkOn: (i: MediaItem, task: SongTask) => void;
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
          {item.missing && item.missing.length > 0 && (
            <p className="small warn-text">{t("The check didn't find everything asked for: {missing}.", { missing: item.missing.join("; ") })}</p>
          )}
          {item.note && <p className="small muted">{tf(item.note)}</p>}
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
          {item.kind === "audio" && features.has("music") && (
            <div className="viewer-tools">
              <div className="filters tight wrap">
                {(["cover", "extend", "repaint", "add", "extract"] as SongTask[]).map((task) => (
                  <button key={task} className="chip" onClick={() => onWorkOn(item, task)}>{t(TASKS.find(([id]) => id === task)![1])}</button>
                ))}
              </div>
            </div>
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
                    placeholder={t("E.g. make it night, add a red scarf, remove the car")} aria-label={t("E.g. make it night, add a red scarf, remove the car")}
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
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder={t("What goes there (optional)")} aria-label={t("What goes there (optional)")} />
                  <div className="row">
                    <button className="btn small" disabled={!painted} onClick={() => painter.current?.clear()}>{t("Clear")}</button>
                    <button className="btn primary small" disabled={!painted} onClick={fill}>{t("Fill in")}</button>
                  </div>
                </div>
              )}
              {tool === "extend" && (
                <div className="form">
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder={t("What's beyond the edges (optional)")} aria-label={t("What's beyond the edges (optional)")} />
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
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder={t("Or describe a style")} aria-label={t("Or describe a style")} onKeyDown={(e) => e.key === "Enter" && text.trim() && run({ op: "restyle", prompt: text })} />
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
