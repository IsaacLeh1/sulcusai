// SPDX-License-Identifier: AGPL-3.0-only
// What a social chat shows inside messages: markdown, safe links, pictures,
// players, link previews, and the pickers for emoji, GIFs and stickers.
import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { errorText } from "../../api";
import { useDismiss } from "../../useDismiss";
import { t } from "../../i18n";
import { openChecked } from "../../linkCheck";
import { blocks, linkKind, type Block, type Inline } from "./chatText";
import { EMOJI, searchEmoji } from "./emoji";
import { social, type FileRef, type Gif, type LinkPreview } from "./socialApi";

/** Opens a link from a chat; a site not opened before is checked with the user first. */
export const openLink = openChecked;

export function SafeLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a
      href={href}
      title={href}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => {
        e.preventDefault();
        openLink(href);
      }}
    >
      {children}
    </a>
  );
}

function Spoiler({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <span className={open ? "chat-spoiler open" : "chat-spoiler"} role="button" tabIndex={0} title={open ? undefined : t("Spoiler: click to show")} onClick={() => setOpen(true)} onKeyDown={(e) => e.key === "Enter" && setOpen(true)}>
      {children}
    </span>
  );
}

function Inlines({ xs, mentionable }: { xs: Inline[]; mentionable: string[] }): ReactNode {
  return xs.map((x, i) => {
    switch (x.k) {
      case "text":
        return <Fragment key={i}>{x.v}</Fragment>;
      case "code":
        return <code key={i}>{x.v}</code>;
      case "bold":
        return <strong key={i}>{Inlines({ xs: x.c, mentionable })}</strong>;
      case "italic":
        return <em key={i}>{Inlines({ xs: x.c, mentionable })}</em>;
      case "strike":
        return <s key={i}>{Inlines({ xs: x.c, mentionable })}</s>;
      case "underline":
        return <u key={i}>{Inlines({ xs: x.c, mentionable })}</u>;
      case "spoiler":
        return <Spoiler key={i}>{Inlines({ xs: x.c, mentionable })}</Spoiler>;
      case "link":
        return (
          <SafeLink key={i} href={x.href}>
            {x.v}
          </SafeLink>
        );
      case "mention":
        return mentionable.some((n) => n.toLowerCase().startsWith(x.v.toLowerCase())) ? (
          <span key={i} className="chat-mention">
            @{x.v}
          </span>
        ) : (
          <Fragment key={i}>@{x.v}</Fragment>
        );
    }
  });
}

function BlockView({ b, mentionable }: { b: Block; mentionable: string[] }) {
  switch (b.k) {
    case "code":
      return (
        <pre className="chat-code">
          <code>{b.v}</code>
        </pre>
      );
    case "quote":
      return <blockquote className="chat-quote">{Inlines({ xs: b.c, mentionable })}</blockquote>;
    case "heading":
      return <div className={`chat-h${b.level}`}>{Inlines({ xs: b.c, mentionable })}</div>;
    case "list":
      return <div className="chat-li">{Inlines({ xs: b.c, mentionable })}</div>;
    default:
      return <div className="chat-para">{Inlines({ xs: b.c, mentionable })}</div>;
  }
}

/** A message's text with markdown, links and mentions. */
export function RichText({ text, mentionable = [] }: { text: string; mentionable?: string[] }) {
  return (
    <div className="chat-text">
      {blocks(text).map((b, i) => (
        <BlockView key={i} b={b} mentionable={mentionable} />
      ))}
    </div>
  );
}

const dataUrl = (f: FileRef) => `data:${f.mime};base64,${f.data}`;

export function isSticker(f: FileRef) {
  return f.name.startsWith("sticker-");
}

/** A person's own emoji (Extras), sent as a small picture. */
export function isCustomEmoji(f: FileRef) {
  return f.name.startsWith("emoji-");
}

const EMOJI_KEY = "sulcus.emoji";

export function savedEmoji(): FileRef[] {
  try {
    return JSON.parse(localStorage.getItem(EMOJI_KEY) ?? "[]");
  } catch {
    return [];
  }
}

/** Keeps a picture as one of this user's emoji, made 96 pixels at most (GIFs stay animated). */
export async function saveEmoji(file: File, name: string): Promise<boolean> {
  const clean = name.toLowerCase().replace(/[^a-z0-9_]/g, "").slice(0, 32) || "emoji";
  const f = await readFile(file);
  if (f.data.length > 400_000) return false;
  const all = [{ ...f, name: `emoji-${clean}` }, ...savedEmoji().filter((e) => e.name !== `emoji-${clean}`)].slice(0, 100);
  try {
    localStorage.setItem(EMOJI_KEY, JSON.stringify(all));
    return true;
  } catch {
    return false;
  }
}

/** A picture shown over the whole window, closed with Escape or a click. */
export function Viewer({ src, alt, onClose }: { src: string; alt: string; onClose: () => void }) {
  // Zoom (wheel, pinch, buttons) and pan (drag) in the full-size view.
  const [z, setZ] = useState({ k: 1, x: 0, y: 0 });
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef(0);
  const moved = useRef(false);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "+" || e.key === "=") zoomBy(1.25);
      if (e.key === "-") zoomBy(0.8);
      if (e.key === "0") setZ({ k: 1, x: 0, y: 0 });
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  // Zooms toward a point (screen coordinates from the centre), keeping it under the pointer.
  const zoomBy = (f: number, cx = 0, cy = 0) =>
    setZ((o) => {
      const k = Math.min(10, Math.max(0.5, o.k * f));
      const r = k / o.k;
      return k === 1 && f < 1 && o.k <= 1 ? { k, x: 0, y: 0 } : { k, x: cx - (cx - o.x) * r, y: cy - (cy - o.y) * r };
    });
  const centre = (e: { clientX: number; clientY: number }) => [e.clientX - window.innerWidth / 2, e.clientY - window.innerHeight / 2] as const;
  return (
    <div
      className="chat-viewer"
      role="dialog"
      aria-label={alt}
      onClick={() => !moved.current && onClose()}
      onWheel={(e) => zoomBy(e.deltaY < 0 ? 1.15 : 1 / 1.15, ...centre(e))}
    >
      <img
        src={src}
        alt={alt}
        draggable={false}
        style={{ transform: `translate(${z.x}px, ${z.y}px) scale(${z.k})`, cursor: z.k > 1 ? "grab" : "zoom-in" }}
        onClick={(e) => {
          e.stopPropagation();
          if (!moved.current && z.k === 1) zoomBy(2, ...centre(e));
        }}
        onPointerDown={(e) => {
          moved.current = false;
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
          if (pointers.current.size === 2) {
            const [a, b] = [...pointers.current.values()];
            pinch.current = Math.hypot(a.x - b.x, a.y - b.y);
          }
        }}
        onPointerMove={(e) => {
          const last = pointers.current.get(e.pointerId);
          if (!last) return;
          pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
          if (pointers.current.size === 2) {
            const [a, b] = [...pointers.current.values()];
            const d = Math.hypot(a.x - b.x, a.y - b.y);
            if (pinch.current) zoomBy(d / pinch.current, ...centre({ clientX: (a.x + b.x) / 2, clientY: (a.y + b.y) / 2 }));
            pinch.current = d;
            moved.current = true;
            return;
          }
          const dx = e.clientX - last.x;
          const dy = e.clientY - last.y;
          if (Math.abs(dx) + Math.abs(dy) > 2) moved.current = true;
          setZ((o) => ({ ...o, x: o.x + dx, y: o.y + dy }));
        }}
        onPointerUp={(e) => {
          pointers.current.delete(e.pointerId);
          pinch.current = 0;
          window.setTimeout(() => (moved.current = false), 0);
        }}
      />
      <div className="chat-viewer-tools" onClick={(e) => e.stopPropagation()}>
        <button onClick={() => zoomBy(1 / 1.25)} aria-label={t("Zoom out")} title={t("Zoom out (-)")}>
          -
        </button>
        <span className="chat-viewer-zoom">{Math.round(z.k * 100)}%</span>
        <button onClick={() => zoomBy(1.25)} aria-label={t("Zoom in")} title={t("Zoom in (+)")}>
          +
        </button>
        <button onClick={() => setZ({ k: 1, x: 0, y: 0 })} title={t("Fit (0)")}>
          {t("Fit")}
        </button>
        <a href={src} download={alt}>
          {t("Save")}
        </a>
        <button onClick={onClose}>{t("Close")}</button>
      </div>
    </div>
  );
}

/** Files sent with a message: pictures, players for video and sound, the rest as downloads. */
export function Attachments({ files, onSticker }: { files: FileRef[]; onSticker?: (f: FileRef) => void }) {
  const [big, setBig] = useState<FileRef | null>(null);
  const save = (f: FileRef) => {
    const a = document.createElement("a");
    a.href = dataUrl(f);
    a.download = f.name;
    a.click();
  };
  return (
    <div className="chat-attachments">
      {files.map((f, i) => {
        if ((isSticker(f) || isCustomEmoji(f)) && f.mime.startsWith("image/"))
          return (
            <span key={i} className={isCustomEmoji(f) ? "chat-emoji-pic" : "chat-sticker"}>
              <img src={dataUrl(f)} alt={f.name} title={f.name} onClick={() => setBig(f)} />
            </span>
          );
        return (
          <span key={i} className="chat-file-item">
            <FileView name={f.name} mime={viewMime(f)} src={dataUrl(f)} onSave={() => save(f)} size={Math.floor((f.data.length * 3) / 4)} />
            {onSticker && f.mime.startsWith("image/") && (
              <button className="link small" onClick={() => onSticker(f)}>
                {t("Save as sticker")}
              </button>
            )}
          </span>
        );
      })}
      {big && <Viewer src={dataUrl(big)} alt={big.name} onClose={() => setBig(null)} />}
    </div>
  );
}

/** PDFs and text are shown by their name when the type wasn't given. */
function viewMime(f: FileRef): string {
  const n = f.name.toLowerCase();
  if (n.endsWith(".pdf")) return "application/pdf";
  if (/\.(txt|md|csv|log|json)$/.test(n)) return "text/plain";
  return f.mime;
}

/** Previews and players for links in a message; nothing loads unless link previews are on. */
export function LinkEmbeds({ links, on }: { links: string[]; on: boolean }) {
  // Video sites show a Play card even with previews off: the player loads only when clicked.
  const shown = on ? links : links.filter((l) => ["player", "youtube"].includes(linkKind(l).k));
  if (shown.length === 0) return null;
  return (
    <div className="chat-embeds">
      {shown.slice(0, 3).map((l) => (
        <Embed key={l} href={l} />
      ))}
    </div>
  );
}

function Embed({ href }: { href: string }) {
  const kind = linkKind(href);
  const [play, setPlay] = useState(false);
  const [p, setP] = useState<LinkPreview | null>(null);
  useEffect(() => {
    if (kind.k === "page" || kind.k === "youtube") social.linkPreview(href).then(setP).catch(() => setP(null));
  }, [href, kind.k]);
  if (kind.k === "image") return <img className="chat-pic" src={kind.src} alt="" title={href} />;
  if (kind.k === "video") return <ChatVideo src={kind.src} />;
  if (kind.k === "audio") return <audio src={kind.src} controls preload="metadata" />;
  if (kind.k === "player")
    return play ? (
      <iframe className="chat-video" referrerPolicy="strict-origin-when-cross-origin" src={kind.src} title={kind.site} allow="autoplay; encrypted-media; picture-in-picture; fullscreen" allowFullScreen sandbox="allow-scripts allow-same-origin allow-presentation allow-popups" />
    ) : (
      <button className="chat-player-card" onClick={() => setPlay(true)} aria-label={t("Play the video from {site}", { site: kind.site })}>
        <span className="chat-play-mark" aria-hidden />
        <span className="small">{t("Play video from {site}", { site: kind.site })}</span>
      </button>
    );
  if (kind.k === "youtube" && play)
    return <iframe className="chat-video" referrerPolicy="strict-origin-when-cross-origin" src={`https://www.youtube-nocookie.com/embed/${kind.id}?autoplay=1`} title={p?.title || href} allow="autoplay; encrypted-media; picture-in-picture; fullscreen" allowFullScreen sandbox="allow-scripts allow-same-origin allow-presentation" />;
  // Without a preview (previews off, or none came back): a Play card; nothing loads before the click.
  if (kind.k === "youtube" && !p)
    return (
      <button className="chat-player-card" onClick={() => setPlay(true)} aria-label={t("Play the video from {site}", { site: "YouTube" })}>
        <span className="chat-play-mark" aria-hidden />
        <span className="small">{t("Play video from {site}", { site: "YouTube" })}</span>
      </button>
    );
  if (!p) return null;
  return (
    <div className="chat-embed">
      <span className="small muted">{p.site}</span>
      <SafeLink href={href}>
        <strong>{p.title || href}</strong>
      </SafeLink>
      {p.description && <span className="small">{p.description}</span>}
      {p.image &&
        (kind.k === "youtube" ? (
          <button className="chat-play" onClick={() => setPlay(true)} aria-label={t("Play")}>
            <img src={p.image} alt="" />
            <span className="chat-play-mark" aria-hidden />
          </button>
        ) : (
          <img src={p.image} alt="" />
        ))}
    </div>
  );
}

const RECENT_KEY = "sulcus.recentEmoji";

function recent(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
  } catch {
    return [];
  }
}

export function EmojiPicker({ onPick, onClose, onCustom }: { onPick: (e: string) => void; onClose: () => void; onCustom?: (f: FileRef) => void }) {
  const [q, setQ] = useState("");
  const [mine, setMine] = useState(savedEmoji);
  const upload = useRef<HTMLInputElement>(null);
  const found = q.trim() ? searchEmoji(q) : null;
  const pick = (e: string) => {
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify([e, ...recent().filter((x) => x !== e)].slice(0, 24)));
    } catch {
      // Recent emoji are a convenience only.
    }
    onPick(e);
  };
  const groups = found ? [{ name: t("Found"), items: found.map((e) => [e, ""] as [string, string]) }] : [{ name: t("Recent"), items: recent().map((e) => [e, ""] as [string, string]) }, ...EMOJI];
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box, ".picker-toggle");
  return (
    <div ref={box} className="chat-picker" role="dialog" aria-label={t("Emoji")}>
      <input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("Find emoji")} />
      {onCustom && (
        <div className="chat-my-emoji">
          <span className="small muted">{t("Yours")}</span>
          {mine
            .filter((f) => !q.trim() || f.name.includes(q.trim().toLowerCase()))
            .map((f) => (
              <button key={f.name} className="chat-emoji" title={`:${f.name.slice(6)}:`} onClick={() => onCustom(f)}>
                <img src={dataUrl(f)} alt={f.name.slice(6)} />
              </button>
            ))}
          <button className="link small" onClick={() => upload.current?.click()}>
            {t("Add your own")}
          </button>
          <input
            ref={upload}
            type="file"
            accept="image/png,image/gif,image/webp,image/jpeg"
            hidden
            onChange={async (e) => {
              const file = e.target.files?.[0];
              if (!file) return;
              const name = window.prompt(t("A name for this emoji"), file.name.replace(/\.[^.]+$/, "")) ?? "";
              if (await saveEmoji(file, name)) setMine(savedEmoji());
              else window.alert(t("That picture is too big for an emoji (300 KB at most)."));
            }}
          />
        </div>
      )}
      <div className="chat-picker-grid">
        {groups
          .filter((g) => g.items.length > 0)
          .map((g) => (
            <Fragment key={g.name}>
              <div className="small muted chat-picker-head">{g.name}</div>
              {g.items.map(([e, k]) => (
                <button key={g.name + e} className="chat-emoji" title={k} onClick={() => pick(e)}>
                  {e}
                </button>
              ))}
            </Fragment>
          ))}
        {found?.length === 0 && <div className="small muted">{t("No emoji found.")}</div>}
      </div>
    </div>
  );
}

export function GifPicker({ start, onPick, onClose }: { start?: string; onPick: (f: FileRef) => void; onClose: () => void }) {
  const [q, setQ] = useState(start ?? "");
  const [gifs, setGifs] = useState<Gif[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const search = (text: string) => {
    if (!text.trim()) return;
    setErr(null);
    social.gifSearch(text).then(setGifs).catch((e) => setErr(errorText(e)));
  };
  useEffect(() => {
    if (start) search(start);
  }, [start]);
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box, ".picker-toggle");
  return (
    <div ref={box} className="chat-picker" role="dialog" aria-label={t("GIFs")}>
      <input autoFocus value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === "Enter" && search(q)} placeholder={t("Search GIFs (sent to GIPHY)")} />
      {err && <div className="small muted">{err}</div>}
      <div className="chat-gif-grid">
        {gifs.map((g) => (
          <button
            key={g.url}
            disabled={busy}
            title={g.title}
            onClick={async () => {
              setBusy(true);
              try {
                onPick({ name: "gif.gif", mime: "image/gif", data: await social.gifFetch(g.url) });
              } catch (e) {
                setErr(errorText(e));
              }
              setBusy(false);
            }}
          >
            <img src={g.preview} alt={g.title} loading="lazy" />
          </button>
        ))}
      </div>
    </div>
  );
}

const STICKER_KEY = "sulcus.stickers";

export function savedStickers(): FileRef[] {
  try {
    return JSON.parse(localStorage.getItem(STICKER_KEY) ?? "[]");
  } catch {
    return [];
  }
}

export function saveSticker(f: FileRef): boolean {
  // Stickers live in this window's storage, so each stays small.
  if (f.data.length > 700_000) return false;
  const all = [{ ...f, name: "sticker-" + f.name }, ...savedStickers().filter((s) => s.data !== f.data)].slice(0, 40);
  try {
    localStorage.setItem(STICKER_KEY, JSON.stringify(all));
    return true;
  } catch {
    return false;
  }
}

export function StickerPicker({ onPick, onClose }: { onPick: (f: FileRef) => void; onClose: () => void }) {
  const [all, setAll] = useState(savedStickers);
  const fileIn = useRef<HTMLInputElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box, ".picker-toggle");
  return (
    <div ref={box} className="chat-picker" role="dialog" aria-label={t("Stickers")}>
      <div className="row">
        <button onClick={() => fileIn.current?.click()}>{t("Add a picture as a sticker")}</button>
        <input
          ref={fileIn}
          type="file"
          accept="image/png,image/gif,image/webp,image/jpeg"
          hidden
          onChange={async (e) => {
            const file = e.target.files?.[0];
            if (file && saveSticker(await readFile(file))) setAll(savedStickers());
          }}
        />
      </div>
      {all.length === 0 && <p className="small muted">{t("No stickers yet. Add a picture, or save one from a chat.")}</p>}
      <div className="chat-gif-grid">
        {all.map((s, i) => (
          <button key={i} onClick={() => onPick(s)} title={s.name.replace(/^sticker-/, "")}>
            <img src={dataUrl(s)} alt="" />
          </button>
        ))}
      </div>
    </div>
  );
}

export function readFile(file: File): Promise<FileRef> {
  return new Promise((ok, fail) => {
    const r = new FileReader();
    r.onload = () => {
      const s = String(r.result);
      ok({ name: file.name || "file", mime: file.type || "application/octet-stream", data: s.slice(s.indexOf(",") + 1) });
    };
    r.onerror = () => fail(r.error);
    r.readAsDataURL(file);
  });
}

/** Records a voice message from the microphone. */
export function VoiceRecorder({ onDone }: { onDone: (f: FileRef) => void }) {
  const [rec, setRec] = useState<MediaRecorder | null>(null);
  const [secs, setSecs] = useState(0);
  useEffect(() => {
    if (!rec) return;
    const id = window.setInterval(() => setSecs((s) => s + 1), 1000);
    return () => window.clearInterval(id);
  }, [rec]);
  const start = async () => {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const r = new MediaRecorder(stream);
    const parts: Blob[] = [];
    r.ondataavailable = (e) => parts.push(e.data);
    r.onstop = async () => {
      stream.getTracks().forEach((x) => x.stop());
      const blob = new Blob(parts, { type: r.mimeType || "audio/webm" });
      onDone(await readFile(new File([blob], `voice-${Date.now()}.webm`, { type: blob.type })));
    };
    r.start();
    setSecs(0);
    setRec(r);
  };
  return rec ? (
    <button
      className="danger"
      onClick={() => {
        rec.stop();
        setRec(null);
      }}
    >
      {t("Stop and send ({n}s)", { n: secs })}
    </button>
  ) : (
    <button onClick={() => start().catch(() => {})} title={t("Record a voice message")}>
      {t("Voice")}
    </button>
  );
}

/** Writes a file dropped from Explorer to this PC in 4 MB pieces, and gives back where it is. */
export async function saveDropped(file: File, onPart?: (done: number) => void): Promise<string> {
  const id = crypto.randomUUID().replace(/-/g, "");
  const PART = 4 << 20;
  let path = "";
  for (let at = 0; at < Math.max(1, file.size); at += PART) {
    const bytes = new Uint8Array(await file.slice(at, at + PART).arrayBuffer());
    let s = "";
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    path = await invoke<string>("drop_piece", { id, name: file.name || "file", data: btoa(s), first: at === 0 });
    onPart?.(Math.min(file.size, at + PART));
  }
  return path;
}

const SHARE_PREFIX = "sulcus://share/";
export const isShareLink = (text: string) => text.trim().startsWith(SHARE_PREFIX) && !/\s/.test(text.trim());

/** A big file sent through a share link: its name and size, and Download. */
export function ShareCard({ link }: { link: string }) {
  const [info, setInfo] = useState<{ name: string; size: number } | null>(null);
  const [state, setState] = useState<"idle" | "busy" | "done">("idle");
  const [err, setErr] = useState("");
  useEffect(() => {
    invoke<{ name: string; size: number }>("share_read", { text: link.trim() })
      .then(setInfo)
      .catch(() => setInfo(null));
  }, [link]);
  const get = async () => {
    const dir = await openDialog({ directory: true, multiple: false, title: t("Save to which folder?") });
    if (typeof dir !== "string") return;
    setState("busy");
    setErr("");
    try {
      await invoke<string>("share_fetch", { text: link.trim(), saveDir: dir });
      setState("done");
    } catch (e) {
      setErr(errorText(e));
      setState("idle");
    }
  };
  return (
    <div className="chat-file share-card">
      <strong className="ellipsis">{info?.name ?? t("A shared file")}</strong>
      <span className="small muted">{info ? sizeText(info.size) : t("Sent peer to peer: it downloads from the sender and anyone else who has it.")}</span>
      <button className="primary small" disabled={state === "busy"} onClick={get}>
        {state === "busy" ? t("Downloading...") : state === "done" ? t("Saved. Download again") : t("Download")}
      </button>
      {err && <span className="small warn-text">{err}</span>}
    </div>
  );
}

export function sizeText(n: number): string {
  return n >= 1 << 30 ? `${(n / (1 << 30)).toFixed(1)} GB` : n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

/** Shows a file in the chat by kind: pictures, video, sound, PDFs and text; anything else is a name with Download. */
export function FileView({ name, mime, src, onSave, size }: { name: string; mime: string; src: string; onSave: () => void; size?: number }) {
  const [big, setBig] = useState(false);
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    if (mime !== "text/plain") return;
    fetch(src)
      .then((r) => r.text())
      .then((x) => setText(x.slice(0, 4000)))
      .catch(() => setText(null));
  }, [mime, src]);
  const save = (
    <button className="link small" onClick={onSave}>
      {t("Download")}
    </button>
  );
  if (mime.startsWith("image/") && !mime.includes("svg"))
    return (
      <span className="chat-pic">
        <img src={src} alt={name} title={name} onClick={() => setBig(true)} />
        {save}
        {big && <Viewer src={src} alt={name} onClose={() => setBig(false)} />}
      </span>
    );
  if (mime.startsWith("video/"))
    return (
      <span className="chat-video-wrap">
        <ChatVideo src={src} title={name} />
        {save}
      </span>
    );
  if (mime.startsWith("audio/"))
    return (
      <div className="chat-audio">
        <span className="small muted">{name}</span>
        <audio src={src} controls preload="metadata" />
        {save}
      </div>
    );
  if (mime === "application/pdf")
    return (
      <div className="chat-doc">
        <iframe src={src} title={name} sandbox="" />
        <span className="row small">
          <span className="ellipsis">{name}</span>
          {save}
        </span>
      </div>
    );
  if (mime === "text/plain" && text !== null)
    return (
      <div className="chat-doc">
        <pre className="chat-doc-text">{text}</pre>
        <span className="row small">
          <span className="ellipsis">{name}</span>
          {save}
        </span>
      </div>
    );
  // Anything else: a card with its name and size, opened in a new view or saved.
  return (
    <div className="chat-file-card">
      <FileIcon />
      <span className="chat-file-text">
        <a className="ellipsis" href={src} target="_blank" rel="noreferrer" download={name} title={t("Save {name}", { name })} onClick={(e) => (e.preventDefault(), onSave())}>
          {name}
        </a>
        {size !== undefined && <span className="small muted">{sizeText(size)}</span>}
      </span>
      <button className="link small" onClick={onSave}>
        {t("Save")}
      </button>
    </div>
  );
}

function FileIcon() {
  return (
    <svg width="28" height="34" viewBox="0 0 24 30" aria-hidden>
      <path d="M3 1h12l6 6v20a2 2 0 01-2 2H3a2 2 0 01-2-2V3a2 2 0 012-2z" fill="var(--accent-soft)" stroke="var(--accent)" />
      <path d="M15 1v6h6" fill="none" stroke="var(--accent)" />
    </svg>
  );
}

/** A video with a real picture before it plays: a frame about a second in,
 *  instead of a black box; playing starts from the beginning. */
export function ChatVideo({ src, title }: { src: string; title?: string }) {
  const posed = useRef(false);
  return (
    <video
      className="chat-video"
      src={src}
      title={title}
      controls
      preload="auto"
      playsInline
      onLoadedMetadata={(e) => {
        const v = e.currentTarget;
        if (v.currentTime === 0 && Number.isFinite(v.duration) && v.duration > 0) {
          posed.current = true;
          v.currentTime = Math.min(1, v.duration / 10);
        }
      }}
      onPlay={(e) => {
        if (!posed.current) return;
        posed.current = false;
        e.currentTarget.currentTime = 0;
      }}
    />
  );
}
