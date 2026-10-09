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
  type MediaModelCard,
  type MediaRequest,
  type Voice,
} from "../api";
import { clock } from "../format";
import { forget, OP_LABEL, useMediaUrl, waitLabel } from "../media";
import { AudioPlayer, JobCard, MediaTile, saveAs, useMediaJobs } from "../components/MediaBits";
import { MediaModels, useMediaView } from "../components/MediaModels";
import { MaskPainter, type MaskHandle } from "../components/MaskPainter";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

type Tab = "pictures" | "video" | "music" | "narrate";
type Shape = NonNullable<MediaRequest["shape"]>;

const TAB_FEATURE: Record<Tab, FeatureId> = { pictures: "images", video: "video", music: "music", narrate: "music" };
const TAB_KIND: Partial<Record<Tab, MediaKind>> = { pictures: "image", video: "video", music: "music" };

const STYLES = ["Watercolor", "Oil painting", "Pencil sketch", "Anime", "3D render", "Pixel art", "Vintage photo", "Comic book"];
const LANGUAGES: [string, string][] = [
  ["auto", "Any language"],
  ["en", "English"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["de", "German"],
  ["it", "Italian"],
  ["pt", "Portuguese"],
  ["ja", "Japanese"],
  ["ko", "Korean"],
  ["zh", "Chinese"],
];

interface Props {
  features: Set<FeatureId>;
  progress: Record<string, InstallProgress>;
  toast: PushToast;
  onGoFeatures: () => void;
}

export function StudioView({ features, progress, toast, onGoFeatures }: Props) {
  const tabs: Tab[] = (["pictures", "video", "music", "narrate"] as Tab[]).filter((t) => features.has(TAB_FEATURE[t]));
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
      } else if (d.error) toast(d.error, "error");
    });
    return () => {
      sub.then((un) => un());
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
      filters: [{ name: "Pictures, clips and sounds", extensions: ["png", "jpg", "jpeg", "webp", "webm", "mp4", "mp3", "wav", "ogg"] }],
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
          <h1>Studio</h1>
          <p className="muted">Turn on Pictures, Video or Music and audio in Features to start making things here.</p>
          <button className="btn primary" onClick={onGoFeatures}>Open Features</button>
        </div>
      </div>
    );
  }

  const installedOf = (kind: MediaKind) => (view?.models ?? []).filter((m) => m.kind === kind && m.installed);
  const kind = TAB_KIND[tab];
  const needsModel = kind && view && installedOf(kind).length === 0;

  return (
    <div className="page studio">
      <header className="page-head">
        <div>
          <h1>Studio</h1>
          <p className="muted">Everything is made on this PC and kept encrypted in your gallery.</p>
        </div>
        <div className="row">
          <button className="btn" onClick={importFile}>Import…</button>
          <button className="btn" onClick={() => setModels(null === models ? (kind ? [kind, ...(kind === "image" ? (["upscale", "background"] as MediaKind[]) : [])] : ["image", "video", "music"]) : null)}>
            Models
          </button>
        </div>
      </header>

      <div className="mode-switch studio-tabs" role="tablist">
        {tabs.map((t) => (
          <button key={t} role="tab" aria-selected={tab === t} className={`mode ${tab === t ? "active" : ""}`} onClick={() => setTab(t)}>
            {{ pictures: "🎨 Pictures", video: "🎬 Video", music: "🎵 Music and sound", narrate: "🗣 Narration" }[t]}
          </button>
        ))}
      </div>

      <section className="card studio-composer">
        {needsModel ? (
          <>
            <p>Pick a model to install for {tab === "pictures" ? "pictures" : tab === "video" ? "video" : "music"}. It downloads once and runs on this PC.</p>
            <MediaModels progress={progress} toast={toast} only={[kind!]} />
          </>
        ) : tab === "pictures" ? (
          <PictureComposer models={installedOf("image")} onStart={start} />
        ) : tab === "video" ? (
          <VideoComposer models={installedOf("video")} source={source} onPickSource={setSource} gallery={items} onStart={start} />
        ) : tab === "music" ? (
          <MusicComposer models={installedOf("music")} onStart={start} />
        ) : (
          <NarrateComposer onStart={start} />
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
              {{ all: "Everything", image: "Pictures", video: "Video", audio: "Sound", favorites: "★ Favorites" }[f]}
            </button>
          ))}
          <span className="spacer" />
          {view && <span className="small muted">{view.gallery} in the gallery</span>}
        </div>
        {items.length === 0 ? (
          <p className="muted small">Nothing here yet. What you make appears here.</p>
        ) : (
          <div className="media-grid">
            {items.map((i) => (
              <MediaTile key={i.id} item={i} selected={fresh.has(i.id)} onOpen={() => setViewing(i)} />
            ))}
          </div>
        )}
        {more && (
          <div className="center pad">
            <button className="btn" onClick={loadMore}>Show more</button>
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
              toast("Started. It appears in the gallery when it's done.", "success");
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
        <Modal title="Models for the studio" onClose={() => setModels(null)}>
          <div className="studio-models">
            <MediaModels progress={progress} toast={toast} />
          </div>
          <div className="modal-actions">
            <button className="btn" onClick={() => setModels(null)}>Close</button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function ModelPick({ models, value, onChange }: { models: MediaModelCard[]; value: string; onChange: (id: string) => void }) {
  if (models.length < 2) return null;
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} title="Model">
      {models.map((m) => (
        <option key={m.id} value={m.id}>
          {m.name}
        </option>
      ))}
    </select>
  );
}

function best(models: MediaModelCard[], can?: string): string {
  return [...models].filter((m) => !can || m.can.includes(can)).sort((a, b) => b.quality - a.quality)[0]?.id ?? "";
}

function PictureComposer({ models, onStart }: { models: MediaModelCard[]; onStart: (r: MediaRequest) => Promise<boolean> }) {
  const [prompt, setPrompt] = useState("");
  const [shape, setShape] = useState<Shape>("square");
  const [count, setCount] = useState(1);
  const [model, setModel] = useState(best(models));
  const m = models.find((x) => x.id === model) ?? models[0];
  const go = async () => {
    if (await onStart({ op: "generate", prompt, shape, count, model_id: m?.id })) setPrompt("");
  };
  return (
    <div className="form">
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder="Describe the picture: the subject, the setting, a style (photo, watercolor, 3D…), the light and mood."
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && (e.ctrlKey || e.metaKey) && prompt.trim() && go()}
        autoFocus
      />
      <div className="row wrap">
        <div className="filters tight">
          {(["square", "portrait", "landscape", "wide", "tall"] as Shape[]).map((s) => (
            <button key={s} className={`chip ${shape === s ? "active" : ""}`} onClick={() => setShape(s)}>
              {{ square: "◻ Square", portrait: "▯ Portrait", landscape: "▭ Landscape", wide: "▬ Wide", tall: "▮ Tall" }[s]}
            </button>
          ))}
        </div>
        <select value={count} onChange={(e) => setCount(+e.target.value)} title="How many">
          {[1, 2, 3, 4].map((n) => (
            <option key={n} value={n}>{n === 1 ? "1 picture" : `${n} versions`}</option>
          ))}
        </select>
        <ModelPick models={models} value={m?.id ?? ""} onChange={setModel} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel(m.fit.est_secs * count)}</span>}
        <button className="btn primary" disabled={!prompt.trim()} onClick={go}>Create</button>
      </div>
    </div>
  );
}

function VideoComposer({
  models,
  source,
  onPickSource,
  gallery,
  onStart,
}: {
  models: MediaModelCard[];
  source: MediaItem | null;
  onPickSource: (i: MediaItem | null) => void;
  gallery: MediaItem[];
  onStart: (r: MediaRequest) => Promise<boolean>;
}) {
  const [prompt, setPrompt] = useState("");
  const [seconds, setSeconds] = useState(3);
  const [shape, setShape] = useState<Shape>("landscape");
  const [model, setModel] = useState(best(models));
  const [picking, setPicking] = useState(false);
  const usable = source ? models.filter((m) => m.can.includes("image")) : models;
  const m = usable.find((x) => x.id === model) ?? usable[0];
  const thumb = useMediaUrl(source, true);
  const go = async () => {
    if (await onStart({ op: "video", prompt, seconds, shape, source: source?.id ?? null, model_id: m?.id })) {
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
        placeholder={source ? "How should the picture move? E.g. \"the camera slowly pushes in, leaves drift in the wind\"" : "Describe the clip: the subject, what happens, the camera and the style."}
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />
      <div className="row wrap">
        {source ? (
          <span className="chip active row">
            {thumb && <img src={thumb} alt="" className="chip-thumb" />} Starts from this picture
            <button className="link" onClick={() => onPickSource(null)}>✕</button>
          </span>
        ) : (
          <button className="btn small" onClick={() => setPicking(true)}>Start from a picture…</button>
        )}
        {!source && (
          <div className="filters tight">
            {(["landscape", "portrait", "square"] as Shape[]).map((s) => (
              <button key={s} className={`chip ${shape === s ? "active" : ""}`} onClick={() => setShape(s)}>
                {({ landscape: "▭ Landscape", portrait: "▯ Portrait", square: "◻ Square" } as Record<string, string>)[s]}
              </button>
            ))}
          </div>
        )}
        <select value={seconds} onChange={(e) => setSeconds(+e.target.value)} title="Length">
          {[2, 3, 5, 8].map((s) => (
            <option key={s} value={s}>{s} seconds</option>
          ))}
        </select>
        <ModelPick models={usable} value={m?.id ?? ""} onChange={setModel} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel((m.fit.est_secs * seconds) / Math.max(1, m.defaults.seconds || 3))}</span>}
        <button className="btn primary" disabled={(!prompt.trim() && !source) || !m} onClick={go}>Make video</button>
      </div>
      {source && !m && <p className="small warn-text">Bringing a picture to life needs Wan 2.2. Install it from Models.</p>}
      {picking && (
        <Modal title="Start from a picture" onClose={() => setPicking(false)}>
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
          {!gallery.some((i) => i.kind === "image") && <p className="muted small">No pictures in the gallery yet.</p>}
        </Modal>
      )}
    </div>
  );
}

function MusicComposer({ models, onStart }: { models: MediaModelCard[]; onStart: (r: MediaRequest) => Promise<boolean> }) {
  const [sound, setSound] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [vocals, setVocals] = useState<"write" | "none" | "mine">("write");
  const [lyrics, setLyrics] = useState("");
  const [seconds, setSeconds] = useState(30);
  const [language, setLanguage] = useState("auto");
  const [model, setModel] = useState(best(models));
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
        <button className={`mode ${!sound ? "active" : ""}`} onClick={() => setSound(false)}>Song</button>
        <button className={`mode ${sound ? "active" : ""}`} onClick={() => setSound(true)}>Sound effect</button>
      </div>
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder={sound ? "Describe the sound, e.g. \"rain on a tin roof with distant thunder\"" : "Describe the song: genre, mood, instruments, tempo, voice. E.g. \"upbeat acoustic folk about a road trip, warm male vocals\""}
        onChange={(e) => setPrompt(e.target.value)}
        autoFocus
      />
      {!sound && (
        <div className="row wrap">
          <div className="filters tight">
            {(["write", "none", "mine"] as const).map((v) => (
              <button key={v} className={`chip ${vocals === v ? "active" : ""}`} onClick={() => setVocals(v)}>
                {{ write: "Vocals, lyrics written for me", none: "Instrumental", mine: "My own lyrics" }[v]}
              </button>
            ))}
          </div>
          {vocals !== "none" && (
            <select value={language} onChange={(e) => setLanguage(e.target.value)} title="Language of the vocals">
              {LANGUAGES.map(([c, n]) => (
                <option key={c} value={c}>{n}</option>
              ))}
            </select>
          )}
        </div>
      )}
      {!sound && vocals === "mine" && (
        <textarea className="input" rows={6} value={lyrics} placeholder={"[Verse]\nYour lines here\n\n[Chorus]\n…"} onChange={(e) => setLyrics(e.target.value)} />
      )}
      <div className="row wrap">
        <select value={secs} onChange={(e) => setSeconds(+e.target.value)} title="Length">
          {(sound ? [10, 15, 20, 30] : [15, 30, 60, 90, 120, 180]).map((s) => (
            <option key={s} value={s}>{s < 60 ? `${s} seconds` : `${s / 60} min${s % 60 ? ` ${s % 60} s` : ""}`}</option>
          ))}
        </select>
        <ModelPick models={models} value={m?.id ?? ""} onChange={setModel} />
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel((m.fit.est_secs * secs) / 30)}</span>}
        <button className="btn primary" disabled={!prompt.trim() || (vocals === "mine" && !sound && !lyrics.trim())} onClick={go}>
          {sound ? "Make the sound" : "Make the song"}
        </button>
      </div>
      {sound && <p className="small muted">Sound effects come from the music model, so they're rough sketches rather than library-quality recordings.</p>}
    </div>
  );
}

function NarrateComposer({ onStart }: { onStart: (r: MediaRequest) => Promise<boolean> }) {
  const [text, setText] = useState("");
  const [voices, setVoices] = useState<Voice[]>([]);
  const [voice, setVoice] = useState("");
  useEffect(() => {
    api.voices().then((v) => {
      setVoices(v);
      if (v[0]) setVoice(v[0].id);
    }).catch(() => {});
  }, []);
  const go = async () => {
    if (await onStart({ op: "narrate", prompt: text, voice: voice || null })) setText("");
  };
  return (
    <div className="form">
      <textarea className="input" rows={6} value={text} placeholder="The text to read aloud. It's saved as a sound you can play, trim and save." onChange={(e) => setText(e.target.value)} autoFocus />
      <div className="row wrap">
        <select value={voice} onChange={(e) => setVoice(e.target.value)} title="Voice">
          {voices.map((v) => (
            <option key={v.id} value={v.id}>{v.name}</option>
          ))}
        </select>
        <span className="spacer" />
        <button className="btn primary" disabled={!text.trim()} onClick={go}>Read aloud</button>
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

  useEffect(() => {
    setTool(null);
    setText("");
  }, [item.id]);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && !confirm && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose, confirm]);

  const run = (req: Omit<MediaRequest, "source">) => onStart({ ...req, source: item.id });
  const fill = async () => {
    const mask = await painter.current?.mask();
    if (!mask) return toast("Paint over the part to change first.", "error");
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
      <div className="modal viewer" role="dialog" aria-modal="true" aria-label="Gallery item">
        <div className="viewer-stage">
          {item.kind === "image" && src && (tool === "fill" ? (
            <MaskPainter ref={painter} src={src} width={item.width} height={item.height} brush={brush} onPainted={setPainted} />
          ) : (
            <img src={src} alt={item.prompt || "Picture"} className={item.mime === "image/png" ? "checker" : ""} />
          ))}
          {item.kind === "video" && src && <video src={src} controls autoPlay loop playsInline />}
          {item.kind === "audio" && <AudioPlayer item={item} toast={toast} onSaved={(i) => { onSaved(); onOpen(i); }} />}
        </div>
        <aside className="viewer-side">
          <div className="row">
            <strong>{OP_LABEL[item.op] ?? "Made"}</strong>
            <span className="small muted">{new Date(item.created_at).toLocaleString()}</span>
            <span className="spacer" />
            <button className="icon-btn" aria-label="Close" onClick={onClose}>✕</button>
          </div>
          {item.prompt && <p className="viewer-prompt small">{item.prompt}</p>}
          <p className="small muted">
            {item.kind === "image" ? `${item.width} × ${item.height}` : clock(item.seconds)}
            {item.kind === "video" && ` · ${item.width} × ${item.height}`}
            {item.seed != null && ` · seed ${item.seed}`}
          </p>
          {item.parent && (
            <button className="link small" onClick={() => api.mediaGet(item.parent!).then(onOpen).catch(() => toast("The original is no longer in the gallery.", "error"))}>
              ← The original
            </button>
          )}
          {item.lyrics && (
            <details className="small">
              <summary>Lyrics</summary>
              <pre className="lyrics">{item.lyrics}</pre>
            </details>
          )}

          {item.kind === "image" && features.has("images") && (
            <div className="viewer-tools">
              <div className="filters tight">
                {can("edit") && <button className={`chip ${tool === "edit" ? "active" : ""}`} onClick={() => setTool(tool === "edit" ? null : "edit")}>✏️ Change it</button>}
                {can("fill") && <button className={`chip ${tool === "fill" ? "active" : ""}`} onClick={() => setTool(tool === "fill" ? null : "fill")}>🖌 Paint to fill in</button>}
                {can("extend") && <button className={`chip ${tool === "extend" ? "active" : ""}`} onClick={() => setTool(tool === "extend" ? null : "extend")}>↔ Extend</button>}
                {can("restyle") && <button className={`chip ${tool === "restyle" ? "active" : ""}`} onClick={() => setTool(tool === "restyle" ? null : "restyle")}>🎨 Restyle</button>}
              </div>
              {tool === "edit" && (
                <div className="form">
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder="E.g. make it night, add a red scarf, remove the car" autoFocus onKeyDown={(e) => e.key === "Enter" && text.trim() && run({ op: "edit", prompt: text })} />
                  <button className="btn primary small" disabled={!text.trim()} onClick={() => run({ op: "edit", prompt: text })}>Change it</button>
                </div>
              )}
              {tool === "fill" && (
                <div className="form">
                  <span className="small muted">Paint over the part to change. Describe what goes there, or leave it empty to remove what's there.</span>
                  <label className="small row">
                    Brush <input type="range" min={10} max={120} value={brush} onChange={(e) => setBrush(+e.target.value)} />
                  </label>
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder="What goes there (optional)" />
                  <div className="row">
                    <button className="btn small" disabled={!painted} onClick={() => painter.current?.clear()}>Clear</button>
                    <button className="btn primary small" disabled={!painted} onClick={fill}>Fill in</button>
                  </div>
                </div>
              )}
              {tool === "extend" && (
                <div className="form">
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder="What's beyond the edges (optional)" />
                  <div className="row">
                    <button className="btn small" onClick={() => extend("wide")}>Wider</button>
                    <button className="btn small" onClick={() => extend("tall")}>Taller</button>
                    <button className="btn small" onClick={() => extend("all")}>All around</button>
                  </div>
                </div>
              )}
              {tool === "restyle" && (
                <div className="form">
                  <div className="filters tight">
                    {STYLES.map((s) => (
                      <button key={s} className="chip" onClick={() => run({ op: "restyle", prompt: s })}>{s}</button>
                    ))}
                  </div>
                  <input className="input" value={text} onChange={(e) => setText(e.target.value)} placeholder="Or describe a style" onKeyDown={(e) => e.key === "Enter" && text.trim() && run({ op: "restyle", prompt: text })} />
                </div>
              )}
              <div className="row wrap">
                <button className="btn small" onClick={() => run({ op: "upscale", prompt: "" })} title="Four times larger and sharper">⤢ Upscale 4×</button>
                <button className="btn small" onClick={() => run({ op: "remove_background", prompt: "" })}>✂ Remove background</button>
                {features.has("video") && <button className="btn small" onClick={() => onAnimate(item)}>🎬 Bring to life</button>}
                {item.op === "generate" && item.prompt && (
                  <button className="btn small" onClick={() => onStart({ op: "generate", prompt: item.prompt, model_id: item.model_id, shape: item.width > item.height ? "landscape" : item.width < item.height ? "portrait" : "square" })}>
                    ↻ More like this
                  </button>
                )}
              </div>
            </div>
          )}

          <span className="spacer" />
          <div className="row">
            <button
              className="btn small"
              onClick={() => api.mediaFavorite(item.id, !item.favorite).then(onChanged).catch((e) => toast(errorText(e), "error"))}
              aria-pressed={item.favorite}
            >
              {item.favorite ? "★ Favorite" : "☆ Favorite"}
            </button>
            <button className="btn small" onClick={() => saveAs(item, toast)}>Save as…</button>
            <span className="spacer" />
            <button className="btn ghost danger small" onClick={() => setConfirm(true)}>Delete</button>
          </div>
        </aside>
      </div>
      {confirm && (
        <Modal title="Delete this from the gallery?" onClose={() => setConfirm(false)}>
          <p>It's deleted from this PC. Copies you saved elsewhere stay.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(false)}>Cancel</button>
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
              Delete
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
