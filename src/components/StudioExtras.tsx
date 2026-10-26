// SPDX-License-Identifier: AGPL-3.0-only
// The studio's helpers for new pictures: example pictures to follow, the
// prompt writer and picture checker (and teaching the writer a model), and
// the wallpaper maker.
import { useEffect, useState } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type MediaItem, type MediaModelCard, type MediaRequest, type PromptStatus, type Screen } from "../api";
import { t, tx } from "../i18n";
import { useMediaUrl, waitLabel } from "../media";
import { MediaTile } from "./MediaBits";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

export type ExampleUse = NonNullable<MediaRequest["example_use"]>;

function remembered(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    return v == null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

function remember(key: string, on: boolean) {
  try {
    localStorage.setItem(key, on ? "1" : "0");
  } catch {
    /* not kept */
  }
}

function ExampleChip({ item, onRemove }: { item: MediaItem; onRemove: () => void }) {
  const thumb = useMediaUrl(item, true);
  return (
    <span className="chip active row">
      {thumb && <img src={thumb} alt="" className="chip-thumb" />}
      <button className="link" aria-label={t("Remove")} onClick={onRemove}>✕</button>
    </span>
  );
}

/** Up to four example pictures, from the gallery or a file, and what to take from them. */
export function ExamplePicker({
  value,
  onChange,
  use,
  onUse,
  gallery,
  toast,
  canRead,
}: {
  value: MediaItem[];
  onChange: (v: MediaItem[]) => void;
  use: ExampleUse;
  onUse: (u: ExampleUse) => void;
  gallery: MediaItem[];
  toast: PushToast;
  /** Something here reads pictures (else only the layout can be used). */
  canRead: boolean;
}) {
  const [picking, setPicking] = useState(false);
  const toggle = (i: MediaItem) => {
    if (value.some((x) => x.id === i.id)) onChange(value.filter((x) => x.id !== i.id));
    else if (value.length < 4) onChange([...value, i]);
  };
  const fromFile = async () => {
    const picked = await openFile({ multiple: true, filters: [{ name: t("Pictures"), extensions: ["png", "jpg", "jpeg", "webp"] }] });
    const paths = typeof picked === "string" ? [picked] : picked ?? [];
    const added: MediaItem[] = [];
    for (const p of paths.slice(0, 4 - value.length)) {
      try {
        added.push(await api.mediaImport(p));
      } catch (e) {
        toast(errorText(e), "error");
      }
    }
    if (added.length) onChange([...value, ...added].slice(0, 4));
    setPicking(false);
  };
  return (
    <div className="row wrap">
      {value.map((i) => (
        <ExampleChip key={i.id} item={i} onRemove={() => onChange(value.filter((x) => x.id !== i.id))} />
      ))}
      {value.length < 4 && (
        <button className="btn small" onClick={() => setPicking(true)} title={t("Pictures the new one should follow: a style, a subject or a layout")}>
          {value.length ? t("＋ Another example") : t("＋ Example pictures…")}
        </button>
      )}
      {value.length > 0 && (
        <select value={use} onChange={(e) => onUse(e.target.value as ExampleUse)} title={t("What to take from the examples")}>
          <option value="style">{t("Follow their style")}</option>
          <option value="subject">{t("The same character or subject")}</option>
          <option value="layout">{t("Keep their layout and colors")}</option>
        </select>
      )}
      {value.length > 0 && !canRead && use !== "layout" && (
        <span className="small warn-text">{t("Nothing here can look at pictures yet, so only the layout is used. Add a model that sees pictures, such as Qwen3 VL 4B.")}</span>
      )}
      {picking && (
        <Modal title={t("Example pictures")} onClose={() => setPicking(false)}>
          <p className="small muted">{t("Pick up to four. The picture reader looks at them and the new picture follows what you choose.")}</p>
          <div className="row">
            <button className="btn small" onClick={fromFile}>{t("From a file…")}</button>
            <span className="spacer" />
            <span className="small muted">{t("{n} of 4 picked", { n: value.length })}</span>
          </div>
          <div className="media-grid small-grid">
            {gallery
              .filter((i) => i.kind === "image")
              .map((i) => (
                <MediaTile key={i.id} item={i} selected={value.some((x) => x.id === i.id)} onOpen={() => toggle(i)} />
              ))}
          </div>
          {!gallery.some((i) => i.kind === "image") && <p className="muted small">{t("No pictures in the gallery yet.")}</p>}
          <div className="modal-actions">
            <button className="btn primary" onClick={() => setPicking(false)}>{t("Done")}</button>
          </div>
        </Modal>
      )}
    </div>
  );
}

const ASK_KEY = "sulcus.studio.ask";

/** "Ask me when something's unclear" is on (it is unless turned off). */
export function asksFirst(): boolean {
  return remembered(ASK_KEY, true);
}

/** The prompt writer and checker for a model, remembered between visits. */
export function usePromptHelp(modelId: string | undefined): {
  status: PromptStatus | null;
  write: boolean;
  setWrite: (on: boolean) => void;
  check: boolean;
  setCheck: (on: boolean) => void;
  ask: boolean;
  setAsk: (on: boolean) => void;
  reload: () => void;
} {
  const [status, setStatus] = useState<PromptStatus | null>(null);
  const [write, setWriteState] = useState(() => remembered("sulcus.studio.write", true));
  const [check, setCheckState] = useState(() => remembered("sulcus.studio.check", true));
  const [ask, setAskState] = useState(asksFirst);
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!modelId || modelId.startsWith("cloud:")) return setStatus(null);
    let live = true;
    api.mediaPromptStatus(modelId).then((s) => live && setStatus(s)).catch(() => live && setStatus(null));
    return () => {
      live = false;
    };
  }, [modelId, n]);
  // Teaching, coaching or a checked job finished: show what it learned.
  useEffect(() => {
    const again = () => setN((x) => x + 1);
    const subs = [on("media:tuned", again), on("media:coached", again), on("media:done", again)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, []);
  return {
    status,
    write,
    setWrite: (on) => {
      setWriteState(on);
      remember("sulcus.studio.write", on);
    },
    check: check && !!status?.reader,
    setCheck: (on) => {
      setCheckState(on);
      remember("sulcus.studio.check", on);
    },
    ask,
    setAsk: (on) => {
      setAskState(on);
      remember(ASK_KEY, on);
      // Chats follow the same choice.
      api.mediaSetAskQuestions(on);
    },
    reload: () => setN((x) => x + 1),
  };
}

/** Questions before a picture is made or changed: `check` asks the core and
 * says whether to go ahead now (false while questions are showing). */
export function useAskFirst(): {
  asking: boolean;
  questions: string[] | null;
  check: (op: "generate" | "edit", prompt: string, source: string | null, modelId: string | null, examples?: number) => Promise<boolean>;
  clear: () => void;
} {
  const [asking, setAsking] = useState(false);
  const [questions, setQuestions] = useState<string[] | null>(null);
  const check = async (op: "generate" | "edit", prompt: string, source: string | null, modelId: string | null, examples = 0) => {
    setAsking(true);
    // Nothing can ask (or it failed): it's made as it is.
    const found = await api.mediaQuestions(op, prompt, source, modelId, examples).catch(() => [] as string[]);
    setAsking(false);
    if (!found.length) return true;
    setQuestions(found);
    return false;
  };
  return { asking, questions, check, clear: () => setQuestions(null) };
}

/** The questions, each with a field for the answer, above the box. "Make it"
 * folds the answers into the request; "Make it anyway" makes it as written. */
export function PictureQuestions({ questions, prompt, onMake }: { questions: string[]; prompt: string; onMake: (prompt: string) => void }) {
  const [answers, setAnswers] = useState<string[]>(() => questions.map(() => ""));
  const [busy, setBusy] = useState(false);
  const answered = answers.some((a) => a.trim());
  const make = async () => {
    if (!answered || busy) return;
    setBusy(true);
    const pairs = questions.map((q, i) => [q, answers[i] ?? ""] as [string, string]);
    const folded = await api.mediaWithAnswers(prompt, pairs).catch(() => prompt);
    setBusy(false);
    onMake(folded);
  };
  return (
    <div className="callout picture-questions">
      <span className="small">
        {questions.length === 1 ? t("One thing to know first, so it comes out the way you want:") : t("A few things to know first, so it comes out the way you want:")}
      </span>
      {questions.map((q, i) => (
        <label key={i} className="picture-question small">
          {q}
          <input
            className="input"
            value={answers[i] ?? ""}
            autoFocus={i === 0}
            onChange={(e) => setAnswers((all) => all.map((a, j) => (j === i ? e.target.value : a)))}
            onKeyDown={(e) => e.key === "Enter" && make()}
          />
        </label>
      ))}
      <div className="row">
        <span className="spacer" />
        <button className="btn small" disabled={busy} onClick={() => onMake(prompt)}>{t("Make it anyway")}</button>
        <button className="btn primary small" disabled={!answered || busy} onClick={make}>{t("Make it")}</button>
      </div>
    </div>
  );
}

const PRACTICE_PICTURES = 27;
/** Practice pictures and edit steps when coaching, as the core estimates it. */
const COACH_PICTURES = 110;
/** Coaching trains the writer's own weights (coach.rs). Out of sight: its
 * first real run did worse (79% → 74%, not kept), and the writer learns
 * from what's made instead (learn.rs). */
const SHOW_COACH = false;

/** "Prompt writer" settings under the composer, and teaching it a model. */
export function PromptHelpPanel({
  model,
  help,
  onStart,
  toast,
  canCheck = true,
  canAsk = false,
}: {
  model: MediaModelCard | undefined;
  help: ReturnType<typeof usePromptHelp>;
  onStart: (r: MediaRequest) => Promise<boolean>;
  toast: PushToast;
  canCheck?: boolean;
  /** Offer "Ask me when something's unclear" (where questions are asked). */
  canAsk?: boolean;
}) {
  const [teaching, setTeaching] = useState(false);
  const [coaching, setCoaching] = useState(false);
  const s = help.status;
  if (!model || model.id.startsWith("cloud:")) return null;
  const tuned = s?.tuned;
  const coach = s?.coach;
  const learned = s?.learned;
  const pct = (x: number) => Math.round(x * 100);
  return (
    <details className="prompt-help small">
      <summary>
        {t("Prompt writer")}
        {help.write ? ` · ${t("on")}` : ` · ${t("off")}`}
        {help.check && canCheck ? ` · ${t("checks pictures")}` : ""}
        {help.ask && canAsk ? ` · ${t("asks when unclear")}` : ""}
        {tuned ? ` · ${t("taught")}` : ""}
        {coach ? ` · ${t("coached")}` : ""}
        {learned?.jobs ? ` · ${t("learned from {n}", { n: learned.jobs })}` : ""}
      </summary>
      <label className="check">
        <input type="checkbox" checked={help.write} onChange={(e) => help.setWrite(e.target.checked)} />
        {t("Rewrite my description the way {model} follows best", { model: model.name })}
      </label>
      {canAsk && (
        <label className="check" title={t("Before a picture is made or changed, it asks about anything that would change the result, like which person you mean or what a sign should say. Chats ask too.")}>
          <input type="checkbox" checked={help.ask} onChange={(e) => help.setAsk(e.target.checked)} />
          {t("Ask me when something's unclear")}
        </label>
      )}
      {canCheck && (
        <label className="check" title={s?.reader ? t("Checked by {reader}", { reader: s.reader }) : undefined}>
          <input type="checkbox" checked={help.check} disabled={!s?.reader} onChange={(e) => help.setCheck(e.target.checked)} />
          {t("Check the picture and make it again if something you asked for is missing")}
        </label>
      )}
      {canCheck && !s?.reader && (
        <p className="muted">
          {t("Checking needs a model that can see pictures. Add Qwen3 VL 4B from the chat models (3.3 GB) and it's used for this too.")}{" "}
          <button
            className="link"
            onClick={() =>
              api
                .install("qwen3-vl-4b", "Q4_K_M")
                .then(() => toast(t("Downloading Qwen3 VL 4B (3.3 GB). Pictures can be checked once it's done; its progress shows on the Models page."), "success"))
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {t("Add Qwen3 VL 4B")}
          </button>
        </p>
      )}
      <label className="check" title={t("What it learns is kept encrypted on this PC. Incognito chats never teach it. In tests so far, it didn't make first tries better.")}>
        <input
          type="checkbox"
          checked={!!s?.learning}
          disabled={!s}
          onChange={(e) =>
            api
              .mediaSetLearning(e.target.checked)
              .then(help.reload)
              .catch((err) => toast(errorText(err), "error"))
          }
        />
        {t("Learn from what I make with {model} (experimental)", { model: model.name })}
      </label>
      {(s?.learning || !!learned?.jobs) && (
        <p className="muted">
          {learned?.jobs
            ? t("Learned from {jobs} checked pictures and edits: {examples} that worked and {rules} rules about how {model} reads prompts.", {
                jobs: learned.jobs,
                examples: learned.pictures + learned.edits,
                rules: learned.rules,
                model: model.name,
              })
            : t("It learns from every picture and edit that's checked: what showed, what didn't, and which wording fixed it.")}
          {learned?.now != null && learned.first != null && (
            <> {t("New pictures: first tries now show {now}% of what was asked for ({first}% at the start).", { now: pct(learned.now), first: pct(learned.first) })}</>
          )}
          {learned?.edit_now != null && learned.edit_first != null && (
            <> {t("Edits: first plans now do {now}% of what was asked for ({first}% at the start).", { now: pct(learned.edit_now), first: pct(learned.edit_first) })}</>
          )}
        </p>
      )}
      <div className="row wrap">
        {tuned ? (
          <span className="muted">
            {t("Taught on this PC: {before}% → {after}% of what was asked for showed up, over {n} practice pictures.", {
              before: Math.round(tuned.baseline * 100),
              after: Math.round(tuned.score * 100),
              n: tuned.pictures,
            })}
          </span>
        ) : (
          <span className="muted">{t("The writer uses built-in examples for this kind of model.")}</span>
        )}
        <span className="spacer" />
        <button className="btn small" disabled={!s?.reader} title={s?.reader ? undefined : t("Teaching needs a model that can see pictures.")} onClick={() => setTeaching(true)}>
          {tuned ? t("Teach it again…") : t("Teach it this model…")}
        </button>
      </div>
      {SHOW_COACH && s?.own_writer && (
        <div className="row wrap">
          {coach ? (
            <span className="muted">
              {t("Coached on {date}: its practice pictures scored {before}% → {after}%, learning from {n} of its best attempts.", {
                date: new Date(coach.at).toLocaleDateString(),
                before: Math.round(coach.before * 100),
                after: Math.round(coach.after * 100),
                n: coach.examples,
              })}
            </span>
          ) : (
            <span className="muted">{t("{model} brings its own language model to write with. It can practise on {model} and learn from what worked.", { model: model.name })}</span>
          )}
          <span className="spacer" />
          <button className="btn small" disabled={!s.reader} title={s.reader ? undefined : t("Coaching needs a model that can see pictures.")} onClick={() => setCoaching(true)}>
            {coach ? t("Coach it again…") : t("Coach the prompt writer…")}
          </button>
        </div>
      )}
      {(tuned || coach || !!learned?.jobs) && (
        <div className="row">
          <span className="spacer" />
          <button
            className="link"
            title={t("Goes back to the built-in examples and the writer as it came.")}
            onClick={() =>
              api
                .mediaForgetTuning(model.id)
                .then(help.reload)
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            {t("Forget what it learned")}
          </button>
        </div>
      )}
      {teaching && (
        <Modal title={t("Teach the prompt writer {model}", { model: model.name })} onClose={() => setTeaching(false)}>
          <p>{t("It writes prompts for {n} practice pictures, makes each one, checks what came out against what was asked for, and keeps the instructions and examples that worked best. Your pictures afterwards follow what you ask more closely.", { n: PRACTICE_PICTURES })}</p>
          <p className="small muted">
            {t("Takes {time}. The graphics card is busy the whole time; the practice pictures aren't kept.", {
              time: waitLabel(model.fit.est_secs * PRACTICE_PICTURES * 0.6 + 300),
            })}
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setTeaching(false)}>{t("Cancel")}</button>
            <button
              className="btn primary"
              autoFocus
              onClick={async () => {
                if (await onStart({ op: "tune", prompt: "", model_id: model.id })) setTeaching(false);
              }}
            >
              {t("Start teaching")}
            </button>
          </div>
        </Modal>
      )}
      {coaching && (
        <Modal title={t("Coach {model}'s prompt writer", { model: model.name })} onClose={() => setCoaching(false)}>
          <p>
            {model.can.includes("edit")
              ? t("{model}'s own language model writes prompts and edit plans for practice pictures. {model} makes each one, {reader} checks what came out, and the writer is trained on its best attempts.", { model: model.name, reader: s?.reader ?? "" })
              : t("{model}'s own language model writes prompts for practice pictures. {model} makes each one, {reader} checks what came out, and the writer is trained on its best attempts.", { model: model.name, reader: s?.reader ?? "" })}
          </p>
          <p>{t("The trained writer is kept only if its pictures then come out better than before.")}</p>
          <p className="small muted">
            {t("Takes {time}. The graphics card is busy the whole time; the practice pictures aren't kept.", {
              time: waitLabel(model.fit.est_secs * COACH_PICTURES * 0.6 + 1500),
            })}
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setCoaching(false)}>{t("Cancel")}</button>
            <button
              className="btn primary"
              autoFocus
              onClick={async () => {
                if (await onStart({ op: "coach", prompt: "", model_id: model.id })) setCoaching(false);
              }}
            >
              {t("Start coaching")}
            </button>
          </div>
        </Modal>
      )}
    </details>
  );
}

const SIZES: [number, number, string][] = [
  [1920, 1080, tx("Full HD")],
  [2560, 1440, tx("1440p")],
  [3840, 2160, tx("4K")],
  [3440, 1440, tx("Ultrawide")],
  [5120, 1440, tx("Super ultrawide")],
  [2560, 1600, tx("Laptop 16:10")],
  [2880, 1800, tx("Laptop 16:10, high resolution")],
  [1080, 1920, tx("Vertical screen")],
];

/** Wallpapers for a screen's exact size: still or moving, OLED-friendly or not. */
export function WallpaperComposer({
  imageModels,
  videoModels,
  gallery,
  onStart,
  toast,
}: {
  imageModels: MediaModelCard[];
  videoModels: MediaModelCard[];
  gallery: MediaItem[];
  onStart: (r: MediaRequest) => Promise<boolean>;
  toast: PushToast;
}) {
  const [screens, setScreens] = useState<Screen[]>([]);
  const [size, setSize] = useState<[number, number] | null>(null);
  const [custom, setCustom] = useState(false);
  const [moving, setMoving] = useState(false);
  const [oled, setOled] = useState(() => remembered("sulcus.studio.oled", false));
  const [prompt, setPrompt] = useState("");
  const [style, setStyle] = useState("auto");
  const [examples, setExamples] = useState<MediaItem[]>([]);
  const [use, setUse] = useState<ExampleUse>("style");
  const local = (list: MediaModelCard[]) => list.filter((m) => !m.id.startsWith("cloud:"));
  const models = moving ? local(videoModels) : local(imageModels);
  const [modelId, setModelId] = useState("");
  const m = models.find((x) => x.id === modelId) ?? [...models].sort((a, b) => b.quality - a.quality)[0];
  const help = usePromptHelp(m?.id);

  useEffect(() => {
    api
      .wallpaperScreens()
      .then((list) => {
        setScreens(list);
        const main = list.find((s) => s.primary) ?? list[0];
        if (main) setSize((cur) => cur ?? [main.width, main.height]);
        else setSize((cur) => cur ?? [1920, 1080]);
      })
      .catch(() => setSize((cur) => cur ?? [1920, 1080]));
  }, []);

  const [w, h] = size ?? [1920, 1080];
  const go = async () => {
    const wallpaper = { width: w, height: h, oled };
    const req: MediaRequest = moving
      ? { op: "video", prompt, model_id: m?.id, wallpaper, write_prompt: help.write }
      : {
          op: "generate",
          prompt,
          style,
          model_id: m?.id,
          wallpaper,
          examples: examples.map((e) => e.id),
          example_use: use,
          write_prompt: help.write,
          check: help.check,
        };
    if (await onStart(req)) toast(t("Started. It appears in the gallery when it's done; open it there to set it as your wallpaper."), "success");
  };

  return (
    <div className="form">
      <div className="row wrap">
        <span className="small muted">{t("Screen")}</span>
        <div className="filters tight">
          {screens.map((s, i) => (
            <button key={`${s.x},${s.y}`} className={`chip ${!custom && w === s.width && h === s.height ? "active" : ""}`} onClick={() => { setCustom(false); setSize([s.width, s.height]); }}>
              {screens.length > 1 ? t("Screen {n}", { n: i + 1 }) : t("This screen")} · {s.width}×{s.height}
              {s.primary && screens.length > 1 ? ` (${t("main")})` : ""}
            </button>
          ))}
        </div>
        <select
          value={custom ? "custom" : SIZES.some(([sw, sh]) => sw === w && sh === h) && !screens.some((s) => s.width === w && s.height === h) ? `${w}x${h}` : ""}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "custom") return setCustom(true);
            const [sw, sh] = v.split("x").map(Number);
            if (sw && sh) {
              setCustom(false);
              setSize([sw, sh]);
            }
          }}
          title={t("Other sizes")}
        >
          <option value="">{t("Other sizes…")}</option>
          {SIZES.map(([sw, sh, label]) => (
            <option key={`${sw}x${sh}`} value={`${sw}x${sh}`}>{`${t(label)} · ${sw}×${sh}`}</option>
          ))}
          <option value="custom">{t("Custom size")}</option>
        </select>
        {custom && (
          <span className="row">
            <input className="input tiny" type="number" min={320} max={16384} value={w} onChange={(e) => setSize([+e.target.value || 0, h])} aria-label={t("Width")} />
            ×
            <input className="input tiny" type="number" min={320} max={16384} value={h} onChange={(e) => setSize([w, +e.target.value || 0])} aria-label={t("Height")} />
          </span>
        )}
      </div>
      <div className="row wrap">
        <div className="mode-switch">
          <button className={`mode ${!moving ? "active" : ""}`} onClick={() => setMoving(false)}>{t("Still picture")}</button>
          <button className={`mode ${moving ? "active" : ""}`} onClick={() => setMoving(true)} disabled={local(videoModels).length === 0} title={local(videoModels).length === 0 ? t("Moving wallpapers need a video model such as Wan 2.2. Add one under Models on the Video tab.") : undefined}>
            {t("Moving")}
          </button>
        </div>
        <label className="check small" title={t("A true black background. Near-black pixels are switched fully off, which saves power and helps prevent burn-in on OLED screens.")}>
          <input
            type="checkbox"
            checked={oled}
            onChange={(e) => {
              setOled(e.target.checked);
              remember("sulcus.studio.oled", e.target.checked);
            }}
          />
          {t("OLED-friendly (true black)")}
        </label>
      </div>
      <textarea
        className="input"
        rows={3}
        value={prompt}
        placeholder={moving ? t("Describe the scene and its slow motion, e.g. \"aurora rippling over a frozen lake at night\"") : t("Describe your wallpaper, e.g. \"a misty mountain valley at sunrise, a lone cabin with warm windows\"")}
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && (e.ctrlKey || e.metaKey) && prompt.trim() && m && go()}
      />
      {!moving && <ExamplePicker value={examples} onChange={setExamples} use={use} onUse={setUse} gallery={gallery} toast={toast} canRead={!!help.status?.reader} />}
      <div className="row wrap">
        {!moving && (
          <select value={style} onChange={(e) => setStyle(e.target.value)} title={t("Look")} aria-label={t("Look")}>
            {WALLPAPER_LOOKS.map(([id, label]) => (
              <option key={id} value={id}>{t(label)}</option>
            ))}
          </select>
        )}
        {models.length > 1 && (
          <select value={m?.id ?? ""} onChange={(e) => setModelId(e.target.value)} title={t("Model")}>
            {models.map((x) => (
              <option key={x.id} value={x.id}>{x.name}</option>
            ))}
          </select>
        )}
        <span className="spacer" />
        {m && <span className="small muted">{waitLabel(moving ? m.fit.est_secs * 2 + 60 : m.fit.est_secs + 30)}</span>}
        <button className="btn primary" disabled={!prompt.trim() || !m || w < 320 || h < 320} onClick={go}>
          {moving ? t("Make the moving wallpaper") : t("Make the wallpaper")}
        </button>
      </div>
      {!m && <p className="small warn-text">{moving ? t("Add a video model such as Wan 2.2 first.") : t("Add a picture model first.")}</p>}
      {moving && <p className="small muted">{t("Moving wallpapers loop smoothly and are saved as MP4. Windows shows them with the free Lively Wallpaper app.")}</p>}
      <PromptHelpPanel model={m} help={help} onStart={onStart} toast={toast} canCheck={!moving} />
    </div>
  );
}

const WALLPAPER_LOOKS: [string, string][] = [
  ["auto", tx("Auto (realistic unless you say)")],
  ["photo", tx("Realistic photo")],
  ["cinematic", tx("Cinematic")],
  ["digital_art", tx("Digital art")],
  ["anime", tx("Anime")],
  ["oil_painting", tx("Oil painting")],
  ["watercolor", tx("Watercolor")],
  ["3d", tx("3D render")],
  ["pixel_art", tx("Pixel art")],
];

/** "Set as wallpaper" in the viewer: pick the screen when there are several. */
export function SetWallpaperButton({ item, toast }: { item: MediaItem; toast: PushToast }) {
  const [screens, setScreens] = useState<Screen[] | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const set = async (s: Screen | null) => {
    setBusy(true);
    setScreens(null);
    try {
      const r = await api.wallpaperSet(item.id, s?.x ?? null, s?.y ?? null);
      if (r.how === "windows") toast(t("Wallpaper set."), "success");
      else if (r.how === "lively") toast(t("Sent to Lively Wallpaper."), "success");
      else setSaved(r.path);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const open = async () => {
    if (item.kind !== "image") return set(null);
    const list = await api.wallpaperScreens().catch(() => [] as Screen[]);
    if (list.length > 1) setScreens(list);
    else set(null);
  };
  return (
    <>
      <button className="btn small" disabled={busy} onClick={open}>{busy ? t("Setting…") : t("Set as wallpaper")}</button>
      {screens && (
        <Modal title={t("Which screen?")} onClose={() => setScreens(null)}>
          <div className="row wrap">
            {screens.map((s, i) => (
              <button key={`${s.x},${s.y}`} className="btn" onClick={() => set(s)}>
                {t("Screen {n}", { n: i + 1 })} · {s.width}×{s.height}
                {s.primary ? ` (${t("main")})` : ""}
              </button>
            ))}
            <button className="btn" onClick={() => set(null)}>{t("All screens")}</button>
          </div>
        </Modal>
      )}
      {saved && (
        <Modal title={t("Saved for a moving wallpaper")} onClose={() => setSaved(null)}>
          <p>{t("Windows can't play a moving wallpaper by itself. The free app Lively Wallpaper can: install it, then drag this video into it. Once it's installed, this button sends it there for you.")}</p>
          <p className="small muted">{saved}</p>
          <div className="modal-actions">
            <a className="btn" href="https://rocksdanister.github.io/lively/" target="_blank" rel="noreferrer">{t("Get Lively Wallpaper")}</a>
            <button className="btn" onClick={() => api.showInFolder(saved)}>{t("Show in folder")}</button>
            <button className="btn primary" onClick={() => setSaved(null)}>{t("Done")}</button>
          </div>
        </Modal>
      )}
    </>
  );
}
