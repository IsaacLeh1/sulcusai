// SPDX-License-Identifier: AGPL-3.0-only
// Settings card for widgets: the switch, adding one from pasted HTML, and per
// widget its preview, sidebar pin, the answers it was given and removal.
import { useState } from "react";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";
import { ACTION_TEXT, WidgetFrame } from "./WidgetFrame";
import { MAX_HTML, MAX_WIDGETS, WIDGET_ACTIONS, addWidget, removeWidget, saveWidgets, updateWidget, useWidgets, type Widget } from "./widgets";

function Answers({ w }: { w: Widget }) {
  const given = WIDGET_ACTIONS.filter((a) => w.grants[a]);
  if (!given.length) return <p className="small muted">{t("It hasn't asked for anything yet.")}</p>;
  return (
    <ul className="small widget-grants">
      {given.map((a) => (
        <li key={a}>
          {w.grants[a] === "allowed" ? t("Allowed to {what}", { what: t(ACTION_TEXT[a]) }) : t("Not allowed to {what}", { what: t(ACTION_TEXT[a]) })}{" "}
          <button
            className="btn ghost small"
            onClick={() => {
              const grants = { ...w.grants };
              delete grants[a];
              updateWidget(w.id, { grants });
            }}
          >
            {t("Ask again")}
          </button>
        </li>
      ))}
    </ul>
  );
}

export function WidgetsSettings({ toast }: { toast: PushToast }) {
  const s = useWidgets();
  const [name, setName] = useState("");
  const [html, setHtml] = useState("");
  return (
    <>
      <div className="toggle-row">
        <div>
          <strong>{t("Widgets")}</strong>
          <span className="small muted block">
            {t("Small pages of your own in the sidebar. Each runs sealed off: no internet, no files, no access to your chats. It can only ask for a few things, and you answer each once.")}
          </span>
        </div>
        <label className="switch" title={s.on ? t("On") : t("Off")}>
          <input type="checkbox" checked={s.on} aria-label={t("Widgets")} onChange={(e) => saveWidgets({ ...s, on: e.target.checked })} />
          <span />
        </label>
      </div>
      {s.on && (
        <>
          <form
            className="widget-add"
            onSubmit={(e) => {
              e.preventDefault();
              const w = addWidget(name, html);
              if (!w) return toast(t("Too big, or already {n} widgets.", { n: MAX_WIDGETS }), "error");
              setName("");
              setHtml("");
            }}
          >
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Name")} aria-label={t("Widget name")} maxLength={60} />
            <textarea
              className="input mono"
              rows={5}
              value={html}
              onChange={(e) => setHtml(e.target.value)}
              placeholder={t("Paste the widget's HTML. It can call sulcus.call(\"notify\", { text }) and similar.")}
              aria-label={t("Widget HTML")}
            />
            <button className="btn small" type="submit" disabled={!html.trim() || html.length > MAX_HTML}>
              {t("Add widget")}
            </button>
          </form>
          {s.widgets.map((w) => (
            <div key={w.id} className="widget-item">
              <div className="row">
                <strong>{w.name}</strong>
                <label className="check small">
                  <input type="checkbox" checked={w.pinned} onChange={(e) => updateWidget(w.id, { pinned: e.target.checked })} /> {t("In the sidebar")}
                </label>
                <button className="btn ghost small" onClick={() => removeWidget(w.id)}>
                  {t("Remove")}
                </button>
              </div>
              <WidgetFrame widget={w} notify={(text) => toast(text, "info")} />
              <Answers w={w} />
            </div>
          ))}
        </>
      )}
    </>
  );
}
