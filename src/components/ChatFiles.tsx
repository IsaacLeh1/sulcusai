// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { api, errorText, type MediaItem, type Message } from "../api";
import { t } from "../i18n";
import { useMediaUrl } from "../media";
import { saveAs } from "./MediaBits";
import type { PushToast } from "./Toasts";

/** Kinds Open hands to their app (the rest: Show in folder). Matches the core's list. */
const OPENABLE = ["docx", "xlsx", "pptx", "pdf", "md", "csv", "txt", "html", "htm", "svg", "png", "jpg", "jpeg", "gif", "webp", "json"];

/** The files the assistant wrote or made as documents in this chat, newest first. */
export function chatFiles(messages: Message[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const m of [...messages].reverse()) {
    const f = m.role === "tool" ? m.meta?.file : undefined;
    if (f && !seen.has(f.toLowerCase())) {
      seen.add(f.toLowerCase());
      out.push(f);
    }
  }
  return out;
}

function Thumb({ item, onOpen, toast }: { item: MediaItem; onOpen: (i: MediaItem) => void; toast: PushToast }) {
  const src = useMediaUrl(item, item.kind === "image");
  return (
    <div className="files-media">
      {item.kind === "image" ? (
        <button className="files-thumb" onClick={() => onOpen(item)} title={t("Open")}>
          {src ? <img src={src} alt={item.prompt || t("Picture")} /> : <span>…</span>}
        </button>
      ) : (
        <div className="files-thumb other">{item.kind === "video" ? t("Video") : t("Sound")}</div>
      )}
      <div className="files-meta">
        <span className="small ellipsis" title={item.prompt}>{item.prompt || (item.hidden ? t("Attached picture") : t("Picture"))}</span>
        <button className="btn ghost small" onClick={() => saveAs(item, toast)}>{t("Save…")}</button>
      </div>
    </div>
  );
}

/** The chat's Files: pictures, video and music made or attached here, and
 * files the assistant wrote. Opened from the chat's top-right corner. */
export function ChatFilesPanel({ chatId, messages, onOpenPicture, onClose, toast }: { chatId: string; messages: Message[]; onOpenPicture: (i: MediaItem) => void; onClose: () => void; toast: PushToast }) {
  const [media, setMedia] = useState<MediaItem[] | null>(null);
  const files = useMemo(() => chatFiles(messages), [messages]);
  // New pictures arrive with new messages.
  useEffect(() => {
    api.mediaInChat(chatId).then(setMedia).catch(() => setMedia([]));
  }, [chatId, messages.length]);

  const name = (p: string) => p.split(/[\\/]/).pop() ?? p;
  const folder = (p: string) => p.slice(0, p.length - name(p).length).replace(/[\\/]$/, "");
  const ext = (p: string) => name(p).split(".").pop()?.toLowerCase() ?? "";
  const act = (f: () => Promise<unknown>) => f().catch((e) => toast(errorText(e), "error"));

  return (
    <aside className="chat-files" aria-label={t("Files in this chat")}>
      <div className="chat-files-head">
        <strong>{t("Files")}</strong>
        <button className="btn ghost small" onClick={onClose} aria-label={t("Close")}>✕</button>
      </div>
      <h4>{t("Pictures, video and music")}</h4>
      {media === null && <p className="muted small">{t("Loading…")}</p>}
      {media?.length === 0 && <p className="muted small">{t("None yet. Ask for a picture, or attach one with the paperclip button.")}</p>}
      <div className="files-grid">
        {media?.map((m) => (
          <Thumb key={m.id} item={m} onOpen={onOpenPicture} toast={toast} />
        ))}
      </div>
      <h4>{t("Files it made")}</h4>
      {files.length === 0 && <p className="muted small">{t("None yet. Share a folder with the folder button and ask it to make something.")}</p>}
      <ul className="files-list">
        {files.map((f) => (
          <li key={f}>
            <span className="ellipsis" title={f}>
              <strong>{name(f)}</strong>
              <span className="muted small block ellipsis">{folder(f)}</span>
            </span>
            <span className="row">
              {OPENABLE.includes(ext(f)) && <button className="btn ghost small" onClick={() => act(() => api.openDocument(f))}>{t("Open")}</button>}
              <button className="btn ghost small" onClick={() => act(() => api.showInFolder(f))}>{t("Show in folder")}</button>
            </span>
          </li>
        ))}
      </ul>
    </aside>
  );
}
