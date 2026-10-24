// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { api, errorText, on, type ViewerArea } from "../api";
import { BackIcon, ExternalIcon, ForwardIcon, ReloadIcon } from "../components/Icons";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

/** A viewer's key: its chat's id, or "main" for the Web page (viewer.rs). */
const keyOf = (chatId: string | null | undefined) => chatId || "main";

/** The web viewer: web pages inside SulcusAI, under an address bar. The page
 * itself is a second webview the core lays over the empty area below the
 * bar (viewer.rs): it follows that area as the window changes, and hides
 * while a dialog is open (it would cover it) and when leaving the page.
 * Each chat has its own (`chatId`), docked beside it; the Web page has one
 * too. `start` changes when a link is sent here (`viewer:open`). */
export function WebViewerView({
  chatId,
  start,
  toast,
  onMinimize,
  onClose,
}: {
  chatId?: string | null;
  start: { url: string | null; at: number };
  toast: PushToast;
  /** Docked beside a chat: out of sight, the page kept. */
  onMinimize?: () => void;
  /** Closes it (a chat's viewer is closed for good; the Web page goes back). */
  onClose?: () => void;
}) {
  const key = keyOf(chatId);
  const area = useRef<HTMLDivElement>(null);
  const editing = useRef(false);
  const [url, setUrl] = useState("");
  const [typed, setTyped] = useState("");
  const [loading, setLoading] = useState(false);
  const [title, setTitle] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [outside, setOutside] = useState(t("your browser"));
  const [privateOn, setPrivateOn] = useState(false);

  const rect = (): ViewerArea => {
    const r = area.current?.getBoundingClientRect();
    return { x: r?.left ?? 0, y: r?.top ?? 0, width: r?.width ?? 1, height: r?.height ?? 1 };
  };
  const shown = (u: string | null) => {
    if (!u) return;
    setUrl(u);
    if (!editing.current) setTyped(u);
  };

  // Opens (or shows again) on arrival, and when a link is sent here.
  useEffect(() => {
    setError(null);
    api
      .viewerShow(chatId ?? null, start.url, rect())
      .then(() => api.viewerUrl(chatId ?? null))
      .then(shown)
      .catch((e) => setError(errorText(e)));
  }, [start.at, key]); // eslint-disable-line react-hooks/exhaustive-deps

  // Follows the area, hides under dialogs, and hides when leaving the page.
  useEffect(() => {
    const el = area.current;
    if (!el) return;
    let covered = false;
    const move = () => !covered && api.viewerArea(chatId ?? null, rect()).catch(() => {});
    const resize = new ResizeObserver(move);
    resize.observe(el);
    window.addEventListener("resize", move);
    const dialogs = new MutationObserver(() => {
      const now = !!document.querySelector(".modal-backdrop");
      if (now === covered) return;
      covered = now;
      (now ? api.viewerHide(chatId ?? null) : api.viewerShow(chatId ?? null, null, rect())).catch(() => {});
    });
    dialogs.observe(document.body, { childList: true, subtree: true });
    const subs = [
      on("viewer:page", (p) => {
        if (p.key !== key) return;
        setLoading(p.loading);
        shown(p.url);
      }),
      on("viewer:title", (p) => p.key === key && setTitle(p.title)),
    ];
    api.browsersView().then((b) => b.current.kind === "app" && setOutside(b.current.name)).catch(() => {});
    api.privacyView().then((v) => setPrivateOn(v.settings.enabled && v.settings.web)).catch(() => {});
    return () => {
      resize.disconnect();
      dialogs.disconnect();
      window.removeEventListener("resize", move);
      subs.forEach((s) => s.then((un) => un()));
      api.viewerHide(chatId ?? null).catch(() => {});
    };
  }, [key]); // eslint-disable-line react-hooks/exhaustive-deps

  const go = (to: string) => api.viewerGo(chatId ?? null, to).catch((e) => toast(errorText(e), "error"));
  return (
    <div className={`web-viewer${chatId ? " docked" : ""}`}>
      <form
        className="viewer-bar"
        onSubmit={(e) => {
          e.preventDefault();
          editing.current = false;
          (document.activeElement as HTMLElement | null)?.blur();
          go(typed);
        }}
      >
        <button type="button" className="icon-btn" title={t("Back")} aria-label={t("Back")} onClick={() => go("back")}>
          <BackIcon />
        </button>
        <button type="button" className="icon-btn" title={t("Forward")} aria-label={t("Forward")} onClick={() => go("forward")}>
          <ForwardIcon />
        </button>
        <button type="button" className={`icon-btn${loading ? " spinning" : ""}`} title={t("Load again")} aria-label={t("Load again")} onClick={() => go("reload")}>
          <ReloadIcon />
        </button>
        <input
          className="viewer-address"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          onFocus={(e) => {
            editing.current = true;
            e.target.select();
          }}
          onBlur={() => {
            editing.current = false;
            setTyped(url);
          }}
          placeholder={t("Search, or type a web address")}
          aria-label={t("Web address")}
          spellCheck={false}
        />
        <button type="button" className="btn small" disabled={!url} title={t("Open this page in {name}", { name: outside })} onClick={() => api.openOutside(url).catch((e) => toast(errorText(e), "error"))}>
          <ExternalIcon /> {chatId ? null : t("Open in {name}", { name: outside })}
        </button>
        {onMinimize && (
          <button type="button" className="icon-btn" title={t("Minimize: the page stays as it is")} aria-label={t("Minimize the browser")} onClick={onMinimize}>
            <span aria-hidden="true" className="viewer-glyph">–</span>
          </button>
        )}
        {onClose && (
          <button type="button" className="icon-btn" title={chatId ? t("Close this chat's browser") : t("Close the web viewer")} aria-label={t("Close the browser")} onClick={onClose}>
            <span aria-hidden="true" className="viewer-glyph">✕</span>
          </button>
        )}
      </form>
      {privateOn && <p className="muted small viewer-note">{t("Pages here are visited directly, not through your private connection: sites see this PC's address.")}</p>}
      <div className="viewer-area" ref={area} title={title} aria-busy={loading}>
        {error && <p className="viewer-error">{error}</p>}
      </div>
    </div>
  );
}

/** A chat's own browser, beside the conversation: what its assistant browses
 * shows here as it works, and the user can browse in it too. */
export function BrowserPanel({
  chatId,
  start,
  onWidth,
  onMinimize,
  onClose,
  toast,
}: {
  chatId: string;
  start: { url: string | null; at: number };
  onWidth: (percent: number) => void;
  onMinimize: () => void;
  onClose: () => void;
  toast: PushToast;
}) {
  const panel = useRef<HTMLElement>(null);
  const [dragging, setDragging] = useState(false);
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
    <aside className={`preview-panel browser-panel ${dragging ? "dragging" : ""}`} ref={panel} aria-label={t("This chat's browser")}>
      <div className="preview-resize" onPointerDown={startDrag} role="separator" aria-orientation="vertical" aria-label={t("Drag to resize the browser")} />
      <WebViewerView chatId={chatId} start={start} toast={toast} onMinimize={onMinimize} onClose={onClose} />
    </aside>
  );
}
