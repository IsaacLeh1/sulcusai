// SPDX-License-Identifier: AGPL-3.0-only
// Language and writing: partner practice, writing help, scam check, PC health and social posts.
import { useEffect, useState } from "react";
import { errorText, api, type Language } from "../api";
import { writingApi, charsLeft, pieces, type WritingTool, type PartnerRound, type PartnerScore, type PartnerProgress, type WritingReport, type ScamVerdict, type PcHealth, type SocialShown } from "../api/writing";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

const TABS: { id: WritingTool; label: () => string; about: () => string }[] = [
  { id: "partner", label: () => t("Language partner"), about: () => t("Put sentences into the language you're learning; each answer is scored word by word.") },
  { id: "writing_help", label: () => t("Writing help"), about: () => t("How your text reads, its tone, and grammar slips. Checks run on this PC.") },
  { id: "scam_check", label: () => t("Scam check"), about: () => t("Paste a message or link to look for the signs scams share. Nothing leaves this PC.") },
  { id: "pc_health", label: () => t("PC health"), about: () => t("A look at disk space, memory and restarts, with advice. It changes nothing.") },
  { id: "social", label: () => t("Social posting"), about: () => t("Post to your own Bluesky or Mastodon account. Nothing is sent until you press Post.") },
];

const LEVELS = ["A1", "A2", "B1", "B2", "C1", "C2"];

export function WritingView({ toast }: { toast: PushToast }) {
  const [tab, setTab] = useState<WritingTool>("partner");
  const [on, setOn] = useState<Record<string, boolean>>({});
  useEffect(() => {
    writingApi.switches().then(setOn).catch(() => {});
  }, []);
  const flip = async (tool: WritingTool, value: boolean) => {
    try {
      await writingApi.setSwitch(tool, value);
      setOn((o) => ({ ...o, [tool]: value }));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const cur = TABS.find((x) => x.id === tab)!;
  return (
    <div className="page writing-view">
      <h2>{t("Language and writing")}</h2>
      <div className="tabs" role="tablist">
        {TABS.map((x) => (
          <button key={x.id} role="tab" aria-selected={tab === x.id} className={tab === x.id ? "active" : ""} onClick={() => setTab(x.id)}>
            {x.label()}
          </button>
        ))}
      </div>
      <p className="muted">{cur.about()}</p>
      <label className="switch-row">
        <input type="checkbox" checked={!!on[tab]} onChange={(e) => flip(tab, e.target.checked)} /> {t("Turned on")}
      </label>
      {on[tab] && tab === "partner" && <Partner toast={toast} />}
      {on[tab] && tab === "writing_help" && <Help toast={toast} />}
      {on[tab] && tab === "scam_check" && <Scam toast={toast} />}
      {on[tab] && tab === "pc_health" && <Health toast={toast} />}
      {on[tab] && tab === "social" && <Social toast={toast} />}
    </div>
  );
}

function Partner({ toast }: { toast: PushToast }) {
  const [languages, setLanguages] = useState<Language[]>([]);
  const [lang, setLang] = useState("es");
  const [level, setLevel] = useState(1);
  const [topic, setTopic] = useState("");
  const [round, setRound] = useState<PartnerRound | null>(null);
  const [answer, setAnswer] = useState("");
  const [marked, setMarked] = useState<PartnerScore | null>(null);
  const [progress, setProgress] = useState<PartnerProgress | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.languages().then(setLanguages).catch(() => {});
  }, []);
  const next = async () => {
    setBusy(true);
    setMarked(null);
    setAnswer("");
    try {
      setRound(await writingApi.partnerRound(lang, level, topic));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const submit = async () => {
    if (!round || !answer.trim()) return;
    try {
      const r = await writingApi.partnerAnswer(lang, level, round.reference, answer);
      setMarked(r.score);
      setProgress(r.progress);
      setLevel(r.next_level);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="stack">
      <div className="row">
        <select value={lang} onChange={(e) => setLang(e.target.value)} aria-label={t("Language")}>
          {languages.map((l) => <option key={l.code} value={l.code}>{l.name}</option>)}
        </select>
        <select value={level} onChange={(e) => setLevel(Number(e.target.value))} aria-label={t("Level")}>
          {LEVELS.map((l, i) => <option key={l} value={i + 1}>{l}</option>)}
        </select>
        <input value={topic} onChange={(e) => setTopic(e.target.value)} placeholder={t("Topic (optional)")} />
        <button onClick={next} disabled={busy}>{busy ? t("Thinking...") : t("New sentence")}</button>
      </div>
      {round && (
        <>
          <p><strong>{round.english}</strong></p>
          {round.hint && <p className="muted">{t("Watch:")} {round.hint}</p>}
          <textarea value={answer} onChange={(e) => setAnswer(e.target.value)} rows={2} placeholder={t("Your translation")} />
          <button onClick={submit} disabled={!answer.trim()}>{t("Check")}</button>
        </>
      )}
      {marked && round && (
        <div>
          <p><strong>{marked.score}</strong> / 100</p>
          <p>{marked.words.map((w, i) => <span key={i} className={`mark-${w.mark}`}>{w.word} </span>)}</p>
          {marked.missing.length > 0 && <p className="muted">{t("Left out:")} {marked.missing.join(", ")}</p>}
          <p className="muted">{t("A natural answer:")} {round.reference}</p>
        </div>
      )}
      {progress && (
        <p className="muted">
          {t("Rounds")}: {progress.rounds} · {t("Average")}: {Math.round(progress.total / Math.max(progress.rounds, 1))} · {t("Best streak")}: {progress.best_streak}
        </p>
      )}
    </div>
  );
}

const TONES = ["formal", "friendly", "concise", "warm", "confident", "plain"];

function Help({ toast }: { toast: PushToast }) {
  const [text, setText] = useState("");
  const [report, setReport] = useState<WritingReport | null>(null);
  const [tone, setTone] = useState("formal");
  const [busy, setBusy] = useState(false);
  const check = async (s = text) => {
    try {
      setReport(await writingApi.check(s));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const fix = async () => {
    const s = await writingApi.fix(text).catch((e) => (toast(errorText(e), "error"), text));
    setText(s);
    check(s);
  };
  const rewrite = async () => {
    setBusy(true);
    try {
      const s = await writingApi.rewrite(text, tone);
      setText(s);
      check(s);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="stack">
      <textarea value={text} onChange={(e) => setText(e.target.value)} rows={8} placeholder={t("Paste or type your text")} />
      <div className="row">
        <button onClick={() => check()} disabled={!text.trim()}>{t("Check")}</button>
        <button onClick={fix} disabled={!report?.issues.some((i) => i.fix)}>{t("Fix all")}</button>
        <select value={tone} onChange={(e) => setTone(e.target.value)} aria-label={t("Tone")}>
          {TONES.map((x) => <option key={x} value={x}>{t(x)}</option>)}
        </select>
        <button onClick={rewrite} disabled={busy || !text.trim()}>{busy ? t("Rewriting...") : t("Rewrite in this tone")}</button>
      </div>
      {report && (
        <>
          <p className="muted">
            {t("Words")}: {report.words} · {t("Sentences")}: {report.sentences} · {t("Reading ease")}: {report.ease} · {t("Grade")}: {report.grade} · {t("Tone")}: {t(report.tone)}
            {report.long_sentences > 0 && <> · {t("Long sentences")}: {report.long_sentences}</>}
          </p>
          <p className="writing-marked">
            {pieces(text, report.issues).map((p, i) => p.issue ? <mark key={i} title={p.issue.message}>{p.text}</mark> : <span key={i}>{p.text}</span>)}
          </p>
          {report.issues.length === 0 && <p>{t("No slips found.")}</p>}
        </>
      )}
    </div>
  );
}

function Scam({ toast }: { toast: PushToast }) {
  const [text, setText] = useState("");
  const [sender, setSender] = useState("");
  const [v, setV] = useState<ScamVerdict | null>(null);
  const run = async () => {
    try {
      setV(await writingApi.scamCheck(text, sender.trim() || null));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const label = { no_signs: t("No common signs found. That doesn't prove it's safe."), careful: t("Be careful: some signs of a scam."), likely_scam: t("Likely a scam. Don't reply, pay or click.") };
  return (
    <div className="stack">
      <input value={sender} onChange={(e) => setSender(e.target.value)} placeholder={t("Sender's address (optional)")} />
      <textarea value={text} onChange={(e) => setText(e.target.value)} rows={6} placeholder={t("Paste the message or link")} />
      <button onClick={run} disabled={!text.trim()}>{t("Check")}</button>
      {v && (
        <>
          <p><strong>{label[v.level]}</strong></p>
          <ul>{v.signs.map((s, i) => <li key={i}>{s.why}</li>)}</ul>
        </>
      )}
    </div>
  );
}

function Health({ toast }: { toast: PushToast }) {
  const [h, setH] = useState<PcHealth | null>(null);
  const run = async () => {
    try {
      setH(await writingApi.pcHealth());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const word = { ok: t("Fine"), watch: t("Worth a look"), act: t("Needs action") };
  return (
    <div className="stack">
      <button onClick={run}>{t("Check this PC")}</button>
      {h && (
        <>
          <p><strong>{word[h.overall]}</strong></p>
          <ul>{h.findings.map((f, i) => <li key={i}><strong>{word[f.level]}:</strong> {f.text}</li>)}</ul>
        </>
      )}
    </div>
  );
}

function Social({ toast }: { toast: PushToast }) {
  const [accounts, setAccounts] = useState<SocialShown[]>([]);
  const [pick, setPick] = useState(0);
  const [text, setText] = useState("");
  const [kind, setKind] = useState<"bluesky" | "mastodon">("bluesky");
  const [name, setName] = useState("");
  const [secret, setSecret] = useState("");
  const [confirming, setConfirming] = useState(false);
  const load = () => writingApi.socialAccounts().then(setAccounts).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  const add = async () => {
    try {
      await writingApi.socialAdd(kind === "bluesky" ? { kind, handle: name, app_password: secret, server: "https://bsky.social" } : { kind, instance: name, token: secret });
      setName("");
      setSecret("");
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const post = async () => {
    setConfirming(false);
    try {
      const link = await writingApi.socialPost(pick, text);
      toast(link ? `${t("Posted")}: ${link}` : t("Posted"), "info");
      setText("");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const acc = accounts[pick];
  const left = acc ? charsLeft(text, acc.limit) : 0;
  return (
    <div className="stack">
      {accounts.length > 0 && (
        <>
          <div className="row">
            <select value={pick} onChange={(e) => setPick(Number(e.target.value))} aria-label={t("Account")}>
              {accounts.map((a, i) => <option key={i} value={i}>{a.kind === "bluesky" ? "Bluesky" : "Mastodon"}: {a.name}</option>)}
            </select>
            <button onClick={() => writingApi.socialRemove(pick).then(load)}>{t("Remove")}</button>
          </div>
          <textarea value={text} onChange={(e) => { setText(e.target.value); setConfirming(false); }} rows={4} placeholder={t("What to post")} />
          <p className={left < 0 ? "error" : "muted"}>{left} {t("characters left")}</p>
          {!confirming ? (
            <button onClick={() => setConfirming(true)} disabled={!text.trim() || left < 0}>{t("Post...")}</button>
          ) : (
            <div className="confirm-box">
              <p>{t("This is posted publicly to")} {acc?.name}:</p>
              <blockquote>{text.trim()}</blockquote>
              <button onClick={post}>{t("Post")}</button> <button onClick={() => setConfirming(false)}>{t("Cancel")}</button>
            </div>
          )}
        </>
      )}
      <h3>{t("Add an account")}</h3>
      <div className="row">
        <select value={kind} onChange={(e) => setKind(e.target.value as "bluesky" | "mastodon")} aria-label={t("Site")}>
          <option value="bluesky">Bluesky</option>
          <option value="mastodon">Mastodon</option>
        </select>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder={kind === "bluesky" ? t("Handle, like name.bsky.social") : t("Server, like mastodon.social")} />
        <input type="password" value={secret} onChange={(e) => setSecret(e.target.value)} placeholder={kind === "bluesky" ? t("App password") : t("Access token")} />
        <button onClick={add} disabled={!name.trim() || !secret.trim()}>{t("Add")}</button>
      </div>
      <p className="muted">{t("Kept on this PC, encrypted. You are responsible for what you post.")}</p>
    </div>
  );
}
