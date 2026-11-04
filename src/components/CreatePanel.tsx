// SPDX-License-Identifier: AGPL-3.0-only
// The Studio's Create tab: storyboards, batches, regions, guides and live painting, tiles, editing video by transcript, lip-sync, sound effects, dialogue tags and a mic check.
import { useEffect, useRef, useState } from "react";
import { errorText, type MediaItem } from "../api";
import type { MediaRequest } from "../api/media";
import { create, keepPairs, monoSamples, regionMask, toggleWord, type BatchItem, type CutWord, type DialoguePart, type Guide, type LikenessClaim, type MicCheck, type Region, type Shot } from "../api/create";
import { t } from "../i18n";
import { FaceChatSettings } from "./FaceChat";
import type { PushToast } from "./Toasts";

type Part = "story" | "batch" | "regions" | "guide" | "video" | "sound";

interface Props {
  gallery: MediaItem[];
  onStart: (req: MediaRequest) => Promise<boolean>;
  toast: PushToast;
}

const b64 = (dataUrl: string) => dataUrl.split(",")[1] ?? "";

export function CreatePanel({ gallery, onStart, toast }: Props) {
  const [part, setPart] = useState<Part>("story");
  const names: Record<Part, string> = {
    story: t("Storyboard"),
    batch: t("Batch from a spreadsheet"),
    regions: t("Regions"),
    guide: t("Sketch, depth and tiles"),
    video: t("Video by transcript"),
    sound: t("Sound and voice"),
  };
  return (
    <div className="create-panel">
      <div className="mode-switch" role="tablist">
        {(Object.keys(names) as Part[]).map((p) => (
          <button key={p} role="tab" aria-selected={part === p} className={`mode ${part === p ? "active" : ""}`} onClick={() => setPart(p)}>
            {names[p]}
          </button>
        ))}
      </div>
      {part === "story" && <Storyboard onStart={onStart} toast={toast} />}
      {part === "batch" && <Batch onStart={onStart} toast={toast} />}
      {part === "regions" && <Regions gallery={gallery} onStart={onStart} toast={toast} />}
      {part === "guide" && <Guides gallery={gallery} onStart={onStart} toast={toast} />}
      {part === "video" && <VideoByText gallery={gallery} onStart={onStart} toast={toast} />}
      {part === "sound" && <Sound onStart={onStart} toast={toast} />}
      {part === "sound" && (
        <>
          <h4>{t("Voice chat with a face")}</h4>
          <FaceChatSettings pictures={gallery.filter((g) => g.kind === "image")} toast={toast} />
        </>
      )}
    </div>
  );
}

function Pick({ items, value, onChange, label }: { items: MediaItem[]; value: string; onChange: (id: string) => void; label: string }) {
  return (
    <label className="field">
      <span>{label}</span>
      <select value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">{t("Choose...")}</option>
        {items.slice(0, 200).map((i) => (
          <option key={i.id} value={i.id}>
            {(i.title || i.prompt || i.id).slice(0, 70)}
          </option>
        ))}
      </select>
    </label>
  );
}

function Storyboard({ onStart, toast }: { onStart: Props["onStart"]; toast: PushToast }) {
  const [script, setScript] = useState("");
  const [style, setStyle] = useState("cinematic film still");
  const [shots, setShots] = useState<Shot[]>([]);
  const split = async () => {
    try {
      setShots(await create.storyboard(script, style, 5));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const drawAll = async () => {
    for (const s of shots) {
      if (!(await onStart({ op: "generate", prompt: s.prompt, shape: "landscape" }))) return;
    }
    toast(t("Drawing {n} shots. They appear in the gallery as they finish.", { n: shots.length }), "info");
  };
  return (
    <div className="stack">
      <p className="muted">{t("Paste a script. Shots start at INT., EXT., SHOT or SCENE headings, or at blank lines.")}</p>
      <textarea rows={8} value={script} onChange={(e) => setScript(e.target.value)} placeholder={t("INT. KITCHEN - NIGHT\nA kettle boils.\nANNA: Is anyone home?")} />
      <label className="field">
        <span>{t("Look")}</span>
        <input value={style} onChange={(e) => setStyle(e.target.value)} />
      </label>
      <div className="row">
        <button className="btn" disabled={!script.trim()} onClick={split}>
          {t("Split into shots")}
        </button>
        <button className="btn primary" disabled={!shots.length} onClick={drawAll}>
          {t("Draw every shot")}
        </button>
      </div>
      {shots.length > 0 && (
        <ol className="shot-list">
          {shots.map((s) => (
            <li key={s.n}>
              <strong>{s.heading || t("Shot {n}", { n: s.n })}</strong> <span className="muted">{t("{s} s", { s: s.seconds.toFixed(1) })}</span>
              <div>{s.prompt}</div>
              {s.dialogue.map((d, i) => (
                <div key={i} className="muted">
                  {d}
                </div>
              ))}
              <button className="btn small" onClick={() => onStart({ op: "generate", prompt: s.prompt, shape: "landscape" })}>
                {t("Draw this shot")}
              </button>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function Batch({ onStart, toast }: { onStart: Props["onStart"]; toast: PushToast }) {
  const [items, setItems] = useState<BatchItem[]>([]);
  const read = async (f: File | undefined) => {
    if (!f) return;
    try {
      setItems(await create.batch(await f.text()));
    } catch (e) {
      setItems([]);
      toast(errorText(e), "error");
    }
  };
  const makeAll = async () => {
    for (const it of items) {
      const ok = await onStart({ op: "generate", prompt: it.prompt, negative: it.negative || null, seed: it.seed, width: it.width ?? undefined, height: it.height ?? undefined });
      if (!ok) return;
    }
    toast(t("{n} pictures queued.", { n: items.length }), "info");
  };
  return (
    <div className="stack">
      <p className="muted">{t("Save the spreadsheet as CSV. It needs a \"prompt\" column; negative, seed, width, height, count and name are used when present. Other columns fill {column} in the prompt.")}</p>
      <input type="file" accept=".csv,text/csv" onChange={(e) => read(e.target.files?.[0])} />
      {items.length > 0 && (
        <>
          <p>{t("{n} pictures from the spreadsheet.", { n: items.length })}</p>
          <ul className="batch-list">
            {items.slice(0, 20).map((it, i) => (
              <li key={i}>
                <strong>{it.name}</strong> {it.prompt}
              </li>
            ))}
          </ul>
          <button className="btn primary" onClick={makeAll}>
            {t("Make them all")}
          </button>
        </>
      )}
    </div>
  );
}

function Regions({ gallery, onStart, toast }: Props) {
  const [base, setBase] = useState("");
  const [regions, setRegions] = useState<Region[]>([{ x: 0, y: 0.5, w: 0.5, h: 0.5, prompt: "" }]);
  const [picture, setPicture] = useState("");
  const set = (i: number, r: Partial<Region>) => setRegions((all) => all.map((x, k) => (k === i ? { ...x, ...r } : x)));
  const pct = (v: number) => Math.round(v * 100);
  const first = async () => {
    try {
      const prompt = await create.regionScene(base, regions);
      await onStart({ op: "generate", prompt });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const refine = async () => {
    const it = gallery.find((g) => g.id === picture);
    if (!it) return;
    for (const r of regions.filter((r) => r.prompt.trim())) {
      const mask = b64(regionMask(r, it.width, it.height));
      if (!(await onStart({ op: "fill", source: it.id, prompt: r.prompt, mask }))) return;
    }
  };
  return (
    <div className="stack">
      <p className="muted">{t("Say what goes where. The whole scene is drawn first, then each region is drawn again on its own.")}</p>
      <label className="field">
        <span>{t("The whole scene")}</span>
        <input value={base} onChange={(e) => setBase(e.target.value)} />
      </label>
      {regions.map((r, i) => (
        <div key={i} className="row region-row">
          <input aria-label={t("What goes here")} placeholder={t("What goes here")} value={r.prompt} onChange={(e) => set(i, { prompt: e.target.value })} />
          {(["x", "y", "w", "h"] as const).map((k) => (
            <label key={k} className="field small">
              <span>{{ x: t("Left %"), y: t("Top %"), w: t("Width %"), h: t("Height %") }[k]}</span>
              <input type="number" min={0} max={100} value={pct(r[k])} onChange={(e) => set(i, { [k]: Math.min(100, Math.max(0, Number(e.target.value))) / 100 })} />
            </label>
          ))}
          <button className="btn small" onClick={() => setRegions((all) => all.filter((_, k) => k !== i))}>
            {t("Remove")}
          </button>
        </div>
      ))}
      <div className="row">
        <button className="btn" disabled={regions.length >= 8} onClick={() => setRegions((all) => [...all, { x: 0.25, y: 0.25, w: 0.5, h: 0.5, prompt: "" }])}>
          {t("Add a region")}
        </button>
        <button className="btn primary" disabled={!base.trim() && !regions.some((r) => r.prompt.trim())} onClick={first}>
          {t("Draw the scene")}
        </button>
      </div>
      <Pick items={gallery.filter((g) => g.kind === "image")} value={picture} onChange={setPicture} label={t("Then draw each region again on")} />
      <button className="btn" disabled={!picture} onClick={refine}>
        {t("Draw the regions")}
      </button>
    </div>
  );
}

function Guides({ gallery, onStart, toast }: Props) {
  const [prompt, setPrompt] = useState("");
  const [kind, setKind] = useState<Guide>("sketch");
  const [from, setFrom] = useState("");
  const [strength, setStrength] = useState(0.9);
  const [tile, setTile] = useState(false);
  const [live, setLive] = useState(false);
  const [nets, setNets] = useState<{ names: string[]; dir: string } | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const drawing = useRef(false);
  const drew = useRef(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => {
    create.controlNets().then(([names, dir]) => setNets({ names, dir })).catch(() => setNets(null));
    const c = canvas.current?.getContext("2d");
    if (c) {
      c.fillStyle = "#fff";
      c.fillRect(0, 0, 512, 512);
    }
    return () => window.clearTimeout(timer.current);
  }, []);

  const sketch = () => (drew.current && canvas.current ? b64(canvas.current.toDataURL("image/png")) : null);
  const make = async (quick: boolean) => {
    const png = from ? null : sketch();
    const guide = from || png ? { image: from || null, png, kind, strength } : null;
    if (!guide && !tile) {
      toast(t("Draw a sketch, pick a guide picture, or ask for a tile."), "info");
      return;
    }
    const tilePrompt = tile && !/seamless/i.test(prompt) ? `${prompt}, seamless tileable pattern, flat even lighting` : prompt;
    await onStart({ op: "generate", prompt: tilePrompt, picture_extra: { tile, guide }, ...(quick ? { steps: 6, width: 512, height: 512, check: false, write_prompt: false } : {}) });
  };
  const point = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * 512, ((e.clientY - r.top) / r.height) * 512] as const;
  };
  const down = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const g = canvas.current?.getContext("2d");
    if (!g) return;
    drawing.current = true;
    drew.current = true;
    g.strokeStyle = "#000";
    g.lineWidth = 4;
    g.lineCap = "round";
    g.beginPath();
    g.moveTo(...point(e));
  };
  const move = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!drawing.current) return;
    const g = canvas.current?.getContext("2d");
    g?.lineTo(...point(e));
    g?.stroke();
  };
  const up = () => {
    drawing.current = false;
    if (live) {
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => make(true), 900);
    }
  };
  const clear = () => {
    const g = canvas.current?.getContext("2d");
    if (g) {
      g.fillStyle = "#fff";
      g.fillRect(0, 0, 512, 512);
    }
    drew.current = false;
  };
  return (
    <div className="stack">
      <label className="field">
        <span>{t("Description")}</span>
        <input value={prompt} onChange={(e) => setPrompt(e.target.value)} />
      </label>
      <div className="row">
        <label className="field">
          <span>{t("Guide")}</span>
          <select value={kind} onChange={(e) => setKind(e.target.value as Guide)}>
            <option value="sketch">{t("A sketch")}</option>
            <option value="edges">{t("The outlines of a photo")}</option>
            <option value="depth">{t("A depth map")}</option>
          </select>
        </label>
        <label className="field">
          <span>{t("How closely to follow it")}</span>
          <input type="range" min={0.2} max={1.5} step={0.1} value={strength} onChange={(e) => setStrength(Number(e.target.value))} />
        </label>
      </div>
      {nets && !nets.names.length && <p className="muted">{t("Guides need a ControlNet file (depth, canny, scribble or lineart) in {dir}.", { dir: nets.dir })}</p>}
      <Pick items={gallery.filter((g) => g.kind === "image")} value={from} onChange={setFrom} label={t("Guide picture from the gallery (or draw one below)")} />
      {!from && (
        <>
          <canvas ref={canvas} width={512} height={512} className="sketch-canvas" style={{ width: 256, height: 256, touchAction: "none", border: "1px solid var(--border)" }} onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerLeave={up} />
          <div className="row">
            <button className="btn small" onClick={clear}>
              {t("Clear")}
            </button>
            <label className="check">
              <input type="checkbox" checked={live} onChange={(e) => setLive(e.target.checked)} />
              {t("Live painting: a quick picture after each pause in drawing")}
            </label>
          </div>
        </>
      )}
      <label className="check">
        <input type="checkbox" checked={tile} onChange={(e) => setTile(e.target.checked)} />
        {t("Seamless tile (edges meet when repeated)")}
      </label>
      <button className="btn primary" onClick={() => make(false)}>
        {t("Make the picture")}
      </button>
    </div>
  );
}

function Consent({ claim, setClaim }: { claim: LikenessClaim; setClaim: (c: LikenessClaim) => void }) {
  return (
    <div className="stack">
      <label className="field">
        <span>{t("Whose face is in it?")}</span>
        <select value={claim.face ?? "someone"} onChange={(e) => setClaim({ ...claim, face: e.target.value as LikenessClaim["face"] })}>
          <option value="myself">{t("Mine")}</option>
          <option value="someone">{t("Someone else, real")}</option>
          <option value="nobody">{t("No one real (made up or drawn)")}</option>
        </select>
      </label>
      {claim.face === "someone" || claim.face === undefined ? (
        <label className="check">
          <input type="checkbox" checked={!!claim.permission_from_them} onChange={(e) => setClaim({ ...claim, permission_from_them: e.target.checked })} />
          {t("I have their permission to change what their lips say")}
        </label>
      ) : null}
    </div>
  );
}

function VideoByText({ gallery, onStart, toast }: Props) {
  const [video, setVideo] = useState("");
  const [words, setWords] = useState<CutWord[]>([]);
  const [busy, setBusy] = useState(false);
  const [clean, setClean] = useState<"" | "gentle" | "strong">("");
  const [claim, setClaim] = useState<LikenessClaim>({ face: "someone" });
  const item = gallery.find((g) => g.id === video);
  const listen = async () => {
    setBusy(true);
    try {
      setWords(await create.transcript(video));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const cut = async () => {
    if (!item) return;
    const [ranges] = words.length ? await create.textCut(words, item.seconds) : [[] as Array<[number, number]>];
    await onStart({ op: "video_edit", prompt: "", source: video, edit: { keep: keepPairs(ranges), clean_speech: clean || null } });
  };
  const lipsync = async () => {
    try {
      await create.consent({ ...claim, voice: "nobody" });
      await onStart({ op: "lipsync", prompt: "", source: video });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const fillers = () => setWords((w) => w.map((x) => (/^(um+|uh+|erm|uhm|hmm+|mm+)[,.!?]*$/i.test(x.text.trim()) ? { ...x, removed: true } : x)));
  return (
    <div className="stack">
      <Pick items={gallery.filter((g) => g.kind === "video")} value={video} onChange={(v) => (setVideo(v), setWords([]))} label={t("Video")} />
      <div className="row">
        <button className="btn" disabled={!video || busy} onClick={listen}>
          {busy ? t("Listening...") : t("Get the transcript")}
        </button>
        <button className="btn" disabled={!words.length} onClick={fillers}>
          {t("Strike um and uh")}
        </button>
      </div>
      {words.length > 0 && (
        <p className="transcript-words">
          {words.map((w, i) => (
            <button key={i} className={`word ${w.removed ? "struck" : ""}`} style={{ textDecoration: w.removed ? "line-through" : undefined, opacity: w.removed ? 0.5 : 1 }} onClick={() => setWords((all) => toggleWord(all, i))}>
              {w.text}
            </button>
          ))}
        </p>
      )}
      <label className="field">
        <span>{t("Clean the speech")}</span>
        <select value={clean} onChange={(e) => setClean(e.target.value as typeof clean)}>
          <option value="">{t("No, keep it as recorded")}</option>
          <option value="gentle">{t("Gently (noise, hum, level)")}</option>
          <option value="strong">{t("Strongly (for noisy rooms)")}</option>
        </select>
      </label>
      <button className="btn primary" disabled={!video || (!words.some((w) => w.removed) && !clean)} onClick={cut}>
        {t("Make the edited video")}
      </button>
      <h4>{t("Lip-sync")}</h4>
      <p className="muted">{t("After dubbing, moves the speaker's lips to the new voice where someone speaks.")}</p>
      <Consent claim={claim} setClaim={setClaim} />
      <button className="btn" disabled={!video} onClick={lipsync}>
        {t("Lip-sync this video")}
      </button>
    </div>
  );
}

function Sound({ onStart, toast }: { onStart: Props["onStart"]; toast: PushToast }) {
  const [sfx, setSfx] = useState("");
  const [script, setScript] = useState("");
  const [parts, setParts] = useState<DialoguePart[]>([]);
  const [mic, setMic] = useState<MicCheck | null>(null);
  const [recording, setRecording] = useState(false);
  const effect = async () => {
    try {
      const e = await create.sfx(sfx);
      await onStart({ op: "sound", prompt: e.prompt, seconds: e.seconds, instrumental: true });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const preview = async () => setParts(await create.dialogue(script, t("Narrator")));
  const check = async () => {
    setRecording(true);
    setMic(null);
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: { noiseSuppression: false, echoCancellation: false, autoGainControl: false } });
      const ctx = new AudioContext();
      const src = ctx.createMediaStreamSource(stream);
      const proc = ctx.createScriptProcessor(4096, 1, 1);
      const got: Float32Array[] = [];
      proc.onaudioprocess = (e) => got.push(new Float32Array(e.inputBuffer.getChannelData(0)));
      src.connect(proc);
      proc.connect(ctx.destination);
      await new Promise((r) => setTimeout(r, 4000));
      proc.disconnect();
      src.disconnect();
      stream.getTracks().forEach((tr) => tr.stop());
      const all = new Float32Array(got.reduce((n, b) => n + b.length, 0));
      let at = 0;
      for (const b of got) {
        all.set(b, at);
        at += b.length;
      }
      const rate = ctx.sampleRate;
      await ctx.close();
      setMic(await create.micCheck(monoSamples([all]), rate));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setRecording(false);
    }
  };
  return (
    <div className="stack">
      <h4>{t("Sound effects")}</h4>
      <div className="row">
        <input value={sfx} onChange={(e) => setSfx(e.target.value)} placeholder={t("30 seconds of rain on a tin roof, looping")} />
        <button className="btn primary" disabled={!sfx.trim()} onClick={effect}>
          {t("Make the sound")}
        </button>
      </div>
      <h4>{t("Reading with dialogue tags")}</h4>
      <p className="muted">{t("One line each: NAME [whispers]: words. Tags like [laughs], [pause 2s], [shouts] or [slowly] change how it's read.")}</p>
      <textarea rows={6} value={script} onChange={(e) => setScript(e.target.value)} />
      <div className="row">
        <button className="btn" disabled={!script.trim()} onClick={preview}>
          {t("Check the script")}
        </button>
        <button className="btn primary" disabled={!script.trim()} onClick={() => onStart({ op: "dialogue", prompt: script })}>
          {t("Read it aloud")}
        </button>
      </div>
      {parts.length > 0 && (
        <ul>
          {parts.map((p, i) => (
            <li key={i}>
              <strong>{p.speaker}</strong> {p.delivery.length > 0 && <em>({p.delivery.join(", ")})</em>} {p.text} {p.pause_after > 0 && <span className="muted">{t("then {s} s pause", { s: p.pause_after })}</span>}
            </li>
          ))}
        </ul>
      )}
      <h4>{t("Mic check")}</h4>
      <p className="muted">{t("Speak normally for 4 seconds. Nothing is kept.")}</p>
      <button className="btn" disabled={recording} onClick={check}>
        {recording ? t("Listening...") : t("Check my microphone")}
      </button>
      {mic && (
        <div>
          <p>{t("Voice {v} dB, background {n} dB, peak {p} dB.", { v: mic.speech_db.toFixed(0), n: mic.noise_db.toFixed(0), p: mic.peak_db.toFixed(1) })}</p>
          {mic.advice.length ? (
            <ul>
              {mic.advice.map((a, i) => (
                <li key={i}>{t(a)}</li>
              ))}
            </ul>
          ) : (
            <p>{t("It sounds fine.")}</p>
          )}
        </div>
      )}
    </div>
  );
}
