// SPDX-License-Identifier: AGPL-3.0-only
// One widget in its sealed frame, plus the once-only question when it first
// asks for an action. Only messages from this frame's own window are heard.
import { useEffect, useRef, useState } from "react";
import type { View } from "../App";
import { t, tx } from "../i18n";
import { resolvedTheme } from "../theme";
import { parseRequest, updateWidget, widgetDoc, widgetState, type Widget, type WidgetAction, type WidgetRequest } from "./widgets";

export const ACTION_TEXT: Record<WidgetAction, string> = {
  notify: tx("show short notices"),
  open_page: tx("open pages like Notes or Tasks"),
  copy: tx("copy text to the clipboard"),
  remember: tx("keep a little of its own data on this PC"),
};

interface Props {
  widget: Widget;
  height?: number;
  notify: (text: string) => void;
  openPage?: (page: View) => void;
}

export function WidgetFrame({ widget, height = 160, notify, openPage }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [asking, setAsking] = useState<WidgetRequest | null>(null);
  const queue = useRef<WidgetRequest[]>([]);

  const reply = (call: number, ok: boolean, payload?: unknown) => {
    const msg = ok ? { sulcusWidgetReply: 1, call, ok: true, result: payload ?? null } : { sulcusWidgetReply: 1, call, ok: false, error: String(payload ?? "refused") };
    // The frame has an opaque origin, so "*" is the only target that reaches it; the content is never secret.
    frame.current?.contentWindow?.postMessage(msg, "*");
  };

  const perform = async (r: WidgetRequest) => {
    const w = widgetState().widgets.find((x) => x.id === widget.id);
    if (!w) return;
    try {
      switch (r.action) {
        case "notify":
          notify(`${w.name}: ${r.args.text as string}`);
          return reply(r.call, true);
        case "open_page":
          if (!openPage) return reply(r.call, false, "not available here");
          openPage(r.args.page as View);
          return reply(r.call, true);
        case "copy":
          await navigator.clipboard.writeText(r.args.text as string);
          return reply(r.call, true);
        case "remember":
          if (typeof r.args.value === "string") {
            updateWidget(w.id, { memory: r.args.value });
            return reply(r.call, true);
          }
          return reply(r.call, true, w.memory);
      }
    } catch (e) {
      reply(r.call, false, e instanceof Error ? e.message : "failed");
    }
  };

  const next = () => {
    const r = queue.current.shift() ?? null;
    setAsking(r);
  };

  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (!frame.current || e.source !== frame.current.contentWindow) return;
      const r = parseRequest(e.data);
      if (!r) return;
      if ("error" in r) return reply(r.call, false, r.error);
      const grant = widgetState().widgets.find((x) => x.id === widget.id)?.grants[r.action];
      if (grant === "allowed") return void perform(r);
      if (grant === "denied") return reply(r.call, false, "not allowed");
      // Asked once: later calls for the same action wait for the same answer.
      if (queue.current.length > 20) return reply(r.call, false, "too many requests");
      queue.current.push(r);
      setAsking((cur) => cur ?? queue.current.shift() ?? null);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [widget.id]);

  const answer = (allow: boolean) => {
    if (!asking) return;
    const w = widgetState().widgets.find((x) => x.id === widget.id);
    if (w) updateWidget(w.id, { grants: { ...w.grants, [asking.action]: allow ? "allowed" : "denied" } });
    const same = [asking, ...queue.current.filter((q) => q.action === asking.action)];
    queue.current = queue.current.filter((q) => q.action !== asking.action);
    for (const r of same) allow ? void perform(r) : reply(r.call, false, "not allowed");
    next();
  };

  const dark = resolvedTheme() !== "light";
  return (
    <div className="widget-box">
      <iframe
        ref={frame}
        className="widget-frame"
        title={widget.name}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        srcDoc={widgetDoc(widget.html, dark)}
        style={{ height }}
      />
      {asking && (
        <div className="widget-ask" role="alertdialog" aria-label={t("Widget permission")}>
          <span className="small">{t("{name} wants to {what}. Allow it from now on?", { name: widget.name, what: t(ACTION_TEXT[asking.action]) })}</span>
          <span className="row">
            <button className="btn small primary" onClick={() => answer(true)}>
              {t("Allow")}
            </button>
            <button className="btn small ghost" onClick={() => answer(false)}>
              {t("Don't allow")}
            </button>
          </span>
        </div>
      )}
    </div>
  );
}
