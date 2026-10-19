// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type Chat, type FinetuneJob, type FinetuneView, type PracticeSkill } from "../api";
import { bytes } from "../format";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

type SourceKind = "chats" | "file" | "practice";

const SKILLS: { id: PracticeSkill; label: string }[] = [
  { id: "code", label: tx("Coding (JavaScript functions, checked by running them; gets the biggest share)") },
  { id: "writing", label: tx("Writing (summaries that keep the facts, proofreading, emails)") },
  { id: "research", label: tx("Research (answering from sources, citing them, saying when they don't know)") },
  { id: "math", label: tx("Word problems") },
  { id: "careful", label: tx("Careful counting and spelling") },
  { id: "time", label: tx("Dates and times") },
  { id: "format", label: tx("Following instructions (bullets, word limits, JSON)") },
  { id: "tools", label: tx("Using tools") },
  { id: "project", label: tx("Your code (which file defines a function, what it takes and what calls it, looked up in folders you pick)") },
];

/** Skills worked with tools: a model that can't use tools can't practise them. */
const needsTools = (s: PracticeSkill) => s === "tools" || s === "project";

/** Advanced → Model work: teach a model from your own examples (LoRA). */
export function TeachModel({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<FinetuneView | null>(null);
  const [job, setJob] = useState<FinetuneJob | null>(null);
  const [chats, setChats] = useState<Chat[]>([]);
  const [form, setForm] = useState<{
    open: boolean;
    name: string;
    base: string;
    kind: SourceKind;
    chatIds: string[];
    file: string;
    epochs: number;
    strength: string;
    skills: PracticeSkill[];
    teacher: string;
    /** Folders with the person's code, for practice on it. */
    folders: string[];
  }>({
    open: false,
    name: "",
    base: "",
    kind: "chats",
    chatIds: [],
    file: "",
    epochs: 3,
    strength: "normal",
    skills: ["code", "writing", "research", "tools"],
    teacher: "",
    folders: [],
  });

  const load = useCallback(() => {
    api.finetuneView().then((x) => {
      setV(x);
      setJob(x.job);
      setForm((f) => ({ ...f, base: f.base || x.bases.find((b) => b.supported)?.id || "" }));
    });
  }, []);
  useEffect(() => {
    load();
    api.chats().then(setChats).catch(() => {});
    const sub = on("finetune:progress", (j) => {
      setJob(j);
      if (j.status === "done" || j.status === "failed" || j.status === "cancelled" || j.status === "not_better") load();
    });
    return () => {
      sub.then((un) => un());
    };
  }, [load]);
  if (!v) return null;

  const running = job && !["done", "failed", "cancelled", "not_better"].includes(job.status);
  const supported = v.bases.filter((b) => b.supported);
  const practising = form.kind === "practice";
  const base = v.bases.find((b) => b.id === form.base);
  // Documents teach any supported model; chats and .jsonl need a chat format the trainer reads.
  const documents = form.kind === "file" && !!form.file && !form.file.toLowerCase().endsWith(".jsonl");
  const usable = (b: { supported: boolean; conversations: boolean; practice: boolean }) => (practising ? b.practice : documents ? b.supported : b.conversations);
  const skills = form.skills.filter((s) => !needsTools(s) || base?.tools);
  const onCode = skills.includes("project");
  const addFolders = async () => {
    const p = await openFile({ directory: true, multiple: true, title: t("Pick a folder with your code") });
    const picked = Array.isArray(p) ? p : typeof p === "string" ? [p] : [];
    if (picked.length) setForm((f) => ({ ...f, folders: [...new Set([...f.folders, ...picked])] }));
  };
  /** Any other model on this PC can teach the one practising. */
  const teachers = v.bases.filter((b) => b.id !== form.base);
  const teacher = teachers.find((b) => b.id === form.teacher);
  /** Opens the form, practising or learning from examples. */
  const openForm = (kind: SourceKind) => {
    const fits = (b: { supported: boolean; conversations: boolean; practice: boolean }) => (kind === "practice" ? b.practice : kind === "chats" ? b.conversations : b.supported);
    const keep = v.bases.find((b) => b.id === form.base && fits(b));
    setForm({
      ...form,
      open: true,
      kind,
      base: keep?.id || v.bases.find(fits)?.id || form.base,
      name: kind === "practice" && !form.name ? t("Practised") : form.name,
      epochs: kind === "practice" ? 2 : form.kind === "practice" ? 3 : form.epochs,
    });
  };
  const start = async () => {
    try {
      if (practising) {
        await api.startPractice(form.name, form.base, skills, form.epochs, form.strength, teacher?.id ?? null, onCode ? form.folders : null);
        setForm({ ...form, open: false });
        toast(t("Practice has started. You can keep using the app; chats with this PC's models wait until it's done."), "success");
        return;
      }
      const source = form.kind === "chats" ? { kind: "chats" as const, chat_ids: form.chatIds } : { kind: "file" as const, path: form.file };
      await api.startFinetune(form.name, form.base, source, form.epochs, form.strength);
      setForm({ ...form, open: false });
      toast(t("Teaching has started. You can keep using the app; chats with this PC's models wait until it's done."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="teach">
      <h3>{t("Teach a model")}</h3>
      <p className="muted small">
        {t("Teach a model on this PC from your own examples, so it picks up your style, terms or answers. It runs on the graphics card and can take from several minutes to a few hours. The result is a small add-on that appears in the model menu as “model + name”; the original model is unchanged.")}
      </p>

      {job && (
        <div className="teach-job">
          <div className="adv-head">
            <span className="small">
              <strong>{job.base_name} + {job.name}</strong> ·{" "}
              {job.status === "preparing" && t("getting ready…")}
              {job.status === "downloading" && t("downloading the trainer ({size}, first time only)…", { size: bytes(v.trainer_size) })}
              {job.status === "training" && `${t("round {n} of {total}", { n: job.epoch, total: job.epochs })}${job.eta ? ` · ${t("about {time} left", { time: job.eta })}` : ""}${job.loss !== null ? ` · ${t("loss {loss}", { loss: job.loss.toFixed(2) })}` : ""}`}
              {job.status === "testing" && t("testing it first: {n} of {total}", { n: job.done, total: job.total })}
              {job.status === "practising" && t("practising: {n} of {total} tasks, {kept} right answers kept", { n: job.done, total: job.total, kept: job.kept })}
              {job.status === "teacher" && t("{teacher} is answering the tasks it got wrong: {n} of {total}, {kept} right answers kept", { teacher: job.teacher ?? "", n: job.done, total: job.total, kept: job.kept })}
              {job.status === "checking" && t("testing what it learnt: {n} of {total}", { n: job.done, total: job.total })}
              {job.status === "done" &&
                (job.before !== null && job.after !== null
                  ? t("done: {before} → {after} of {total} test tasks right. Pick it in a chat's model menu.", { before: job.before, after: job.after, total: job.total })
                  : t("done. Pick it in a chat's model menu."))}
              {job.status === "not_better" &&
                (job.after === null
                  ? t("it already gets {before} of {total} test tasks right, so practice has nothing to add. Nothing was changed.", { before: job.before ?? 0, total: job.total })
                  : t("it didn't get better ({before} → {after} of {total} test tasks right), so nothing was kept.", { before: job.before ?? 0, after: job.after, total: job.total }))}
              {job.status === "cancelled" && t("stopped.")}
              {job.status === "failed" && <span className="warn-text">{t("didn't finish: {error}", { error: errorText(job.error ?? "") })}</span>}
            </span>
            {running && (
              <button className="btn ghost small danger" onClick={() => api.cancelFinetune()}>
                {t("Stop")}
              </button>
            )}
          </div>
          {running && (
            <div className="progress">
              <span style={{ width: `${Math.round(job.fraction * 100)}%` }} />
            </div>
          )}
          {running && job.low_memory && (
            <p className="small warn-text">
              {t("The graphics card is short of free memory for training. Closing other apps that use it, such as games, browsers and video players, helps.")}
            </p>
          )}
        </div>
      )}

      {v.adapters.length > 0 && (
        <ul className="teach-list small">
          {v.adapters.map((a) => (
            <li key={a.id}>
              <span>
                <strong>{a.base_name} + {a.name}</strong>{" "}
                <span className="muted">
                  ·{" "}
                  {a.scores && a.teacher
                    ? t("practised with {teacher} teaching, test {before} → {after} of {total} right", { teacher: a.teacher, before: a.scores.before, after: a.scores.after, total: a.scores.out_of })
                    : a.scores
                      ? t("practised, test {before} → {after} of {total} right", { before: a.scores.before, after: a.scores.after, total: a.scores.out_of })
                      : t("from {source}, {examples} examples, {rounds} rounds", { source: a.source, examples: a.examples, rounds: a.epochs })}
                </span>
              </span>
              <button
                className="btn ghost small"
                onClick={async () => {
                  try {
                    await api.removeAdapter(a.id);
                    load();
                  } catch (e) {
                    toast(errorText(e), "error");
                  }
                }}
              >
                {t("Remove")}
              </button>
            </li>
          ))}
        </ul>
      )}

      {!form.open ? (
        <div className="row wrap">
          <button className="btn" disabled={!!running || supported.length === 0} onClick={() => openForm("chats")} title={supported.length === 0 ? t("Install a Qwen3, Gemma 3 or Llama model first") : undefined}>
            {t("Teach a model…")}
          </button>
          <button
            className="btn"
            disabled={!!running || !v.bases.some((b) => b.practice)}
            onClick={() => openForm("practice")}
            title={v.bases.some((b) => b.practice) ? t("It practises tasks whose answers can be checked and learns from its own right answers") : t("Install a Qwen3 or Gemma 3 model first")}
          >
            {t("Make a model better by practice…")}
          </button>
        </div>
      ) : (
        <div className="teach-form form">
          <label>
            {t("Name")}
            <input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder={t("e.g. My writing style")} maxLength={60} />
          </label>
          <label>
            {t("Model to teach")}
            <select className="input" value={form.base} onChange={(e) => setForm({ ...form, base: e.target.value })}>
              {v.bases.map((b) => (
                <option key={b.id} value={b.id} disabled={!usable(b)}>
                  {b.name}
                  {usable(b) ? "" : " " + (practising ? t("(can't practise yet)") : b.supported ? t("(documents only)") : t("(can't be taught yet)"))}
                </option>
              ))}
            </select>
          </label>
          <span className="small">{t("Learn from")}</span>
          <label className="check">
            <input type="radio" name="teach-src" checked={practising} onChange={() => openForm("practice")} />
            <span>{t("Practice: it works through tasks whose answers can be checked, learns from its own right answers, and is kept only if it then does better on a test")}</span>
          </label>
          {practising && (
            <>
              <div className="teach-chats">
                <span className="small">{t("What should it become an expert in?")}</span>
                {SKILLS.map((s) => (
                  <label key={s.id} className="check" title={needsTools(s.id) && !base?.tools ? t("This model can't use tools") : undefined}>
                    <input
                      type="checkbox"
                      disabled={needsTools(s.id) && !base?.tools}
                      checked={form.skills.includes(s.id) && (!needsTools(s.id) || !!base?.tools)}
                      onChange={(e) => setForm({ ...form, skills: e.target.checked ? [...form.skills, s.id] : form.skills.filter((x) => x !== s.id) })}
                    />
                    <span>{t(s.label)}</span>
                  </label>
                ))}
                <p className="muted small">{t("It gets better at the kinds of tasks it practises, not at everything, and it doesn't learn new facts. It usually takes 20–90 minutes on the graphics card.")}</p>
              </div>
              {onCode && (
                <div className="teach-chats">
                  <span className="small">{t("Folders with your code")}</span>
                  {form.folders.map((f) => (
                    <div key={f} className="row">
                      <span className="small ellipsis">{f}</span>
                      <button className="btn ghost small" onClick={() => setForm((x) => ({ ...x, folders: x.folders.filter((y) => y !== f) }))}>
                        {t("Remove")}
                      </button>
                    </div>
                  ))}
                  <div className="row wrap">
                    <button className="btn small" onClick={addFolders}>
                      {t("Add a folder…")}
                    </button>
                    {form.folders.length === 0 && <span className="muted small">{t("None chosen")}</span>}
                  </div>
                  <p className="muted small">
                    {t("It looks things up in your Rust, TypeScript, JavaScript and Python files with the file tools chats use, and its answers are checked against the files. It only reads them, and never opens .env files, keys, lockfiles, or dependency and build folders. It learns how to look, not your code itself.")}
                  </p>
                </div>
              )}
              <label>
                {t("Teacher")}
                <select className="input" value={teacher?.id ?? ""} onChange={(e) => setForm({ ...form, teacher: e.target.value })}>
                  <option value="">{t("None: it learns only from its own right answers")}</option>
                  {teachers.map((b) => (
                    <option key={b.id} value={b.id}>
                      {b.name}
                    </option>
                  ))}
                </select>
              </label>
              <p className="muted small">
                {t("A bigger model on this PC answers the tasks it keeps getting wrong. Its answers are checked the same way, and it learns from the right ones too. Only one model fits on the graphics card at a time, so they take turns, which adds 10–30 minutes.")}
                {teacher && !teacher.tools && skills.includes("tools") ? " " + t("This teacher can't use tools, so it skips the tool tasks.") : ""}
                {teacher && onCode ? " " + t("Tasks about your code stay its own: they take several turns with the file tools, and a teacher answers in one.") : ""}
              </p>
            </>
          )}
          <label className="check">
            <input type="radio" name="teach-src" checked={form.kind === "chats"} onChange={() => openForm("chats")} />
            <span>{t("Your chats: each question and the answer it got")}</span>
          </label>
          {form.kind === "chats" && (
            <div className="teach-chats">
              {chats.filter((c) => !c.incognito).map((c) => (
                <label key={c.id} className="check">
                  <input
                    type="checkbox"
                    checked={form.chatIds.includes(c.id)}
                    onChange={(e) => setForm({ ...form, chatIds: e.target.checked ? [...form.chatIds, c.id] : form.chatIds.filter((x) => x !== c.id) })}
                  />
                  <span>{c.title}</span>
                </label>
              ))}
              {chats.length === 0 && <span className="muted small">{t("No chats yet.")}</span>}
            </div>
          )}
          <label className="check">
            <input type="radio" name="teach-src" checked={form.kind === "file"} onChange={() => openForm("file")} />
            <span>{t("A file: example conversations (.jsonl) or your documents (.txt, .md)")}</span>
          </label>
          {form.kind === "file" && (
            <div className="row wrap">
              <button
                className="btn small"
                onClick={async () => {
                  const p = await openFile({ multiple: false, filters: [{ name: t("Examples or documents"), extensions: ["jsonl", "txt", "md"] }] });
                  if (typeof p === "string") setForm({ ...form, file: p });
                }}
              >
                {t("Choose a file…")}
              </button>
              <span className="muted small ellipsis">{form.file || t("None chosen")}</span>
            </div>
          )}
          {!practising && (
            <p className="muted small">
              {t("A .jsonl has one conversation per line, like")} <code>{'{"messages": [{"role": "user", "content": "…"}, {"role": "assistant", "content": "…"}]}'}</code>. {t("At least 10 examples; 50–500 work best.")}
            </p>
          )}
          <div className="adv-grid">
            <label className="adv-field">
              <span>{t("Rounds through the examples")}</span>
              <input
                className="input"
                type="number"
                min={1}
                max={practising ? 5 : 10}
                value={form.epochs}
                onChange={(e) => setForm({ ...form, epochs: Number(e.target.value) || (practising ? 2 : 3) })}
              />
            </label>
            <label className="adv-field">
              <span>{t("How strongly")}</span>
              <select className="input" value={form.strength} onChange={(e) => setForm({ ...form, strength: e.target.value })}>
                <option value="light">{t("Lightly")}</option>
                <option value="normal">{t("Normal")}</option>
                <option value="strong">{t("Strongly")}</option>
              </select>
            </label>
          </div>
          {!v.trainer_installed && <p className="muted small">{t("The first time, this downloads the trainer ({size}, QVAC Fabric's build of llama.cpp) from GitHub.", { size: bytes(v.trainer_size) })}</p>}
          <p className="muted small">{t("While it trains, chats with this PC's models wait (they need the same graphics memory); cloud models still work. Training data from your chats is deleted as soon as it's done.")}</p>
          <div className="adv-actions">
            <button
              className="btn primary"
              onClick={start}
              disabled={!form.name.trim() || !base || !usable(base) || (practising ? skills.length === 0 || (onCode && form.folders.length === 0) : form.kind === "chats" ? form.chatIds.length === 0 : !form.file)}
            >
              {practising ? t("Start practice") : t("Start teaching")}
            </button>
            <button className="btn ghost" onClick={() => setForm({ ...form, open: false })}>{t("Cancel")}</button>
          </div>
        </div>
      )}
    </div>
  );
}
