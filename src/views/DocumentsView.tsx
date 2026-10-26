// SPDX-License-Identifier: AGPL-3.0-only
// The documents library: folders and files the person adds, read and
// indexed on this PC, so any chat can answer from them and say where each
// answer came from. Also a search to try it.
import { useCallback, useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type DocFile, type DocHit, type LibraryView } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import type { PushToast } from "../components/Toasts";

export function DocumentsView({ toast }: { toast: PushToast }) {
  const [view, setView] = useState<LibraryView | null>(null);
  const [opened, setOpened] = useState<string | null>(null);
  const [files, setFiles] = useState<DocFile[]>([]);
  const [removing, setRemoving] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<DocHit[] | null>(null);
  const [searching, setSearching] = useState(false);

  const load = useCallback(() => api.docsLibrary().then(setView).catch((e) => toast(errorText(e), "error")), [toast]);
  useEffect(() => {
    load();
    const p = on("docs:progress", (pr) => setView((v) => (v ? { ...v, progress: pr.running ? pr : null } : v)));
    const c = on("docs:changed", () => load());
    return () => {
      p.then((un) => un());
      c.then((un) => un());
    };
  }, [load]);

  useEffect(() => {
    if (!opened) return;
    api.docsFiles(opened).then(setFiles).catch(() => setFiles([]));
  }, [opened, view?.files]);

  const add = async (folder: boolean) => {
    const picked = await openDialog(folder ? { directory: true, multiple: true } : { multiple: true, filters: [{ name: t("Documents"), extensions: view?.kinds ?? [] }] });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    if (paths.length === 0) return;
    try {
      setView(await api.docsAdd(paths));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const search = async () => {
    if (!query.trim()) return;
    setSearching(true);
    try {
      setHits(await api.docsSearch(query, 8));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSearching(false);
    }
  };

  if (!view) return null;
  const progress = view.progress;

  return (
    <div className="page documents">
      <header className="page-head">
        <div>
          <h1>{t("Documents")}</h1>
          <p className="muted">
            {t("Add folders and files, then ask about them in any chat. Answers say which file and page they came from, and the link opens it. Everything is read and kept on this PC, encrypted.")}
          </p>
        </div>
        <div className="row">
          <button className="btn" onClick={() => add(true)}>{t("Add a folder…")}</button>
          <button className="btn" onClick={() => add(false)}>{t("Add files…")}</button>
        </div>
      </header>

      {progress ? (
        <section className="card docs-progress">
          <div className="row">
            <strong className="small">{progress.stage}</strong>
            {progress.file && <span className="small muted ellipsis">{progress.file}</span>}
            <span className="spacer" />
            {progress.total > 0 && <span className="small muted">{t("{done} of {total}", { done: progress.done, total: progress.total })}</span>}
            <button className="btn ghost small" onClick={() => api.docsStop().catch(() => {})}>{t("Stop")}</button>
          </div>
          <div className="progress"><span style={{ width: `${progress.total ? Math.round((progress.done / progress.total) * 100) : 5}%` }} /></div>
        </section>
      ) : (
        view.sources.length > 0 && (
          <p className="small muted">
            {t("{files} files, {passages} passages.", { files: view.files, passages: view.passages })}{" "}
            <button className="link small" onClick={() => api.docsUpdate().then(setView).catch((e) => toast(errorText(e), "error"))}>{t("Look for changes now")}</button>
          </p>
        )
      )}
      {!view.model_ready && view.sources.length > 0 && (
        <p className="small muted">
          {t("The first time, a small model that finds passages by their meaning downloads ({size}). Until it's here, passages are found by their words.", { size: bytes(view.model_size) })}
        </p>
      )}

      {view.sources.length === 0 ? (
        <section className="card empty-state">
          <h2>{t("Ask questions across your documents")}</h2>
          <p className="muted">
            {t("Add the folders where you keep contracts, reports, school notes or manuals. SulcusAI reads PDFs, Word, PowerPoint, Excel, text and web pages, and finds the passages that answer a question, even when it's worded differently.")}
          </p>
          <div className="row">
            <button className="btn primary" onClick={() => add(true)}>{t("Add a folder…")}</button>
            <button className="btn" onClick={() => add(false)}>{t("Add files…")}</button>
          </div>
        </section>
      ) : (
        <section className="docs-sources">
          {view.sources.map((s) => (
            <div key={s.id} className="card docs-source">
              <div className="row wrap">
                <div className="grow">
                  <strong>{s.name}</strong>
                  <div className="small muted ellipsis" title={s.path}>{s.path}</div>
                </div>
                <span className="small muted">
                  {s.folder ? t("{n} files", { n: s.files }) : ""} {t("{n} passages", { n: s.passages })}
                  {s.scanned_at ? ` · ${t("checked {when}", { when: new Date(s.scanned_at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) })}` : ""}
                </span>
                <button className="btn ghost small" aria-expanded={opened === s.id} onClick={() => setOpened(opened === s.id ? null : s.id)}>
                  {opened === s.id ? t("Hide files") : t("Files")}
                </button>
                <button
                  className="btn ghost small danger"
                  onBlur={() => setRemoving(null)}
                  onClick={() => {
                    if (removing !== s.id) return setRemoving(s.id);
                    setRemoving(null);
                    api.docsRemove(s.id).then(setView).catch((e) => toast(errorText(e), "error"));
                  }}
                >
                  {removing === s.id ? t("Take it out of the library?") : t("Remove")}
                </button>
              </div>
              {s.problems > 0 && <p className="small warn-text">{t("{n} file(s) couldn't be read: open Files to see why.", { n: s.problems })}</p>}
              {s.words_only > 0 && view.model_ready && <p className="small muted">{t("{n} file(s) are found by their words for now; they're indexed by meaning on the next look for changes.", { n: s.words_only })}</p>}
              {opened === s.id && (
                <ul className="docs-files">
                  {files.map((f) => (
                    <li key={f.id} className="row">
                      <button className="link small ellipsis" title={f.path} onClick={() => api.docsOpen(f.id).catch((e) => toast(errorText(e), "error"))}>{f.name}</button>
                      <span className="spacer" />
                      {f.error ? <span className="small warn-text">{f.error}</span> : <span className="small muted">{t("{n} passages", { n: f.passages })}</span>}
                    </li>
                  ))}
                  {files.length === 0 && <li className="small muted">{progress ? t("Reading…") : t("No documents found here.")}</li>}
                </ul>
              )}
            </div>
          ))}
        </section>
      )}

      {view.sources.length > 0 && (
        <section className="card docs-try">
          <h2>{t("Try a question")}</h2>
          <p className="small muted">{t("See which passages a chat would get. In a chat, just ask: the assistant searches the library when your question is about your documents.")}</p>
          <div className="row">
            <input className="input grow" value={query} placeholder={t("When does my lease end?")} aria-label={t("Question")} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === "Enter" && search()} />
            <button className="btn" disabled={!query.trim() || searching} onClick={search}>{searching ? t("Searching…") : t("Search")}</button>
          </div>
          {hits && hits.length === 0 && <p className="small muted">{t("Nothing matched.")}</p>}
          {hits && hits.length > 0 && (
            <ol className="docs-hits">
              {hits.map((h, i) => (
                <li key={i}>
                  <button className="link small" onClick={() => api.docsOpen(h.file_id).catch((e) => toast(errorText(e), "error"))}>
                    {h.place ? `${h.name}, ${h.place}` : h.name}
                  </button>
                  <p className="small">{h.text.length > 420 ? `${h.text.slice(0, 420)}…` : h.text}</p>
                </li>
              ))}
            </ol>
          )}
        </section>
      )}
    </div>
  );
}
