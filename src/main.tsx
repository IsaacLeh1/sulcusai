// SPDX-License-Identifier: AGPL-3.0-only
import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CrashScreen } from "./components/CrashScreen";
import { initLanguage, useLanguage } from "./i18n";
import { initTheme } from "./theme";
import "./styles.css";

// The quick-ask window loads only what it needs.
const App = lazy(() => import("./App"));
const QuickAsk = lazy(() => import("./views/QuickAsk").then((m) => ({ default: m.QuickAsk })));

// The same page runs the quick-ask box, in its own small window.
const quick = getCurrentWindow().label === "quick";
document.documentElement.classList.toggle("quick-window", quick);
initTheme();

// For crash reports (kept only if turned on): errors the window didn't catch,
// and that the window is still responding.
if (!quick) {
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
  const ping = () => import("./api").then(({ api }) => api.crashPing().catch(() => {}));
  ping();
  setInterval(ping, 30_000);
}

// Changing the language redraws the window where it is (the new elements
// re-render everything below them).
function Root() {
  useLanguage();
  return <Suspense fallback={null}>{quick ? <QuickAsk /> : <App />}</Suspense>;
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
