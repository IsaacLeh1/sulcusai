// SPDX-License-Identifier: AGPL-3.0-only
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import { QuickAsk } from "./views/QuickAsk";
import "./styles.css";

// The same page runs the quick-ask box, in its own small window.
const quick = getCurrentWindow().label === "quick";
document.documentElement.classList.toggle("quick-window", quick);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {quick ? <QuickAsk /> : <App />}
  </StrictMode>,
);
