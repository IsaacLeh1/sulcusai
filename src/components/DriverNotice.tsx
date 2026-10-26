// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, type DriverNote } from "../api";
import { t } from "../i18n";

/** Once a run, shortly after SulcusAI starts: graphics drivers that need
 * updating, each with why and where to get it. Nothing when they're fine.
 * "Not now" isn't asked about again for that driver. */
export function DriverNotice() {
  const [notes, setNotes] = useState<DriverNote[]>([]);
  useEffect(() => {
    // After the window has settled, so it doesn't slow the start.
    const timer = window.setTimeout(() => {
      api.driverCheck().then((n) => setNotes(n ?? [])).catch(() => {});
    }, 8000);
    return () => clearTimeout(timer);
  }, []);
  const n = notes[0];
  if (!n) return null;
  const putOff = () => {
    setNotes((cur) => cur.slice(1));
    api.driverPutOff(n.key, n.need).catch(() => {});
  };
  return (
    <div className="driver-notice" role="status">
      <div className="grow">
        <strong>
          {n.need === "missing" ? t("{card} needs its driver", { card: n.card }) : t("Update the driver for {card}", { card: n.card })}
        </strong>
        <span className="small muted block">
          {t(n.why)}.{n.need === "old" && n.version ? ` ${t("Installed: {version}", { version: n.version })}.` : ""}
        </span>
      </div>
      <a className="btn small primary" href={n.link} target="_blank" rel="noreferrer">
        {t("Get the driver")}
      </a>
      <button className="btn small ghost" onClick={putOff}>
        {t("Not now")}
      </button>
      {notes.length > 1 && <span className="small muted">{t("{n} more", { n: notes.length - 1 })}</span>}
    </div>
  );
}
