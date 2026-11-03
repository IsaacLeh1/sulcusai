// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState, type ReactNode } from "react";
import { errorText } from "../api";
import { t } from "../i18n";
import { consentApi, documentLink, documentText, DOCUMENT_TITLES, dueAtStart, isUpdate, type ConsentStatus } from "./ConsentApi";
import { Markdown } from "./Markdown";

/** The points that matter most, in front of the full text. */
export const KEY_POINTS = [
  "SulcusAI runs on your PC. Its developer hosts nothing that carries your data, and can't see, moderate or stop anything you do with it.",
  "You're responsible for how you use it: your chats, models, groups, networks, files and anything you run for others.",
  "AI can be wrong. Its answers aren't medical, legal, financial or other professional advice.",
  "Use real people's faces and voices only with their consent, and follow the law where you are.",
];

/** The acceptance screen at install or first start, and again whenever a
 * document's version changes. Nothing else opens until it's accepted. */
export function ConsentScreen({ due, onAccepted, onDecline }: { due: ConsentStatus[]; onAccepted: (all: ConsentStatus[]) => void; onDecline?: () => void }) {
  const [agreed, setAgreed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reading, setReading] = useState<{ id: string; text: string | null } | null>(null);
  const updated = due.length > 0 && due.every(isUpdate);

  const read = async (d: ConsentStatus) => {
    if (reading?.id === d.id) return setReading(null);
    setReading({ id: d.id, text: null });
    const text = d.file ? await documentText(d.file).catch(() => null) : null;
    setReading({ id: d.id, text: text ?? t("This copy of the app doesn't include the text. Read it online instead.") });
  };

  const accept = async () => {
    setBusy(true);
    setError(null);
    try {
      onAccepted(await consentApi.accept(due.map((d) => ({ id: d.id, version: d.version }))));
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <div className="onboarding">
      <div className="onboarding-card" role="dialog" aria-labelledby="consent-title">
        <h1 id="consent-title">{updated ? t("The terms have changed") : t("Before you start")}</h1>
        <p className="small">
          {updated ? t("Please read what changed and accept the new versions to keep using SulcusAI.") : t("SulcusAI is free, open-source software. Please read and accept these first.")}
        </p>
        <ul className="small">
          {KEY_POINTS.map((p) => (
            <li key={p}>{t(p)}</li>
          ))}
        </ul>
        <div className="form">
          {due.map((d) => (
            <div key={d.id}>
              <strong>{t(DOCUMENT_TITLES[d.id] ?? d.id)}</strong>{" "}
              <span className="muted small">{t("Version {n}", { n: d.version })}</span>{" "}
              <button className="btn small" onClick={() => read(d)} aria-expanded={reading?.id === d.id}>
                {reading?.id === d.id ? t("Hide") : t("Read")}
              </button>{" "}
              {d.file && (
                <a className="small" href={documentLink(d.file)} target="_blank" rel="noreferrer">
                  {t("Open online")}
                </a>
              )}
              {reading?.id === d.id && (
                <div className="card" style={{ maxHeight: "40vh", overflowY: "auto", marginTop: 8 }}>
                  {reading.text === null ? <p className="muted small">{t("Loading…")}</p> : <Markdown text={reading.text} />}
                </div>
              )}
            </div>
          ))}
        </div>
        <label className="check">
          <input type="checkbox" checked={agreed} onChange={(e) => setAgreed(e.target.checked)} />
          <span className="small">{t("I've read and agree to these. I'm at least 13, and if I'm under 18, a parent or guardian agrees for me.")}</span>
        </label>
        {error && <p className="warn-text small">{error}</p>}
        <div className="onboarding-nav">
          {onDecline ? (
            <button className="btn" onClick={onDecline} disabled={busy}>
              {t("I don't agree (close SulcusAI)")}
            </button>
          ) : (
            <span />
          )}
          <button className="btn primary" onClick={accept} disabled={!agreed || busy}>
            {busy ? t("Saving…") : t("I agree")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Wraps the app: shows the acceptance screen while any start document is
 * due, and the app once they're all accepted. */
export function ConsentGate({ children, onDecline }: { children: ReactNode; onDecline?: () => void }) {
  const [all, setAll] = useState<ConsentStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let live = true;
    consentApi
      .status()
      .then((s) => live && setAll(s))
      .catch((e) => live && setError(errorText(e)));
    return () => {
      live = false;
    };
  }, [attempt]);
  if (error) {
    return (
      <div className="onboarding">
        <div className="onboarding-card">
          <p className="warn-text small">{t("Couldn't check which terms you've accepted: {error}", { error })}</p>
          <button
            className="btn primary"
            onClick={() => {
              setError(null);
              setAttempt((n) => n + 1);
            }}
          >
            {t("Try again")}
          </button>
        </div>
      </div>
    );
  }
  if (!all) return null;
  const due = dueAtStart(all);
  if (due.length > 0) return <ConsentScreen due={due} onAccepted={setAll} onDecline={onDecline} />;
  return <>{children}</>;
}
