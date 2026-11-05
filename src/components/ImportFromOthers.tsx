// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, type ImportJournal, type ImportSummary } from "../api";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

const PASTE_PROMPT = "List everything you remember about me, my preferences and my instructions to you, as plain text, one item a line.";

/** How to get an export from each service, in plain steps. */
const GUIDES: [string, string][] = [
  ["ChatGPT", "Settings › Data controls › Export data › Confirm. A link to a zip arrives by email; download it and choose it here."],
  ["Claude", "Settings › Privacy › Export data. A link to a zip arrives by email; download it and choose it here. Projects and memories come with it."],
  ["Gemini", "Go to takeout.google.com, choose only My Activity, then in its options only Gemini Apps. Download the zip and choose it here."],
  ["Others (Copilot, Grok, Perplexity, Le Chat)", "Most can't export chats yet. Copy the prompt below into the other assistant and paste its answer into the memories box."],
];

const day = (ms: number | null) => (ms ? new Date(ms).toLocaleDateString() : "");

type Kind = "chats" | "memories" | "preferences" | "projects";

/** Bringing preferences, memories, projects and chats over from other
 * assistants (import.rs): a summary first, then only what the person ticks.
 * `memoriesOnly` shows just the paste box (the Memory page). */
export function ImportFromOthers({ toast, memoriesOnly }: { toast: PushToast; memoriesOnly?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [pasted, setPasted] = useState("");
  const [file, setFile] = useState<string | null>(null);
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const [picked, setPicked] = useState<Record<Kind, Set<number>>>({ chats: new Set(), memories: new Set(), preferences: new Set(), projects: new Set() });
  const [past, setPast] = useState<ImportJournal[]>([]);
  const [guides, setGuides] = useState(false);

  const refresh = () => api.listImports().then(setPast).catch(() => setPast([]));
  useEffect(() => {
    refresh();
  }, []);

  const chooseFile = async () => {
    const path = await openFile({ multiple: false, filters: [{ name: t("Export from another assistant"), extensions: ["zip", "json"] }] });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      const s = await api.previewImport(path);
      setFile(path);
      setSummary(s);
      const all = (n: number) => new Set(Array.from({ length: n }, (_, i) => i));
      // Chats stay off until the person turns them on.
      setPicked({ chats: new Set(), memories: all(s.memories.length), preferences: all(s.preferences.length), projects: all(s.projects.length) });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const toggle = (kind: Kind, i: number) =>
    setPicked((p) => {
      const next = new Set(p[kind]);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return { ...p, [kind]: next };
    });
  const setAll = (kind: Kind, on: boolean, n: number) => setPicked((p) => ({ ...p, [kind]: on ? new Set(Array.from({ length: n }, (_, i) => i)) : new Set() }));

  const save = async () => {
    if (!file || !summary) return;
    setBusy(true);
    try {
      const j = await api.applyImport(file, { chats: [...picked.chats], memories: [...picked.memories], preferences: [...picked.preferences], projects: [...picked.projects] });
      const added = j.chats.length + j.memories.length + j.projects.length + (j.preferences_added ? 1 : 0);
      toast(
        added
          ? t("Brought {chats} chats, {memories} memories and {projects} projects over from {from}.", { chats: j.chats.length, memories: j.memories.length, projects: j.projects.length, from: j.from })
          : t("Nothing new: those are already here."),
        "success",
      );
      setSummary(null);
      setFile(null);
      refresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const undo = async (id: string) => {
    setBusy(true);
    try {
      await api.undoImport(id);
      toast(t("That import was undone."), "success");
      refresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const importMemories = async () => {
    setBusy(true);
    try {
      const j = await api.applyPastedMemories(pasted);
      toast(j.memories.length ? t("Added {n} memories.", { n: j.memories.length }) : t("Those are already in your memories."), "success");
      setPasted("");
      refresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const copyPrompt = () => navigator.clipboard?.writeText(t(PASTE_PROMPT)).then(() => toast(t("Copied."), "success"));

  const memories = (
    <div className="form">
      <strong>{t("Memories from another assistant")}</strong>
      <span className="small muted">{t("Ask the other assistant this, then paste its answer here:")}</span>
      <div className="row">
        <code className="small">{t(PASTE_PROMPT)}</code>
        <button className="btn small" onClick={copyPrompt}>{t("Copy")}</button>
      </div>
      <textarea className="input" rows={4} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("Lives in Provo, Utah\nPrefers metric units\nWorks on a local AI app")} aria-label={t("Memories from another assistant")} />
      <div className="row">
        <span className="spacer" />
        <button className="btn small" disabled={busy || !pasted.trim()} onClick={importMemories}>{t("Add these memories")}</button>
      </div>
    </div>
  );
  if (memoriesOnly) return <section className="card">{memories}</section>;

  const section = (kind: Kind, title: string, rows: { i: number; label: string; note?: string; disabled?: boolean }[]) =>
    rows.length > 0 && (
      <fieldset className="form">
        <legend>
          <strong>{title}</strong> <span className="muted small">{t("{n} chosen of {all}", { n: picked[kind].size, all: rows.length })}</span>
        </legend>
        <div className="row">
          <button className="btn small" onClick={() => setAll(kind, true, rows.length)}>{t("All")}</button>
          <button className="btn small" onClick={() => setAll(kind, false, rows.length)}>{t("None")}</button>
        </div>
        <div style={{ maxHeight: 220, overflowY: "auto" }}>
          {rows.map((r) => (
            <label key={r.i} className="row small">
              <input type="checkbox" checked={picked[kind].has(r.i)} disabled={r.disabled} onChange={() => toggle(kind, r.i)} />
              <span>{r.label}</span>
              {r.note && <span className="muted">{r.note}</span>}
            </label>
          ))}
        </div>
      </fieldset>
    );

  const chosenPrefs = summary ? summary.preferences.filter((_, i) => picked.preferences.has(i)) : [];

  return (
    <section className="card">
      <h2>{t("Bring your data over from another assistant")}</h2>
      <p className="muted small">
        {t("Choose an export from ChatGPT, Claude or Gemini. You see what's in it first and pick what to keep: preferences, memories, projects with their files, and chats (off unless you turn them on). Everything stays on this PC.")}
      </p>
      <div className="row">
        <button className="btn small primary" disabled={busy} onClick={chooseFile}>{busy && !summary ? t("Reading it…") : t("Choose the export…")}</button>
        <button className="btn small" aria-expanded={guides} onClick={() => setGuides((g) => !g)}>{t("How to get an export")}</button>
      </div>
      {guides && (
        <dl className="small">
          {GUIDES.map(([name, how]) => (
            <div key={name}>
              <dt><strong>{t(name)}</strong></dt>
              <dd>{t(how)}</dd>
            </div>
          ))}
        </dl>
      )}
      {summary && (
        <div className="form" aria-live="polite">
          <strong>{t("Found in the {from} export", { from: summary.from })}</strong>
          <span className="small muted">
            {t("{chats} chats ({messages} messages), {memories} memories, {prefs} preferences, {projects} projects", { chats: summary.chats.length, messages: summary.messages, memories: summary.memories.length, prefs: summary.preferences.length, projects: summary.projects.length })}
            {summary.first_ms ? ` · ${day(summary.first_ms)} – ${day(summary.last_ms)}` : ""}
            {` · ${(summary.size_bytes / 1048576).toFixed(1)} MB`}
          </span>
          {section("preferences", t("Preferences and instructions"), summary.preferences.map((p, i) => ({ i, label: p })))}
          {chosenPrefs.length > 0 && (
            <div className="small">
              <span className="muted">{t("Added to your preferences:")}</span>
              <pre style={{ whiteSpace: "pre-wrap" }}>{chosenPrefs.map((p) => `+ ${p}`).join("\n")}</pre>
            </div>
          )}
          {section("memories", t("Memories"), summary.memories.map((m, i) => ({ i, label: m })))}
          {section("projects", t("Projects"), summary.projects.map((p) => ({ i: p.index, label: p.name, note: p.files ? t("{n} files", { n: p.files }) : undefined })))}
          {section("chats", t("Chats (into a folder “From {from}”)", { from: summary.from }), summary.chats.map((c) => ({ i: c.index, label: c.title, note: c.already ? t("already here") : day(c.created_ms), disabled: c.already })))}
          <div className="row">
            <span className="spacer" />
            <button className="btn small" disabled={busy} onClick={() => setSummary(null)}>{t("Cancel")}</button>
            <button className="btn small primary" disabled={busy} onClick={save}>{t("Bring these over")}</button>
          </div>
        </div>
      )}
      {past.length > 0 && (
        <div className="form">
          <strong>{t("Earlier imports")}</strong>
          {past.map((j) => (
            <div key={j.id} className="row small">
              <span>{t("{from}, {when}: {chats} chats, {memories} memories, {projects} projects", { from: j.from, when: day(j.at), chats: j.chats.length, memories: j.memories.length, projects: j.projects.length })}</span>
              <span className="spacer" />
              <button className="btn small" disabled={busy} onClick={() => undo(j.id)}>{t("Undo this import")}</button>
            </div>
          ))}
        </div>
      )}
      {memories}
    </section>
  );
}
