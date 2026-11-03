// SPDX-License-Identifier: AGPL-3.0-only
import { t } from "../i18n";

export interface ReportPlace {
  what: string;
  where: string;
  url?: string;
  phone?: string;
}

/** Where crimes go. The developer hosts nothing and can't act on anything
 * between people's PCs, so these point straight to the authorities and
 * hotlines that can. Matches "Reporting" in community-guidelines.md. */
export const REPORT_PLACES: ReportPlace[] = [
  { what: "Someone in immediate danger", where: "Call your local emergency number (911 in the US and Canada, 112 in the EU, 999 in the UK, 000 in Australia)." },
  { what: "Child sexual abuse material or child exploitation (US)", where: "NCMEC CyberTipline", url: "https://report.cybertip.org/", phone: "1-800-843-5678" },
  { what: "Child sexual abuse material (outside the US)", where: "Your country's hotline, listed by INHOPE", url: "https://www.inhope.org/" },
  { what: "Intimate images shared without consent", where: "StopNCII", url: "https://stopncii.org/" },
  { what: "Intimate images of someone under 18", where: "Take It Down (NCMEC)", url: "https://takeitdown.ncmec.org/" },
  { what: "Online fraud and other cybercrime (US)", where: "FBI Internet Crime Complaint Center", url: "https://www.ic3.gov/" },
  { what: "Threats, stalking or harassment", where: "Your local police." },
];

/** Links for reporting crimes to the right authorities, for Settings, a
 * group's report menu or the guidelines. */
export function ConsentReport({ compact = false }: { compact?: boolean }) {
  return (
    <div className="consent-report">
      {!compact && (
        <p className="small">
          {t("Reports inside a group go to its moderators. Crimes go to the authorities: SulcusAI's developer can't see or act on anything between people's PCs.")}
        </p>
      )}
      <ul className="small">
        {REPORT_PLACES.map((p) => (
          <li key={p.what}>
            <strong>{t(p.what)}:</strong>{" "}
            {p.url ? (
              <a href={p.url} target="_blank" rel="noreferrer">
                {t(p.where)}
              </a>
            ) : (
              t(p.where)
            )}
            {p.phone && <> ({p.phone})</>}
          </li>
        ))}
      </ul>
      {!compact && <p className="small muted">{t("Never copy, open or forward abuse material, even to report it. Report where you saw it.")}</p>}
    </div>
  );
}
