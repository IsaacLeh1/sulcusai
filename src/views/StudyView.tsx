// SPDX-License-Identifier: AGPL-3.0-only
// Study: flashcards, quizzes and mind maps made from what the person is
// learning (their notes, files, or documents in the library), practiced
// here; and podcasts and explainer videos about it, made in the Studio.
import { useCallback, useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type DocFile, type MapNode, type MediaRequest, type StudySet, type TalkSpeaker } from "../api";
import { t, tx } from "../i18n";
import { Busy, useModelPlace, useSince } from "../components/Busy";
import { ModelSelect } from "../components/ModelChoices";
import { LanguageSelect, talkLanguage, useVoices, VoiceSelect } from "../components/Podcast";
import { useOwnVoices } from "../components/OwnVoice";
import type { PushToast } from "../components/Toasts";

type Make = "cards" | "quiz" | "map" | "podcast" | "explainer";
const MAKES: [Make, string][] = [
  ["cards", tx("Flashcards")],
  ["quiz", tx("Quiz")],
  ["map", tx("Mind map")],
  ["podcast", tx("Podcast")],
  ["explainer", tx("Explainer video")],
];
const DOC_TYPES = ["pdf", "docx", "pptx", "xlsx", "csv", "txt", "md", "html"];

export function StudyView({ toast, onOpenStudio }: { toast: PushToast; onOpenStudio: () => void }) {
  const [sets, setSets] = useState<StudySet[]>([]);
  const [open, setOpen] = useState<StudySet | null>(null);
  const load = useCallback(() => api.studySets().then(setSets).catch((e) => toast(errorText(e), "error")), [toast]);
  useEffect(() => {
    load();
  }, [load]);

  if (open) {
    return (
      <div className="page study">
        <button className="link small" onClick={() => { setOpen(null); load(); }}>{t("← All study sets")}</button>
        <h1>{open.title}</h1>
        <p className="small muted">{t("From {from}", { from: open.from })}</p>
        {open.kind === "cards" && <Flashcards set={open} onChange={setOpen} toast={toast} />}
        {open.kind === "quiz" && <Quiz set={open} onChange={setOpen} toast={toast} />}
        {open.kind === "map" && open.map && <MindMap root={open.map} />}
      </div>
    );
  }
  return (
    <div className="page study">
      <header className="page-head">
        <div>
          <h1>{t("Study")}</h1>
          <p className="muted">{t("Turn what you're learning into flashcards, quizzes and mind maps, or a podcast or explainer video about it. Made from your notes, files or documents, on this PC.")}</p>
        </div>
      </header>
      <MakeCard toast={toast} onMade={(s) => { load(); setOpen(s); }} onOpenStudio={onOpenStudio} />
      {sets.length > 0 && (
        <section className="study-sets">
          <h2>{t("Your study sets")}</h2>
          {sets.map((s) => (
            <SetRow key={s.id} set={s} onOpen={() => setOpen(s)} onDelete={() => api.studyDelete(s.id).then(load).catch((e) => toast(errorText(e), "error"))} />
          ))}
        </section>
      )}
    </div>
  );
}

function SetRow({ set, onOpen, onDelete }: { set: StudySet; onOpen: () => void; onDelete: () => void }) {
  const [sure, setSure] = useState(false);
  const due = set.cards.filter((c) => c.due <= Date.now()).length;
  const what = set.kind === "cards" ? t("{n} flashcards", { n: set.cards.length }) : set.kind === "quiz" ? t("{n} questions", { n: set.quiz.length }) : t("Mind map");
  const extra = set.kind === "cards" ? (due > 0 ? t("{n} to practice now", { n: due }) : t("All practiced for now")) : set.kind === "quiz" && set.best != null ? t("Best: {p}%", { p: Math.round(set.best * 100) }) : "";
  return (
    <div className="card study-set row wrap">
      <button className="study-set-open grow" onClick={onOpen}>
        <strong>{set.title}</strong>
        <span className="small muted">{what}{extra ? ` · ${extra}` : ""}</span>
      </button>
      <button className="btn ghost small danger" onBlur={() => setSure(false)} onClick={() => (sure ? onDelete() : setSure(true))}>{sure ? t("Delete it?") : t("Delete")}</button>
    </div>
  );
}

/** What to study and what to make of it. */
function MakeCard({ toast, onMade, onOpenStudio }: { toast: PushToast; onMade: (s: StudySet) => void; onOpenStudio: () => void }) {
  const [make, setMake] = useState<Make>("cards");
  const [text, setText] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [docs, setDocs] = useState<DocFile[]>([]);
  const [library, setLibrary] = useState<DocFile[]>([]);
  const [picking, setPicking] = useState(false);
  const [count, setCount] = useState(20);
  const [minutes, setMinutes] = useState(5);
  const [lang, setLang] = useState(talkLanguage());
  const [writer, setWriter] = useState<string | null>(null);
  const [voice, setVoice] = useState<string | null>(null);
  const [busy, setBusy] = useState<{ stage: string; fraction: number } | null>(null);
  const since = useSince(!!busy);
  const place = useModelPlace(!!busy);
  const voices = useVoices();
  const own = (useOwnVoices()?.voices ?? []).filter((v) => v.can_speak);

  useEffect(() => {
    const sub = on("study:progress", (p) => setBusy((b) => (b ? p : b)));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  // The library's files, to pick from.
  useEffect(() => {
    api.docsLibrary()
      .then(async (v) => {
        const all = await Promise.all(v.sources.map((s) => api.docsFiles(s.id).catch(() => [] as DocFile[])));
        setLibrary(all.flat().filter((f) => !f.error));
      })
      .catch(() => {});
  }, []);

  const addFiles = async () => {
    const picked = await openDialog({ multiple: true, filters: [{ name: t("Documents"), extensions: DOC_TYPES }] });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    setFiles((f) => [...f, ...paths.filter((p) => !f.includes(p))]);
  };

  const ready = !busy && (text.trim() || files.length > 0 || docs.length > 0);
  const go = async () => {
    if (make === "podcast" || make === "explainer") {
      // Made in the Studio: the library's files go by their paths.
      const all = [...files, ...docs.map((d) => d.path)];
      const speakers: TalkSpeaker[] = make === "podcast"
        ? [{ name: "Alex", voice: null, about: t("the host, who asks the questions a student would") }, { name: "Sam", voice, about: t("a teacher who knows the material well") }]
        : [{ name: t("Narrator"), voice }];
      const req: MediaRequest = { op: make, prompt: text.trim() || t("Teach the material in these files."), language: lang, talk: { speakers, minutes, style: make === "podcast" ? "lesson" : null, files: all, writer } };
      try {
        await api.mediaStart(req);
        toast(make === "podcast" ? t("The podcast is being made in the Studio.") : t("The explainer video is being made in the Studio."), "success");
        onOpenStudio();
      } catch (e) {
        toast(errorText(e), "error");
      }
      return;
    }
    setBusy({ stage: t("Starting"), fraction: 0 });
    try {
      const set = await api.studyMake({ kind: make, text, files, docs: docs.map((d) => d.id), count, language: lang, writer });
      setText("");
      setFiles([]);
      setDocs([]);
      onMade(set);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="card study-make form">
      <h2>{t("Make something to study")}</h2>
      <textarea className="input" rows={4} value={text} placeholder={t("Paste your notes, or name a topic. Add files or library documents below.")} aria-label={t("What to study")} onChange={(e) => setText(e.target.value)} />
      <div className="row wrap">
        <button className="btn small" onClick={addFiles}>{t("Add files…")}</button>
        {library.length > 0 && <button className="btn small" aria-expanded={picking} onClick={() => setPicking((p) => !p)}>{t("From your documents…")}</button>}
        {files.map((f) => (
          <span key={f} className="chip">
            {f.split(/[\\/]/).pop()}
            <button className="chip-x" onClick={() => setFiles(files.filter((x) => x !== f))} aria-label={t("Remove")}>×</button>
          </span>
        ))}
        {docs.map((d) => (
          <span key={d.id} className="chip">
            {d.name}
            <button className="chip-x" onClick={() => setDocs(docs.filter((x) => x.id !== d.id))} aria-label={t("Remove")}>×</button>
          </span>
        ))}
      </div>
      {picking && (
        <div className="study-library">
          {library.map((f) => (
            <label key={f.id} className="check small">
              <input type="checkbox" checked={docs.some((d) => d.id === f.id)} onChange={(e) => setDocs((d) => (e.target.checked ? [...d, f] : d.filter((x) => x.id !== f.id)))} />
              {f.name}
            </label>
          ))}
        </div>
      )}
      <div className="seg" role="tablist" aria-label={t("What to make")}>
        {MAKES.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={make === id} className={make === id ? "active" : ""} onClick={() => setMake(id)}>{t(label)}</button>
        ))}
      </div>
      <div className="row wrap">
        {(make === "cards" || make === "quiz") && (
          <label className="small">
            {make === "cards" ? t("How many cards") : t("How many questions")}
            <select value={count} onChange={(e) => setCount(Number(e.target.value))}>
              {[5, 10, 20, 30, 50, 80].map((n) => <option key={n} value={n}>{n}</option>)}
            </select>
          </label>
        )}
        {(make === "podcast" || make === "explainer") && (
          <>
            <label className="small">
              {t("Length")}
              <select value={minutes} onChange={(e) => setMinutes(Number(e.target.value))}>
                {[2, 3, 5, 10, 15].map((m) => <option key={m} value={m}>{t("about {m} min", { m })}</option>)}
              </select>
            </label>
            <label className="small">
              {make === "podcast" ? t("The teacher's voice") : t("Narrator's voice")}
              <VoiceSelect value={voice} onChange={setVoice} voices={voices} own={own} label={t("Voice")} pickForMe />
            </label>
          </>
        )}
        <label className="small">
          {t("Language")}
          <LanguageSelect value={lang} onChange={setLang} />
        </label>
        <label className="small grow">
          {t("Written by")}
          <ModelSelect value={writer} onChange={setWriter} label={t("The model that writes it")} />
        </label>
      </div>
      {busy ? (
        <Busy stage={busy.stage} fraction={busy.fraction > 0 && busy.fraction < 1 ? busy.fraction : null} since={since} where={place} hint={t("Long material is read in parts; each part adds its share.")} />
      ) : (
        <div className="row">
          <span className="small muted">
            {make === "podcast" ? t("Two people talk it through, made in the Studio.") : make === "explainer" ? t("Slides with a voice explaining them, made in the Studio.") : t("Made from only what you give it.")}
          </span>
          <span className="spacer" />
          <button className="btn primary" disabled={!ready} onClick={go}>{t("Make it")}</button>
        </div>
      )}
    </section>
  );
}

/** Flashcards: the due ones first; flip, then say whether you knew it. */
function Flashcards({ set, onChange, toast }: { set: StudySet; onChange: (s: StudySet) => void; toast: PushToast }) {
  const order = useMemo(() => {
    const now = Date.now();
    const idx = set.cards.map((_, i) => i);
    const due = idx.filter((i) => set.cards[i].due <= now);
    return due.length > 0 ? due : idx;
    // Ordered once per opening, so answering doesn't reshuffle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [set.id]);
  const [at, setAt] = useState(0);
  const [flipped, setFlipped] = useState(false);
  const [knew, setKnew] = useState(0);
  const done = at >= order.length;
  const answer = async (k: boolean) => {
    try {
      onChange(await api.studyCard(set.id, order[at], k));
      if (k) setKnew((n) => n + 1);
      setFlipped(false);
      setAt((a) => a + 1);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (done || (e.target as HTMLElement)?.tagName === "INPUT") return;
      if (e.key === " ") { e.preventDefault(); setFlipped((f) => !f); }
      if (flipped && e.key === "1") answer(false);
      if (flipped && e.key === "2") answer(true);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });
  if (done) {
    return (
      <div className="card study-done">
        <h2>{t("Done for now")}</h2>
        <p>{t("You knew {knew} of {n}. Cards you knew come back later; the others come back soon.", { knew, n: order.length })}</p>
        <button className="btn" onClick={() => { setAt(0); setKnew(0); }}>{t("Go through them again")}</button>
      </div>
    );
  }
  const card = set.cards[order[at]];
  return (
    <div className="flashcards">
      <p className="small muted">{t("Card {n} of {of}", { n: at + 1, of: order.length })}</p>
      <button className={`flashcard ${flipped ? "flipped" : ""}`} onClick={() => setFlipped((f) => !f)} aria-label={flipped ? t("Show the front") : t("Show the answer")}>
        <span className="flashcard-side">{flipped ? card.back : card.front}</span>
        <span className="small muted">{flipped ? t("Answer") : t("Click or press Space to see the answer")}</span>
      </button>
      {flipped && (
        <div className="row flashcard-answers">
          <button className="btn" onClick={() => answer(false)}>{t("Didn't know it (1)")}</button>
          <button className="btn primary" onClick={() => answer(true)}>{t("Knew it (2)")}</button>
        </div>
      )}
    </div>
  );
}

/** A quiz: one question at a time, then why the answer is right, then the score. */
function Quiz({ set, onChange, toast }: { set: StudySet; onChange: (s: StudySet) => void; toast: PushToast }) {
  const [at, setAt] = useState(0);
  const [picked, setPicked] = useState<number | null>(null);
  const [right, setRight] = useState(0);
  const done = at >= set.quiz.length;
  useEffect(() => {
    if (done) api.studyQuizDone(set.id, right / Math.max(1, set.quiz.length)).then(onChange).catch((e) => toast(errorText(e), "error"));
    // Once, when it's finished.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [done]);
  if (done) {
    return (
      <div className="card study-done">
        <h2>{t("{right} of {n} right", { right, n: set.quiz.length })}</h2>
        {set.best != null && <p className="small muted">{t("Your best: {p}%", { p: Math.round(set.best * 100) })}</p>}
        <button className="btn" onClick={() => { setAt(0); setRight(0); setPicked(null); }}>{t("Take it again")}</button>
      </div>
    );
  }
  const q = set.quiz[at];
  return (
    <div className="quiz card">
      <p className="small muted">{t("Question {n} of {of}", { n: at + 1, of: set.quiz.length })}</p>
      <h2 className="quiz-question">{q.question}</h2>
      <div className="quiz-choices">
        {q.choices.map((c, i) => (
          <button
            key={i}
            className={`quiz-choice ${picked == null ? "" : i === q.answer ? "right" : i === picked ? "wrong" : "dim"}`}
            disabled={picked != null}
            onClick={() => {
              setPicked(i);
              if (i === q.answer) setRight((r) => r + 1);
            }}
          >
            <span className="quiz-letter">{String.fromCharCode(65 + i)}</span> {c}
          </button>
        ))}
      </div>
      {picked != null && (
        <>
          <p className={picked === q.answer ? "quiz-verdict right" : "quiz-verdict wrong"}>{picked === q.answer ? t("Right.") : t("Not quite: it's {answer}.", { answer: q.choices[q.answer] })} {q.why}</p>
          <button className="btn primary" onClick={() => { setAt((a) => a + 1); setPicked(null); }}>{at + 1 < set.quiz.length ? t("Next") : t("See your score")}</button>
        </>
      )}
    </div>
  );
}

/** A mind map: the topic on the left, branches opening to the right; click a branch to fold it. */
function MindMap({ root }: { root: MapNode }) {
  return (
    <div className="mindmap" role="tree" aria-label={root.label}>
      <MapBranch node={root} depth={0} />
    </div>
  );
}

function MapBranch({ node, depth }: { node: MapNode; depth: number }) {
  const [folded, setFolded] = useState(false);
  const has = node.children.length > 0;
  return (
    <div className={`map-branch depth-${Math.min(depth, 3)}`} role="treeitem" aria-expanded={has ? !folded : undefined}>
      <button className="map-node" onClick={() => has && setFolded((f) => !f)} title={has ? (folded ? t("Show its branches") : t("Fold it")) : undefined}>
        {node.label}
        {has && folded && <span className="map-more"> +{node.children.length}</span>}
      </button>
      {has && !folded && (
        <div className="map-children" role="group">
          {node.children.map((c, i) => (
            <MapBranch key={i} node={c} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}
