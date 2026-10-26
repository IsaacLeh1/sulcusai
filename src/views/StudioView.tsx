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
  type Composition,
  type MediaModelCard,
  type MediaRequest,
  type MusicSpec,
  type Persona,
  type Taste,
  type VocalEffect,
  type Voice,
} from "../api";
import { bytes, clock } from "../format";
import { t, tf, tx } from "../i18n";
import { forget, forgetThumb, OP_LABEL, useMediaUrl, waitLabel } from "../media";
import { AudioPlayer, JobCard, MadeStatsLine, MediaTile, saveAs, SongListener, useMediaJobs } from "../components/MediaBits";
import { SongView } from "../components/SongView";
import { ComposeEditor, TIMES } from "../components/ComposeEditor";
import {
  BEATS, Chips, EMOTIONS, LANGUAGES, ENDINGS, ENERGIES, ERAS, GENRES, HARMONY_INSTRUMENTS, HARMONY_STYLES, HARMONY_VOICES, HARMONY_WHERE, INSTRUMENTS, LEAD_INSTRUMENTS, MOODS, NOTES,
  PACES, PARTS, PickMany, PickOne, PRODUCTIONS, RHYMES, SECS_PER_LINE, SOFT_VOCALS, SongSections, structureSeconds, TASKS, VOCAL_STYLES, VocalEffects, fitFor, mmss, sungLines,
  joined, listed, type SongTask,
} from "../components/MusicFields";
import { MediaModels, useMediaView } from "../components/MediaModels";
import { MaskPainter, type MaskHandle } from "../components/MaskPainter";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { asksFirst, ExamplePicker, PictureQuestions, PromptHelpPanel, SetWallpaperButton, useAskFirst, usePromptHelp, WallpaperComposer, type ExampleUse } from "../components/StudioExtras";
import { OwnVoicePanel, useOwnVoices } from "../components/OwnVoice";
import { AddToPlaylist, Playlists } from "../components/Library";

type Tab = "pictures" | "video" | "wallpaper" | "music" | "narrate" | "voice";
type Shape = NonNullable<MediaRequest["shape"]>;

const TAB_FEATURE: Record<Tab, FeatureId> = { pictures: "images", video: "video", wallpaper: "images", music: "music", narrate: "music", voice: "music" };
const TAB_KIND: Partial<Record<Tab, MediaKind>> = { pictures: "image", video: "video", wallpaper: "image", music: "music" };
const ALL_KINDS: MediaKind[] = ["image", "upscale", "background", "video", "music"];

const STYLES = [tx("Watercolor"), tx("Oil painting"), tx("Pencil sketch"), tx("Anime"), tx("3D render"), tx("Pixel art"), tx("Vintage photo"), tx("Comic book")];

interface Props {
  features: Set<FeatureId>;
  progress: Record<string, InstallProgress>;
  toast: PushToast;
  onGoFeatures: () => void;
}

export function StudioView({ features, progress, toast, onGoFeatures }: Props) {
  const tabs: Tab[] = (["pictures", "video", "wallpaper", "music", "narrate", "voice"] as Tab[]).filter((tb) => features.has(TAB_FEATURE[tb]));
  // A song written as notes, open in Compose's editor (item: where it's kept).
  const [composing, setComposing] = useState<{ item: MediaItem | null; composition: Composition } | null>(null);
  const [tab, setTab] = useState<Tab>(tabs[0] ?? "pictures");
  // A song picked in the viewer to work on in the music tab.
  const [songToWork, setSongToWork] = useState<{ item: MediaItem; task: SongTask } | null>(null);
  // A song whose settings fill the song form again (Reuse these settings).
  const [songToReuse, setSongToReuse] = useState<MediaItem | null>(null);
  // A playlist's songs, whose shared sound fills the song form.
  const [inspire, setInspire] = useState<string[] | null>(null);
  const [view, reloadView] = useMediaView();
  const jobs = useMediaJobs();
  const [items, setItems] = useState<MediaItem[]>([]);
  const [more, setMore] = useState(false);
  const [filter, setFilter] = useState<"all" | "image" | "video" | "audio" | "favorites" | "playlists">("all");
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
        // A song with new cover art: its old picture is let go.
        d.items.filter((i) => i.kind === "audio" && i.poster).forEach((i) => forgetThumb(i.id));
        setFresh((s) => new Set([...s, ...d.items!.map((i) => i.id)]));
        load();
        // A song listened to or mapped while open: the viewer shows it.
        setViewing((v) => (v ? d.items!.find((i) => i.id === v.id) ?? v : v));
      }
    });
    const tuned = on("media:tuned", (r) =>
      toast(t("The prompt writer learned this model: {before}% → {after}% of what was asked for showed up.", { before: Math.round(r.baseline * 100), after: Math.round(r.score * 100) }), "success"),
    );
    const coached = on("media:coached", (r) => {
      const scores = { before: Math.round(r.before * 100), after: Math.round(r.after * 100) };
      if (r.kept) toast(t("The prompt writer was coached: its practice pictures scored {before}% → {after}%.", scores), "success");
      else toast(t("Coaching didn't make the prompt writer better ({before}% → {after}%), so it writes as it did before.", scores), "info");
    });
    return () => {
      sub.then((un) => un());
      tuned.then((un) => un());
      coached.then((un) => un());
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
            {{ pictures: t("Pictures"), video: t("Video"), wallpaper: t("Wallpapers"), music: t("Music and sound"), narrate: t("Narration"), voice: t("Your voice") }[tb]}
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
            reuse={songToReuse}
            inspire={inspire}
            onCompose={(composition) => setComposing({ item: null, composition })}
            onYourVoice={() => setTab("voice")}
            canArt={installedOf("image").some((m) => m.can.includes("generate"))}
          />
        ) : tab === "voice" ? (
          <OwnVoicePanel toast={toast} />
        ) : (
          <NarrateComposer cloud={(view?.cloud ?? []).filter((c) => c.kind === "speech")} onStart={start} onYourVoice={() => setTab("voice")} />
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
          {(["all", "image", "video", "audio", "favorites", "playlists"] as const).map((f) => (
            <button key={f} className={`chip ${filter === f ? "active" : ""}`} onClick={() => setFilter(f)}>
              {{ all: t("Everything"), image: t("Pictures"), video: t("Video"), audio: t("Sound"), favorites: t("★ Favorites"), playlists: t("Playlists") }[f]}
            </button>
          ))}
          <span className="spacer" />
          {view && <span className="small muted">{t("{n} in the gallery", { n: view.gallery })}</span>}
        </div>
        {filter === "playlists" ? (
          <Playlists
            toast={toast}
            onOpen={setViewing}
            onInspire={(ids) => {
              setInspire([...ids]);
              setTab("music");
            }}
          />
        ) : items.length === 0 ? (
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
            // Listening to a song and seeing how it's built show their result here.
            const here = req.op === "listen" || req.op === "map";
            if (await start(req)) {
              if (here) return;
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
          onReuse={(i) => {
            setSongToReuse({ ...i });
            setTab("music");
            setViewing(null);
          }}
          onCompose={(i) => {
            setViewing(null);
            if (i.composition) setComposing({ item: i, composition: i.composition });
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

      {composing && (
        <ComposeEditor
          item={composing.item}
          composition={composing.composition}
          musicModels={installedOf("music")}
          defaultMusic={view?.defaults?.music ?? null}
          onClose={() => setComposing(null)}
          onSaved={(i) => {
            setItems((all) => [i, ...all.filter((x) => x.id !== i.id)]);
            setFresh((s) => new Set([...s, i.id]));
            setComposing((c) => (c ? { ...c, item: i } : c));
          }}
          onStarted={() => {}}
          toast={toast}
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
          {src && <audio controls controlsList="nodownload" src={src} className="music-player" />}
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
              {s.title || s.prompt || t("A song")} {s.seconds > 0 ? `· ${mmss(s.seconds)}` : ""}
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
  onCompose,
  onYourVoice,
  canArt,
  reuse,
  inspire,
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
  /** Written as notes first: opens in Compose's editor. */
  onCompose: (c: Composition) => void;
  /** Opens Your voice, to record one. */
  onYourVoice: () => void;
  /** A picture model is here to make cover art. */
  canArt: boolean;
  /** A song whose settings fill the form again. */
  reuse?: MediaItem | null;
  /** Songs whose shared sound fills the form (a playlist's). */
  inspire?: string[] | null;
}) {
  const [mode, setMode] = useState<"song" | "edit" | "sound">("song");
  // Sung in a recorded voice (the song as made is under "← The original").
  const ownVoices = (useOwnVoices()?.voices ?? []).filter((v) => v.can_sing);
  const [ownVoice, setOwnVoice] = useState("");
  const [prompt, setPrompt] = useState("");
  // Several styles, moods and feelings can be picked together.
  const [genre, setGenre] = useState<string[]>([]);
  const [mood, setMood] = useState<string[]>([]);
  const [instruments, setInstruments] = useState<string[]>([]);
  const [vocals, setVocals] = useState<"write" | "none" | "mine">("write");
  const [voice, setVoice] = useState("");
  const [lyrics, setLyrics] = useState("");
  const [writing, setWriting] = useState(false);
  const [language, setLanguage] = useState("auto");
  const [bpm, setBpm] = useState<number | null>(null);
  const [beat, setBeat] = useState("");
  const [meter, setMeter] = useState("");
  const [keyNote, setKeyNote] = useState("");
  const [keyMode, setKeyMode] = useState<"major" | "minor">("major");
  const [vocalStyle, setVocalStyle] = useState("");
  const [harmonyVoice, setHarmonyVoice] = useState<MusicSpec["harmony_voice"] | "">("");
  const [harmonyStyle, setHarmonyStyle] = useState<NonNullable<MusicSpec["harmony_style"]>>("thirds");
  const [harmonyWhere, setHarmonyWhere] = useState<NonNullable<MusicSpec["harmony_where"]>>("chorus");
  const [harmonyInstrument, setHarmonyInstrument] = useState("");
  const [harmonyLayer, setHarmonyLayer] = useState(false);
  const [leadInstrument, setLeadInstrument] = useState("");
  const [sections, setSections] = useState<string[]>([]);
  const [rhyme, setRhyme] = useState<NonNullable<MusicSpec["rhyme"]> | "">("");
  const [energy, setEnergy] = useState<NonNullable<MusicSpec["energy"]> | "">("");
  const [ending, setEnding] = useState<NonNullable<MusicSpec["ending"]> | "">("");
  const [production, setProduction] = useState("");
  const [era, setEra] = useState("");
  const [emotion, setEmotion] = useState<string[]>([]);
  const [vocalEffects, setVocalEffects] = useState<VocalEffect[]>([]);
  // An artist's style: the name typed, and what the chat model said of them.
  const [soundLike, setSoundLike] = useState("");
  const [artist, setArtist] = useState<{ name: string; sound: string; writing: string } | null>(null);
  const [lookingUp, setLookingUp] = useState(false);
  const key = keyNote ? `${keyNote} ${keyMode}` : "";
  const [seconds, setSeconds] = useState(60);
  // Auto: the song is as long as it needs (a jingle short, a full song about three minutes).
  const [autoLength, setAutoLength] = useState(true);
  // Vocals come out clearly about three times in four: two versions on the
  // graphics card (seconds each) make one clear one likely. Changed by hand, it stays.
  const [versionsSet, setVersions] = useState<number | null>(null);
  // Best of 3: three made, each listened to, the one whose words come through clearest kept.
  const [bestOf, setBestOf] = useState(false);
  // Suno's advanced options: a title, Weirdness and Style influence (null: the model's own), and styles left out.
  const [title, setTitle] = useState("");
  const [weird, setWeird] = useState<number | null>(null);
  const [influence, setInfluence] = useState<number | null>(null);
  const [exclude, setExclude] = useState("");
  // Cover art for each song, made by the picture model after it (kept on for next time).
  const [coverArt, setCoverArtState] = useState(() => {
    try {
      return localStorage.getItem("sulcusai.songs.coverArt") === "1";
    } catch {
      return false;
    }
  });
  const setCoverArt = (on: boolean) => {
    setCoverArtState(on);
    try {
      localStorage.setItem("sulcusai.songs.coverArt", on ? "1" : "0");
    } catch {
      /* not kept */
    }
  };
  const [more, setMore] = useState(false);
  const [reference, setReference] = useState<MediaItem | null>(null);
  // Saved artists: one picked fills in their sound and voice; settings can be saved as one.
  const [personas, setPersonas] = useState<Persona[]>([]);
  const [personaId, setPersonaId] = useState("");
  const [naming, setNaming] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [voiceWords, setVoiceWords] = useState("");
  const [followVoice, setFollowVoice] = useState(false);
  const loadPersonas = () => api.musicPersonas().then(setPersonas).catch(() => {});
  useEffect(() => {
    loadPersonas();
  }, []);
  const persona = personas.find((p) => p.id === personaId);
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
  const versions = versionsSet ?? (mode === "song" && vocals !== "none" && m?.fit.on_gpu ? 2 : 1);
  const secs = mode === "sound" ? Math.min(Math.max(seconds, 10), 30) : seconds;
  const auto = mode === "song" && autoLength;
  // What Auto will likely come to, when it can be told before the song is made.
  const autoGuess = vocals === "mine" && sungLines(lyrics) > 0 ? fitFor(lyrics) + 5 : sections.length ? structureSeconds(sections) : null;
  const sings = mode === "song" ? vocals !== "none" : mode === "edit" && ((task === "add" && part.includes("vocals")) || task === "cover" || task === "repaint");
  const styled = mode === "song" || (mode === "edit" && (task === "cover" || task === "complete"));
  const singing = mode === "song" ? vocals !== "none" : sings;
  const harmonized = (singing && !!harmonyVoice) || !!harmonyInstrument;
  // Harmonies layered on afterwards need the model that adds parts.
  const canLayer = models.some((x) => (x.can ?? []).includes("parts"));
  const spec = (): MusicSpec => ({
    genre: joined(genre),
    mood: joined(mood),
    instruments,
    voice: voice || null,
    beat: beat && beat !== "Steady" ? beat : null,
    bpm,
    key: key || null,
    time_signature: meter || null,
    reference: reference?.id ?? null,
    vocal_style: singing ? vocalStyle || null : null,
    harmony_voice: singing && harmonyVoice ? harmonyVoice : null,
    harmony_instrument: harmonyInstrument || null,
    harmony_where: harmonized ? harmonyWhere : null,
    harmony_style: singing && harmonyVoice ? harmonyStyle : null,
    harmony_layer: harmonized && harmonyLayer && canLayer,
    lead_instrument: leadInstrument || null,
    sections,
    rhyme: singing && rhyme ? rhyme : null,
    energy: energy || null,
    ending: ending || null,
    production: production || null,
    era: era || null,
    auto_length: auto,
    own_voice: mode === "song" && singing && ownVoices.some((v) => v.id === ownVoice) ? ownVoice : null,
    best_of: mode === "song" && singing && bestOf,
    cover_art: mode === "song" && canArt && coverArt,
    title: mode === "song" && title.trim() ? title.trim() : null,
    weirdness: mode !== "edit" && weird != null ? weird / 100 : null,
    influence: influence != null ? influence / 100 : null,
    exclude: exclude.trim() || null,
    persona: mode === "song" && personaId ? personaId : null,
    voice_detail: mode === "song" && personaId && voiceWords.trim() ? voiceWords.trim() : null,
    emotion: joined(emotion),
    vocal_effects: singing ? vocalEffects : [],
    sound_like: soundLike.trim() || null,
    // Looked up for this name (else it's looked up when the song is made).
    artist_sound: artist && artist.name === soundLike.trim() ? artist.sound : null,
    artist_writing: artist && artist.name === soundLike.trim() ? artist.writing : null,
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
  // Written as notes first (Compose): the plan follows every option picked
  // here, and Make it real starts from them.
  const [arranging, setArranging] = useState(false);
  const writeAsNotes = async () => {
    setArranging(true);
    try {
      const [beats, unit] = (meter || "4/4").split("/").map(Number);
      const s = spec();
      onCompose(
        await api.composeWrite({
          description: prompt.trim() || [joined(genre), joined(mood)].filter(Boolean).join(", ") || "a song",
          beats,
          unit,
          bpm,
          key: key || null,
          style: joined(genre),
          sections: s.sections?.length || (auto ? 6 : Math.max(2, Math.min(12, Math.round(secs / 25)))),
          vocals: vocals !== "none",
          voice,
          era,
          song: s,
          lyrics: vocals === "mine" && lyrics.trim() ? lyrics : null,
          language: language === "auto" ? null : language,
        }),
      );
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setArranging(false);
    }
  };
  // My taste: what the liked and made songs share, filled in on request.
  const [taste, setTaste] = useState<Taste | null>(null);
  const fillMyTaste = async (ids?: string[]) => {
    try {
      const s = await api.musicTaste(ids);
      if (!s.songs) return toast(t("Make or like a few songs first: My taste comes from them."), "info");
      setTaste(s);
      const known = (list: string[], v: string) => list.find((o) => o.toLowerCase() === v.toLowerCase()) ?? v;
      if (s.genres.length) setGenre(s.genres.map((g) => known(GENRES, g)));
      if (s.moods.length) setMood(s.moods.map((x) => known(MOODS, x)));
      if (s.instruments.length) setInstruments(s.instruments.map((i) => known(INSTRUMENTS, i)));
      if (s.voice) setVoice(s.voice);
      if (s.era) setEra(s.era);
      if (s.production) setProduction(s.production);
      if (s.avoid.length) setExclude(s.avoid.join(", "));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  // Lines selected in the lyrics, to be written again (whole lines: the selection widened to them).
  const [picked, setPicked] = useState<[number, number] | null>(null);
  const [how, setHow] = useState("");
  const [rewriting, setRewriting] = useState(false);
  const pickLines = (el: HTMLTextAreaElement) => {
    const { selectionStart: a, selectionEnd: b, value } = el;
    if (b - a < 2) return setPicked(null);
    const from = value.lastIndexOf("\n", a - 1) + 1;
    const end = value.indexOf("\n", b - (value[b - 1] === "\n" ? 1 : 0));
    setPicked([from, end < 0 ? value.length : end]);
  };
  const pickedLines = picked ? lyrics.slice(picked[0], picked[1]) : "";
  const rewriteLines = async () => {
    if (!picked) return;
    setRewriting(true);
    try {
      const fresh = await api.musicRewriteLines(lyrics, pickedLines, how, language === "auto" ? null : language);
      setLyrics(lyrics.slice(0, picked[0]) + fresh + lyrics.slice(picked[1]));
      setPicked([picked[0], picked[0] + fresh.length]);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setRewriting(false);
    }
  };
  const writeLyrics = async () => {
    setWriting(true);
    try {
      setLyrics(await api.musicWriteLyrics(prompt, spec(), auto ? null : secs, language === "auto" ? null : language));
      setVocals("mine");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setWriting(false);
    }
  };
  // Their genre, voice, instruments and tempo filled in to see and change;
  // their sound and way of writing go with the song.
  const lookUpArtist = async () => {
    const name = soundLike.trim();
    if (!name) return;
    setLookingUp(true);
    try {
      const s = await api.musicArtistStyle(name);
      // "piano" lights the Piano chip rather than adding another.
      const known = (list: string[], v: string) => list.find((o) => o.toLowerCase() === v.toLowerCase()) ?? v;
      if (s.genre) setGenre(listed(s.genre).map((g) => known(GENRES, g)));
      if (s.voice) setVoice(s.voice);
      if (s.instruments.length) setInstruments(s.instruments.map((i) => known(INSTRUMENTS, i)));
      if (s.mood) setMood(listed(s.mood).map((x) => known(MOODS, x)));
      if (s.bpm) setBpm(s.bpm);
      setArtist({ name, sound: [s.vocal, s.production].filter(Boolean).join(", "), writing: s.writing });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setLookingUp(false);
    }
  };
  // An artist picked: their whole sound filled in, to see and change, and their voice song to follow.
  const pickArtist = (id: string) => {
    setPersonaId(id);
    setRemoving(false);
    const p = personas.find((x) => x.id === id);
    setVoiceWords(p?.voice_words ?? "");
    setFollowVoice(!!p?.follow_voice_song);
    if (!p) return;
    applySound(p.sound);
    if (p.language) setLanguage(p.language);
    setReference(p.follow_voice_song ? gallery.find((g) => g.id === p.voice_song) ?? null : null);
    if (vocals === "none") setVocals("write");
  };
  // A sound (an artist's, or a song's own settings) filled in, to see and change.
  const applySound = (s: MusicSpec) => {
    const [note, scale] = (s.key ?? "").split(" ");
    setGenre(listed(s.genre));
    setMood(listed(s.mood));
    setInstruments(s.instruments ?? []);
    setVoice(s.voice ?? "");
    setBeat(s.beat ?? "");
    setBpm(s.bpm ?? null);
    setKeyNote(note ?? "");
    setKeyMode(scale === "minor" ? "minor" : "major");
    setMeter(s.time_signature ?? "");
    setVocalStyle(s.vocal_style ?? "");
    setHarmonyVoice(s.harmony_voice ?? "");
    setHarmonyInstrument(s.harmony_instrument ?? "");
    if (s.harmony_style) setHarmonyStyle(s.harmony_style);
    if (s.harmony_where) setHarmonyWhere(s.harmony_where);
    setHarmonyLayer(!!s.harmony_layer);
    setLeadInstrument(s.lead_instrument ?? "");
    setRhyme(s.rhyme ?? "");
    setEnergy(s.energy ?? "");
    setEnding(s.ending ?? "");
    // A decade saved as the production (before the era had its own choice) is the era.
    const decade = /^\d{4}s$/.test(s.production ?? "");
    setProduction(decade ? "" : s.production ?? "");
    setEra(s.era ?? (decade ? s.production! : ""));
    setEmotion(listed(s.emotion));
    setVocalEffects(s.vocal_effects ?? []);
    setSoundLike(s.sound_like ?? "");
    setArtist(s.sound_like && s.artist_sound ? { name: s.sound_like, sound: s.artist_sound, writing: s.artist_writing ?? "" } : null);
  };
  // A playlist's sound (what its songs share), filled in for a new song.
  useEffect(() => {
    if (!inspire?.length) return;
    setMode("song");
    fillMyTaste(inspire);
  }, [inspire]); // eslint-disable-line react-hooks/exhaustive-deps
  // A song's own settings again (Suno's Reuse prompt): its description,
  // sound, words and choices, for another take or a change.
  useEffect(() => {
    if (!reuse) return;
    const s = reuse.music ?? {};
    setMode("song");
    setPrompt(reuse.prompt);
    applySound(s);
    setSections(s.sections ?? []);
    setAutoLength(!!s.auto_length);
    if (!s.auto_length && reuse.seconds) setSeconds(Math.round(reuse.seconds));
    setOwnVoice(s.own_voice ?? "");
    setPersonaId(s.persona ?? "");
    setWeird(s.weirdness != null ? Math.round(s.weirdness * 100) : null);
    setInfluence(s.influence != null ? Math.round(s.influence * 100) : null);
    setExclude(s.exclude ?? "");
    setReference(s.reference ? gallery.find((g) => g.id === s.reference) ?? null : null);
    if (reuse.lyrics) {
      setLyrics(reuse.lyrics);
      setVocals("mine");
    } else setVocals("none");
  }, [reuse]); // eslint-disable-line react-hooks/exhaustive-deps
  // These settings as a new artist, or as the picked one's.
  const saveArtist = async (asNew: boolean) => {
    const name = asNew ? (naming ?? "").trim() : persona?.name ?? "";
    if (!name) return;
    try {
      const saved = await api.musicPersonaSave({
        id: asNew ? "" : persona?.id ?? "",
        name,
        voice_words: asNew ? "" : voiceWords,
        voice_song: followVoice && reference ? reference.id : asNew ? null : persona?.voice_song ?? null,
        seed: asNew ? null : persona?.seed ?? null,
        follow_voice_song: !asNew && followVoice,
        sound: spec(),
        language: language === "auto" ? null : language,
        created_at: 0,
        updated_at: 0,
      });
      await loadPersonas();
      setPersonaId(saved.id);
      setVoiceWords(saved.voice_words);
      setFollowVoice(saved.follow_voice_song);
      setNaming(null);
      toast(
        asNew ? t("Saved {name}, with a voice of their own: {voice}.", { name, voice: saved.voice_words }) : t("Saved the changes to {name}.", { name }),
        "success",
      );
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const removeArtist = async () => {
    if (!persona) return;
    if (!removing) return setRemoving(true);
    await api.musicPersonaDelete(persona.id).catch((e) => toast(errorText(e), "error"));
    setRemoving(false);
    setPersonaId("");
    await loadPersonas();
  };
  const voiceSong = persona?.voice_song ? gallery.find((g) => g.id === persona.voice_song) : undefined;
  const tag = (s: string) => setLyrics((l) => `${l}${l && !l.endsWith("\n") ? "\n\n" : ""}[${s}]\n`);
  const ready =
    mode === "sound"
      ? !!prompt.trim()
      : mode === "song"
        ? (!!prompt.trim() || genre.length > 0 || instruments.length > 0) && !(vocals === "mine" && !lyrics.trim())
        : !!song && (task !== "cover" || !!prompt.trim() || genre.length > 0 || instruments.length > 0);
  const go = async () => {
    const ok = await onStart({
      op: mode === "sound" ? "sound" : "music",
      prompt,
      seconds: mode === "edit" ? (task === "extend" ? extendBy : undefined) : auto ? undefined : secs,
      instrumental: mode === "song" ? vocals === "none" : undefined,
      lyrics: sings && (mode === "edit" || vocals === "mine") && lyrics.trim() ? lyrics : null,
      language: sings && language !== "auto" ? language : null,
      model_id: m?.id,
      count: mode === "song" ? versions : undefined,
      music: mode === "sound" ? null : spec(),
    });
    if (ok && mode !== "edit") {
      setPrompt("");
      setTitle("");
    }
  };
  // Each layered harmony is another pass, by a slower model (about twice the song's time).
  const layers = mode === "song" && harmonyLayer && canLayer ? (singing && harmonyVoice ? 1 : 0) + (harmonyInstrument ? 1 : 0) : 0;
  const songSecs = mode === "edit" ? Math.max(song?.seconds ?? 60, 30) : auto ? autoGuess ?? 150 : secs;
  // Singing the first version again in a recorded voice: about twice the song's length.
  const revoicing = mode === "song" && !!spec().own_voice ? 2 * songSecs : 0;
  // Best of 3: each version listened to (about a quarter of its length).
  const listening = mode === "song" && bestOf && singing ? versions * songSecs * 0.25 : 0;
  const est = m ? (m.fit.est_secs * songSecs * (mode === "song" ? versions : 1) * (1 + 2 * layers)) / 30 + revoicing + listening : 0;
  return (
    <div className="form music-composer">
      <div className="mode-switch">
        <button className={`mode ${mode === "song" ? "active" : ""}`} onClick={() => setMode("song")}>{t("New song")}</button>
        <button className={`mode ${mode === "edit" ? "active" : ""}`} onClick={() => setMode("edit")}>{t("From a song you have")}</button>
        <button className={`mode ${mode === "sound" ? "active" : ""}`} onClick={() => setMode("sound")}>{t("Sound effect")}</button>
      </div>

      {mode === "song" && (
        <div className="music-field">
          <span className="music-label">{t("Artist")}</span>
          <div className="row wrap">
            <select value={personaId} onFocus={loadPersonas} onChange={(e) => pickArtist(e.target.value)} aria-label={t("Artist")}>
              <option value="">{t("No saved artist")}</option>
              {personas.map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
            {persona && naming === null && (
              <>
                <button className="btn ghost small" onClick={() => saveArtist(false)}>{t("Save changes to {name}", { name: persona.name })}</button>
                <button className="btn ghost small danger" onClick={removeArtist}>{removing ? t("Remove {name}?", { name: persona.name }) : t("Remove")}</button>
              </>
            )}
            {naming === null ? (
              <button className="btn ghost small" onClick={() => setNaming("")}>{t("Save these settings as an artist")}</button>
            ) : (
              <>
                <input className="input" value={naming} autoFocus placeholder={t("Artist name")} aria-label={t("Artist name")} onChange={(e) => setNaming(e.target.value)} onKeyDown={(e) => e.key === "Enter" && void saveArtist(true)} />
                <button className="btn small" disabled={!naming.trim()} onClick={() => saveArtist(true)}>{t("Save")}</button>
                <button className="btn ghost small" onClick={() => setNaming(null)}>{t("Cancel")}</button>
              </>
            )}
          </div>
          {persona ? (
            <>
              <label className="small row wrap">
                {t("Their voice:")}
                <input className="input grow" value={voiceWords} aria-label={t("Their voice")} onChange={(e) => setVoiceWords(e.target.value)} />
              </label>
              <label className="small row" title={t("Their songs follow the voice of their first song (or the one picked under More settings). Closer voice, but the words can come out less clear.")}>
                <input type="checkbox" checked={followVoice} onChange={(e) => setFollowVoice(e.target.checked)} disabled={!voiceSong} />
                {voiceSong ? t("Also follow the voice of “{song}”", { song: voiceSong.prompt || t("A song") }) : t("Also follow the voice of their first song, once it's made")}
              </label>
              <p className="small muted">{t("The same voice, seed and sound go into each of their songs, so they sound like one artist. Save the changes to keep edits.")}</p>
            </>
          ) : (
            <p className="small muted">{t("Save an artist to make more songs that sound like the same singer: their voice, style, instruments and way of writing are kept.")}</p>
          )}
        </div>
      )}

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

      {mode === "song" && (
        <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("Title (optional): else it's named by its hook")} aria-label={t("Title")} />
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
        <div className="music-field">
          <span className="music-label">{t("Sound like an artist")}</span>
          <div className="row wrap">
            <input
              className="input grow"
              value={soundLike}
              placeholder={t("A singer or band, e.g. Lewis Capaldi")}
              aria-label={t("Sound like an artist")}
              onChange={(e) => setSoundLike(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void lookUpArtist()}
            />
            <button className="btn small" onClick={lookUpArtist} disabled={!soundLike.trim() || lookingUp}>{lookingUp ? t("Reading their style…") : t("Use their style")}</button>
          </div>
          {artist && artist.name === soundLike.trim() && artist.sound && (
            <p className="small muted">{t("Their sound, as the music model hears it: {sound}", { sound: artist.sound })}</p>
          )}
          <p className="small muted">
            {t("Written and arranged their way and sung by a voice of the same kind, not their own voice. You can also write it in the description: \"Sing as Taylor Swift, a song about bears\".")}
          </p>
        </div>
      )}

      {mode === "song" && (
        <div className="row wrap small">
          <button className="btn small" onClick={() => fillMyTaste()} title={t("Fill in the style, mood, instruments and voice of the songs you liked and made (and leave out what's in the ones marked Not for me)")}>
            {t("Use my taste")}
          </button>
          {taste && <span className="muted ellipsis">{tasteLine(taste)}</span>}
        </div>
      )}
      {styled && (
        <>
          <PickMany label={t("Style")} options={GENRES} value={genre} onChange={setGenre} other={t("Add another style (Enter)")} />
          <PickMany label={t("Mood of the music")} options={MOODS} value={mood} onChange={setMood} other={t("Add another mood (Enter)")} />
          {(mode !== "song" || vocals !== "none") && <PickMany label={t("Emotion in the words and voice")} options={EMOTIONS} value={emotion} onChange={setEmotion} other={t("Add another feeling (Enter)")} />}
          <PickMany label={t("Instruments")} options={INSTRUMENTS} value={instruments} onChange={setInstruments} other={t("Add another (Enter)")} />
        </>
      )}

      {mode === "song" && vocals !== "none" && (m?.id === "ace-step-1.5" || m?.id === "ace-step-1.5-small") && (
        <p className="callout small">
          {t("Voices from this model can sound synthetic and get buried under the music. For clear, natural vocals, use ACE-Step 1.5 XL (every word heard in tests) or ACE-Step 1.5 Clear voice (just as quick).")}{" "}
          <button className="link" onClick={onModels}>{t("Get them")}</button>
        </p>
      )}
      {mode === "song" && vocals !== "none" && genre.some((g) => SOFT_VOCALS.includes(g)) && (
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
                <select value={vocalStyle} onChange={(e) => setVocalStyle(e.target.value)} aria-label={t("How they sing")}>
                  <option value="">{t("Any singing style")}</option>
                  {VOCAL_STYLES.map(([v, n]) => (
                    <option key={v} value={v}>{t(n)}</option>
                  ))}
                </select>
                <select value={language} onChange={(e) => setLanguage(e.target.value)} aria-label={t("Language of the vocals")}>
                  {LANGUAGES.map(([c, n]) => (
                    <option key={c} value={c}>{t(n)}</option>
                  ))}
                </select>
              </>
            )}
          </div>
          {vocals !== "none" &&
            (ownVoices.length > 0 ? (
              <label className="small row wrap">
                {t("Sung in")}
                <select value={ownVoice} onChange={(e) => setOwnVoice(e.target.value)} aria-label={t("Sung in")}>
                  <option value="">{t("The music model's singer")}</option>
                  {ownVoices.map((v) => (
                    <option key={v.id} value={v.id}>{t("{name} (recorded voice)", { name: v.name })}</option>
                  ))}
                </select>
                {ownVoice && <span className="muted">{t("The song is made with this voice's singing as its guide, then sung again in it. The version before is under ← The original. It takes a few minutes more.")}</span>}
              </label>
            ) : (
              <button className="link small" onClick={onYourVoice}>{t("Sing it in your own voice…")}</button>
            ))}
        </div>
      )}

      {mode === "song" && vocals !== "none" && <VocalEffects value={vocalEffects} onChange={setVocalEffects} />}

      {mode === "song" && (
        <SongSections
          value={sections}
          onChange={(v) => {
            setSections(v);
            // The length follows the structure (it can still be changed after).
            if (v.length) setSeconds(structureSeconds(v));
          }}
        />
      )}
      {mode === "song" && vocals !== "none" && <Chips label={t("Rhyme")} options={RHYMES} value={rhyme} onChange={setRhyme} />}

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
            <textarea
              className="input lyrics-box"
              rows={8}
              value={lyrics}
              aria-label={t("Lyrics")}
              placeholder={"[Verse]\n…\n\n[Chorus]\n…"}
              onChange={(e) => {
                setLyrics(e.target.value);
                setPicked(null);
              }}
              onSelect={(e) => pickLines(e.currentTarget)}
            />
          )}
          {picked && pickedLines.trim() && (vocals === "mine" || mode === "edit") && (
            <div className="row wrap small">
              <span className="muted">{t("{n} line(s) selected", { n: pickedLines.split("\n").filter((l) => l.trim() && !/^\[.*\]$/.test(l.trim())).length })}</span>
              <input className="input" value={how} onChange={(e) => setHow(e.target.value)} placeholder={t("How to change them (optional), e.g. more hopeful, about the sea")} aria-label={t("How to change them")} onKeyDown={(e) => e.key === "Enter" && rewriteLines()} />
              <button className="btn small" disabled={rewriting} onClick={rewriteLines}>{rewriting ? t("Writing…") : t("Write these lines again")}</button>
            </div>
          )}
          {mode === "song" && !auto && vocals === "mine" && sungLines(lyrics) > Math.round(secs / SECS_PER_LINE) * 1.25 && (
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
          <span className="music-label">{t("Harmonies")}</span>
          {vocals !== "none" && (
            <div className="filters tight wrap" role="group" aria-label={t("Harmony singers")}>
              <span className="small muted">{t("Singers:")}</span>
              {HARMONY_VOICES.map(([v, n]) => (
                <button key={v || "none"} className={`chip ${harmonyVoice === v ? "active" : ""}`} onClick={() => setHarmonyVoice(v)}>{t(n)}</button>
              ))}
            </div>
          )}
          {vocals !== "none" && harmonyVoice && (
            <div className="filters tight wrap" role="group" aria-label={t("How the harmonies are sung")}>
              <span className="small muted">{t("Sung:")}</span>
              {HARMONY_STYLES.map(([v, n, why]) => (
                <button key={v} className={`chip ${harmonyStyle === v ? "active" : ""}`} onClick={() => setHarmonyStyle(v)} title={t(why)}>{t(n)}</button>
              ))}
            </div>
          )}
        </div>
      )}
      {mode === "song" && <PickOne label={t("Harmony instrument")} options={HARMONY_INSTRUMENTS} value={harmonyInstrument} onChange={setHarmonyInstrument} other={t("Other instrument")} />}
      {mode === "song" && harmonized && (
        <div className="music-field">
          <div className="filters tight wrap" role="group" aria-label={t("Where the harmonies come in")}>
            <span className="small muted">{t("Where:")}</span>
            {HARMONY_WHERE.map(([v, n]) => (
              <button key={v} className={`chip ${harmonyWhere === v ? "active" : ""}`} onClick={() => setHarmonyWhere(v)}>{t(n)}</button>
            ))}
          </div>
          <label className="check small">
            <input type="checkbox" checked={harmonyLayer && canLayer} disabled={!canLayer} onChange={(e) => setHarmonyLayer(e.target.checked)} />
            {t("Layer them on afterwards, for fuller harmonies (takes longer)")}
          </label>
          {!canLayer && (
            <p className="small muted">
              {t("Layering needs ACE-Step 1.5 Base.")} <button className="link" onClick={onModels}>{t("Get it")}</button>
            </p>
          )}
          {vocals === "mine" && harmonyVoice && <p className="small muted">{t("In your lyrics, put what the harmony singers sing in round brackets, e.g. We'll find our way (find our way).")}</p>}
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
        <div className="music-field">
          <span className="music-label">{t("Key and time")}</span>
          <div className="row wrap">
            <select value={keyNote} onChange={(e) => setKeyNote(e.target.value)} aria-label={t("Key")}>
              <option value="">{t("Any key")}</option>
              {NOTES.map((n) => (
                <option key={n} value={n}>{n}</option>
              ))}
            </select>
            {keyNote && (
              <div className="filters tight" role="group" aria-label={t("Major or minor")}>
                <button className={`chip ${keyMode === "major" ? "active" : ""}`} onClick={() => setKeyMode("major")}>{t("Major")}</button>
                <button className={`chip ${keyMode === "minor" ? "active" : ""}`} onClick={() => setKeyMode("minor")}>{t("Minor")}</button>
              </div>
            )}
            <select value={meter} onChange={(e) => setMeter(e.target.value)} aria-label={t("Time signature")}>
              <option value="">{t("Any time signature")}</option>
              {TIMES.map(([b, u]) => `${b}/${u}`).map((x) => (
                <option key={x} value={x}>{x}</option>
              ))}
            </select>
          </div>
          {keyNote && <p className="small muted">{keyMode === "major" ? t("Major keys sound bright and settled.") : t("Minor keys sound darker or more emotional.")}</p>}
        </div>
      )}

      {mode === "song" && (
        <>
          <PickOne label={t("Lead instrument")} options={LEAD_INSTRUMENTS} value={leadInstrument} onChange={setLeadInstrument} other={t("Other instrument")} />
          <Chips label={t("Energy")} options={ENERGIES} value={energy} onChange={setEnergy} />
          <Chips label={t("Ending")} options={ENDINGS} value={ending} onChange={setEnding} />
          <Chips label={t("Era")} options={ERAS} value={era} onChange={setEra} />
          <Chips label={t("Production")} options={PRODUCTIONS} value={production} onChange={setProduction} />
        </>
      )}

      {mode !== "sound" && (
        <button className="link small music-more" onClick={() => setMore((x) => !x)} aria-expanded={more}>
          {more ? t("Fewer settings") : t("More settings: weirdness, style influence, leave out, a song to sound like")}
        </button>
      )}
      {more && mode !== "sound" && (
        <div className="row wrap small">
          {mode === "song" && (
            <label className="row music-length" title={t("How far from the usual: surprising sounds, words and structure at the top, by the book at the bottom")}>
              {t("Weirdness")}
              <input type="range" min={0} max={100} step={5} value={weird ?? 50} onChange={(e) => setWeird(+e.target.value)} />
              {weird == null ? t("Auto") : `${weird}%`}
            </label>
          )}
          <label className="row music-length" title={t("How closely the music model follows the description and style: loosely at the bottom, strictly at the top")}>
            {t("Style influence")}
            <input type="range" min={0} max={100} step={5} value={influence ?? 50} onChange={(e) => setInfluence(+e.target.value)} />
            {influence == null ? t("Auto") : `${influence}%`}
          </label>
          {(weird != null || influence != null) && (
            <button className="link small" onClick={() => { setWeird(null); setInfluence(null); }}>{t("Back to Auto")}</button>
          )}
        </div>
      )}
      {more && mode !== "sound" && (
        <input className="input" value={exclude} onChange={(e) => setExclude(e.target.value)} placeholder={t("Leave out (optional), e.g. drums, autotune, saxophone")} aria-label={t("Leave out")} />
      )}
      {more && mode !== "sound" && (
        <div className="row wrap">
          <span className="small">{t("Sound like:")}</span>
          <select value={reference?.id ?? ""} onChange={(e) => setReference(gallery.find((g) => g.id === e.target.value) ?? null)} aria-label={t("A song to sound like")}>
            <option value="">{t("No song")}</option>
            {gallery.filter((g) => g.kind === "audio").map((g) => (
              <option key={g.id} value={g.id}>{g.title || g.prompt || t("A song")}</option>
            ))}
          </select>
        </div>
      )}

      <div className="row wrap">
        {mode === "song" && (
          <button
            className={`chip ${auto ? "active" : ""}`}
            onClick={() => setAutoLength((a) => !a)}
            title={t("The song is as long as it needs: a length named in the description, your lyrics or the structure, or the lyrics written for it")}
          >
            {t("Auto length")}
          </button>
        )}
        {auto && <span className="small muted">{autoGuess ? t("about {time}", { time: mmss(autoGuess) }) : t("fits the song")}</span>}
        {mode !== "edit" && !auto && (
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
        {mode === "song" && (
          <select
            value={bestOf && singing ? "best" : versions}
            onChange={(e) => {
              const best = e.target.value === "best";
              setBestOf(best);
              setVersions(best ? 3 : +e.target.value);
            }}
            aria-label={t("Versions")}
            title={t("Make several versions at once and keep the one you like")}
          >
            <option value={1}>{t("1 version")}</option>
            <option value={2}>{t("2 versions")}</option>
            <option value={3}>{t("3 versions")}</option>
            {singing && <option value="best">{t("Best of 3 (keeps the clearest)")}</option>}
          </select>
        )}
        {mode === "song" && canArt && (
          <label className="row small" title={t("A square picture for each song from the picture model, made right after it")}>
            <input type="checkbox" checked={coverArt} onChange={(e) => setCoverArt(e.target.checked)} /> {t("Cover art")}
          </label>
        )}
        <ModelPick models={fit.length ? fit : models} value={m?.id ?? ""} onChange={setModel} onMore={onModels} />
        <DefaultLink id={m?.id} defaultId={defaultId} onSetDefault={onSetDefault} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel(est)}</span>}
        {mode === "song" && (
          <button className="btn" disabled={!ready || arranging} onClick={writeAsNotes} title={t("See and change every note in an editor first, then make it real with these settings")}>
            {arranging ? t("Writing the arrangement…") : t("Write it as notes first")}
          </button>
        )}
        <button className="btn primary" disabled={!ready || !m} onClick={go}>
          {mode === "sound" ? t("Make the sound") : mode === "edit" ? t("Make it") : t("Make the song")}
        </button>
      </div>
      {mode === "song" && (
        <p className="small muted">
          {t("Write it as notes first: the chat model arranges the song in bars and chords for every part, you can see, hear and change any note, then Make it real records it with these settings.")}
        </p>
      )}
      {mode === "sound" && <p className="small muted">{t("Sound effects come from the music model, so they're rough sketches rather than library-quality recordings.")}</p>}
      {mode === "song" && secs > 240 && <p className="small muted">{t("Long songs take a while: about {time} here.", { time: waitLabel(est) })}</p>}
    </div>
  );
}

const CLOUD_VOICES = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer"];

function NarrateComposer({ cloud, onStart, onYourVoice }: { cloud: CloudMediaChoice[]; onStart: (r: MediaRequest) => Promise<boolean>; onYourVoice: () => void }) {
  const [text, setText] = useState("");
  const [voices, setVoices] = useState<Voice[]>([]);
  const [voice, setVoice] = useState("");
  const [engine, setEngine] = useState("");
  const [cloudVoice, setCloudVoice] = useState("alloy");
  // Recorded voices that can read ("own:<id>").
  const own = useOwnVoices();
  const ownVoices = (own?.voices ?? []).filter((v) => v.can_speak);
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
            {ownVoices.length > 0 && (
              <optgroup label={t("Recorded voices")}>
                {ownVoices.map((v) => (
                  <option key={v.id} value={`own:${v.id}`}>{v.name}</option>
                ))}
              </optgroup>
            )}
            <optgroup label={t("Voices on this PC")}>
              {voices.map((v) => (
                <option key={v.id} value={v.id}>{v.name}</option>
              ))}
            </optgroup>
          </select>
        )}
        {ownVoices.length === 0 && !engine && <button className="link small" onClick={onYourVoice}>{t("Read it in your own voice…")}</button>}
        <span className="spacer" />
        <button className="btn primary" disabled={!text.trim()} onClick={go}>{t("Read aloud")}</button>
      </div>
      {!engine && voice.startsWith("own:") && (own?.speak_size ?? 0) > 0 && (
        <p className="small muted">{t("Read in your recorded voice. The first time, the voice engine and its model download ({size}).", { size: bytes(own!.speak_size) })}</p>
      )}
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
  onReuse,
  onCompose,
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
  /** A song's settings back in the song form. */
  onReuse: (i: MediaItem) => void;
  /** A composition: opened in Compose's editor. */
  onCompose: (i: MediaItem) => void;
  onChanged: (i: MediaItem) => void;
  onDeleted: (id: string) => void;
  onSaved: () => void;
}) {
  const src = useMediaUrl(item);
  const [tool, setTool] = useState<Tool>(null);
  const [text, setText] = useState("");
  const [brush, setBrush] = useState(40);
  // The song view, open; and a map being made for it.
  const [songView, setSongView] = useState(false);
  const [mapping, setMapping] = useState(false);
  useEffect(() => {
    if (mapping && item.map) {
      setMapping(false);
      setSongView(true);
    }
  }, [mapping, item.map]);
  const [painted, setPainted] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const painter = useRef<MaskHandle>(null);
  const can = (op: string) => imageModels.some((m) => m.can.includes(op));
  const ask = useAskFirst();
  // Recorded voices a song can be sung again in.
  const singers = (useOwnVoices()?.voices ?? []).filter((v) => v.can_sing);
  const [singer, setSinger] = useState("");
  // A song's cover art, its name being changed, and words for new art.
  const art = useMediaUrl(item.kind === "audio" && item.poster ? item : null, true);
  const [naming, setNaming] = useState<string | null>(null);
  const [artWords, setArtWords] = useState("");
  const rename = async () => {
    if (naming === null) return;
    try {
      onChanged(await api.mediaSetTitle(item.id, naming));
      setNaming(null);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  useEffect(() => {
    setTool(null);
    setText("");
    setNaming(null);
    setArtWords("");
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
          {item.kind === "audio" && art && <img src={art} alt={t("Cover art")} className="song-art" />}
          {item.kind === "audio" && <AudioPlayer item={item} toast={toast} onSaved={(i) => { onSaved(); onOpen(i); }} />}
        </div>
        <aside className="viewer-side">
          <div className="row">
            <strong>{OP_LABEL[item.op] ? t(OP_LABEL[item.op]) : t("Made")}</strong>
            <span className="small muted">{new Date(item.created_at).toLocaleString()}</span>
            <span className="spacer" />
            <button className="icon-btn" aria-label={t("Close")} onClick={onClose}>✕</button>
          </div>
          {item.kind === "audio" &&
            (naming !== null ? (
              <div className="row">
                <input className="input" value={naming} onChange={(e) => setNaming(e.target.value)} onKeyDown={(e) => e.key === "Enter" && rename()} aria-label={t("Title")} placeholder={t("Title")} autoFocus />
                <button className="btn small primary" onClick={rename}>{t("Save")}</button>
                <button className="btn small" onClick={() => setNaming(null)}>{t("Cancel")}</button>
              </div>
            ) : (
              <div className="row">
                <strong className="song-title">{item.title || t("Untitled")}</strong>
                <button className="link small" onClick={() => setNaming(item.title ?? "")}>{t("Rename")}</button>
              </div>
            ))}
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
          {item.composition && features.has("music") && (
            <button className="btn small primary" onClick={() => onCompose(item)}>{t("Edit its notes")}</button>
          )}
          {item.kind === "audio" && !item.composition && features.has("music") && (
            <button
              className="btn small"
              disabled={mapping}
              onClick={() => {
                if (item.map) setSongView(true);
                else {
                  setMapping(true);
                  run({ op: "map", prompt: "" });
                }
              }}
            >
              {mapping ? t("Seeing how it's built…") : t("See how it's built")}
            </button>
          )}
          {songView && item.map && (
            <SongView
              item={{ ...item, map: item.map }}
              onClose={() => setSongView(false)}
              onStart={(r) => {
                setSongView(false);
                onStart({ ...r, source: r.source ?? null });
              }}
              onSaved={(i) => {
                setSongView(false);
                onSaved();
                onOpen(i);
              }}
              onMapSaved={onChanged}
              toast={toast}
            />
          )}
          {item.kind === "audio" && item.lyrics && !item.composition && features.has("music") && (
            <SongListener
              item={item}
              onListen={() => run({ op: "listen", prompt: "" })}
              onRemake={() => run({ op: "remake", prompt: item.prompt, model_id: item.model_id })}
            />
          )}
          {item.kind === "audio" && features.has("music") && (
            <div className="viewer-tools">
              <div className="filters tight wrap">
                {(["cover", "extend", "repaint", "add", "extract"] as SongTask[]).map((task) => (
                  <button key={task} className="chip" onClick={() => onWorkOn(item, task)}>{t(TASKS.find(([id]) => id === task)![1])}</button>
                ))}
                {item.op === "music" && (
                  <button className="chip" onClick={() => onReuse(item)} title={t("Its description, sound, words and choices back in the song form, for another take or a change")}>
                    {t("Reuse these settings")}
                  </button>
                )}
                {item.op !== "stem" && item.op !== "narrate" && (
                  <button className="chip" onClick={() => run({ op: "song_video", prompt: "" })} title={t("An MP4 of the song with its cover art, title and words as they're sung, to share")}>
                    {t("Make a video")}
                  </button>
                )}
                {item.op !== "stem" && (
                  <>
                    <button className="chip" onClick={() => run({ op: "stems", prompt: "" })} title={t("The vocals and the instrumental, each saved as a song under this one")}>
                      {t("Get stems: vocals and instrumental")}
                    </button>
                    <button className="chip" onClick={() => run({ op: "stems", prompt: "", music: { track: "all" } })} title={t("The vocals, drums, bass and other instruments, each saved as a song under this one")}>
                      {t("Get stems: every part")}
                    </button>
                  </>
                )}
              </div>
              {can("generate") && (
                <div className="row wrap small">
                  <input className="input" value={artWords} onChange={(e) => setArtWords(e.target.value)} placeholder={t("What the cover shows (optional)")} aria-label={t("What the cover shows (optional)")} />
                  <button className="btn small" onClick={() => run({ op: "cover_art", prompt: artWords })} title={t("A square picture for the song from the picture model, without words in it")}>
                    {item.poster ? t("New cover art") : t("Make cover art")}
                  </button>
                </div>
              )}
              {item.op !== "narrate" && singers.length > 0 && (
                <div className="row wrap small">
                  <select value={singer} onChange={(e) => setSinger(e.target.value)} aria-label={t("Recorded voice")}>
                    {singers.map((v) => (
                      <option key={v.id} value={v.id}>{v.name}</option>
                    ))}
                  </select>
                  <button className="btn small" onClick={() => run({ op: "revoice", prompt: "", voice: singer || singers[0].id })}>{t("Sing it in this voice")}</button>
                </div>
              )}
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
            {item.kind === "audio" && (
              <button
                className={`btn small ${item.disliked ? "active" : ""}`}
                onClick={() => api.mediaDislike(item.id, !item.disliked).then(onChanged).catch((e) => toast(errorText(e), "error"))}
                aria-pressed={!!item.disliked}
                title={t("Not for you: My taste steers away from songs like it")}
              >
                {item.disliked ? t("Not for me (marked)") : t("Not for me")}
              </button>
            )}
            {item.kind === "audio" && <AddToPlaylist item={item} toast={toast} />}
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

/** My taste in a line: "country, folk · warm · acoustic guitar, pedal steel · male voice". */
function tasteLine(s: Taste): string {
  return [s.genres.join(", "), s.moods.join(", "), s.instruments.join(", "), s.voice ? t("{voice} voice", { voice: s.voice }) : ""].filter(Boolean).join(" · ");
}
