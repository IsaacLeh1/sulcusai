// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type Chat, type FinetuneJob, type FinetuneView } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

type SourceKind = "chats" | "file";

/** Advanced → Model work: teach a model from your own examples (LoRA). */
export function TeachModel({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<FinetuneView | null>(null);
  const [job, setJob] = useState<FinetuneJob | null>(null);
  const [chats, setChats] = useState<Chat[]>([]);
  const [form, setForm] = useState<{ open: boolean; name: string; base: string; kind: SourceKind; chatIds: string[]; file: string; epochs: number; strength: string }>({
    open: false,
    name: "",
    base: "",
    kind: "chats",
    chatIds: [],
    file: "",
    epochs: 3,
    strength: "normal",
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
      if (j.status === "done" || j.status === "failed" || j.status === "cancelled") load();
    });
    return () => {
      sub.then((un) => un());
    };
  }, [load]);
  if (!v) return null;

  const running = job && !["done", "failed", "cancelled"].includes(job.status);
  const supported = v.bases.filter((b) => b.supported);
  const start = async () => {
    try {
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
              {job.status === "done" && t("done. Pick it in a chat's model menu.")}
              {job.status === "cancelled" && t("stopped.")}
              {job.status === "failed" && <span className="warn-text">{t("didn't finish: {error}", { error: job.error ?? "" })}</span>}
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
        </div>
      )}

      {v.adapters.length > 0 && (
        <ul className="teach-list small">
          {v.adapters.map((a) => (
            <li key={a.id}>
              <span>
                <strong>{a.base_name} + {a.name}</strong> <span className="muted">· {t("from {source}, {examples} examples, {rounds} rounds", { source: a.source, examples: a.examples, rounds: a.epochs })}</span>
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
        <button className="btn" disabled={!!running || supported.length === 0} onClick={() => setForm({ ...form, open: true })} title={supported.length === 0 ? t("Install a Qwen3, Gemma 3 or Llama model first") : undefined}>
          {t("Teach a model…")}
        </button>
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
                <option key={b.id} value={b.id} disabled={!b.supported}>
                  {b.name}
                  {b.supported ? "" : " " + t("(can't be taught yet)")}
                </option>
              ))}
            </select>
          </label>
          <span className="small">{t("Learn from")}</span>
          <label className="check">
            <input type="radio" name="teach-src" checked={form.kind === "chats"} onChange={() => setForm({ ...form, kind: "chats" })} />
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
            <input type="radio" name="teach-src" checked={form.kind === "file"} onChange={() => setForm({ ...form, kind: "file" })} />
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
          <p className="muted small">
            {t("A .jsonl has one conversation per line, like")} <code>{'{"messages": [{"role": "user", "content": "…"}, {"role": "assistant", "content": "…"}]}'}</code>. {t("At least 10 examples; 50–500 work best.")}
          </p>
          <div className="adv-grid">
            <label className="adv-field">
              <span>{t("Rounds through the examples")}</span>
              <input className="input" type="number" min={1} max={10} value={form.epochs} onChange={(e) => setForm({ ...form, epochs: Number(e.target.value) || 3 })} />
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
            <button className="btn primary" onClick={start} disabled={!form.name.trim() || !form.base || (form.kind === "chats" ? form.chatIds.length === 0 : !form.file)}>
              {t("Start teaching")}
            </button>
            <button className="btn ghost" onClick={() => setForm({ ...form, open: false })}>{t("Cancel")}</button>
          </div>
        </div>
      )}
    </div>
  );
}
