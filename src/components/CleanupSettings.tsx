// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type CacheItem, type CleanItem, type CleanView } from "../api";
import { bytes } from "../format";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";
import { confirmed } from "../confirm";

const DAYS = [7, 14, 30, 60, 90, 180, 365];

/** What each cache holds and what clearing it does, by id (the core lists the files). */
const CACHE_TEXT: Record<string, { name: string; what: string }> = {
  media_work: { name: tx("Leftovers from making pictures, video and music"), what: tx("Scratch files from jobs that were stopped or crashed. Nothing in the gallery is touched.") },
  downloads: { name: tx("Unfinished downloads"), what: tx("Model and engine downloads that didn't finish. Paused downloads start over if you resume them.") },
  old_engines: { name: tx("Old engine versions"), what: tx("Engines from earlier versions of SulcusAI. The ones it uses now are kept.") },
  undo: { name: tx("Undo copies of files the assistant changed"), what: tx("Copies kept so the assistant's file changes can be undone. Changes made before clearing can't be undone afterwards.") },
  browser: { name: tx("Built-in browser data"), what: tx("History, cookies and sign-ins of SulcusAI's built-in browser. Sites there will ask you to sign in again.") },
  window: { name: tx("The app window's temporary files"), what: tx("Files the app's window keeps to start faster. It makes them again as needed. Files in use stay until SulcusAI restarts.") },
  tor: { name: tx("Private connection (Tor) saved list"), what: tx("The saved list of Tor relays. It's downloaded again the next time the private connection starts.") },
  restore_copies: { name: tx("Copies kept from before a restore"), what: tx("Your old data, kept when a backup was restored over it.") },
  crashes: { name: tx("Saved crash reports"), what: tx("Crash reports that weren't sent. The crash log in Settings → Logs stays.") },
  screens: { name: tx("Screenshots for questions about the screen"), what: tx("Pictures of the screen taken when you asked about it.") },
  logs: { name: tx("Logs"), what: tx("SulcusAI's logs (Settings → Logs). They start again from now.") },
};

const KIND_WORD: Record<CleanItem["kind"], string> = { chat: tx("Chat"), model: tx("Model"), media: tx("Studio model") };

function DaysPick({ label, value, onChange }: { label: string; value: number | null; onChange: (d: number | null) => void }) {
  return (
    <label className="row small cleanup-pick">
      <span>{label}</span>
      <select className="input" value={value ?? ""} onChange={(e) => onChange(e.target.value ? +e.target.value : null)}>
        <option value="">{t("Never (keep them)")}</option>
        {DAYS.map((d) => (
          <option key={d} value={d}>
            {d === 365 ? t("1 year") : t("{n} days", { n: d })}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Settings → Cleanup and space: the automatic cleanup of unused chats and
 * models, and clearing caches. */
export function CleanupSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<CleanView | null>(null);
  const [cache, setCache] = useState<CacheItem[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const load = () => {
    api.cleanView().then(setV).catch(() => {});
    api.cacheView().then(setCache).catch(() => {});
  };
  useEffect(load, []);
  if (!v) return null;
  const s = v.settings;
  const on = s.chat_days !== null || s.model_days !== null;

  const set = async (chat: number | null, model: number | null) => {
    try {
      const next = await api.setAutoClean(chat, model);
      setV(next);
      if (!on && (chat !== null || model !== null)) toast(t("The first cleanup runs in a day. Check the list below and lock anything you want to keep."), "info");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const keep = async (i: CleanItem) => {
    try {
      await api.setKept(i.kind === "chat" ? "chat" : "model", i.id, true);
      setV(await api.cleanView());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const total = v.would_remove.reduce((n, i) => n + i.size, 0);
  const chosen = (cache ?? []).filter((c) => picked.has(c.id));

  return (
    <>
      <section className="card cleanup">
        <h2>{t("Automatic cleanup")}</h2>
        <p className="muted small">
          {t("Removes chats and models you haven't used for a while, once a day. It never removes locked chats and models, chats in a locked or shared folder, chats shared with people, your default models or the model in use. Off unless you turn it on.")}
        </p>
        <div className="cleanup-picks">
          <DaysPick label={t("Remove chats not used for")} value={s.chat_days} onChange={(d) => set(d, s.model_days)} />
          <DaysPick label={t("Remove models not used for")} value={s.model_days} onChange={(d) => set(s.chat_days, d)} />
        </div>
        <p className="small muted">
          {t("Lock a chat by right-clicking it in the sidebar, and a model with its lock button on the Models page. Chats removed here are removed from your other synced PCs too; models can be downloaded again.")}
        </p>
        {on && (
          <p className="small">
            {v.next_run && t("Next cleanup: {when}.", { when: new Date(Math.max(v.next_run, Date.now())).toLocaleString() })}{" "}
            {s.last_result && <span className="muted">{t("Last time: {what}.", { what: s.last_result })}</span>}
          </p>
        )}
        {on && (
          <div className="cleanup-list">
            {v.would_remove.length === 0 ? (
              <p className="small muted">{t("Nothing would be removed right now.")}</p>
            ) : (
              <>
                <strong className="small">
                  {t("It would remove {n} now", { n: v.would_remove.length })}
                  {total > 0 && ` · ${t("{size} freed", { size: bytes(total) })}`}
                </strong>
                <ul className="teach-list small">
                  {v.would_remove.map((i) => (
                    <li key={`${i.kind}:${i.id}`}>
                      <span className="ellipsis">
                        <span className="muted">{t(KIND_WORD[i.kind])} · </span>{i.name}
                        <span className="muted"> · {t("last used {when}", { when: new Date(i.last_used).toLocaleDateString() })}{i.size > 0 && ` · ${bytes(i.size)}`}</span>
                      </span>
                      <button className="btn ghost small" onClick={() => keep(i)} title={t("Lock it: the cleanup never removes it")}>
                        {t("Keep")}
                      </button>
                    </li>
                  ))}
                </ul>
                <button
                  className="btn small danger"
                  disabled={busy}
                  onClick={async () => {
                    if (!await confirmed(t("Remove the {n} chats and models listed now? This can't be undone.", { n: v.would_remove.length }))) return;
                    setBusy(true);
                    try {
                      const next = await api.cleanNow();
                      setV(next);
                      toast(next.settings.last_result ?? t("Done."), "success");
                      api.cacheView().then(setCache).catch(() => {});
                    } catch (e) {
                      toast(errorText(e), "error");
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {t("Clean up now")}
                </button>
              </>
            )}
          </div>
        )}
      </section>

      <section className="card cleanup">
        <h2>{t("Clear temporary files")}</h2>
        <p className="muted small">{t("Free up space. Your chats, notes, gallery, settings and installed models are never touched.")}</p>
        {!cache ? (
          <p className="muted small">{t("Loading…")}</p>
        ) : (
          <ul className="cache-list">
            {cache.map((c) => {
              const text = CACHE_TEXT[c.id] ?? { name: c.name, what: c.what };
              return (
                <li key={c.id}>
                  <label className="check">
                    <input
                      type="checkbox"
                      disabled={c.size === 0}
                      checked={picked.has(c.id)}
                      onChange={(e) => {
                        const next = new Set(picked);
                        if (e.target.checked) next.add(c.id);
                        else next.delete(c.id);
                        setPicked(next);
                      }}
                    />
                    <span>
                      <strong>{t(text.name)}</strong> <span className="muted small">· {c.size > 0 ? bytes(c.size) : t("empty")}</span>
                      <span className="muted small block">{t(text.what)}</span>
                    </span>
                  </label>
                </li>
              );
            })}
          </ul>
        )}
        <button
          className="btn small"
          disabled={busy || chosen.length === 0}
          onClick={async () => {
            const names = chosen.map((c) => t((CACHE_TEXT[c.id] ?? c).name)).join(", ");
            if (!await confirmed(t("Delete these now? {names}", { names }))) return;
            setBusy(true);
            try {
              const r = await api.clearCache(chosen.map((c) => c.id));
              toast(
                r.skipped > 0
                  ? t("Freed {size}. {n} files in use stay until SulcusAI restarts.", { size: bytes(r.freed), n: r.skipped })
                  : t("Freed {size}.", { size: bytes(r.freed) }),
                "success",
              );
              setPicked(new Set());
              api.cacheView().then(setCache).catch(() => {});
            } catch (e) {
              toast(errorText(e), "error");
            } finally {
              setBusy(false);
            }
          }}
        >
          {chosen.length ? t("Clear selected ({size})", { size: bytes(chosen.reduce((n, c) => n + c.size, 0)) }) : t("Clear selected")}
        </button>
      </section>
    </>
  );
}
