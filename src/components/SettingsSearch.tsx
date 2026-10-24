// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo, useRef, useState, type RefObject } from "react";
import { rank } from "../fuzzy";
import { t } from "../i18n";

/** Words people type for a setting, and words the settings use for it. */
const SYNONYMS: Record<string, string[]> = {
  dark: ["theme", "appearance"],
  light: ["theme", "appearance"],
  color: ["theme", "appearance"],
  colour: ["theme", "appearance"],
  look: ["appearance", "theme"],
  password: ["lock", "hello", "pin", "passphrase"],
  pin: ["lock", "hello"],
  unlock: ["lock", "hello"],
  login: ["lock", "sign"],
  fingerprint: ["hello"],
  face: ["hello"],
  gpu: ["graphics", "performance"],
  vram: ["graphics", "memory"],
  nvidia: ["graphics"],
  amd: ["graphics"],
  temp: ["temperature", "performance", "cool"],
  heat: ["temperature", "performance", "cool"],
  hot: ["temperature", "performance", "cool"],
  fan: ["temperature", "performance"],
  cpu: ["processor", "performance"],
  speed: ["performance"],
  slow: ["performance"],
  fast: ["performance"],
  battery: ["battery", "performance"],
  power: ["battery", "performance"],
  ram: ["memory", "performance"],
  api: ["cloud", "key", "server"],
  key: ["key"],
  keys: ["key"],
  openai: ["cloud"],
  anthropic: ["cloud"],
  claude: ["cloud"],
  chatgpt: ["cloud"],
  gpt: ["cloud"],
  gemini: ["cloud"],
  openrouter: ["cloud"],
  openwebui: ["cloud"],
  litellm: ["cloud"],
  provider: ["cloud"],
  budget: ["cloud", "budget"],
  upgrade: ["updates", "update"],
  version: ["updates", "about"],
  changelog: ["update notes"],
  release: ["update notes"],
  releases: ["update notes"],
  bug: ["crash", "feedback"],
  mobile: ["phone"],
  iphone: ["phone"],
  android: ["phone"],
  computer: ["devices", "pc", "sync"],
  laptop: ["devices", "pc", "sync"],
  desktop: ["devices", "pc", "sync"],
  pair: ["devices", "pair", "phone"],
  restore: ["backup"],
  export: ["backup"],
  translate: ["language"],
  mic: ["voice", "microphone", "dictation"],
  microphone: ["voice", "dictation"],
  speak: ["voice", "speech"],
  talk: ["voice", "speech"],
  dictate: ["dictation", "voice"],
  internet: ["connectivity", "offline", "cloud", "web"],
  online: ["connectivity", "cloud", "web"],
  wifi: ["connectivity", "network"],
  network: ["connectivity", "network"],
  google: ["web", "search"],
  brave: ["web", "search"],
  duckduckgo: ["web", "search"],
  chrome: ["browser"],
  edge: ["browser"],
  firefox: ["browser"],
  disk: ["storage"],
  space: ["storage"],
  drive: ["storage"],
  tracking: ["privacy"],
  telemetry: ["privacy", "crash"],
  me: ["about", "you", "name"],
  profile: ["about", "you"],
  city: ["where", "location"],
  email: ["sign", "accounts", "mail"],
  gmail: ["sign", "accounts", "mail"],
  outlook: ["sign", "accounts", "mail"],
  calendar: ["sign", "accounts", "calendar"],
  files: ["folders"],
  port: ["server", "api"],
};

interface Spot {
  el: HTMLElement;
  text: string;
  section: string;
}

/** What a heading or label says, without the text of its inputs and lists. */
function ownText(el: HTMLElement): string {
  const copy = el.cloneNode(true) as HTMLElement;
  copy.querySelectorAll("input, select, textarea, option, button, .muted, .badge").forEach((n) => n.remove());
  // Text pieces joined with spaces, so "Port" and "Running" don't run together.
  const parts: string[] = [];
  const walk = document.createTreeWalker(copy, NodeFilter.SHOW_TEXT);
  while (walk.nextNode()) parts.push(walk.currentNode.textContent ?? "");
  return parts.join(" ").replace(/\s+/g, " ").trim().slice(0, 140);
}

/** Every heading and labeled control on the Settings page. */
function collect(root: HTMLElement): Spot[] {
  const out: Spot[] = [];
  const seen = new Set<string>();
  root.querySelectorAll<HTMLElement>("section h2, section h3, section h4, section label, section .toggle-row strong, section summary, section .level strong, section .adv-head strong, section .callout, section input[aria-label]").forEach((el) => {
    if (el.closest(".settings-search")) return;
    const text = el.tagName === "INPUT" ? (el.getAttribute("aria-label") ?? "") : ownText(el);
    const section = el.closest("section")?.querySelector("h2")?.textContent?.trim() ?? "";
    const key = `${section}|${text}`;
    if (text.length < 2 || seen.has(key)) return;
    seen.add(key);
    out.push({ el, text, section });
  });
  return out;
}

/** Search box for the Settings page: finds a setting by its name, part of
 * it, a misspelling or another word for it, and scrolls to it. */
export function SettingsSearch({ pageRef }: { pageRef: RefObject<HTMLDivElement | null> }) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [pick, setPick] = useState(0);
  const box = useRef<HTMLInputElement>(null);

  const matches = useMemo(() => {
    const root = pageRef.current;
    if (!root || !query.trim()) return [];
    // Headings rank above labels with the same words; whole sections first.
    return rank(collect(root), query, (s) => `${s.text} ${s.section}`, SYNONYMS)
      .map((m) => ({ ...m, score: m.score + (m.item.el.tagName === "H2" ? 0.05 : 0) - (words(m.item.text) > 8 ? 0.05 : 0) }))
      .sort((a, b) => b.score - a.score)
      .slice(0, 8);
  }, [query, pageRef]);
  const corrected = matches[0]?.corrected ?? [];

  const go = (i: number) => {
    const m = matches[i];
    if (!m) return;
    setOpen(false);
    const el = m.item.el;
    el.scrollIntoView({ behavior: "smooth", block: "center" });
    el.classList.remove("search-hit");
    // Restarts the highlight if the same one is picked again.
    void el.offsetWidth;
    el.classList.add("search-hit");
    setTimeout(() => el.classList.remove("search-hit"), 2200);
    const control = el.querySelector<HTMLElement>("input, select, textarea") ?? (el.tagName === "LABEL" ? null : el.closest(".toggle-row")?.querySelector<HTMLElement>("input"));
    control?.focus({ preventScroll: true });
  };

  return (
    <div className="settings-search" role="search">
      <input
        ref={box}
        className="input search"
        type="search"
        placeholder={t("Search settings")}
        aria-label={t("Search settings")}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setPick(0);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setPick((p) => Math.min(p + 1, matches.length - 1));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setPick((p) => Math.max(p - 1, 0));
          } else if (e.key === "Enter") {
            go(pick);
          } else if (e.key === "Escape") {
            setOpen(false);
          }
        }}
      />
      {open && query.trim() && (
        <div className="settings-results" role="listbox">
          {corrected.length > 0 && <p className="muted small">{t("Showing results for “{words}”", { words: corrected.map(([, to]) => to).join(" ") })}</p>}
          {matches.length === 0 && <p className="muted small">{t("No settings match “{query}”. Try another word.", { query: query.trim() })}</p>}
          {matches.map((m, i) => (
            <button
              key={i}
              role="option"
              aria-selected={i === pick}
              className={`settings-result ${i === pick ? "active" : ""}`}
              onMouseDown={(e) => e.preventDefault()}
              onMouseEnter={() => setPick(i)}
              onClick={() => go(i)}
            >
              <span>{m.item.text}</span>
              {m.item.section && m.item.section !== m.item.text && <span className="muted small">{m.item.section}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function words(text: string): number {
  return text.split(/\s+/).length;
}
