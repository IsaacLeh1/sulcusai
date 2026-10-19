// SPDX-License-Identifier: AGPL-3.0-only
// A model being tried out on this PC (the last step of installing, or a
// speed test): it uses the graphics card hard for a moment, so say what's
// happening, how long it's been, and that it stops by itself. The ticking
// clock shows it's alive even with Windows' animations turned off.
import { useEffect, useRef, useState } from "react";
import { clock } from "../format";
import { t } from "../i18n";

/** The steps of trying a model out, as install progress reports them. */
export const TESTING = new Set(["loading", "benchmark", "optimizing"]);

/** When the current test started: resets when `testing` turns on again. */
export function useTestStart(testing: boolean): number {
  const start = useRef(Date.now());
  const was = useRef(false);
  if (testing && !was.current) start.current = Date.now();
  was.current = testing;
  return start.current;
}

export function TestingNote({ since, short = false }: { since: number; short?: boolean }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  const time = clock((now - since) / 1000);
  return (
    <p className="muted small testing-note" role="status">
      <span className="spinner" aria-hidden /> <span className="testing-time">{t("{time} so far", { time })}</span>
      {" · "}
      {short
        ? t("This may take a minute; it stops by itself.")
        : t("This may take a minute. SulcusAI runs the model once to see how fast it is on this PC, so the graphics card works hard for a moment. It stops by itself.")}
    </p>
  );
}
