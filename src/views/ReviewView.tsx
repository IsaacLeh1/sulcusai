// SPDX-License-Identifier: AGPL-3.0-only
// Review my code: a file, a folder or a GitHub repository checked for what
// the person picks, by the model they pick or the best at code on this PC.
// It only reads the code; the result is a report of findings.
import { useCallback, useEffect, useState } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type CodeReport, type ReviewSetup } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { Busy, useModelPlace, useSince } from "../components/Busy";
import { ModelSelect } from "../components/ModelChoices";
import type { PushToast } from "../components/Toasts";

const CHECK_LABEL: Record<string, string> = {
  bugs: "Functionality and bugs",
  security: "Security",
  clean: "Cleanliness",
  style: "Professionalism and style",
  speed: "Performance",
  tests: "Tests",
  docs: "Documentation",
  access: "Accessibility",
};

export function ReviewView({ toast }: { toast: PushToast }) {
  const [setup, setSetup] = useState<ReviewSetup | null>(null);
  const [reports, setReports] = useState<CodeReport[]>([]);
  const [open, setOpen] = useState<CodeReport | null>(null);
  const [source, setSource] = useState("");
  const [checks, setChecks] = useState<string[]>(["bugs", "security"]);
  const [notes, setNotes] = useState("");
  const [model, setModel] = useState<string | null>(null);
  const [busy, setBusy] = useState<{ stage: string; fraction: number } | null>(null);
  const since = useSince(!!busy);
  const place = useModelPlace(!!busy);

  const load = useCallback(() => api.reviews().then(setReports).catch(() => {}), []);
  useEffect(() => {
    api.reviewSetup().then(setSetup).catch(() => {});
    load();
    const sub = on("review:progress", (p) => setBusy((b) => (b ? p : b)));
    return () => {
      sub.then((un) => un());
    };
  }, [load]);

  const pick = async (folder: boolean) => {
    const p = await openDialog(folder ? { directory: true, multiple: false } : { multiple: false });
    if (typeof p === "string") setSource(p);
  };
  const start = async () => {
    setBusy({ stage: t("Starting"), fraction: 0 });
    try {
      const r = await api.reviewStart({ source, checks, notes, model });
      load();
      setOpen(r);
    } catch (e) {
      const msg = errorText(e);
      if (!/cancel/i.test(msg)) toast(msg, "error");
    } finally {
      setBusy(null);
    }
  };
  const saveReport = async (r: CodeReport) => {
    const to = await saveDialog({ defaultPath: "code-review.md", filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (!to) return;
    try {
      await api.reviewSave(r.id, to);
      toast(t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  if (open) {
    const by = (sev: string) => open.findings.filter((f) => f.severity === sev);
    return (
      <div className="page review">
        <button className="link small" onClick={() => setOpen(null)}>{t("← All reviews")}</button>
        <header className="page-head">
          <div>
            <h1 className="ellipsis" title={open.source}>{open.source.split(/[\\/]/).filter(Boolean).pop() ?? open.source}</h1>
            <p className="small muted">
              {t("{files} files, {lines} lines, checked by {model} in {time}.", { files: open.files, lines: open.lines, model: open.model, time: clock(open.took_ms / 1000) })}
              {open.skipped > 0 && ` ${t("{n} files were left out (too big, generated or too many).", { n: open.skipped })}`}
            </p>
          </div>
          <button className="btn" onClick={() => saveReport(open)}>{t("Save as Markdown…")}</button>
        </header>
        {open.summary && <section className="card"><p>{open.summary}</p></section>}
        <div className="review-counts">
          {(["high", "medium", "low"] as const).map((s) => (
            <span key={s} className={`review-count sev-${s}`}>{s === "high" ? t("{n} high", { n: by(s).length }) : s === "medium" ? t("{n} medium", { n: by(s).length }) : t("{n} low", { n: by(s).length })}</span>
          ))}
          {open.dropped > 0 && <span className="small muted">{t("{n} more were dropped when checked again.", { n: open.dropped })}</span>}
        </div>
        {open.findings.length === 0 && <p className="muted">{t("Nothing that needs fixing was found for what was checked.")}</p>}
        <div className="review-findings">
          {open.findings.map((f, i) => (
            <div key={i} className={`card review-finding sev-${f.severity}`}>
              <div className="row wrap">
                <span className={`review-sev sev-${f.severity}`}>{f.severity === "high" ? t("High") : f.severity === "medium" ? t("Medium") : t("Low")}</span>
                <strong>{f.title}</strong>
                <span className="spacer" />
                <span className="small muted">{t(CHECK_LABEL[f.check] ?? f.check)}</span>
              </div>
              <code className="small review-where">{f.file}{f.line ? `:${f.line}` : ""}</code>
              <p className="small">{f.why}</p>
              {f.fix && <p className="small review-fix"><strong>{t("Fix:")}</strong> {f.fix}</p>}
            </div>
          ))}
        </div>
      </div>
    );
  }

  const best = setup?.best;
  return (
    <div className="page review">
      <header className="page-head">
        <div>
          <h1>{t("Review my code")}</h1>
          <p className="muted">{t("Checks a file, a folder or a GitHub repository for what you pick, and lists what it finds with where it is. It only reads the code: nothing is changed.")}</p>
        </div>
      </header>
      <section className="card form review-start">
        <label>
          {t("What to review")}
          <div className="row">
            <input className="input grow" value={source} placeholder={t("A folder, a file, or https://github.com/owner/repository")} onChange={(e) => setSource(e.target.value)} />
            <button className="btn small" onClick={() => pick(true)}>{t("Folder…")}</button>
            <button className="btn small" onClick={() => pick(false)}>{t("File…")}</button>
          </div>
          {/github\.com/i.test(source) && <span className="small muted">{t("Public repositories only: its code is downloaded from GitHub to this PC for the review, and listed under What left this PC.")}</span>}
        </label>
        <div>
          <span className="field-label">{t("What to check")}</span>
          <div className="review-checks">
            {(setup?.checks ?? []).map(([id]) => (
              <label key={id} className="check small">
                <input type="checkbox" checked={checks.includes(id)} onChange={(e) => setChecks((c) => (e.target.checked ? [...c, id] : c.filter((x) => x !== id)))} />
                {t(CHECK_LABEL[id] ?? id)}
              </label>
            ))}
          </div>
        </div>
        <label>
          {t("Anything else to look at (optional)")}
          <input className="input" value={notes} placeholder={t("e.g. how the login works, or whether it's ready to publish")} onChange={(e) => setNotes(e.target.value)} />
        </label>
        <label>
          {t("Reviewed by")}
          <ModelSelect value={model} onChange={setModel} label={t("The model that reviews")} defaultLabel={best ? t("Best for code on this PC ({name})", { name: best[1] }) : t("Your default model")} />
          <span className="small muted">{t("Each check reads the code separately, so more checks take longer. A bigger model finds more.")}</span>
        </label>
        {busy ? (
          <Busy stage={busy.stage} fraction={busy.fraction > 0 && busy.fraction < 1 ? busy.fraction : null} since={since} where={place} onStop={() => api.reviewStop().catch(() => {})} />
        ) : (
          <div className="row">
            <span className="spacer" />
            <button className="btn primary" disabled={!source.trim() || checks.length === 0} onClick={start}>{t("Review it")}</button>
          </div>
        )}
      </section>
      {reports.length > 0 && (
        <section className="review-list">
          <h2>{t("Earlier reviews")}</h2>
          {reports.map((r) => (
            <div key={r.id} className="card review-row row wrap">
              <button className="review-open grow" onClick={() => setOpen(r)}>
                <strong className="ellipsis">{r.source}</strong>
                <span className="small muted">
                  {new Date(r.created_at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })} · {t("{n} findings", { n: r.findings.length })} · {r.model}
                </span>
              </button>
              <button className="btn ghost small danger" onClick={() => api.reviewDelete(r.id).then(load).catch((e) => toast(errorText(e), "error"))}>{t("Delete")}</button>
            </div>
          ))}
        </section>
      )}
    </div>
  );
}
