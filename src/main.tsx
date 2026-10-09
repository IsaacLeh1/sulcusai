// SPDX-License-Identifier: AGPL-3.0-only
import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CrashScreen } from "./components/CrashScreen";
import { initLanguage, useLanguage } from "./i18n";
import "./styles.css";

// The quick-ask window loads only what it needs.
const App = lazy(() => import("./App"));
const QuickAsk = lazy(() => import("./views/QuickAsk").then((m) => ({ default: m.QuickAsk })));

// The same page runs the quick-ask box, in its own small window.
const quick = getCurrentWindow().label === "quick";
document.documentElement.classList.toggle("quick-window", quick);

// Changing the language redraws everything in it.
function Root() {
  const lang = useLanguage();
  return <Suspense fallback={null}>{quick ? <QuickAsk key={lang} /> : <App key={lang} />}</Suspense>;
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
