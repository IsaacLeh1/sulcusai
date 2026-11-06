// SPDX-License-Identifier: AGPL-3.0-only
import { lazy, StrictMode, Suspense, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CrashScreen } from "./components/CrashScreen";
import { CallProvider } from "./views/groups/CallProvider";
import { initLanguage, useLanguage } from "./i18n";
import { playing, trackAudioContexts } from "./playing";
import { initTheme } from "./theme";
import { initLayout } from "./workspace/layout";
import "./styles.css";
import "./styles-more.css";

// The quick-ask window loads only what it needs.
const App = lazy(() => import("./App"));
// The terms are asked for in the main window only; the other windows open from it.
const ConsentGate = lazy(() => import("./components/ConsentScreen").then((m) => ({ default: m.ConsentGate })));
const QuickAsk = lazy(() => import("./views/QuickAsk").then((m) => ({ default: m.QuickAsk })));
const PeopleWindow = lazy(() => import("./views/PeopleWindow"));
const LanWindow = lazy(() => import("./views/LanWindow"));
const SideWindow = lazy(() => import("./views/SideWindow"));
const WorkingBanner = lazy(() => import("./views/WorkingBanner"));
const RemoteFrame = lazy(() => import("./components/RemoteFrame"));

// The same page runs the quick-ask box, in its own small window, People in
// a window of its own, and chats, the Studio and song studio projects in
// windows of their own.
const label = getCurrentWindow().label;
const quick = label === "quick";
const people = label === "people" || label === "lan";
const side = /^(chat|studio|songstudio)-/.test(label);
// The strip shown while the assistant works another program.
const working = label === "working";
// The border and Stop bar shown round the screen while it is controlled.
const remoteFrame = label === "remote-border" || label === "remote-bar";
document.documentElement.classList.toggle("quick-window", quick);
initTheme();
initLayout();

// For crash reports (kept only if turned on): errors the window didn't catch,
// and that the window is still responding.
if (!quick && !working && !remoteFrame) {
  window.addEventListener("error", (e) => {
    import("./api").then(({ api }) => api.reportWindowError(String(e.message), e.error?.stack ?? null).catch(() => {}));
  });
  window.addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    // A plain message from SulcusAI's core (an action that couldn't be done,
    // such as deleting a note that's gone): shown to the person, not kept as
    // a crash. A real error in the window's code is still reported.
    if (typeof r === "string") {
      e.preventDefault();
      window.dispatchEvent(new CustomEvent("sulcus:error", { detail: r }));
      return;
    }
    import("./api").then(({ api }) => api.reportWindowError(String(r?.message ?? r), r?.stack ?? null).catch(() => {}));
  });
  // The main window's: another window answering doesn't mean it does.
  // Only while it shows: a hidden window has no hang to notice, and its
  // timers would wake the PC for nothing.
  if (!people && !side) {
    trackAudioContexts();
    const ping = (visible = true) => import("./api").then(({ api }) => api.crashPing(visible).catch(() => {}));
    let timer: number | undefined;
    const follow = () => {
      window.clearInterval(timer);
      timer = undefined;
      if (document.hidden) {
        ping(false);
      } else {
        ping();
        timer = window.setInterval(ping, 30_000);
      }
    };
    follow();
    document.addEventListener("visibilitychange", follow);
    // A minute after hiding, with nothing playing, the core suspends this
    // window and trims its memory (rest.rs). An event from the core wakes it
    // briefly ("resume"); it rests again a minute later.
    let rest: number | undefined;
    const armRest = () => {
      window.clearTimeout(rest);
      rest = document.hidden ? window.setTimeout(restNow, 60_000) : undefined;
    };
    const restNow = () => {
      if (!document.hidden || label !== "main") return;
      if (playing()) return armRest();
      import("./api").then(({ api }) => api.windowRest().catch(() => {}));
    };
    document.addEventListener("visibilitychange", armRest);
    document.addEventListener("resume", armRest);
  }
}

/** The light social app keeps its hidden main window empty until the full app is opened from it. */
function useLightStart(): boolean {
  const [light, setLight] = useState(label === "main");
  useEffect(() => {
    if (label !== "main") return;
    let alive = true;
    const check = () =>
      import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke<boolean>("parts_light"))
        .then((l) => {
          if (!alive) return;
          setLight(l);
          if (l) window.setTimeout(check, 1500);
        })
        .catch(() => alive && setLight(false));
    check();
    return () => {
      alive = false;
    };
  }, []);
  return light;
}

// Changing the language redraws the window where it is (the new elements
// re-render everything below them).
function Root() {
  useLanguage();
  const light = useLightStart();
  const main = light ? null : (
    <ConsentGate onDecline={() => import("./components/ConsentApi").then(({ consentApi }) => consentApi.decline())}>
      <App />
    </ConsentGate>
  );
  return <Suspense fallback={null}>{remoteFrame ? <RemoteFrame label={label} /> : quick ? <QuickAsk /> : working ? <WorkingBanner /> : label === "lan" ? <LanWindow /> : people ? <CallProvider><PeopleWindow /></CallProvider> : side ? <SideWindow label={label} /> : <CallProvider>{main}</CallProvider>}</Suspense>;
}

initLanguage().then(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <CrashScreen>
        <Root />
      </CrashScreen>
    </StrictMode>,
  ),
);
