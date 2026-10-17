// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api, errorText, type Message } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** What the Preview panel shows: a site SulcusAI serves from a chat's
 * folder, or a server the assistant started (by its port). */
export interface PreviewTarget {
  url: string;
  site?: string;
  root?: string;
  start?: string;
  port?: number;
}

/** The newest site previewed in this chat, from its steps. */
export function latestPreview(messages: Message[]): PreviewTarget | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const meta = messages[i].role === "tool" ? messages[i].meta : undefined;
    if (meta?.preview_url && meta.status === "ok") {
      return { url: meta.preview_url, site: meta.preview_site, root: meta.preview_root, start: meta.preview_start, port: meta.preview_port };
    }
  }
  return null;
}

const WIDTH_KEY = "sulcus.preview.width";

/** The panel's width, as a share of the chat (remembered on this PC). */
export function usePreviewWidth(): [number, (w: number) => void] {
  const [width, setWidth] = useState(() => {
    try {
      const w = Number(localStorage.getItem(WIDTH_KEY));
      return w >= 25 && w <= 75 ? w : 48;
    } catch {
      return 48;
    }
  });
  const set = (w: number) => {
    const v = Math.round(Math.min(75, Math.max(25, w)));
    setWidth(v);
    try {
      localStorage.setItem(WIDTH_KEY, String(v));
    } catch {
      /* not kept */
    }
  };
  return [width, set];
}

/** A site the assistant built, beside the chat: it runs here, and opens in a
 * browser too. `stamp` changes when the assistant previews it again. */
export function PreviewPanel({
  chatId,
  target,
  stamp,
  onWidth,
  onClose,
  toast,
}: {
  chatId: string;
  target: PreviewTarget;
  stamp: number;
  onWidth: (percent: number) => void;
  onClose: () => void;
  toast: PushToast;
}) {
  const [url, setUrl] = useState<string | null>(null);
  const [site, setSite] = useState<string | undefined>(target.site);
  const [offline, setOffline] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [reloads, setReloads] = useState(0);
  const [dragging, setDragging] = useState(false);
  const panel = useRef<HTMLElement>(null);

  // A site is served again if it stopped (SulcusAI restarted): its address can change.
  useEffect(() => {
    let live = true;
    setProblem(null);
    if (target.root) {
      api
        .previewOpen(chatId, target.root, target.start ?? null)
        .then((s) => {
          if (!live) return;
          setUrl(s.url);
          setSite(s.id);
          setOffline(!s.web);
        })
        .catch((e) => {
          if (!live) return;
          setUrl(null);
          setProblem(errorText(e));
        });
    } else {
      setUrl(target.url);
      setSite(undefined);
      setOffline(false);
    }
    return () => {
      live = false;
    };
  }, [chatId, target.url, target.root, target.start, stamp]);

  const act = (f: () => Promise<unknown>) => f().catch((e) => toast(errorText(e), "error"));

  // Dragging the left edge sets the width.
  const startDrag = (e: ReactPointerEvent) => {
    const box = panel.current?.parentElement?.getBoundingClientRect();
    if (!box) return;
    e.preventDefault();
    setDragging(true);
    const move = (ev: PointerEvent) => onWidth(((box.right - ev.clientX) / box.width) * 100);
    const up = () => {
      setDragging(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <aside className={`preview-panel ${dragging ? "dragging" : ""}`} ref={panel} aria-label={t("Site preview")}>
      <div className="preview-resize" onPointerDown={startDrag} role="separator" aria-orientation="vertical" aria-label={t("Drag to resize the preview")} />
      <div className="preview-bar">
        <strong>{t("Preview")}</strong>
        <span className="muted small ellipsis preview-address" title={url ?? ""}>{url ?? ""}</span>
        <button className="btn ghost small" onClick={() => setReloads((r) => r + 1)} disabled={!url} title={t("Load the page again, with your latest changes")}>
          ↻ {t("Reload")}
        </button>
        <button className="btn ghost small" onClick={() => url && act(() => api.previewOpenExternal(url))} disabled={!url} title={t("Open it in your main browser (Settings → Browser)")}>
          ↗ {t("Open in browser")}
        </button>
        {site && (
          <button
            className="btn ghost small"
            onClick={() =>
              act(async () => {
                await api.previewStop(site);
                onClose();
              })
            }
            title={t("Stop serving this site")}
          >
            ■ {t("Stop")}
          </button>
        )}
        <button className="btn ghost small" onClick={onClose} aria-label={t("Close")}>✕</button>
      </div>
      {offline && <p className="preview-note small">{t("This chat is offline, so the site can't reach the internet. Turn on web for this chat to let it.")}</p>}
      {problem ? (
        <p className="muted small pad">{problem}</p>
      ) : url ? (
        <iframe
          key={`${url}#${reloads}#${stamp}`}
          className="preview-frame"
          src={url}
          title={t("Site preview")}
          sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-pointer-lock"
        />
      ) : (
        <p className="muted small pad">{t("Starting the preview…")}</p>
      )}
    </aside>
  );
}
