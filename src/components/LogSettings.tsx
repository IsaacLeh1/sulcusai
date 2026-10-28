// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type LogsView } from "../api";
import { bytes } from "../format";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";
import { confirmed } from "../confirm";

/** What each log holds, by file name (engines' logs share a start). */
const ABOUT: [RegExp, string][] = [
  [/^app\.(old\.)?log$/, tx("What SulcusAI did: starting up, models loaded, pictures made (and on which card, and how long), downloads, updates, settings changed, and errors.")],
  [/^crashes\.(old\.)?log$/, tx("Every crash, freeze and model that didn't load, with where it happened and what the engine's lines mean. Kept here whether or not crash reports are sent.")],
  [/^dev-sent\.log$/, tx("Every report developer mode sent to the developer, exactly as sent.")],
  [/^engine\.log$/, tx("The chat engine (llama.cpp) while it runs a model.")],
  [/^media\.log$/, tx("The picture and video engine (stable-diffusion.cpp).")],
  [/^music-/, tx("The music engine (acestep.cpp).")],
  [/^speech-/, tx("Speech recognition (whisper.cpp).")],
  [/^finetune-/, tx("Teaching a model (fine-tuning).")],
];

function about(name: string): string {
  return ABOUT.find(([re]) => re.test(name))?.[1] ?? "";
}

/** Settings → Logs: what SulcusAI keeps track of on this PC, and the logs to read. */
export function LogSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<LogsView | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [text, setText] = useState<string>("");
  const [meanings, setMeanings] = useState<{ line: string; meaning: string }[]>([]);
  const load = () => api.logsView().then(setV).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  const show = async (name: string) => {
    setOpen(name);
    try {
      setText(await api.readLog(name, 400));
      setMeanings(await api.logMeanings(name, 400));
    } catch (e) {
      setText(errorText(e));
      setMeanings([]);
    }
  };
  const act = (f: () => Promise<unknown>) => f().catch((e) => toast(errorText(e), "error"));
  if (!v) return null;
  return (
    <section className="card">
      <h2>{t("Logs")}</h2>
      <p className="muted small">
        {t("SulcusAI keeps logs on this PC to help find out what went wrong. They never hold what you say in chats, your prompts, notes, mail or files' contents, and nothing here is sent anywhere.")}
      </p>
      <div className="row wrap">
        <button className="btn small" onClick={() => act(() => api.openAppFolder(false))}>{t("Open logs folder")}</button>
        <button className="btn small" onClick={() => act(() => api.openAppFolder(true))}>{t("Open SulcusAI's data folder")}</button>
        <span className="muted small ellipsis" title={v.folder}>{v.folder}</span>
      </div>
      {v.files.length === 0 ? (
        <p className="muted small">{t("No logs yet.")}</p>
      ) : (
        <ul className="teach-list small logs-list">
          {v.files.map((f) => (
            <li key={f.name}>
              <span className="ellipsis">
                <strong>{f.name}</strong> <span className="muted">· {bytes(f.size)} · {new Date(f.modified).toLocaleString()}</span>
                {about(f.name) && <span className="muted block">{t(about(f.name))}</span>}
              </span>
              <button className={`btn ghost small ${open === f.name ? "active" : ""}`} onClick={() => (open === f.name ? setOpen(null) : show(f.name))}>
                {open === f.name ? t("Hide") : t("View")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {open && (
        <div className="log-viewer">
          <div className="adv-head">
            <strong className="small">{t("Last lines of {name}", { name: open })}</strong>
            <span className="row">
              <button className="btn ghost small" onClick={() => show(open)}>{t("Refresh")}</button>
              <button className="btn ghost small" onClick={() => navigator.clipboard.writeText(text).then(() => toast(t("Copied."), "success")).catch(() => toast(t("Couldn't copy it."), "error"))}>{t("Copy")}</button>
            </span>
          </div>
          {meanings.length > 0 && (
            <div className="log-meanings">
              <strong className="small">{t("What these lines mean")}</strong>
              <ul className="small">
                {meanings.map((m) => (
                  <li key={m.meaning}>
                    <code className="block ellipsis" title={m.line}>{m.line}</code>
                    <span>{t(m.meaning)}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <pre className="log-text">{text || t("(empty)")}</pre>
        </div>
      )}
      {v.files.length > 0 && (
        <button
          className="btn ghost small danger"
          onClick={async () => {
            if (await confirmed(t("Delete SulcusAI's logs on this PC? They start again from now."))) act(async () => setV(await api.clearLogs()));
          }}
        >
          {t("Clear logs")}
        </button>
      )}
    </section>
  );
}
