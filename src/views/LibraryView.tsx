// SPDX-License-Identifier: AGPL-3.0-only
// The Library: the Media index and player, Photos, Podcasts, News, Books, live captions and screen history.
import { useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import {
  byMonth,
  clock,
  cueAt,
  dayStart,
  medialibApi as lib,
  readProgress,
  type Book,
  type Cue,
  type Feed,
  type IndexEntry,
  type IndexHit,
  type IndexSummary,
  type Photo,
  type SavedFeed,
  type Shot,
  type TimelineSettings,
} from "../api/medialib";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";
import { confirmed } from "../confirm";

type Tab = "media" | "photos" | "podcasts" | "news" | "books" | "captions" | "history";

const MEDIA_KINDS = ["mp4", "webm", "mkv", "mov", "m4v", "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus"];

export function LibraryView({ toast }: { toast: PushToast }) {
  const [tab, setTab] = useState<Tab>("media");
  const [playing, setPlaying] = useState<{ path: string; at?: number } | null>(null);
  const tabs: [Tab, string][] = [
    ["media", t("Media")],
    ["photos", t("Photos")],
    ["podcasts", t("Podcasts")],
    ["news", t("News")],
    ["books", t("Books")],
    ["captions", t("Live captions")],
    ["history", t("Screen history")],
  ];
  const play = (path: string, at?: number) => {
    setPlaying({ path, at });
    setTab("media");
  };
  return (
    <div className="page library-page">
      <div className="folder-chips" role="tablist">
        {tabs.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={`chip ${tab === id ? "active" : ""}`} onClick={() => setTab(id)}>
            {label}
          </button>
        ))}
      </div>
      {tab === "media" && <MediaTab toast={toast} playing={playing} onPlay={play} />}
      {tab === "photos" && <PhotosTab toast={toast} />}
      {tab === "podcasts" && <FeedsTab toast={toast} podcasts onPlay={(url) => setPlaying({ path: url })} />}
      {tab === "news" && <FeedsTab toast={toast} podcasts={false} />}
      {tab === "books" && <BooksTab toast={toast} />}
      {tab === "captions" && <CaptionsTab toast={toast} />}
      {tab === "history" && <HistoryTab toast={toast} />}
    </div>
  );
}

// ---------- the player ----------

function Player({ path, at, toast }: { path: string; at?: number; toast: PushToast }) {
  const [src, setSrc] = useState<string | null>(null);
  const [cues, setCues] = useState<Cue[]>([]);
  const [now, setNow] = useState(0);
  const el = useRef<HTMLVideoElement>(null);
  const web = /^https?:\/\//.test(path);
  useEffect(() => {
    setSrc(null);
    setCues([]);
    if (web) {
      setSrc(path);
      return;
    }
    lib
      .playFile(path)
      .then(setSrc)
      .catch((e) => toast(errorText(e), "error"));
    lib
      .findSubtitles(path)
      .then((found) => (found[0] ? lib.subtitles(found[0]).then(setCues) : undefined))
      .catch(() => {});
  }, [path]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (src && el.current && at !== undefined) el.current.currentTime = at;
  }, [src, at]);
  const pickSubs = async () => {
    const file = await openDialog({ filters: [{ name: t("Subtitles"), extensions: ["srt", "vtt"] }] });
    if (typeof file === "string") lib.subtitles(file).then(setCues).catch((e) => toast(errorText(e), "error"));
  };
  const line = cueAt(cues, now);
  return (
    <div className="library-player">
      {src ? <video ref={el} src={src} controls autoPlay playsInline onTimeUpdate={(e) => setNow(e.currentTarget.currentTime)} /> : <p className="muted small">{t("Opening...")}</p>}
      {line && <div className="library-subtitle">{line.text}</div>}
      <div className="row">
        <span className="muted small">{path.split(/[\\/]/).pop()}</span>
        {!web && <button className="btn small" onClick={pickSubs}>{cues.length ? t("Other subtitles") : t("Add subtitles")}</button>}
      </div>
    </div>
  );
}

// ---------- the Media index ----------

function MediaTab({ toast, playing, onPlay }: { toast: PushToast; playing: { path: string; at?: number } | null; onPlay: (path: string, at?: number) => void }) {
  const [list, setList] = useState<IndexSummary[] | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<IndexHit[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [open, setOpen] = useState<IndexEntry | null>(null);
  const load = () => lib.indexList().then(setList).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const pick = async (index: boolean) => {
    const file = await openDialog({ filters: [{ name: t("Sound and video"), extensions: MEDIA_KINDS }] });
    if (typeof file !== "string") return;
    if (!index) return onPlay(file);
    setBusy(file);
    try {
      await lib.indexAdd(file);
      toast(t("Added to the Media index."), "success");
      load();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const search = (q: string) => {
    setQuery(q);
    if (!q.trim()) return setHits(null);
    lib.indexSearch(q).then(setHits).catch((e) => toast(errorText(e), "error"));
  };
  const byId = useMemo(() => new Map((list ?? []).map((m) => [m.id, m])), [list]);
  const openEntry = (id: string) => lib.indexGet(id).then(setOpen).catch((e) => toast(errorText(e), "error"));
  const saveCaptions = async (e: IndexEntry) => {
    const path = await saveDialog({ defaultPath: `${e.title}.srt`, filters: [{ name: t("Subtitles"), extensions: ["srt", "vtt"] }] });
    if (path) lib.saveCaptions(e.id, path).then(() => toast(t("Captions saved."), "success")).catch((x) => toast(errorText(x), "error"));
  };
  const [renaming, setRenaming] = useState<{ from: string; to: string } | null>(null);
  const rename = async (e: IndexEntry) => {
    if (!renaming || !renaming.to.trim() || renaming.to === renaming.from) return setRenaming(null);
    await lib.renameSpeaker(e.id, renaming.from, renaming.to).catch((x) => toast(errorText(x), "error"));
    setRenaming(null);
    openEntry(e.id);
  };
  const speakers = open ? [...new Set(open.segments.map((s) => s.speaker).filter((s): s is string => !!s))] : [];

  return (
    <div className="library-media">
      {playing && <Player key={playing.path + (playing.at ?? "")} path={playing.path} at={playing.at} toast={toast} />}
      <div className="row">
        <input className="input" placeholder={t("Search what was said")} value={query} onChange={(e) => search(e.target.value)} aria-label={t("Search what was said")} />
        <button className="btn small" onClick={() => pick(false)}>{t("Play a file")}</button>
        <button className="btn primary small" onClick={() => pick(true)} disabled={!!busy}>
          {busy ? t("Transcribing...") : t("Add to the index")}
        </button>
      </div>
      {hits && (
        <ul className="plain-list">
          {hits.length === 0 && <li className="muted small">{t("Nothing matches.")}</li>}
          {hits.map((h, i) => {
            const m = byId.get(h.media);
            return (
              <li key={i}>
                <button className="link" onClick={() => m && onPlay(m.path, h.start)}>
                  {m?.title} {clock(h.start)}
                </button>{" "}
                {h.speaker && <b>{h.speaker}: </b>}
                {h.text}
              </li>
            );
          })}
        </ul>
      )}
      {!hits && list && list.length === 0 && <p className="muted small">{t("Nothing in the Media index yet. Add a recording or video to get its transcript, chapters and speakers.")}</p>}
      {!hits && (
        <ul className="plain-list">
          {(list ?? []).map((m) => (
            <li key={m.id} className="row">
              <button className="link" onClick={() => openEntry(m.id)}>{m.title}</button>
              <span className="muted small">
                {clock(m.seconds)} · {t("{n} chapters", { n: m.chapters })} · {m.speakers.map(([s]) => s).join(", ")}
              </span>
              <button className="btn small" onClick={() => onPlay(m.path)}>{t("Play")}</button>
              <button className="btn small" onClick={() => lib.indexRemove(m.id).then(load)}>{t("Remove")}</button>
            </li>
          ))}
        </ul>
      )}
      {open && (
        <div className="card">
          <div className="row">
            <h3>{open.title}</h3>
            <button className="btn small" onClick={() => saveCaptions(open)}>{t("Save captions")}</button>
            <button className="btn small" onClick={() => setOpen(null)}>{t("Close")}</button>
          </div>
          {speakers.length > 0 && (
            <div className="folder-chips">
              {speakers.map((s) =>
                renaming?.from === s ? (
                  <input
                    key={s}
                    className="input small"
                    autoFocus
                    aria-label={t("A name for {name}", { name: s })}
                    value={renaming.to}
                    onChange={(e) => setRenaming({ from: s, to: e.target.value })}
                    onBlur={() => rename(open)}
                    onKeyDown={(e) => e.key === "Enter" && rename(open)}
                  />
                ) : (
                  <button key={s} className="chip" onClick={() => setRenaming({ from: s, to: s })} title={t("Rename")}>{s}</button>
                ),
              )}
            </div>
          )}
          <h4>{t("Chapters")}</h4>
          <ol>
            {open.chapters.map((c, i) => (
              <li key={i}>
                <button className="link" onClick={() => onPlay(open.path, c.start)}>{clock(c.start)}</button> {c.title}
              </li>
            ))}
          </ol>
          <h4>{t("Transcript")}</h4>
          <div className="library-transcript">
            {open.segments.map((s, i) => (
              <p key={i}>
                <button className="link small" onClick={() => onPlay(open.path, s.start)}>{clock(s.start)}</button> {s.speaker && <b>{s.speaker}: </b>}
                {s.text}
              </p>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// ---------- Photos and screenshots ----------

function Thumb({ photo }: { photo: Photo }) {
  const [src, setSrc] = useState<string | null>(null);
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const io = new IntersectionObserver((seen) => {
      if (seen.some((s) => s.isIntersecting)) {
        io.disconnect();
        lib.photoThumb(photo.path).then(setSrc).catch(() => {});
      }
    });
    if (el.current) io.observe(el.current);
    return () => io.disconnect();
  }, [photo.path]);
  return <div ref={el} className="library-thumb">{src ? <img src={src} alt={photo.name} title={photo.name} /> : <span className="media-ph" aria-hidden />}</div>;
}

function PhotosTab({ toast }: { toast: PushToast }) {
  const [folders, setFolders] = useState<string[]>([]);
  const [photos, setPhotos] = useState<Photo[] | null>(null);
  const [big, setBig] = useState<{ photo: Photo; src: string } | null>(null);
  const load = () => {
    lib.photoFolders().then(setFolders).catch(() => {});
    lib.photos().then(setPhotos).catch((e) => toast(errorText(e), "error"));
  };
  useEffect(load, []); // eslint-disable-line react-hooks/exhaustive-deps
  const addFolder = async () => {
    const dir = await openDialog({ directory: true });
    if (typeof dir !== "string") return;
    await lib.setPhotoFolders([...folders, dir]).catch((e) => toast(errorText(e), "error"));
    load();
  };
  const removeFolder = async (f: string) => {
    await lib.setPhotoFolders(folders.filter((x) => x !== f)).catch((e) => toast(errorText(e), "error"));
    load();
  };
  const shoot = () =>
    lib
      .screenshot()
      .then((p) => toast(t("Screenshot saved to {path}", { path: p }), "success"))
      .catch((e) => toast(errorText(e), "error"));
  const show = (p: Photo) => lib.photoThumb(p.path, 1600).then((src) => setBig({ photo: p, src })).catch((e) => toast(errorText(e), "error"));
  return (
    <div>
      <div className="row">
        <button className="btn primary small" onClick={shoot}>{t("Take a screenshot")}</button>
        <button className="btn small" onClick={addFolder}>{t("Add a folder")}</button>
      </div>
      <div className="folder-chips">
        {folders.map((f) => (
          <button key={f} className="chip" onClick={() => removeFolder(f)} title={t("Remove this folder")}>{f}</button>
        ))}
      </div>
      {photos && photos.length === 0 && <p className="muted small">{t("Add the folders your pictures are in. They stay where they are; nothing is copied.")}</p>}
      {byMonth(photos ?? []).map(([month, list]) => (
        <section key={month}>
          <h4>{month}</h4>
          <div className="library-grid">
            {list.slice(0, 400).map((p) => (
              <button key={p.path} className="library-cell" onClick={() => show(p)}>
                <Thumb photo={p} />
              </button>
            ))}
          </div>
        </section>
      ))}
      {big && (
        <div className="library-lightbox" onClick={() => setBig(null)} role="dialog" aria-label={big.photo.name}>
          <img src={big.src} alt={big.photo.name} />
          <p>{big.photo.name}</p>
        </div>
      )}
    </div>
  );
}

// ---------- Podcasts and News ----------

function FeedsTab({ toast, podcasts, onPlay }: { toast: PushToast; podcasts: boolean; onPlay?: (url: string) => void }) {
  const [saved, setSaved] = useState<SavedFeed[]>([]);
  const [url, setUrl] = useState("");
  const [open, setOpen] = useState<{ url: string; feed: Feed } | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    lib.feeds().then(setSaved).catch(() => {});
  }, []);
  const mine = saved.filter((s) => s.podcast === podcasts);
  const add = async () => {
    setBusy(true);
    try {
      setSaved(await lib.addFeed(url));
      setUrl("");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const read = (u: string) => lib.fetchFeed(u).then((feed) => setOpen({ url: u, feed })).catch((e) => toast(errorText(e), "error"));
  return (
    <div>
      <div className="row">
        <input className="input" placeholder={podcasts ? t("A podcast's feed address") : t("A news site's feed address (RSS or Atom)")} value={url} onChange={(e) => setUrl(e.target.value)} />
        <button className="btn primary small" onClick={add} disabled={busy || !url.trim()}>{t("Add")}</button>
      </div>
      {!podcasts && <p className="muted small">{t("The morning brief includes the newest headlines from these feeds.")}</p>}
      <ul className="plain-list">
        {mine.map((s) => (
          <li key={s.url} className="row">
            <button className="link" onClick={() => read(s.url)}>{s.title}</button>
            <button className="btn small" onClick={() => lib.removeFeed(s.url).then(setSaved)}>{t("Remove")}</button>
          </li>
        ))}
      </ul>
      {open && (
        <div className="card">
          <h3>{open.feed.title}</h3>
          <ul className="plain-list">
            {open.feed.items.slice(0, 50).map((i, n) => (
              <li key={n}>
                <b>{i.title}</b> <span className="muted small">{i.published}</span>
                {i.summary && <div className="small">{i.summary}</div>}
                <div className="row">
                  {i.audio && onPlay && <button className="btn small" onClick={() => onPlay(i.audio!)}>{t("Play")}</button>}
                  {i.link && <a href={i.link} target="_blank" rel="noreferrer">{t("Open")}</a>}
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

// ---------- Books ----------

function BooksTab({ toast }: { toast: PushToast }) {
  const [book, setBook] = useState<Book | null>(null);
  const [part, setPart] = useState(0);
  const [within, setWithin] = useState(0);
  const pick = async () => {
    const file = await openDialog({ filters: [{ name: t("E-books"), extensions: ["epub"] }] });
    if (typeof file !== "string") return;
    lib
      .openBook(file)
      .then((b) => {
        setBook(b);
        setPart(0);
        setWithin(0);
      })
      .catch((e) => toast(errorText(e), "error"));
  };
  const section = book?.sections[part];
  return (
    <div>
      <div className="row">
        <button className="btn primary small" onClick={pick}>{t("Open a book")}</button>
        {book && <span className="muted small">{book.title} · {book.author} · {readProgress(book, part, within)}%</span>}
      </div>
      {book && (
        <div className="library-book">
          <nav>
            {book.sections.map((s, i) => (
              <button key={i} className={`link ${i === part ? "active" : ""}`} onClick={() => (setPart(i), setWithin(0))}>
                {s.title || t("Part {n}", { n: i + 1 })}
              </button>
            ))}
          </nav>
          <article onScroll={(e) => setWithin(e.currentTarget.scrollTop / Math.max(1, e.currentTarget.scrollHeight - e.currentTarget.clientHeight))}>
            {section?.text.split("\n\n").map((p, i) => <p key={i}>{p}</p>)}
            <div className="row">
              <button className="btn small" disabled={part === 0} onClick={() => (setPart(part - 1), setWithin(0))}>{t("Previous")}</button>
              <button className="btn small" disabled={part + 1 >= book.sections.length} onClick={() => (setPart(part + 1), setWithin(0))}>{t("Next")}</button>
            </div>
          </article>
        </div>
      )}
    </div>
  );
}

// ---------- Live captions ----------

function CaptionsTab({ toast }: { toast: PushToast }) {
  const [lines, setLines] = useState<Cue[]>([]);
  const [running, setRunning] = useState(false);
  const [translate, setTranslate] = useState(false);
  useEffect(() => {
    const a = listen<Cue>("media:caption", (e) => setLines((l) => [...l.slice(-200), e.payload]));
    const b = listen("media:caption-stopped", () => setRunning(false));
    return () => {
      a.then((un) => un());
      b.then((un) => un());
    };
  }, []);
  const start = () =>
    lib
      .liveCaptionsStart(translate)
      .then(() => {
        setLines([]);
        setRunning(true);
      })
      .catch((e) => toast(errorText(e), "error"));
  const save = async () => {
    const path = await saveDialog({ defaultPath: "captions.srt", filters: [{ name: t("Subtitles"), extensions: ["srt", "vtt"] }] });
    if (path) lib.liveCaptionsSave(path).then(() => toast(t("Captions saved."), "success")).catch((e) => toast(errorText(e), "error"));
  };
  return (
    <div>
      <p className="muted small">{t("Captions for whatever this PC is playing: a stream, a video or a call. Heard on this PC; nothing is sent anywhere.")}</p>
      <div className="row">
        {running ? <button className="btn small" onClick={() => lib.liveCaptionsStop()}>{t("Stop")}</button> : <button className="btn primary small" onClick={start}>{t("Start captions")}</button>}
        <label className="row small">
          <input type="checkbox" checked={translate} disabled={running} onChange={(e) => setTranslate(e.target.checked)} /> {t("In English")}
        </label>
        <button className="btn small" onClick={save} disabled={lines.length === 0}>{t("Save captions")}</button>
      </div>
      <div className="library-captions" aria-live="polite">
        {lines.slice(-12).map((c, i) => (
          <p key={i}>{c.text}</p>
        ))}
      </div>
    </div>
  );
}

// ---------- Screen history ----------

function HistoryTab({ toast }: { toast: PushToast }) {
  const [view, setView] = useState<{ settings: TimelineSettings; locked: boolean } | null>(null);
  const [pin, setPin] = useState("");
  const [unlocked, setUnlocked] = useState(false);
  const [day, setDay] = useState(dayStart(Date.now()));
  const [shots, setShots] = useState<Shot[]>([]);
  const [query, setQuery] = useState("");
  const [shown, setShown] = useState<{ at: number; src: string } | null>(null);
  const [newPin, setNewPin] = useState("");
  const load = () => lib.timeline().then(setView).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const p = pin || undefined;
  const open = (d = day) =>
    lib
      .timelineDay(d, p)
      .then((s) => {
        setShots(s);
        setUnlocked(true);
      })
      .catch((e) => toast(errorText(e), "error"));
  const set = (s: TimelineSettings) => lib.setTimeline(s, p).then(load).catch((e) => toast(errorText(e), "error"));
  if (!view) return null;
  const s = view.settings;
  return (
    <div>
      <p className="muted small">{t("Screen history keeps a picture of the screen every so often, so you can find what you saw. Off until you turn it on; kept encrypted on this PC only; private browser windows and the apps you list are never kept.")}</p>
      {view.locked && !unlocked && (
        <div className="row">
          <input className="input" type="password" placeholder={t("PIN")} value={pin} onChange={(e) => setPin(e.target.value)} aria-label={t("PIN")} />
          <button className="btn primary small" onClick={() => open()}>{t("Unlock")}</button>
        </div>
      )}
      {(!view.locked || unlocked) && (
        <>
          <div className="row">
            <label className="row small">
              <input type="checkbox" checked={s.enabled} onChange={(e) => set({ ...s, enabled: e.target.checked })} /> {t("Keep screen history")}
            </label>
            <label className="small">
              {t("Every (seconds)")} <input className="input small" type="number" min={5} max={600} value={s.every_secs} onChange={(e) => set({ ...s, every_secs: Number(e.target.value) })} />
            </label>
            <label className="small">
              {t("Keep (days)")} <input className="input small" type="number" min={1} max={365} value={s.keep_days} onChange={(e) => set({ ...s, keep_days: Number(e.target.value) })} />
            </label>
          </div>
          <label className="small">
            {t("Never keep these programs (e.g. bank.exe), comma separated")}
            <input className="input" defaultValue={s.skip_apps.join(", ")} onBlur={(e) => set({ ...s, skip_apps: e.target.value.split(",").map((x) => x.trim()).filter(Boolean) })} />
          </label>
          <div className="row">
            <input className="input" type="password" placeholder={view.locked ? t("New PIN (empty removes it)") : t("Set a PIN")} value={newPin} onChange={(e) => setNewPin(e.target.value)} />
            <button className="btn small" onClick={() => lib.setTimelinePin(newPin, p).then(() => (setPin(newPin), setNewPin(""), load())).catch((e) => toast(errorText(e), "error"))}>{t("Save PIN")}</button>
            <button className="btn small" onClick={async () => (await confirmed(t("Delete all screen history?"))) && lib.timelineClear(p).then(() => setShots([])).catch((e) => toast(errorText(e), "error"))}>{t("Delete all")}</button>
          </div>
          <div className="row">
            <input
              className="input"
              type="date"
              onChange={(e) => {
                const [y, m, d] = e.target.value.split("-").map(Number);
                if (!y) return;
                const start = dayStart(new Date(y, m - 1, d).getTime());
                setDay(start);
                open(start);
              }}
            />
            <button className="btn small" onClick={() => open()}>{t("Show the day")}</button>
            <input className="input" placeholder={t("Search window titles")} value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Enter" && lib.timelineSearch(query, p).then(setShots).catch((x) => toast(errorText(x), "error"))} />
          </div>
          <ul className="plain-list library-history">
            {shots.map((sh) => (
              <li key={sh.at}>
                <button className="link" onClick={() => lib.timelineImage(sh.at, p).then((src) => setShown({ at: sh.at, src })).catch((e) => toast(errorText(e), "error"))}>
                  {new Date(sh.at * 1000).toLocaleString()}
                </button>{" "}
                <span className="muted small">{sh.exe}</span> {sh.title}
              </li>
            ))}
          </ul>
          {shown && (
            <div className="library-lightbox" onClick={() => setShown(null)} role="dialog" aria-label={t("Screen history")}>
              <img src={shown.src} alt={new Date(shown.at * 1000).toLocaleString()} />
            </div>
          )}
        </>
      )}
    </div>
  );
}
