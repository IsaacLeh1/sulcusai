// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { errorText } from "../api";
import * as files from "../api/files";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

const pick = async (exts: string[], name: string) => {
  const p = await openFile({ multiple: false, filters: [{ name, extensions: exts }] });
  return typeof p === "string" ? p : null;
};

/** Scans, home inventory, PDF questions/compare/redact and sheet plans. */
export function DocumentTools({ toast }: { toast: PushToast }) {
  const [busy, setBusy] = useState(false);
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
  return (
    <section className="card">
      <h2>{t("Documents")}</h2>
      <Scans busy={busy} work={work} />
      <Inventory busy={busy} work={work} toast={toast} />
      <Forms busy={busy} work={work} toast={toast} />
      <PdfTools busy={busy} work={work} toast={toast} />
      <SheetPlan busy={busy} work={work} toast={toast} />
    </section>
  );
}

type Work = { busy: boolean; work: (f: () => Promise<void>) => Promise<void> };

function Scans({ busy, work }: Work) {
  const [list, setList] = useState<files.FileRecord[]>([]);
  const load = () => files.recordsList("scan").then(setList).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  return (
    <div className="small">
      <h3>{t("Scan")}</h3>
      <p className="muted">{t("Take a photo of a page (on the phone it opens the camera). Its words are read on this PC and it becomes findable.")}</p>
      <button
        className="btn small"
        disabled={busy}
        onClick={async () => {
          const p = await pick(["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp", "gif"], t("Photos"));
          if (p) work(async () => { await files.scanSave(p); await load(); });
        }}
      >
        {t("Scan a page")}
      </button>
      {list.map((r) => (
        <details key={r.id}>
          <summary>{String(r.data.title || t("Untitled scan"))}</summary>
          <pre className="wrap">{String(r.data.text || r.data.unread || "")}</pre>
        </details>
      ))}
    </div>
  );
}

function Inventory({ busy, work, toast }: Work & { toast: PushToast }) {
  const [items, setItems] = useState<files.FileRecord[]>([]);
  const [totals, setTotals] = useState<files.RoomTotal[]>([]);
  const [draft, setDraft] = useState({ name: "", room: "", value: "", serial: "" });
  const load = async () => {
    setItems(await files.recordsList("inventory"));
    setTotals(await files.inventoryTotals());
  };
  useEffect(() => {
    load().catch(() => {});
  }, []);
  return (
    <div className="small">
      <h3>{t("Home inventory")}</h3>
      <form
        className="row wrap"
        onSubmit={(e) => {
          e.preventDefault();
          work(async () => { await files.inventorySave(draft); setDraft({ name: "", room: "", value: "", serial: "" }); await load(); });
        }}
      >
        <input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder={t("Item")} aria-label={t("Item")} />
        <input value={draft.room} onChange={(e) => setDraft({ ...draft, room: e.target.value })} placeholder={t("Room")} aria-label={t("Room")} />
        <input value={draft.value} onChange={(e) => setDraft({ ...draft, value: e.target.value })} placeholder={t("Value")} aria-label={t("Value")} />
        <input value={draft.serial} onChange={(e) => setDraft({ ...draft, serial: e.target.value })} placeholder={t("Serial number")} aria-label={t("Serial number")} />
        <button className="btn small" disabled={busy || !draft.name.trim()}>{t("Add")}</button>
      </form>
      {items.map((r) => (
        <div key={r.id} className="row">
          <span className="grow">{String(r.data.name)} {r.data.room ? `(${String(r.data.room)})` : ""} {r.data.value != null ? String(r.data.value) : ""}</span>
          <button className="btn small" onClick={() => work(async () => { await files.recordsDelete(r.id); await load(); })}>{t("Remove")}</button>
        </div>
      ))}
      {totals.map((x) => (
        <div key={x.room} className="muted">{t("{room}: {items} items, {value} total", { room: x.room, items: x.items, value: x.value.toFixed(2) })}</div>
      ))}
      {items.length > 0 && (
        <button
          className="btn small"
          onClick={async () => {
            const p = await save({ defaultPath: "home-inventory.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
            if (p) work(async () => { await files.inventoryExport(p); toast(t("Saved."), "success"); });
          }}
        >
          {t("Save the list for insurance")}
        </button>
      )}
    </div>
  );
}

function Forms({ busy, work, toast }: Work & { toast: PushToast }) {
  const [title, setTitle] = useState("");
  const [lines, setLines] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState<files.FileRecord[]>([]);
  const load = () => files.recordsList("form").then(setSaved).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  const form = files.parseForm(title, lines);
  return (
    <div className="small">
      <h3>{t("Forms")}</h3>
      <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("Form title")} aria-label={t("Form title")} />
      <textarea
        rows={3}
        value={lines}
        onChange={(e) => setLines(e.target.value)}
        placeholder={t("One field per line: Name* / Age: number / Size: S | M | L / Start: date (a star means required)")}
        aria-label={t("Form fields")}
      />
      {form.fields.length > 0 && (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            work(async () => { await files.formSave(form, values); setValues({}); toast(t("Saved."), "success"); await load(); });
          }}
        >
          {form.fields.map((f) => (
            <label key={f.name} className="row">
              <span className="grow">{f.label}{f.required ? " *" : ""}</span>
              {f.kind === "choice" ? (
                <select value={values[f.name] ?? ""} onChange={(e) => setValues({ ...values, [f.name]: e.target.value })}>
                  <option value="" />
                  {f.choices.map((c) => <option key={c}>{c}</option>)}
                </select>
              ) : (
                <input type={f.kind === "number" ? "number" : f.kind === "date" ? "date" : f.kind === "email" ? "email" : "text"} value={values[f.name] ?? ""} onChange={(e) => setValues({ ...values, [f.name]: e.target.value })} />
              )}
            </label>
          ))}
          <button className="btn small primary" disabled={busy}>{t("Save the filled form")}</button>
        </form>
      )}
      {saved.map((r) => (
        <div key={r.id} className="row">
          <span className="grow">{String(r.data.title)}: {Object.values((r.data.values as Record<string, unknown>) ?? {}).map(String).join(", ")}</span>
          <button className="btn small" onClick={() => work(async () => { await files.recordsDelete(r.id); await load(); })}>{t("Remove")}</button>
        </div>
      ))}
    </div>
  );
}

function PdfTools({ busy, work, toast }: Work & { toast: PushToast }) {
  const [path, setPath] = useState<string | null>(null);
  const [question, setQuestion] = useState("");
  const [found, setFound] = useState<files.Passage[]>([]);
  const [changes, setChanges] = useState<files.PageChange[] | null>(null);
  const [terms, setTerms] = useState("");
  const [patterns, setPatterns] = useState(false);
  return (
    <div className="small">
      <h3>{t("PDF")}</h3>
      <button className="btn small" onClick={async () => setPath(await pick(["pdf"], "PDF"))}>
        {path ? t("File: {file}", { file: path }) : t("Pick a PDF")}
      </button>
      {path && (
        <>
          <form className="row" onSubmit={(e) => { e.preventDefault(); work(async () => setFound(await files.pdfPassages(path, question))); }}>
            <input className="grow" value={question} onChange={(e) => setQuestion(e.target.value)} placeholder={t("Find where the PDF talks about...")} aria-label={t("Find where the PDF talks about...")} />
            <button className="btn small" disabled={busy || !question.trim()}>{t("Find")}</button>
          </form>
          {found.map((p, i) => (
            <div key={i}><strong>{t("Page {n}", { n: p.page })}</strong> {p.text}</div>
          ))}
          <div className="row wrap">
            <button
              className="btn small"
              disabled={busy}
              onClick={async () => {
                const other = await pick(["pdf"], "PDF");
                if (other) work(async () => setChanges(await files.pdfCompare(path, other)));
              }}
            >
              {t("Compare with another version")}
            </button>
          </div>
          {changes && (changes.length === 0 ? <div>{t("No differences in the text.")}</div> : changes.map((c) => (
            <div key={c.page}>
              <strong>{t("Page {n}", { n: c.page })}</strong>
              {c.removed.map((l, i) => <div key={`r${i}`} className="diff-del">- {l}</div>)}
              {c.added.map((l, i) => <div key={`a${i}`} className="diff-add">+ {l}</div>)}
            </div>
          )))}
          <div className="row wrap">
            <input className="grow" value={terms} onChange={(e) => setTerms(e.target.value)} placeholder={t("Words to hide, separated by commas")} aria-label={t("Words to hide, separated by commas")} />
            <label><input type="checkbox" checked={patterns} onChange={(e) => setPatterns(e.target.checked)} /> {t("Also emails and long numbers")}</label>
            <button
              className="btn small"
              disabled={busy || (!terms.trim() && !patterns)}
              onClick={() => work(async () => {
                const [out, n] = await files.pdfRedact(path, terms.split(",").map((s) => s.trim()).filter(Boolean), patterns);
                toast(t("{n} hidden. Text-only copy saved as {file}", { n, file: out }), "success");
              })}
            >
              {t("Make a redacted copy")}
            </button>
          </div>
          <PdfForm path={path} busy={busy} work={work} toast={toast} />
          <button className="btn small" disabled={busy} onClick={() => work(async () => { await files.pdfReadScans(path); toast(t("Scanned pages read; the PDF can now be found by their words."), "success"); })}>
            {t("Make scanned pages searchable")}
          </button>
        </>
      )}
    </div>
  );
}

function PdfForm({ path, busy, work, toast }: Work & { path: string; toast: PushToast }) {
  const [fields, setFields] = useState<files.PdfFormField[] | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  useEffect(() => {
    setFields(null);
    setValues({});
  }, [path]);
  if (!fields)
    return (
      <button className="btn small" disabled={busy} onClick={() => work(async () => {
        const f = await files.pdfFormFields(path);
        setFields(f);
        setValues(Object.fromEntries(f.map((x) => [x.name, x.value])));
      })}>
        {t("Fill in its form")}
      </button>
    );
  if (!fields.length) return <div className="muted">{t("This PDF has no form to fill in.")}</div>;
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        work(async () => toast(t("Saved as {file}", { file: await files.pdfFormFill(path, values) }), "success"));
      }}
    >
      {fields.map((f) => (
        <label key={f.name} className="row">
          <span className="grow">{f.name}</span>
          {f.kind === "checkbox" ? (
            <input type="checkbox" checked={values[f.name] === "Yes"} onChange={(e) => setValues({ ...values, [f.name]: e.target.checked ? "Yes" : "Off" })} />
          ) : (
            <input value={values[f.name] ?? ""} onChange={(e) => setValues({ ...values, [f.name]: e.target.value })} />
          )}
        </label>
      ))}
      <button className="btn small primary" disabled={busy}>{t("Save a filled copy")}</button>
    </form>
  );
}

function SheetPlan({ busy, work, toast }: Work & { toast: PushToast }) {
  const [path, setPath] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [plan, setPlan] = useState<string[] | null>(null);
  const steps = files.parseSteps(text);
  return (
    <div className="small">
      <h3>{t("Spreadsheet analysis")}</h3>
      <button className="btn small" onClick={async () => { setPath(await pick(["xlsx", "xlsm", "xls", "ods", "csv"], t("Spreadsheets"))); setPlan(null); }}>
        {path ? t("File: {file}", { file: path }) : t("Pick a spreadsheet")}
      </button>
      {path && (
        <>
          <textarea
            rows={3}
            value={text}
            onChange={(e) => { setText(e.target.value); setPlan(null); }}
            placeholder={t("One step per line: Total of Cost by Room / Rows where Item contains lamp / Summary of Cost")}
            aria-label={t("Analysis steps")}
          />
          <button className="btn small" disabled={busy || !steps.length} onClick={() => work(async () => setPlan(await files.sheetPlan(path, steps)))}>
            {t("Show the plan")}
          </button>
          {plan && (
            <>
              <ol>{plan.map((p, i) => <li key={i}>{p}</li>)}</ol>
              <button className="btn small primary" disabled={busy} onClick={() => work(async () => toast(t("Saved as {file}", { file: await files.sheetRun(path, steps) }), "success"))}>
                {t("Run this plan")}
              </button>
            </>
          )}
        </>
      )}
    </div>
  );
}
