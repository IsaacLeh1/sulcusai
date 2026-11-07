// SPDX-License-Identifier: AGPL-3.0-only
import { ImportFromOthers } from "../components/ImportFromOthers";
import { keepDrafts } from "../drafts";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { ThemePicker } from "../components/ThemePicker";
import { LayoutAssistant } from "../workspace/LayoutAssistant";
import { setTokensShown, useTokensShown } from "../tokenUsage";
import { WidgetsSettings } from "../workspace/WidgetsSettings";
import { api, errorText, type AppInfo, type Connectivity, type FeatureId, type Profile, type SearchProvider, type SecurityStatus, type WebSettings, type BrowsersView, type BrowserSearch, type MainBrowser, type SavedSiteKey } from "../api";
import { APP_NAME } from "../brand";
import { chosenLanguage, language as currentLanguage, LANGUAGES, MACHINE_MADE, setLanguage, t, tx } from "../i18n";
import { CloudConfirm, LEVELS } from "../components/ConnectivityMenu";
import { SecuritySection } from "../components/Security";
import { FoldersSection } from "../components/AgentCards";
import { SignInApps } from "../components/SignIn";
import { CloudFiles } from "../components/CloudFiles";
import { WakeWord } from "../components/WakeWord";
import { VoiceSection } from "../components/VoiceSettings";
import { PerformanceSection } from "../components/Performance";
import { QuietSection } from "../components/Quiet";
import { PcCheck } from "../components/PcCheck";
import { ModelStorage } from "../components/ModelStorage";
import { CleanupSettings } from "../components/CleanupSettings";
import type { PushToast } from "../components/Toasts";
import { AdvancedSettings } from "../components/AdvancedSettings";
import { ApiServerSettings } from "../components/ApiServerSettings";
import { CloudSettings } from "../components/CloudSettings";
import { BackupSettings } from "../components/BackupSettings";
import { FindSettings } from "../components/FindSettings";
import { DocumentTools } from "../components/DocumentTools";
import { SyncSettings } from "../components/SyncSettings";
import { PhoneSettings } from "../components/PhoneSettings";
import { TelegramSettings } from "../components/TelegramSettings";
import { MatrixSettings } from "../components/MatrixSettings";
import { DiscordSettings } from "../components/DiscordSettings";
import { PortsSettings } from "../components/PortsSettings";
import { HandoffSettings } from "../components/HandoffSettings";
import { LowTokenSettings } from "../components/LowTokenSettings";
import { PermissionRules } from "../components/PermissionRules";
import { LogSettings } from "../components/LogSettings";
import { DevModeSetting } from "../components/DevMode";
import { PrivacySettings } from "../components/PrivacySettings";
import { PrivacyModeSettings, SentLog } from "../components/SharePermission";
import { UpdateNotes, UpdateSettings } from "../components/Updates";
import { CrashReportsSetting } from "../components/Feedback";
import { VOICE_FEATURES } from "../components/SettingsNav";
import { SettingsSearch } from "../components/SettingsSearch";
import { SecurityPage } from "../components/SecurityPage";
import { VaultPage } from "../components/VaultPage";
import { YourDataSettings } from "../components/YourDataSettings";
import { MarketSettings } from "../components/MarketSettings";
import { NetworkSettings } from "../components/NetworkSettings";
import { foldAll, unfold, useCardFolds } from "../components/CardFolds";
import { CATEGORIES, GUIDES, SECTION_CATEGORY, WHATS_THIS, categoryOf, guidesFor, type CategoryId } from "../components/settings/catalog";
import { SettingsHome, type SpecialPage } from "../components/settings/SettingsHome";
import { SetupGuide } from "../components/settings/SetupGuide";
import { HealthPage } from "../components/settings/HealthPage";
import { PrivacyPlain } from "../components/settings/PrivacyPlain";
import { SettingsHelper } from "../components/settings/SettingsHelper";

const THIRD_PARTY = [
  { name: "Tauri", license: "MIT / Apache-2.0", url: "https://github.com/tauri-apps/tauri" },
  { name: tx("axum (local API server)"), license: "MIT", url: "https://github.com/tokio-rs/axum" },
  { name: "llama.cpp", license: "MIT", url: "https://github.com/ggml-org/llama.cpp" },
  { name: tx("whisper.cpp (downloaded with a speech model)"), license: "MIT", url: "https://github.com/ggml-org/whisper.cpp" },
  { name: tx("Silero VAD (voice detection model)"), license: "MIT", url: "https://github.com/snakers4/silero-vad" },
  { name: tx("ONNX Runtime (downloaded with natural voices)"), license: "MIT", url: "https://github.com/microsoft/onnxruntime" },
  { name: "SQLite", license: tx("Public domain"), url: "https://sqlite.org/copyright.html" },
  { name: "React", license: "MIT", url: "https://github.com/facebook/react" },
  { name: "react-markdown / remark-gfm", license: "MIT", url: "https://github.com/remarkjs/react-markdown" },
  { name: "KaTeX / remark-math / rehype-katex", license: "MIT", url: "https://github.com/KaTeX/KaTeX" },
  { name: "RustCrypto (aes-gcm, argon2, sha2)", license: "MIT / Apache-2.0", url: "https://github.com/RustCrypto" },
  { name: tx("Email and calendar (async-imap, mail-parser, lettre, quick-xml, chrono-tz)"), license: "MIT / Apache-2.0", url: "" },
  { name: tx("Documents (docx-rs, rust_xlsxwriter, calamine, pdf-extract)"), license: "MIT / Apache-2.0", url: "" },
  { name: tx("PowerPoint template (from python-pptx)"), license: "MIT", url: "https://github.com/scanny/python-pptx" },
  { name: tx("stable-diffusion.cpp (downloaded with a picture or video model)"), license: "MIT", url: "https://github.com/leejet/stable-diffusion.cpp" },
  { name: tx("acestep.cpp (downloaded with a music model)"), license: "MIT", url: "https://github.com/ServeurpersoCom/acestep.cpp" },
  { name: tx("image (pictures)"), license: "MIT / Apache-2.0", url: "https://github.com/image-rs/image" },
  { name: tx("Arti (Tor, for the private connection)"), license: "MIT / Apache-2.0", url: "https://gitlab.torproject.org/tpo/core/arti" },
  { name: tx("qwentts.cpp and Qwen3-TTS (reading in your voice)"), license: "MIT / Apache-2.0", url: "https://github.com/ServeurpersoCom/qwentts.cpp" },
  { name: tx("Seed-VC (singing in your voice)"), license: "GPL-3.0", url: "https://github.com/Plachtaa/seed-vc" },
  { name: tx("BigVGAN, RMVPE, CAMPPlus and Whisper (parts of singing in your voice)"), license: "MIT / Apache-2.0", url: "" },
  { name: tx("Rust crates (serde, tokio, reqwest, rusqlite, sysinfo, zip, windows…)"), license: "MIT / Apache-2.0", url: "" },
];

function WebSearchSettings({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<WebSettings | null>(null);
  const [key, setKey] = useState("");
  useEffect(() => {
    api.webSettings().then(setS).catch(() => {});
  }, []);
  if (!s) return null;
  const save = async (next: WebSettings, braveKey: string | null = null) => {
    setS(next);
    try {
      await api.setWebSettings({ provider: next.provider, searxng_url: next.searxng_url, no_fallback: next.no_fallback }, braveKey);
      if (braveKey !== null) {
        setKey("");
        setS({ ...next, has_brave_key: braveKey !== "" });
      }
      toast(t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>{t("Web search")}</h3>
      <label>
        {t("Search with")}
        <select value={s.provider} onChange={(e) => save({ ...s, provider: e.target.value as SearchProvider })}>
          <option value="duckduckgo">{t("DuckDuckGo (no account needed)")}</option>
          <option value="brave">{t("Brave Search (your API key)")}</option>
          <option value="searxng">{t("SearXNG (your own server)")}</option>
        </select>
      </label>
      {s.provider === "duckduckgo" && (
        <>
          <p className="muted small">{t("Reads DuckDuckGo's plain results page. If searches get limited, switch to Brave or SearXNG.")}</p>
          <label className="check">
            <input type="checkbox" checked={!s.no_fallback} onChange={(e) => save({ ...s, no_fallback: !e.target.checked })} />{" "}
            {t("When DuckDuckGo turns searches away, try its full page in a hidden window, then Brave or Bing")}
          </label>
        </>
      )}
      {s.provider === "brave" && (
        <label>
          {t("Brave Search API key")} {s.has_brave_key && <span className="muted small">{t("(saved, encrypted)")}</span>}
          <span className="row">
            <input className="input" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={s.has_brave_key ? t("Enter a new key to replace it") : t("Paste your key")} autoComplete="off" />
            <button className="btn small" disabled={!key.trim()} onClick={() => save(s, key.trim())}>{t("Save key")}</button>
            {s.has_brave_key && <button className="btn ghost small danger" onClick={() => save(s, "")}>{t("Remove")}</button>}
          </span>
          <span className="muted small">{t("Free tier at brave.com/search/api.")}</span>
        </label>
      )}
      {s.provider === "searxng" && (
        <label>
          {t("SearXNG address")}
          <span className="row">
            <input className="input" value={s.searxng_url} onChange={(e) => setS({ ...s, searxng_url: e.target.value })} placeholder="https://search.example.com" />
            <button className="btn small" onClick={() => save(s)}>{t("Save")}</button>
          </span>
          <span className="muted small">{t("The server must allow JSON output (search.formats: json).")}</span>
        </label>
      )}
    </div>
  );
}

/** Keys the user gave for sites the assistant built (live prices and the like). */
function SiteKeys({ toast }: { toast: PushToast }) {
  const [keys, setKeys] = useState<SavedSiteKey[]>([]);
  useEffect(() => {
    api.siteKeys().then(setKeys).catch(() => {});
  }, []);
  if (keys.length === 0) return null;
  const forget = async (host: string) => {
    try {
      await api.siteKeyForget(host);
      setKeys((all) => all.filter((k) => k.host !== host));
      toast(t("Removed."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>{t("Keys for sites the assistant built")}</h3>
      <p className="muted small">{t("Saved encrypted on this PC. Each is sent only to its own service, from sites in the chats you allowed it for.")}</p>
      {keys.map((k) => (
        <div key={k.host} className="row">
          <span>
            <strong>{k.platform}</strong> <span className="muted small">{k.host}</span>
          </span>
          <span className="spacer" />
          <button className="btn ghost small danger" onClick={() => forget(k.host)}>{t("Remove")}</button>
        </div>
      ))}
    </div>
  );
}

function BrowserChoice({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<BrowsersView | null>(null);
  useEffect(() => {
    api.browsersView().then(setV).catch(() => {});
  }, []);
  if (!v) return null;
  const value = v.current.kind === "app" ? `app:${v.current.name}` : v.current.kind;
  const pick = async (val: string) => {
    const choice: MainBrowser = val.startsWith("app:") ? { kind: "app", name: val.slice(4) } : val === "system" ? { kind: "system" } : { kind: "builtin" };
    try {
      await api.setMainBrowser(choice);
      setV({ ...v, current: choice });
      window.dispatchEvent(new Event("browser-changed"));
      toast(t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="form web-search">
      <h3>{t("Main browser")}</h3>
      <label>
        {t("Links and the globe button open in")}
        <select value={value} onChange={(e) => pick(e.target.value)}>
          <option value="builtin">{t("SulcusAI's web viewer (inside this window)")}</option>
          <option value="system">{t("Windows' default browser")}</option>
          {v.installed.map((b) => (
            <option key={b.name} value={`app:${b.name}`}>{b.name}</option>
          ))}
        </select>
      </label>
      <p className="muted small">{t("Unless you pick one, it's Chrome (or Windows' default browser without Chrome). The web viewer has its own button next to the globe either way, and the assistant browses in a browser of its own. For DuckDuckGo or another browser not listed, make it your Windows default and pick “Windows' default browser”.")}</p>
      <label>
        {t("SulcusAI's own browser searches with")}
        <select
          value={v.search}
          onChange={async (e) => {
            const search = e.target.value as BrowserSearch;
            try {
              await api.setBrowserSearch(search);
              setV({ ...v, search });
              toast(t("Saved."), "success");
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        >
          <option value="duckduckgo">DuckDuckGo</option>
          <option value="google">Google</option>
          <option value="bing">Bing</option>
          <option value="brave">Brave Search</option>
        </select>
      </label>
      <p className="muted small">{t("The web viewer and the browser the assistant uses start on it, and what you type there that isn't an address is searched with it. To have the assistant work in your own Chrome instead, turn on Browser control in Features.")}</p>
    </div>
  );
}

interface Props {
  connectivity: Connectivity;
  onConnectivity: (l: Connectivity) => void;
  security: SecurityStatus;
  onSecurityChanged: () => void;
  features: Set<FeatureId>;
  toast: PushToast;
  /** A quick link was picked: scroll to that section (`at` makes a repeat count). */
  jump: { id: string; at: number } | null;
  /** The section being read, for the sidebar's highlight. */
  onSection: (id: string | null) => void;
}

interface Change {
  el: HTMLInputElement | HTMLSelectElement;
  prev: string | boolean;
  at: number;
}

type Place ={ kind: "home" } | { kind: "cat"; cat: CategoryId } | { kind: "page"; page: SpecialPage };

/** One existing section, shown only on its category's page. It stays in the
 * page while hidden so search can find it and bring its page up. */
function Block({ id, cat, adv, children }: { id: string; cat: CategoryId | null; adv: boolean; children: ReactNode }) {
  const where = SECTION_CATEGORY[id] ?? { cat: categoryOf(id) };
  const advanced = !!where.advanced;
  const shown = cat === where.cat && (!advanced || adv);
  const [explain, setExplain] = useState(false);
  return (
    <div className={`settings-block ${advanced ? "advanced" : ""}`} data-section={id} hidden={!shown}>
      <span id={`settings-${id}`} className="settings-anchor" />
      {WHATS_THIS[id] && (
        <div className="whats-this small">
          <button className="btn link small" aria-expanded={explain} onClick={() => setExplain(!explain)}>{t("What's this?")}</button>
          {explain && <span className="muted">{t(WHATS_THIS[id])}</span>}
        </div>
      )}
      {children}
    </div>
  );
}

export function SettingsView({ connectivity, onConnectivity, security, onSecurityChanged, features, toast: pushToast, jump, onSection }: Props) {
  const [place, setPlace] = useState<Place>({ kind: "home" });
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [openGuide, setOpenGuide] = useState<string | null>(null);
  const [savedAt, setSavedAt] = useState(0);
  const history = useRef<Place[]>([]);
  const cat = place.kind === "cat" ? place.cat : null;
  const advancedCount = cat ? Object.values(SECTION_CATEGORY).filter((s) => s.cat === cat && s.advanced).length : 0;
  const crumb = place.kind === "cat" ? t(CATEGORIES.find((c) => c.id === place.cat)!.label) : place.kind === "page" ? { health: t("Health"), "privacy-plain": t("Privacy in plain words"), helper: t("Settings helper") }[place.page] : null;

  // Every setting saves at once; a success also shows a quiet check by the
  // breadcrumbs, so a change made out of view of the toast is still confirmed.
  const toast: PushToast = (text, kind) => {
    if (kind === "success") {
      // A save right after a switch or choice changed: that change can be undone.
      const c = lastChange.current;
      undoable.current = c && Date.now() - c.at < 3000 ? c : null;
      setSavedAt(Date.now());
    }
    pushToast(text, kind);
  };
  useEffect(() => {
    if (!savedAt) return;
    // Long enough to reach the Undo button.
    const id = setTimeout(() => setSavedAt(0), undoable.current ? 8000 : 2500);
    return () => clearTimeout(id);
  }, [savedAt]);

  // Undo works on the page's controls themselves, so every switch and
  // choice in every existing section gets it without changes there.
  const lastChange = useRef<Change | null>(null);
  const undoable = useRef<Change | null>(null);
  const before = useRef(new WeakMap<HTMLElement, string | boolean>());
  const remember = (e: { target: EventTarget }) => {
    const el = e.target as HTMLElement;
    if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) before.current.set(el, el.checked);
    else if (el instanceof HTMLSelectElement) before.current.set(el, el.value);
  };
  const changed = (e: { target: EventTarget }) => {
    const el = e.target as HTMLElement;
    if (!before.current.has(el)) return;
    lastChange.current = { el: el as HTMLInputElement | HTMLSelectElement, prev: before.current.get(el)!, at: Date.now() };
    before.current.delete(el);
  };
  const undo = () => {
    const c = undoable.current;
    undoable.current = null;
    lastChange.current = null;
    setSavedAt(0);
    if (!c || !c.el.isConnected) return;
    if (c.el instanceof HTMLInputElement) {
      // A click runs the control's own handler, which saves the old value back.
      if (c.el.checked !== c.prev) c.el.click();
    } else {
      // React tracks the value; setting it through the prototype makes the change event count.
      const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set;
      set?.call(c.el, c.prev as string);
      c.el.dispatchEvent(new Event("change", { bubbles: true }));
    }
  };

  const move = (next: Place) => {
    setPlace((cur) => {
      history.current.push(cur);
      return next;
    });
    pageRef.current?.scrollTo({ top: 0 });
  };
  const back = () => setPlace(history.current.pop() ?? { kind: "home" });
  // Alt+Left and the mouse's back button, like a browser.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.altKey && e.key === "ArrowLeft") {
        e.preventDefault();
        back();
      }
    };
    const mouse = (e: MouseEvent) => {
      if (e.button === 3) {
        e.preventDefault();
        back();
      }
    };
    window.addEventListener("keydown", key);
    window.addEventListener("mouseup", mouse);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("mouseup", mouse);
    };
  }, []);

  /** Brings up the page a section lives on, opening Advanced if it is there. */
  const showSection = (id: string) => {
    const where = SECTION_CATEGORY[id] ?? { cat: categoryOf(id) };
    if (where.advanced) setShowAdvanced(true);
    if (place.kind !== "cat" || place.cat !== where.cat) move({ kind: "cat", cat: where.cat });
  };
  /** Go to a section ("cat:<id>" for a whole category), optionally opening a guide. */
  const goTo = (target: string, guide?: string) => {
    if (target.startsWith("cat:")) return move({ kind: "cat", cat: target.slice(4) as CategoryId });
    setOpenGuide(guide ?? null);
    showSection(target);
    setTimeout(() => {
      const el = guide ? document.getElementById(`guide-${guide}`) : document.getElementById(`settings-${target}`);
      unfold(document.getElementById(`settings-${target}`)?.nextElementSibling);
      el?.scrollIntoView({ behavior: "smooth", block: "start" });
    }, 60);
  };
  const reveal = (el: HTMLElement) => {
    const id = el.closest<HTMLElement>(".settings-block")?.dataset.section;
    if (id) showSection(id);
  };

  const [locating, setLocating] = useState(false);
  const [language, setLanguageChoice] = useState(chosenLanguage());
  const [profile, setProfile] = useState<Profile | null>(null);
  const [saved, setSaved] = useState<Profile | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [confirmCloud, setConfirmCloud] = useState(false);

  useEffect(() => {
    api.profile().then((p) => {
      setProfile(p);
      setSaved(p);
    });
    api.appInfo().then(setInfo);
  }, []);

  const dirty = JSON.stringify(profile) !== JSON.stringify(saved);
  const save = async () => {
    if (!profile) return;
    try {
      await api.setProfile(profile);
      setSaved(profile);
      toast(t("Saved. New messages will use your profile."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const pick = (l: Connectivity) => {
    if (l === "cloud" && connectivity !== "cloud") setConfirmCloud(true);
    else onConnectivity(l);
  };

  // The section at the top of the page (the last anchor scrolled past).
  // After a quick link, the picked section stays marked until the person
  // scrolls themselves (a section near the end can't reach the top).
  const pageRef = useRef<HTMLDivElement>(null);
  // Each card folds by its heading.
  useCardFolds(pageRef);
  const held = useRef(false);
  const release = () => {
    held.current = false;
  };
  const spy = () => {
    const page = pageRef.current;
    if (!page || held.current) return;
    // Sections of other categories are in the page but not shown.
    const anchors = [...page.querySelectorAll<HTMLElement>(".settings-anchor")].filter((a) => !a.closest("[hidden]"));
    const top = page.getBoundingClientRect().top + 80;
    let current = anchors[0];
    for (const a of anchors) if (a.getBoundingClientRect().top <= top) current = a;
    if (page.scrollTop + page.clientHeight >= page.scrollHeight - 4) current = anchors[anchors.length - 1];
    onSection(current ? current.id.replace("settings-", "") : null);
  };
  useEffect(() => {
    spy();
    return () => onSection(null);
  }, []);

  // Quick links: smooth while the page is settled; on a fresh open, cards
  // load and grow above the target, so it lands again a moment later.
  const opened = useRef(Date.now());
  useEffect(() => {
    if (!jump) return;
    held.current = true;
    onSection(jump.id);
    showSection(jump.id);
    // A folded card opens when its quick link is picked.
    unfold(document.getElementById(`settings-${jump.id}`)?.nextElementSibling);
    const go = (behavior: ScrollBehavior) => document.getElementById(`settings-${jump.id}`)?.scrollIntoView({ behavior, block: "start" });
    if (Date.now() - opened.current > 1500) {
      // After the section's page has rendered.
      requestAnimationFrame(() => go("smooth"));
      // Lands anyway if the animation didn't run (a window not being drawn).
      const id = setTimeout(() => {
        const page = pageRef.current;
        const el = document.getElementById(`settings-${jump.id}`);
        if (page && el && Math.abs(el.getBoundingClientRect().top - page.getBoundingClientRect().top) > 40) go("auto");
      }, 900);
      return () => clearTimeout(id);
    }
    go("auto");
    const timers = [250, 700].map((ms) => setTimeout(() => go("auto"), ms));
    return () => timers.forEach(clearTimeout);
  }, [jump]);

  return (
    <div className="page" ref={pageRef} onPointerDownCapture={remember} onKeyDownCapture={remember} onChangeCapture={changed} onScroll={spy}onWheel={release} onPointerDown={release} onKeyDown={release} onTouchStart={release}>
      <div className="narrow">
      <nav className="settings-crumbs small" aria-label={t("Where you are")}>
        {place.kind !== "home" && (
          <button className="btn ghost small" onClick={back} title={t("Back (Alt+Left)")}>
            ← {t("Back")}
          </button>
        )}
        <button className="btn link small" onClick={() => setPlace({ kind: "home" })}>{t("Settings")}</button>
        {crumb && (
          <>
            <span className="muted" aria-hidden> › </span>
            <span aria-current="page">{crumb}</span>
          </>
        )}
        {savedAt > 0 && (
          <span key={savedAt} className={`settings-saved small ${undoable.current ? "with-undo" : ""}`} role="status">
            ✓ {t("Saved")}
            {undoable.current && (
              <button className="btn link small" onClick={undo}>
                {t("Undo")}
              </button>
            )}
          </span>
        )}
      </nav>
      <header className="page-head settings-head">
        <h1>{crumb ?? t("Settings")}</h1>
        <SettingsSearch pageRef={pageRef} onReveal={reveal} extra={GUIDES.map((g) => ({ text: t(g.title), section: t("Step-by-step setup"), go: () => goTo(g.section, g.id) }))} />
        <span className="row settings-folds" hidden={place.kind !== "cat"}>
          <button className="btn ghost small" onClick={() => foldAll(pageRef.current, false)} title={t("Open every section")}>
            {t("Expand all")}
          </button>
          <button className="btn ghost small" onClick={() => foldAll(pageRef.current, true)} title={t("Fold every section to its heading")}>
            {t("Collapse all")}
          </button>
        </span>
      </header>

      {place.kind === "home" && <SettingsHome onCategory={(c) => setPlace({ kind: "cat", cat: c })} onPage={(p) => setPlace({ kind: "page", page: p })} goTo={goTo} />}
      {place.kind === "page" && place.page === "health" && <HealthPage toast={toast} goTo={goTo} />}
      {place.kind === "page" && place.page === "privacy-plain" && <PrivacyPlain goTo={goTo} />}
      {place.kind === "page" && place.page === "helper" && <SettingsHelper toast={toast} goTo={goTo} features={features} />}
      {place.kind === "cat" && (
        <div className="settings-cat-head">
          <p className="muted">{t(CATEGORIES.find((c) => c.id === place.cat)!.sentence)}</p>
          {guidesFor(place.cat).length > 0 && (
            <section className="card setup-guides">
              <h2>{t("Step-by-step setup")}</h2>
              {guidesFor(place.cat).map((g) => (
                <SetupGuide key={g.id} guide={g} toast={toast} open={openGuide === g.id} />
              ))}
            </section>
          )}
        </div>
      )}

      <div className={`settings-blocks ${showAdvanced ? "with-advanced" : ""}`}>
      {place.kind === "cat" && advancedCount > 0 && (
        <button className="btn ghost settings-adv-toggle" aria-expanded={showAdvanced} onClick={() => setShowAdvanced(!showAdvanced)}>
          {showAdvanced ? t("Hide advanced options") : t("Advanced options ({n})", { n: advancedCount })}
        </button>
      )}
      <Block id="appearance" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("Appearance")}</h2>
          <ThemePicker />
          <LayoutAssistant />
          <WidgetsSettings toast={toast} />
        </section>
      </Block>

      <Block id="language" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("Language")}</h2>
          <label className="form small">
            {t("The app's language")}
            <select
              className="input"
              value={language}
              onChange={(e) => {
                setLanguageChoice(e.target.value);
                setLanguage(e.target.value);
              }}
            >
              <option value="system">{t("Same as Windows")}</option>
              {LANGUAGES.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.name}
                </option>
              ))}
            </select>
          </label>
          {MACHINE_MADE.has(language === "system" ? currentLanguage() : language) && (
            <p className="muted small">{t("This language was translated by a language model and hasn't been checked by a native speaker yet, so some wording may be off.")}</p>
          )}
          <p className="muted small">{t("The assistant answers in the language you write in, whatever this is set to.")}</p>
        </section>
      </Block>

      <Block id="chats" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("Chats")}</h2>
          <KeepDraftsRow />
          <ShowTokensRow />
        </section>
      </Block>

      <Block id="about-you" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("About you")}</h2>
          <p className="muted small">{t("Every chat includes this, so you don't have to repeat yourself. It's encrypted on this PC.")}</p>
          {profile && (
            <div className="form">
              <label>
                {t("Name")}
                <input className="input" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} placeholder={t("What should it call you?")} aria-label={t("What should it call you?")} maxLength={80} />
              </label>
              <label>
                {t("About you")}
                <textarea className="input" rows={4} value={profile.about} onChange={(e) => setProfile({ ...profile, about: e.target.value })} placeholder={t("Your work, interests, or anything that helps it understand you.")} aria-label={t("Your work, interests, or anything that helps it understand you.")} maxLength={4000} />
              </label>
              <label>
                {t("Where you are")}
                <span className="row">
                  <input
                    className="input"
                    value={profile.location ?? ""}
                    onChange={(e) => setProfile({ ...profile, location: e.target.value, lat: null, lon: null })}
                    placeholder={t("Your city, e.g. Orem, Utah")} aria-label={t("Your city, e.g. Orem, Utah")}
                    maxLength={120}
                  />
                  <button
                    className="btn small"
                    disabled={locating}
                    onClick={async () => {
                      setLocating(true);
                      try {
                        const p = await api.detectLocation();
                        setProfile({ ...profile, location: p.label, lat: p.lat, lon: p.lon });
                      } catch (e) {
                        toast(errorText(e), "error");
                      } finally {
                        setLocating(false);
                      }
                    }}
                  >
                    {locating ? t("Finding…") : t("Use this PC's location")}
                  </button>
                </span>
                <span className="muted small">{t("For the weather and local questions. Kept on this PC; it goes out only with a weather lookup. Leave it empty and the assistant asks Windows when it needs to.")}</span>
              </label>
              <label>
                {t("How it should respond")}
                <textarea className="input" rows={3} value={profile.preferences} onChange={(e) => setProfile({ ...profile, preferences: e.target.value })} placeholder={t("For example: keep answers short, use plain language, give examples.")} aria-label={t("For example: keep answers short, use plain language, give examples.")} maxLength={4000} />
              </label>
              <div>
                <button className="btn primary" onClick={save} disabled={!dirty}>
                  {t("Save")}
                </button>
              </div>
            </div>
          )}
        </section>
      </Block>

      <Block id="security" cat={cat} adv={showAdvanced}>
        <SecuritySection status={security} onChanged={onSecurityChanged} toast={toast} />
        <SecurityPage toast={toast} />
        <VaultPage toast={toast} />
      </Block>

      <Block id="folders" cat={cat} adv={showAdvanced}>
        {features.has("files") && <FoldersSection toast={toast} />}
      </Block>

      <Block id="low-tokens" cat={cat} adv={showAdvanced}>
        <LowTokenSettings toast={toast} />
      </Block>

      <Block id="handoffs" cat={cat} adv={showAdvanced}>
        {features.has("files") && <HandoffSettings toast={toast} />}
      </Block>

      <Block id="permissions" cat={cat} adv={showAdvanced}>
        <PermissionRules toast={toast} />
      </Block>

      <Block id="voice" cat={cat} adv={showAdvanced}>
        {VOICE_FEATURES.some((f) => features.has(f)) && <VoiceSection toast={toast} />}
        {features.has("voice_chat") && <WakeWord toast={toast} />}
      </Block>

      <Block id="performance" cat={cat} adv={showAdvanced}>
        <PerformanceSection toast={toast} />
      </Block>

      <Block id="pause" cat={cat} adv={showAdvanced}>
        <QuietSection toast={toast} />
      </Block>

      <Block id="check" cat={cat} adv={showAdvanced}>
        <PcCheck />
      </Block>

      <Block id="servers" cat={cat} adv={showAdvanced}>
        <PortsSettings toast={toast} />
      </Block>

      <Block id="storage" cat={cat} adv={showAdvanced}>
        <ModelStorage toast={toast} />
      </Block>

      <Block id="cleanup" cat={cat} adv={showAdvanced}>
        <CleanupSettings toast={toast} />
      </Block>

      <Block id="sign-in" cat={cat} adv={showAdvanced}>
        {(features.has("email") || features.has("calendar")) && <SignInApps toast={toast} />}
        <CloudFiles toast={toast} />
      </Block>

      <Block id="connectivity" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("Connectivity")}</h2>
          <p className="muted small">{t("Controls what may leave this PC. You can also switch it from the status bar at the bottom.")}</p>
          <div className="levels" role="radiogroup" aria-label={t("Connectivity level")}>
            {LEVELS.map((l) => (
              <button key={l.id} role="radio" aria-checked={connectivity === l.id} className={`level ${connectivity === l.id ? "active" : ""}`} onClick={() => pick(l.id)}>
                <span>
                  <strong>{t(l.label)}</strong>
                  {l.id === "offline" && <span className="badge subtle">{t("Default")}</span>}
                  <span className="small muted block">{t(l.detail)}</span>
                </span>
              </button>
            ))}
          </div>
          <p className="muted small">{t("Tip: while Offline, the Web button in a chat (or Ctrl+Shift+W) turns web on for just that chat.")}</p>
          <WebSearchSettings toast={toast} />
          <SiteKeys toast={toast} />
          <BrowserChoice toast={toast} />
        </section>
      </Block>

      <Block id="private" cat={cat} adv={showAdvanced}>
        <PrivacySettings toast={toast} />
      </Block>

      <Block id="privacy-mode" cat={cat} adv={showAdvanced}>
        <PrivacyModeSettings toast={toast} />
      </Block>

      <Block id="sent" cat={cat} adv={showAdvanced}>
        <SentLog toast={toast} />
      </Block>

      <Block id="network" cat={cat} adv={showAdvanced}>
        <NetworkSettings toast={toast} />
      </Block>

      <Block id="devices" cat={cat} adv={showAdvanced}>
        <SyncSettings toast={toast} />
      </Block>

      <Block id="phone" cat={cat} adv={showAdvanced}>
        <PhoneSettings toast={toast} />
        <TelegramSettings toast={toast} />
        <MatrixSettings toast={toast} />
        <DiscordSettings toast={toast} />
      </Block>

      <Block id="backup" cat={cat} adv={showAdvanced}>
        <BackupSettings toast={toast} />
        <FindSettings toast={toast} />
        <DocumentTools toast={toast} />
        <ImportFromOthers toast={toast} />
      </Block>

      <Block id="your-data" cat={cat} adv={showAdvanced}>
        <YourDataSettings toast={toast} />
      </Block>

      <Block id="cloud" cat={cat} adv={showAdvanced}>
        <CloudSettings toast={toast} cloudOn={connectivity === "cloud"} />
      </Block>

      <Block id="market" cat={cat} adv={showAdvanced}>
        <MarketSettings toast={toast} />
      </Block>

      <Block id="api" cat={cat} adv={showAdvanced}>
        <ApiServerSettings toast={toast} />
      </Block>

      <Block id="advanced" cat={cat} adv={showAdvanced}>
        <AdvancedSettings toast={toast} />
      </Block>

      <Block id="logs" cat={cat} adv={showAdvanced}>
        <LogSettings toast={toast} />
      </Block>

      <Block id="about" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("About {app}", { app: APP_NAME })}</h2>
          <UpdateSettings toast={toast} />
          <CrashReportsSetting toast={toast} />
          <DevModeSetting toast={toast} />
          {info && (
            <p className="small muted row wrap">
              <span>{t("Data folder: {path}", { path: info.data_dir })}</span>
              <button className="btn ghost small" onClick={() => api.openAppFolder(true).catch((e) => toast(errorText(e), "error"))}>{t("Open")}</button>
              <button className="btn ghost small" onClick={() => api.openAppFolder(false).catch((e) => toast(errorText(e), "error"))}>{t("Open logs folder")}</button>
            </p>
          )}
          <p className="small">
            {t("{app} is free software under the GNU Affero General Public License v3.0. The name and logo are not covered by that license.", { app: APP_NAME })}
          </p>
          <h3>{t("Open-source components")}</h3>
          <ul className="licenses small">
            {THIRD_PARTY.map((c) => (
              <li key={c.name}>
                <strong>{t(c.name)}</strong> <span className="muted">({t(c.license)})</span>
              </li>
            ))}
          </ul>
          <p className="muted small">{t("AI models have their own licenses, shown on each model card. Full notices: THIRD_PARTY_NOTICES.md.")}</p>
        </section>
      </Block>

      <Block id="update-notes" cat={cat} adv={showAdvanced}>
        <section className="card">
          <h2>{t("Update notes")}</h2>
          <p className="muted small">{t("What each version of {app} changed, newest first. Click a version to open or close it.", { app: APP_NAME })}</p>
          <UpdateNotes />
        </section>
      </Block>
      </div>

      {confirmCloud && (
        <CloudConfirm
          onCancel={() => setConfirmCloud(false)}
          onConfirm={() => {
            setConfirmCloud(false);
            onConnectivity("cloud");
          }}
        />
      )}
      </div>
    </div>
  );
}

/** Messages typed and not sent: kept while SulcusAI runs; this keeps them after a restart as well. */
function ShowTokensRow() {
  const on = useTokensShown();
  return (
    <div className="toggle-row">
      <div>
        <strong>{t("Show token counts")}</strong>
        <span className="small muted block">{t("Tokens beside each message, under each reply and for the whole chat.")}</span>
      </div>
      <label className="switch" title={on ? t("On") : t("Off")}>
        <input type="checkbox" checked={on} aria-label={t("Show token counts")} onChange={(e) => setTokensShown(e.target.checked)} />
        <span />
      </label>
    </div>
  );
}

function KeepDraftsRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    api.draftsKeep().then(setOn).catch(() => setOn(false));
  }, []);
  if (on === null) return null;
  return (
    <div className="toggle-row">
      <div>
        <strong>{t("Keep unsent messages after a restart")}</strong>
        <span className="small muted block">
          {t("What you type in a chat stays there when you switch chats or pages. With this on, it's also kept when SulcusAI closes, saved encrypted on this PC until you send or clear it.")}
        </span>
      </div>
      <label className="switch" title={on ? t("On") : t("Off")}>
        <input
          type="checkbox"
          checked={on}
          aria-label={t("Keep unsent messages after a restart")}
          onChange={async (e) => {
            const next = e.target.checked;
            setOn(next);
            await api.setDraftsKeep(next).catch(() => {});
            keepDrafts(next);
          }}
        />
        <span />
      </label>
    </div>
  );
}
