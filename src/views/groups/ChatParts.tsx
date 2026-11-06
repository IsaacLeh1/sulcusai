// SPDX-License-Identifier: AGPL-3.0-only
// What a social chat shows inside messages: markdown, safe links, pictures,
// players, link previews, and the pickers for emoji, GIFs and stickers.
import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
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

/** A picture shown over the whole window, closed with Escape or a click. */
export function Viewer({ src, alt, onClose }: { src: string; alt: string; onClose: () => void }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="chat-viewer" role="dialog" aria-label={alt} onClick={onClose}>
      <img src={src} alt={alt} />
      <a className="chat-viewer-save" href={src} download={alt} onClick={(e) => e.stopPropagation()}>
        {t("Save")}
      </a>
    </div>
  );
}

/** Files sent with a message: pictures, players for video and sound, the rest as downloads. */
export function Attachments({ files, onSticker }: { files: FileRef[]; onSticker?: (f: FileRef) => void }) {
  const [big, setBig] = useState<FileRef | null>(null);
  return (
    <div className="chat-files">
      {files.map((f, i) => {
        const src = dataUrl(f);
        if (f.mime.startsWith("image/") && !f.mime.includes("svg"))
          return (
            <span key={i} className={isSticker(f) ? "chat-sticker" : "chat-pic"}>
              <img src={src} alt={f.name} title={f.name} onClick={() => setBig(f)} />
              {onSticker && !isSticker(f) && (
                <button className="link small" onClick={() => onSticker(f)}>
                  {t("Save as sticker")}
                </button>
              )}
            </span>
          );
        if (f.mime.startsWith("video/")) return <video key={i} className="chat-video" src={src} controls preload="metadata" title={f.name} />;
        if (f.mime.startsWith("audio/"))
          return (
            <div key={i} className="chat-audio">
              <span className="small muted">{f.name.startsWith("voice-") ? t("Voice message") : f.name}</span>
              <audio src={src} controls preload="metadata" />
            </div>
          );
        return (
          <a key={i} className="chat-file" href={src} download={f.name}>
            {f.name} <span className="small muted">({Math.ceil((f.data.length * 3) / 4 / 1024)} KB)</span>
          </a>
        );
      })}
      {big && <Viewer src={dataUrl(big)} alt={big.name} onClose={() => setBig(null)} />}
    </div>
  );
}

/** Previews and players for links in a message; nothing loads unless link previews are on. */
export function LinkEmbeds({ links, on }: { links: string[]; on: boolean }) {
  if (!on || links.length === 0) return null;
  return (
    <div className="chat-embeds">
      {links.slice(0, 3).map((l) => (
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
  if (kind.k === "video") return <video className="chat-video" src={kind.src} controls preload="metadata" />;
  if (kind.k === "audio") return <audio src={kind.src} controls preload="metadata" />;
  if (kind.k === "youtube" && play)
    return <iframe className="chat-video" src={`https://www.youtube-nocookie.com/embed/${kind.id}?autoplay=1`} title={p?.title || href} allow="autoplay; encrypted-media; picture-in-picture; fullscreen" sandbox="allow-scripts allow-same-origin allow-presentation" />;
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

export function EmojiPicker({ onPick, onClose }: { onPick: (e: string) => void; onClose: () => void }) {
  const [q, setQ] = useState("");
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
