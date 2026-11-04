// SPDX-License-Identifier: AGPL-3.0-only
import type { SecurityNotice } from "../api/security";
import { t, tx } from "../i18n";

const LEVEL = { critical: tx("Fix now"), warning: tx("Should fix"), info: tx("Good to know") } as const;

/** What the last check did, in words; null when nothing worth saying. */
function checkLine(n: SecurityNotice): string | null {
  if (n.status === "no_key") return null;
  if (n.last_result === "failed" || n.last_result === "rejected") return n.last_error;
  if (n.last_result === "none") return t("No security notice has been published yet.");
  return null;
}

/** The signed security notices: what the project says about this version,
 * engines and helpers. Shows only what passed the signature check. */
export function SecurityNoticesView({ notice, busy, onCheck }: { notice: SecurityNotice; busy: boolean; onCheck: () => void }) {
  const line = checkLine(notice);
  return (
    <>
      <div className="adv-head">
        <h3>{t("Security notices")}</h3>
        {notice.status !== "no_key" && (
          <button className="btn ghost small" disabled={busy} onClick={onCheck}>{busy ? t("Working…") : t("Check for notices")}</button>
        )}
      </div>
      {notice.status === "no_key" && (
        <p className="muted small">{t("No signing key yet, so security notices can't be checked and none are shown. This version does not look for them.")}</p>
      )}
      {notice.status === "waiting" && <p className="muted small">{t("No security notice has been received yet.")}</p>}
      {line && <p className="muted small">{line}</p>}
      {notice.status === "current" && (
        <>
          {notice.update_needed && (
            <p className="sec-finding critical">
              <strong>{t("This version is below the lowest safe version, {version}. Update SulcusAI.", { version: notice.lowest_safe_version })}</strong>
            </p>
          )}
          {notice.revoked.length > 0 && (
            <ul className="teach-list sec-findings">
              {notice.revoked.map((r) => (
                <li key={r.id} className="sec-finding critical">
                  <span>
                    <strong>{t("{name} has been withdrawn", { name: r.id })}</strong>
                    {r.reason && <span className="muted small block">{r.reason}</span>}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {notice.messages.length > 0 && (
            <ul className="teach-list sec-findings">
              {notice.messages.map((m, i) => (
                <li key={i} className={`sec-finding ${m.severity === "critical" ? "critical" : m.severity === "warning" ? "warning" : "info"}`}>
                  <span>
                    <span className="tag">{t(LEVEL[m.severity])}</span> {m.text}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {!notice.update_needed && notice.revoked.length === 0 && notice.messages.length === 0 && (
            <p className="muted small">{t("Nothing to report. The latest notice is number {n}.", { n: notice.sequence })}</p>
          )}
        </>
      )}
    </>
  );
}
