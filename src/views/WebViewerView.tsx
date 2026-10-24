// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type ViewerArea } from "../api";
import { BackIcon, ExternalIcon, ForwardIcon, ReloadIcon } from "../components/Icons";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

/** The web viewer: web pages inside SulcusAI, under an address bar. The page
 * itself is a second webview the core lays over the empty area below the
 * bar (viewer.rs): it follows that area as the window changes, and hides
 * while a dialog is open (it would cover it) and when leaving the page.
 * `start` changes when a link is sent here (`viewer:open`). */
export function WebViewerView({ start, toast }: { start: { url: string | null; at: number }; toast: PushToast }) {
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
      .viewerShow(start.url, rect())
      .then(() => api.viewerUrl())
      .then(shown)
      .catch((e) => setError(errorText(e)));
  }, [start.at]);

  // Follows the area, hides under dialogs, and hides when leaving the page.
  useEffect(() => {
    const el = area.current;
    if (!el) return;
    let covered = false;
    const move = () => !covered && api.viewerArea(rect()).catch(() => {});
    const resize = new ResizeObserver(move);
    resize.observe(el);
    window.addEventListener("resize", move);
    const dialogs = new MutationObserver(() => {
      const now = !!document.querySelector(".modal-backdrop");
      if (now === covered) return;
      covered = now;
      (now ? api.viewerHide() : api.viewerShow(null, rect())).catch(() => {});
    });
    dialogs.observe(document.body, { childList: true, subtree: true });
    const subs = [
      on("viewer:page", (p) => {
        setLoading(p.loading);
        shown(p.url);
      }),
      on("viewer:title", (p) => setTitle(p.title)),
    ];
    api.browsersView().then((b) => b.current.kind === "app" && setOutside(b.current.name)).catch(() => {});
    api.privacyView().then((v) => setPrivateOn(v.settings.enabled && v.settings.web)).catch(() => {});
    return () => {
      resize.disconnect();
      dialogs.disconnect();
      window.removeEventListener("resize", move);
      subs.forEach((s) => s.then((un) => un()));
      api.viewerHide().catch(() => {});
    };
  }, []);

  const go = (to: string) => api.viewerGo(to).catch((e) => toast(errorText(e), "error"));
  return (
    <div className="web-viewer">
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
          <ExternalIcon /> {t("Open in {name}", { name: outside })}
        </button>
      </form>
      {privateOn && <p className="muted small viewer-note">{t("Pages here are visited directly, not through your private connection: sites see this PC's address.")}</p>}
      <div className="viewer-area" ref={area} title={title} aria-busy={loading}>
        {error && <p className="viewer-error">{error}</p>}
      </div>
    </div>
  );
}
