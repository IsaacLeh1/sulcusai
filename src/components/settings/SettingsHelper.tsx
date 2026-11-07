// SPDX-License-Identifier: AGPL-3.0-only
import { useRef, useState } from "react";
import { api, errorText, type FeatureId } from "../../api";
import { confirmed } from "../../confirm";
import { rank } from "../../fuzzy";
import { t } from "../../i18n";
import type { PushToast } from "../Toasts";
import { CATEGORIES, GUIDES, type Guide } from "./catalog";
import { openLink } from "./SetupGuide";

interface Action {
  label: string;
  run: () => Promise<string | void> | string | void;
}

interface Line {
  from: "you" | "helper";
  text: string;
  actions?: Action[];
}

/** Things the helper can switch on itself, each behind a question first. */
const SWITCHES: { words: string; feature: FeatureId; label: string }[] = [
  { words: "dictation dictate speak type voice typing", feature: "dictation", label: "Dictation" },
  { words: "talk voice chat conversation speak", feature: "voice_chat", label: "Voice chat" },
  { words: "read aloud speak answers out loud", feature: "read_aloud", label: "Read aloud" },
  { words: "meetings record transcribe notes", feature: "meetings", label: "Meetings" },
  { words: "mail email inbox gmail outlook", feature: "email" as FeatureId, label: "Email" },
  { words: "calendar events schedule", feature: "calendar" as FeatureId, label: "Calendar" },
  { words: "files folders documents", feature: "files" as FeatureId, label: "Files" },
];

/** A chat that finds settings, walks through guides and turns things on,
 * asking before every change. It runs on this PC without a model, so it
 * works offline and before any model is downloaded. */
export function SettingsHelper({ toast, goTo, features }: { toast: PushToast; goTo: (section: string, guide?: string) => void; features: Set<FeatureId> }) {
  const [lines, setLines] = useState<Line[]>([
    { from: "helper", text: t("Tell me what you want to do, like \"link my phone\", \"add a Claude key\" or \"turn on dictation\". I ask before changing anything.") },
  ]);
  const [text, setText] = useState("");
  // The guide being walked through, so "next" and "done" mean something.
  const walking = useRef<{ guide: Guide; step: number } | null>(null);
  const end = useRef<HTMLDivElement>(null);

  const say = (l: Line) => {
    setLines((prev) => [...prev, l]);
    setTimeout(() => end.current?.scrollIntoView({ block: "end" }), 0);
  };

  const stepLine = (g: Guide, i: number): Line => {
    const s = g.steps[i];
    const actions: Action[] = [];
    if (s.link) actions.push({ label: t("Open {site}", { site: new URL(s.link).hostname }), run: () => openLink(s.link!, toast) });
    if (s.value) actions.push({ label: t("Copy the value"), run: () => { navigator.clipboard.writeText(s.value!).catch(() => {}); } });
    if (i + 1 < g.steps.length) actions.push({ label: t("Next step"), run: () => advance() });
    else actions.push({ label: t("Show me the setting"), run: () => goTo(g.section, g.id) });
    return { from: "helper", text: `${t("Step {n} of {all}:", { n: i + 1, all: g.steps.length })} ${t(s.text)}${s.value ? `\n${s.value}` : ""}`, actions };
  };

  const startGuide = (g: Guide) => {
    walking.current = { guide: g, step: 0 };
    say({ from: "helper", text: t(g.why) });
    say(stepLine(g, 0));
  };

  const advance = () => {
    const w = walking.current;
    if (!w) return;
    w.step += 1;
    if (w.step < w.guide.steps.length) say(stepLine(w.guide, w.step));
    else finish(w.guide);
  };

  /** After the person has signed in or deployed on their side, the parts
   * that happen in this app are done here, once they agree. */
  const finish = async (g: Guide) => {
    walking.current = null;
    if (g.id === "phone-anywhere") {
      if (!(await confirmed(t("Turn on phone use and Reach this phone from anywhere?")))) return say({ from: "helper", text: t("Left as it was.") });
      try {
        await api.setPhone(true);
        await api.setPhoneLink(true);
        say({ from: "helper", text: t("Done. Now pair your phone in Phone."), actions: [{ label: t("Open Phone"), run: () => goTo("phone") }] });
      } catch (e) {
        say({ from: "helper", text: errorText(e) });
      }
      return;
    }
    say({ from: "helper", text: t("That's every step. The last part is pasting your value on the settings page."), actions: [{ label: t("Open it"), run: () => goTo(g.section, g.id) }] });
  };

  const turnOn = async (feature: FeatureId, label: string) => {
    if (features.has(feature)) return say({ from: "helper", text: t("{name} is already on.", { name: t(label) }) });
    if (!(await confirmed(t("Turn on {name}?", { name: t(label) })))) return say({ from: "helper", text: t("Left as it was.") });
    try {
      await api.setFeature(feature, true);
      say({ from: "helper", text: t("{name} is on.", { name: t(label) }), actions: [{ label: t("Undo"), run: async () => { await api.setFeature(feature, false); return t("{name} is off again.", { name: t(label) }); } }] });
    } catch (e) {
      say({ from: "helper", text: errorText(e) });
    }
  };

  const answer = (q: string) => {
    const lower = q.toLowerCase().trim();
    if (walking.current && /^(next|done|ok|okay|signed in|finished|yes)\b/.test(lower)) return advance();
    if (/^(stop|cancel|never ?mind)\b/.test(lower)) {
      walking.current = null;
      return say({ from: "helper", text: t("Stopped.") });
    }
    const wantsOn = /\b(turn on|enable|switch on|start using)\b/.test(lower);
    const sw = rank(SWITCHES, q, (s) => `${s.label} ${s.words}`)[0];
    if (wantsOn && sw && sw.score > 0.3) return void turnOn(sw.item.feature, sw.item.label);
    const guide = rank(GUIDES, q, (g) => `${g.title} ${g.why}`)[0];
    const cat = rank(CATEGORIES, q, (c) => `${c.label} ${c.sentence}`)[0];
    if (guide && guide.score >= (cat?.score ?? 0) && guide.score > 0.25) {
      return say({ from: "helper", text: t("I can walk you through \"{title}\", one step at a time.", { title: t(guide.item.title) }), actions: [{ label: t("Start"), run: () => startGuide(guide.item) }, { label: t("Show the page"), run: () => goTo(guide.item.section, guide.item.id) }] });
    }
    if (sw && sw.score > 0.35) return say({ from: "helper", text: t("Do you want {name} on?", { name: t(sw.item.label) }), actions: [{ label: t("Turn it on"), run: () => turnOn(sw.item.feature, sw.item.label) }] });
    if (cat && cat.score > 0.2) return say({ from: "helper", text: t(cat.item.sentence), actions: [{ label: t("Open {name}", { name: t(cat.item.label) }), run: () => goTo(`cat:${cat.item.id}`) }] });
    say({ from: "helper", text: t("I didn't find that. Try other words, or use the search box at the top.") });
  };

  const send = () => {
    const q = text.trim();
    if (!q) return;
    setText("");
    say({ from: "you", text: q });
    answer(q);
  };

  return (
    <section className="card settings-helper">
      <h2>{t("Settings helper")}</h2>
      <div className="helper-lines" aria-live="polite">
        {lines.map((l, i) => (
          <div key={i} className={`helper-line ${l.from}`}>
            <span className="helper-text">{l.text}</span>
            {l.actions && (
              <span className="row wrap">
                {l.actions.map((a) => (
                  <button
                    key={a.label}
                    className="btn small"
                    onClick={async () => {
                      const r = await a.run();
                      if (typeof r === "string") say({ from: "helper", text: r });
                    }}
                  >
                    {a.label}
                  </button>
                ))}
              </span>
            )}
          </div>
        ))}
        <div ref={end} />
      </div>
      <span className="row">
        <input className="input grow" value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && send()} placeholder={t("What do you want to set up?")} aria-label={t("What do you want to set up?")} />
        <button className="btn primary small" onClick={send} disabled={!text.trim()}>{t("Ask")}</button>
      </span>
    </section>
  );
}
