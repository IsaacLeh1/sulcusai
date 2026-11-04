// SPDX-License-Identifier: AGPL-3.0-only
import { CreatePanel } from "../components/CreatePanel";
import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type FeatureId, type InstallProgress, type MediaItem, type MediaKind, type CloudMediaChoice, type Composition, type MediaModelCard, type MediaRequest, type Voice } from "../api";
import { bytes, clock } from "../format";
import { t, tf, tx } from "../i18n";
import { forget, forgetThumb, OP_LABEL, useMediaUrl, waitLabel } from "../media";
import { AudioPlayer, JobCard, MadeStatsLine, MediaTile, saveAs, SongListener, useMediaJobs } from "../components/MediaBits";
import { SongView } from "../components/SongView";
import { ComposeEditor } from "../components/ComposeEditor";
import { TASKS, type SongTask } from "../components/MusicFields";
import { MediaModels, useMediaView } from "../components/MediaModels";
import { MaskPainter, type MaskHandle } from "../components/MaskPainter";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { asksFirst, ExamplePicker, PosePicker, PictureQuestions, PromptHelpPanel, SetWallpaperButton, useAskFirst, usePromptHelp, WallpaperComposer, type ExampleUse } from "../components/StudioExtras";
import { OwnVoicePanel, useOwnVoices } from "../components/OwnVoice";
import { LiveVoice } from "../components/LiveVoice";
import { LanguageSelect, namesIn, PodcastComposer, ScriptVoices, talkLanguage, VoiceSelect } from "../components/Podcast";
import { AddToPlaylist, Playlists } from "../components/Library";
import { LoraField } from "../components/LoraField";
import { VideoEditor } from "../components/VideoEditor";
import { SpeakPanel } from "../components/SpeakPanel";
import { NewWindowIcon } from "../components/Icons";
import { SongStudio } from "../components/SongStudio";
import { projectForSong } from "../studio/open";
import { StudioProjects } from "../components/StudioProjects";
import type { Project } from "../studio/model";
import { cloudCards, ModelPick, useModelPick, DefaultLink, ImproveButton, type Shape } from "../components/StudioBits";
import { MusicComposer } from "../components/MusicComposer";
import { VideoComposer } from "../components/VideoComposer";

type Tab = "pictures" | "video" | "wallpaper" | "music" | "narrate" | "podcast" | "voice" | "create";

const TAB_FEATURE: Record<Tab, FeatureId> = { pictures: "images", video: "video", wallpaper: "images", music: "music", narrate: "music", podcast: "music", voice: "music", create: "images" };
const TAB_KIND: Partial<Record<Tab, MediaKind>> = { pictures: "image", video: "video", wallpaper: "image", music: "music" };
const ALL_KINDS: MediaKind[] = ["image", "upscale", "background", "video", "music"];

const STYLES = [tx("Watercolor"), tx("Oil painting"), tx("Pencil sketch"), tx("Anime"), tx("3D render"), tx("Pixel art"), tx("Vintage photo"), tx("Comic book")];

interface Props {
  features: Set<FeatureId>;
  progress: Record<string, InstallProgress>;
  toast: PushToast;
  onGoFeatures: () => void;
  /** Opens the Studio in another window of its own. */
  onPopOut?: () => void;
}

export function StudioView({ features, progress, toast, onGoFeatures, onPopOut }: Props) {
  const tabs: Tab[] = (["pictures", "video", "wallpaper", "music", "narrate", "podcast", "voice", "create"] as Tab[]).filter((tb) => features.has(TAB_FEATURE[tb]));
  // A song written as notes, open in Compose's editor (item: where it's kept).
  const [composing, setComposing] = useState<{ item: MediaItem | null; composition: Composition } | null>(null);
  const [tab, setTab] = useState<Tab>(tabs[0] ?? "pictures");
  // A song picked in the viewer to work on in the music tab.
  const [songToWork, setSongToWork] = useState<{ item: MediaItem; task: SongTask; reference?: MediaItem } | null>(null);
  // A song whose settings fill the song form again (Reuse these settings).
  const [songToReuse, setSongToReuse] = useState<MediaItem | null>(null);
  // A playlist's songs, whose shared sound fills the song form.
  const [inspire, setInspire] = useState<string[] | null>(null);
  const [view, reloadView] = useMediaView();
  const jobs = useMediaJobs();
  const [items, setItems] = useState<MediaItem[]>([]);
  const [more, setMore] = useState(false);
  const [filter, setFilter] = useState<"all" | "image" | "video" | "audio" | "favorites" | "playlists" | "projects">("all");
  // A project open in the song studio.
  const [studioProject, setStudioProject] = useState<Project | null>(null);
  const [viewing, setViewing] = useState<MediaItem | null>(null);
  const [models, setModels] = useState<MediaKind[] | null>(null);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  const [source, setSource] = useState<MediaItem | null>(null);

  /** A song studio project opened: brought forward where it's open in a window of its own, else here. */
  const openProject = (p: Project) => {
    api
      .openSongStudioWindow(p.id, true)
      .then((there) => !there && setStudioProject(p))
      .catch(() => setStudioProject(p));
  };

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
  // Video models for clips: not the one that only moves lips to a voice (Wan 2.2 S2V).
  const clipModels = installedOf("video").filter((m) => !(m as { can?: string[] }).can?.includes("speech"));
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
          {onPopOut && (
            <button className="icon-btn" onClick={onPopOut} aria-label={t("Open the Studio in a new window")} title={t("Open the Studio in a new window: pictures, video and music beside your chats (open as many as you like)")}>
              <NewWindowIcon />
            </button>
          )}
          <button className="btn" onClick={importFile}>{t("Import…")}</button>
          <button className="btn" onClick={() => setModels(null === models ? (kind ? [kind, ...(kind === "image" ? (["upscale", "background"] as MediaKind[]) : [])] : ["image", "video", "music"]) : null)}>
            {t("Models")}
          </button>
        </div>
      </header>

      <div className="mode-switch studio-tabs" role="tablist">
        {tabs.map((tb) => (
          <button key={tb} role="tab" aria-selected={tab === tb} className={`mode ${tab === tb ? "active" : ""}`} onClick={() => setTab(tb)}>
            {{ pictures: t("Pictures"), video: t("Video"), wallpaper: t("Wallpapers"), music: t("Music and sound"), narrate: t("Narration"), podcast: t("Podcast"), voice: t("Your voice"), create: t("Create") }[tb]}
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
            models={clipModels}
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
          <WallpaperComposer imageModels={installedOf("image")} videoModels={features.has("video") ? clipModels : []} gallery={items} onStart={start} toast={toast} />
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
        ) : tab === "create" ? (
          <CreatePanel gallery={items} onStart={start} toast={toast} />
        ) : tab === "voice" ? (
          <>
            <OwnVoicePanel toast={toast} />
            <LiveVoice toast={toast} />
          </>
        ) : tab === "podcast" ? (
          <PodcastComposer onStart={start} onYourVoice={() => setTab("voice")} />
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
          {(["all", "image", "video", "audio", "favorites", "playlists", "projects"] as const).map((f) => (
            <button key={f} className={`chip ${filter === f ? "active" : ""}`} onClick={() => setFilter(f)}>
              {{ all: t("Everything"), image: t("Pictures"), video: t("Video"), audio: t("Sound"), favorites: t("★ Favorites"), playlists: t("Playlists"), projects: t("Song studio") }[f]}
            </button>
          ))}
          <span className="spacer" />
          {view && <span className="small muted">{t("{n} in the gallery", { n: view.gallery })}</span>}
        </div>
        {filter === "projects" ? (
          <StudioProjects onOpen={openProject} toast={toast} />
        ) : filter === "playlists" ? (
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

      {studioProject && (
        <SongStudio
          initial={studioProject}
          songs={items.filter((i) => i.kind === "audio")}
          onClose={() => {
            setStudioProject(null);
            load();
          }}
          onPopOut={() => {
            api.openSongStudioWindow(studioProject.id).catch((e) => toast(errorText(e), "error"));
            setStudioProject(null);
          }}
          toast={toast}
        />
      )}

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
          onOpenStudio={(i) =>
            projectForSong(i)
              .then((pr) => {
                setViewing(null);
                setStudioProject(pr);
              })
              .catch((e) => toast(errorText(e), "error"))
          }
          songs={items.filter((x) => x.kind === "audio" && x.id !== viewing.id && x.op !== "stem")}
          onMashup={(a, b) => {
            setSongToWork({ item: a, task: "cover", reference: b });
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
  const [loras, setLoras] = useState<[string, number][]>([]);
  const [pose, setPose] = useState<MediaItem | null>(null);
  useEffect(() => {
    const same = (e: Event) => {
      const item = (e as CustomEvent<MediaItem>).detail;
      setExamples([item]);
      setUse("subject");
      document.querySelector(".picture-composer")?.scrollIntoView({ behavior: "smooth", block: "start" });
    };
    window.addEventListener("studio:same-character", same);
    return () => window.removeEventListener("studio:same-character", same);
  }, []);
  const help = usePromptHelp(m?.id);
  const ask = useAskFirst();
  const local = !!m && !m.id.startsWith("cloud:");
  const make = async (description: string) => {
    const extra: Partial<MediaRequest> = local ? { examples: examples.map((e) => e.id), example_use: use, write_prompt: help.write, check: help.check, loras, pose: pose?.id ?? null } : {};
    if (await onStart({ op: "generate", prompt: description, shape, count, style, model_id: m?.id, ...extra })) {
      setPrompt("");
      setExamples([]);
      setPose(null);
      ask.clear();
    }
  };
  const go = async () => {
    if (ask.asking) return;
    if (help.ask && !(await ask.check("generate", prompt, null, m?.id ?? null, local ? examples.length : 0))) return;
    await make(prompt);
  };
  return (
    <div className="form picture-composer">
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
      {local && m?.can.includes("edit") && <PosePicker value={pose} onChange={setPose} gallery={gallery} toast={toast} />}
      {local && m && <LoraField modelId={m.id} value={loras} onChange={setLoras} toast={toast} />}
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

const CLOUD_VOICES = ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer"];

function NarrateComposer({ cloud, onStart, onYourVoice }: { cloud: CloudMediaChoice[]; onStart: (r: MediaRequest) => Promise<boolean>; onYourVoice: () => void }) {
  const [text, setText] = useState("");
  const [voices, setVoices] = useState<Voice[]>([]);
  const [voice, setVoice] = useState("");
  const [engine, setEngine] = useState("");
  const [cloudVoice, setCloudVoice] = useState("alloy");
  const [lang, setLang] = useState(talkLanguage());
  // A script read in several voices: each name's voice.
  const [several, setSeveral] = useState(false);
  const [picks, setPicks] = useState<Record<string, string | null>>({});
  // Recorded and made voices that can read ("own:<id>").
  const own = useOwnVoices();
  const ownVoices = (own?.voices ?? []).filter((v) => v.can_speak);
  useEffect(() => {
    api.voices().then((v) => {
      setVoices(v);
      if (v[0]) setVoice(v[0].id);
    }).catch(() => {});
  }, []);
  const go = async () => {
    const req: MediaRequest = several
      ? { op: "dialogue", prompt: text, language: lang, talk: { speakers: namesIn(text).map((name) => ({ name, voice: picks[name] ?? null })) } }
      : engine
        ? { op: "narrate", prompt: text, voice: cloudVoice, model_id: engine }
        : { op: "narrate", prompt: text, voice: voice || null, language: lang };
    if (await onStart(req)) setText("");
  };
  const usesOwn = several ? Object.values(picks).some((v) => v?.startsWith("own:")) : !engine && voice.startsWith("own:");
  return (
    <div className="form">
      <div className="filters tight">
        <button className={`chip ${!several ? "active" : ""}`} onClick={() => setSeveral(false)}>{t("One voice")}</button>
        <button className={`chip ${several ? "active" : ""}`} onClick={() => setSeveral(true)}>{t("Several voices")}</button>
      </div>
      <textarea
        className="input"
        rows={6}
        value={text}
        placeholder={several ? t("Ana: Welcome back to the show!\nBen: Thanks, it's good to be here.\nAna: So, where do we start?") : t("The text to read aloud. It's saved as a sound you can play, trim and save.")}
        aria-label={several ? t("The script: one line for each turn, “Name: what they say”") : t("The text to read aloud. It's saved as a sound you can play, trim and save.")}
        onChange={(e) => setText(e.target.value)}
        autoFocus
      />
      {several && <ScriptVoices script={text} picks={picks} onPick={(n, v) => setPicks((p) => ({ ...p, [n]: v }))} voices={voices} own={ownVoices} />}
      <div className="row wrap">
        {cloud.length > 0 && !several && (
          <select value={engine} onChange={(e) => setEngine(e.target.value)} title={t("Voices from")}>
            <option value="">{t("This PC's voices")}</option>
            {cloud.map((c) => (
              <option key={c.id} value={c.id}>{c.name} ({c.provider})</option>
            ))}
          </select>
        )}
        {several ? null : engine ? (
          <select value={cloudVoice} onChange={(e) => setCloudVoice(e.target.value)} title={t("Voice")}>
            {CLOUD_VOICES.map((v) => (
              <option key={v} value={v}>{v[0].toUpperCase() + v.slice(1)}</option>
            ))}
          </select>
        ) : (
          <VoiceSelect value={voice || null} onChange={(v) => setVoice(v ?? "")} voices={voices} own={ownVoices} label={t("Voice")} />
        )}
        {!engine && <LanguageSelect value={lang} onChange={setLang} />}
        {ownVoices.length === 0 && !engine && <button className="link small" onClick={onYourVoice}>{t("Read it in your own voice…")}</button>}
        <span className="spacer" />
        <button className="btn primary" disabled={!text.trim() || (several && namesIn(text).length === 0)} onClick={go}>{t("Read aloud")}</button>
      </div>
      {usesOwn && (own?.speak_size ?? 0) > 0 && (
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
  onOpenStudio,
  songs,
  onMashup,
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
  /** A song opened in the song studio. */
  onOpenStudio: (i: MediaItem) => void;
  /** Other songs in the gallery, to mash this one up with. */
  songs: MediaItem[];
  /** A mashup: the first song's tune and words in the second's sound (a cover). */
  onMashup: (a: MediaItem, b: MediaItem) => void;
  /** A composition: opened in Compose's editor. */
  onCompose: (i: MediaItem) => void;
  onChanged: (i: MediaItem) => void;
  onDeleted: (id: string) => void;
  onSaved: () => void;
}) {
  const src = useMediaUrl(item);
  const [tool, setTool] = useState<Tool>(null);
  const [talking, setTalking] = useState(false);
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
          {item.lyrics && item.op === "captions" && (
            <details className="small">
              <summary>{t("Captions")}</summary>
              <pre className="lyrics">{item.lyrics}</pre>
              <div className="row">
                <button className="btn ghost small" onClick={() => navigator.clipboard.writeText(item.lyrics ?? "").then(() => toast(t("Copied."), "success"))}>{t("Copy")}</button>
                <button
                  className="btn ghost small"
                  onClick={async () => {
                    const path = await saveFile({ defaultPath: "captions.srt", filters: [{ name: t("Subtitles"), extensions: ["srt"] }] });
                    if (!path) return;
                    try {
                      await api.mediaSaveCaptions(item.id, path);
                      toast(t("Saved the captions."), "success");
                    } catch (e) {
                      toast(errorText(e), "error");
                    }
                  }}
                >
                  {t("Save as .srt")}
                </button>
              </div>
            </details>
          )}
          {item.lyrics && item.op !== "captions" && (
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
                {item.op === "music" && songs.length > 0 && (
                  <select
                    className="chip"
                    value=""
                    aria-label={t("Mash up with another song")}
                    title={t("This song's tune and words in another song's sound: a cover that follows it")}
                    onChange={(e) => {
                      const other = songs.find((x) => x.id === e.target.value);
                      if (other) onMashup(item, other);
                    }}
                  >
                    <option value="">{t("Mash up with…")}</option>
                    {songs.map((x) => (
                      <option key={x.id} value={x.id}>{x.title || x.prompt || t("A song")}</option>
                    ))}
                  </select>
                )}
                {item.op !== "narrate" && (
                  <button className="chip" onClick={() => onOpenStudio(item)} title={t("Its tracks as clips to arrange, mix, record into and add parts to")}>
                    {t("Open in the song studio")}
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
              <div className="row wrap small">
                {can("generate") && (
                  <>
                    <input className="input" value={artWords} onChange={(e) => setArtWords(e.target.value)} placeholder={t("What the cover shows (optional)")} aria-label={t("What the cover shows (optional)")} />
                    <button className="btn small" onClick={() => run({ op: "cover_art", prompt: artWords })} title={t("A square picture for the song from the picture model, without words in it")}>
                      {item.poster ? t("New cover art") : t("Make cover art")}
                    </button>
                  </>
                )}
                <button
                  className="btn small ghost"
                  title={t("A picture of your own as the song's cover (cut to a square)")}
                  onClick={async () => {
                    try {
                      const p = await openFile({ multiple: false, filters: [{ name: t("Pictures"), extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp"] }] });
                      if (typeof p === "string") onChanged(await api.mediaSetCover(item.id, p));
                    } catch (e) {
                      toast(errorText(e), "error");
                    }
                  }}
                >
                  {t("Use my own picture…")}
                </button>
              </div>
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
                <button
                  className="chip"
                  title={t("A new picture with the same character or subject as this one, in a scene you describe")}
                  onClick={() => {
                    window.dispatchEvent(new CustomEvent("studio:same-character", { detail: item }));
                    onClose();
                  }}
                >
                  {t("Same character, new picture")}
                </button>
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
                {features.has("video") && <button className={`btn small ${talking ? "active" : ""}`} onClick={() => setTalking((v) => !v)} title={t("Its face and lips move to words, a song or a sound")}>{t("Make it talk")}</button>}
                <SetWallpaperButton item={item} toast={toast} />
                {item.op === "generate" && item.prompt && (
                  <button className="btn small" onClick={() => onStart({ op: "generate", prompt: item.prompt, model_id: item.model_id, shape: item.width > item.height ? "landscape" : item.width < item.height ? "portrait" : "square" })}>
                    {t("↻ More like this")}
                  </button>
                )}
              </div>
              {talking && features.has("video") && <SpeakPanel run={(r) => (run(r), setTalking(false))} />}
            </div>
          )}

          {item.kind === "video" && item.op === "wallpaper" && (
            <div className="row">
              <SetWallpaperButton item={item} toast={toast} />
            </div>
          )}
          {item.kind === "video" && features.has("video") && <VideoEditor item={item} run={run} video={() => document.querySelector<HTMLVideoElement>(".viewer video")} />}
          <span className="spacer" />
          {item.kind === "audio" && (
            <div className="row wrap">
              <button
                className={`btn small ${item.disliked ? "active" : ""}`}
                onClick={() => api.mediaDislike(item.id, !item.disliked).then(onChanged).catch((e) => toast(errorText(e), "error"))}
                aria-pressed={!!item.disliked}
                title={t("Not for you: My taste steers away from songs like it")}
              >
                {item.disliked ? t("Not for me (marked)") : t("Not for me")}
              </button>
              <AddToPlaylist item={item} toast={toast} />
            </div>
          )}
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
