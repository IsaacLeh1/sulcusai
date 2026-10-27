// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type CloudChoice, type ModelCard } from "../api";
import { t } from "../i18n";
import { Markdown } from "./Markdown";
import type { PushToast } from "./Toasts";

interface Side {
  model: string;
  content: string;
  thinking: string;
  state: "idle" | "waiting" | "writing" | "done" | "error";
  note: string;
}

const WINS_KEY = "sulcusai.compare.wins";

function wins(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(WINS_KEY) ?? "{}");
  } catch {
    return {};
  }
}

/** The same message to two models, their replies side by side; optionally
 *  without their names until the better one is picked. */
export function CompareModels({ installed, defaultModel, toast }: { installed: ModelCard[]; defaultModel: string | null; toast: PushToast }) {
  const [cloud, setCloud] = useState<CloudChoice[]>([]);
  useEffect(() => {
    api.cloudChoices().then(setCloud).catch(() => {});
  }, []);
  const choices = [...installed.map((m) => ({ id: m.id, name: m.name })), ...cloud.map((c) => ({ id: c.id, name: `${c.name} (${c.provider})` }))];
  const nameOf = (id: string) => choices.find((c) => c.id === id)?.name ?? id;
  const [picked, setPicked] = useState<[string, string]>(() => [defaultModel ?? installed[0]?.id ?? "", installed.find((m) => m.id !== defaultModel)?.id ?? ""]);
  const [prompt, setPrompt] = useState("");
  const [blind, setBlind] = useState(false);
  const [order, setOrder] = useState<[number, number]>([0, 1]);
  const [chosen, setChosen] = useState<number | null>(null);
  const [sides, setSides] = useState<Side[]>([]);
  const [tally, setTally] = useState(wins);
  const run = useRef<string | null>(null);
  const busy = sides.some((s) => s.state === "waiting" || s.state === "writing");

  useEffect(() => {
    const mine = <P extends { run: string }>(f: (p: P) => void) => (p: P) => p.run === run.current && f(p);
    const set = (i: number, f: (s: Side) => Side) => setSides((all) => all.map((s, j) => (j === i ? f(s) : s)));
    const subs = [
      on("compare:start", mine((p) => set(p.index, (s) => ({ ...s, state: "waiting", note: t("Loading the model…") })))),
      on("compare:delta", mine((p) => set(p.index, (s) => ({ ...s, state: "writing", note: "", content: s.content + (p.content ?? ""), thinking: s.thinking + (p.thinking ?? "") })))),
      on("compare:done", mine((p) =>
        set(p.index, (s) =>
          p.error
            ? { ...s, state: "error", note: p.error }
            : {
                ...s,
                state: "done",
                note: [p.loading && p.loading >= 1 ? t("loaded in {n} s", { n: p.loading.toFixed(1) }) : t("already loaded"), t("answered in {n} s", { n: (p.answering ?? 0).toFixed(1) }), p.tps ? t("{n} tokens a second", { n: Math.round(p.tps) }) : ""].filter(Boolean).join(" · "),
              },
        ),
      )),
    ];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []);

  const start = async () => {
    const id = `cmp-${Date.now()}`;
    run.current = id;
    setChosen(null);
    // Blind: which one shows on the left is chosen at random.
    setOrder(blind && Math.random() < 0.5 ? [1, 0] : [0, 1]);
    setSides(picked.map((model) => ({ model, content: "", thinking: "", state: "waiting", note: t("Waiting its turn…") })));
    try {
      await api.compareModels(id, [...picked], prompt.trim());
    } catch (e) {
      toast(errorText(e), "error");
      setSides([]);
    }
  };
  const pick = (i: number) => {
    setChosen(i);
    const all = wins();
    all[sides[i].model] = (all[sides[i].model] ?? 0) + 1;
    try {
      localStorage.setItem(WINS_KEY, JSON.stringify(all));
    } catch {
      /* kept for this visit only */
    }
    setTally(all);
  };
  const hidden = blind && chosen === null;

  return (
    <section className="card compare">
      <h2>{t("Compare models")}</h2>
      <p className="muted small">{t("Ask two models the same thing and read their replies side by side. On this PC they answer one after the other; with room on the graphics card the first stays loaded, so asking again is quick.")}</p>
      <div className="row wrap">
        {[0, 1].map((i) => (
          <label key={i} className="adv-field">
            <span>{t("Model {n}", { n: i + 1 })}</span>
            <select className="input" value={picked[i]} onChange={(e) => setPicked((p) => (i === 0 ? [e.target.value, p[1]] : [p[0], e.target.value]))}>
              <option value="" disabled>{t("Pick a model")}</option>
              {choices.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </label>
        ))}
        <label className="check compare-blind">
          <input type="checkbox" checked={blind} onChange={(e) => setBlind(e.target.checked)} />
          <span>{t("Hide which is which until I pick the better reply")}</span>
        </label>
      </div>
      <textarea className="input" rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder={t("What should both of them answer?")} aria-label={t("What should both of them answer?")} />
      <div className="row">
        {busy ? (
          <button className="btn" onClick={() => run.current && api.compareStop(run.current)}>{t("Stop")}</button>
        ) : (
          <button className="btn primary" disabled={!prompt.trim() || !picked[0] || !picked[1] || picked[0] === picked[1]} onClick={start}>
            {t("Compare")}
          </button>
        )}
        {picked[0] && picked[0] === picked[1] && <span className="muted small">{t("Pick two different models.")}</span>}
      </div>
      {sides.length === 2 && (
        <div className="compare-sides">
          {order.map((i, place) => {
            const s = sides[i];
            return (
              <div key={i} className={`compare-side ${chosen === i ? "chosen" : ""}`}>
                <div className="compare-head">
                  <strong>{hidden ? t("Reply {n}", { n: place + 1 }) : nameOf(s.model)}</strong>
                  <span className="muted small">{s.note}</span>
                </div>
                {s.thinking && !s.content && <p className="muted small">{t("Thinking…")}</p>}
                {s.content ? <Markdown text={s.content} /> : s.state === "error" ? null : <span className="typing" aria-label={t("Writing")} />}
                {blind && chosen === null && !busy && sides.every((x) => x.state === "done") && (
                  <button className="btn small" onClick={() => pick(i)}>{t("This one is better")}</button>
                )}
              </div>
            );
          })}
        </div>
      )}
      {Object.keys(tally).length > 0 && (
        <p className="muted small">
          {t("Picked as better so far:")}{" "}
          {Object.entries(tally)
            .sort((a, b) => b[1] - a[1])
            .map(([id, n]) => `${nameOf(id)} ${n}`)
            .join(" · ")}
        </p>
      )}
    </section>
  );
}
