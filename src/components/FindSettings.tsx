// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { errorText } from "../api";
import * as files from "../api/files";
import { bytes } from "../format";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

/** Folders read into the Find index, search, duplicates and tidying. */
export function FindSettings({ toast }: { toast: PushToast }) {
  const [folders, setFolders] = useState<string[]>([]);
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<files.FindHit[]>([]);
  const [dupes, setDupes] = useState<files.DupeGroup[] | null>(null);
  const [plan, setPlan] = useState<files.TidyMove[] | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    files.findFolders().then((f) => setFolders(f ?? [])).catch(() => {});
  }, []);
  const work = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const report = (r: files.IndexReport) => toast(t("{added} added, {updated} updated, {removed} removed", { added: r.added, updated: r.updated, removed: r.removed }), "info");
  return (
    <section className="card">
      <h2>{t("Find")}</h2>
      <p className="small muted">{t("Search file names, document text and picture descriptions in folders you add. Nothing is read until you add a folder, and the index stays on this PC.")}</p>
      <ul className="small">
        {folders.map((f) => (
          <li key={f} className="row">
            <span className="grow">{f}</span>
            <button className="btn small" onClick={() => work(async () => { await files.findRemoveFolder(f); setFolders((await files.findFolders()) ?? []); })}>
              {t("Remove")}
            </button>
          </li>
        ))}
      </ul>
      <div className="row wrap small">
        <button
          className="btn small"
          disabled={busy}
          onClick={async () => {
            const dir = await openFile({ directory: true, multiple: false });
            if (typeof dir === "string") work(async () => { report(await files.findAddFolder(dir)); setFolders((await files.findFolders()) ?? []); });
          }}
        >
          {t("Add a folder")}
        </button>
        <button className="btn small" disabled={busy || !folders.length} onClick={() => work(async () => report(await files.findRefresh()))}>
          {t("Update the index")}
        </button>
        <button className="btn small" disabled={busy || !folders.length} onClick={() => work(async () => setDupes(await files.findDuplicates()))}>
          {t("Find duplicates")}
        </button>
        <button
          className="btn small"
          disabled={busy || !folders.length}
          title={t("Uses the picture model on this PC to describe up to 20 pictures, so they can be found by what they show.")}
          onClick={() => work(async () => toast(t("{n} pictures described", { n: await files.describePictures(20) }), "success"))}
        >
          {t("Describe pictures")}
        </button>
        <button
          className="btn small"
          disabled={busy}
          onClick={async () => {
            const dir = await openFile({ directory: true, multiple: false });
            if (typeof dir === "string") work(async () => setPlan(await files.tidyPlan(dir)));
          }}
        >
          {t("Tidy a folder")}
        </button>
      </div>
      <form className="row" onSubmit={(e) => { e.preventDefault(); work(async () => setHits(await files.findSearch(q))); }}>
        <input className="grow" value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("Search your files")} aria-label={t("Search your files")} />
        <button className="btn small" disabled={busy || !q.trim()}>{t("Search")}</button>
      </form>
      {hits.map((h) => (
        <div key={h.path} className="small find-hit">
          <strong>{h.name}</strong> <span className="muted">{bytes(h.size)}</span>
          <div className="muted">{h.path}</div>
          <div>{files.snippetParts(h.snippet).map((p, i) => (p.hit ? <b key={i}>{p.text}</b> : <span key={i}>{p.text}</span>))}</div>
        </div>
      ))}
      {dupes && (
        <div className="small">
          <strong>{dupes.length ? t("{n} sets of duplicates", { n: dupes.length }) : t("No duplicates found.")}</strong>
          {dupes.map((g) => (
            <div key={g.paths[0]}>
              {bytes(g.size)}: {g.paths.join(", ")}
            </div>
          ))}
        </div>
      )}
      {plan && (
        <div className="small">
          <strong>{plan.length ? t("This would move {n} files into folders by kind:", { n: plan.length }) : t("Nothing to tidy.")}</strong>
          {plan.map((m) => (
            <div key={m.from}>{m.from} → {m.to}</div>
          ))}
          <div className="row">
            {plan.length > 0 && (
              <button
                className="btn small primary"
                disabled={busy}
                onClick={() => work(async () => {
                  const [done, skipped] = await files.tidyApply(plan);
                  toast(t("{done} moved, {skipped} left in place", { done, skipped: skipped.length }), "info");
                  setPlan(null);
                })}
              >
                {t("Move them")}
              </button>
            )}
            <button className="btn small" onClick={() => setPlan(null)}>{t("Cancel")}</button>
          </div>
        </div>
      )}
    </section>
  );
}
