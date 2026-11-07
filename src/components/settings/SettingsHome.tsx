// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { t } from "../../i18n";
import { CATEGORIES, type CategoryId } from "./catalog";

export type SpecialPage = "health" | "privacy-plain" | "helper";

const FIRST_RUN_KEY = "settings.firstRunDone";

function firstRunDone(): boolean {
  try {
    return localStorage.getItem(FIRST_RUN_KEY) === "1";
  } catch {
    return true;
  }
}

/** The categories screen: one card per category with a sentence, plus the
 * helper, health and privacy pages, and a short guide the first time. */
export function SettingsHome({ onCategory, onPage, goTo }: { onCategory: (c: CategoryId) => void; onPage: (p: SpecialPage) => void; goTo: (section: string) => void }) {
  const [showFirst, setShowFirst] = useState(!firstRunDone());
  const closeFirst = () => {
    setShowFirst(false);
    try {
      localStorage.setItem(FIRST_RUN_KEY, "1");
    } catch {
      // Shown again next time; nothing lost.
    }
  };
  const first: { text: string; section: string }[] = [
    { text: t("Pick a theme you like"), section: "appearance" },
    { text: t("Tell the assistant your name and what you do"), section: "about-you" },
    { text: t("See what this PC can run"), section: "check" },
    { text: t("Decide whether the app may go online"), section: "connectivity" },
  ];
  return (
    <>
      {showFirst && (
        <section className="card first-run">
          <h2>{t("Getting started")}</h2>
          <p className="muted small">{t("Four short steps. Everything else can wait until you need it.")}</p>
          <ol className="first-run-steps">
            {first.map((f) => (
              <li key={f.section}>
                <button className="btn link" onClick={() => goTo(f.section)}>{f.text}</button>
              </li>
            ))}
          </ol>
          <button className="btn ghost small" onClick={closeFirst}>{t("Done, hide this")}</button>
        </section>
      )}
      <div className="settings-cats">
        {CATEGORIES.map((c) => (
          <button key={c.id} className="settings-cat card" onClick={() => onCategory(c.id)}>
            <strong>{t(c.label)}</strong>
            <span className="muted small">{t(c.sentence)}</span>
          </button>
        ))}
        <button className="settings-cat card" onClick={() => onPage("helper")}>
          <strong>{t("Settings helper")}</strong>
          <span className="muted small">{t("Say what you want to set up; it walks you through and asks before each change.")}</span>
        </button>
        <button className="settings-cat card" onClick={() => onPage("health")}>
          <strong>{t("Health")}</strong>
          <span className="muted small">{t("Checks the microphone, camera, network, relay, Tor, graphics card and disk, with Fix buttons.")}</span>
        </button>
        <button className="settings-cat card" onClick={() => onPage("privacy-plain")}>
          <strong>{t("Privacy in plain words")}</strong>
          <span className="muted small">{t("What stays on this PC, what can leave and who decides.")}</span>
        </button>
      </div>
    </>
  );
}
