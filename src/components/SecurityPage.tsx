// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState } from "react";
import { errorText } from "../api";
import { securityApi, type Finding, type SecurityNotice, type SecurityOverview, type SecurityReport, type Severity } from "../api/security";
import { SecurityNoticesView } from "./SecurityNotices";
import { confirmed } from "../confirm";
import { t, tx } from "../i18n";
import { fixAfterAsking, findingText, groupFindings, keyAge, lastUsed } from "./securityPageLogic";
import type { PushToast } from "./Toasts";
import "./securityPage.css";

// The texts security_page.rs writes, so they get translated.
void [
  tx("Your data is not encrypted"),
  tx("SulcusAI could not find its data key, so chats and notes on this PC are not protected on disk. Restart SulcusAI; if this stays, reinstall it."),
  tx("App lock is off"),
  tx("Anyone signed in to this Windows account can open SulcusAI and read your chats. A PIN keeps them closed."),
  tx("SulcusAI never locks by itself"),
  tx("SulcusAI stays unlocked for a long time"),
  tx("If you walk away, SulcusAI stays open for anyone at the screen. Locking after {minutes} minutes of no use is a common choice."),
  tx("Lock after {minutes} minutes"),
  tx("Lock SulcusAI after {minutes} minutes of no use?"),
  tx("The phone page is on but no phone is paired"),
  tx("It still listens on your local network. Turn it off until you pair a phone."),
  tx("Turn off the phone page"),
  tx("Turn off the phone page?"),
  tx("Your phone can reach this PC from anywhere"),
  tx("It goes through SulcusAI's relay, protected by the pairing key and the phone's password. Turn this off to allow your home or office network only."),
  tx("Local network only"),
  tx("Let phones reach this PC on the local network only?"),
  tx("Phones keep working while SulcusAI is locked"),
  tx("Locking the screen does not cut off a paired phone, so someone with the phone can still use your chats."),
  tx("Stop phones while locked"),
  tx("Stop paired phones from working while SulcusAI is locked?"),
  tx("{name} has no password"),
  tx("It was paired before phones needed one. Remove it and pair it again with a password."),
  tx("Remove {name}"),
  tx("Remove {name}? You can pair it again afterwards."),
  tx("{name} has not been used for {days} days"),
  tx("A paired phone you no longer use is a way in you do not need."),
  tx("{name} is open to other computers"),
  tx("It should only answer programs on this PC. Turn it off until that is sorted out."),
  tx("Turn off"),
  tx("Turn off {name}?"),
  tx("{name} could not start"),
  tx("Programs on this PC can use your models"),
  tx("Any program that has the local API key can ask your models questions. Turn it off when you are not using it."),
  tx("Turn off the local API server?"),
  tx("Programs made for Ollama can use your models"),
  tx("They reach SulcusAI on this PC without a key. Turn it off when you are not using it."),
  tx("Turn off the server for programs made for Ollama?"),
  tx("{name} runs actions without asking"),
  tx("These tools run without asking, and what they work on goes to {host}: {tools}."),
  tx("These tools can change things on this PC and run without asking: {tools}."),
  tx("Ask first"),
  tx("Make {name} ask before using these tools?"),
  tx("Make a new key"),
  tx("Make a new key? Programs using the old one stop working until you give them the new one."),
  tx("Remove the key"),
  tx("Remove the saved key for {name}? SulcusAI asks again the next time a site needs it."),
  tx("The key for {name} is old"),
  tx("SulcusAI has had it for {days} days. Keys that live a long time are more likely to have leaked; replacing them is cheap."),
  tx("Local API server"),
  tx("Server for programs made for Ollama"),
  tx("Phone page"),
];

/** Where a finding without a one-click fix sends the person (Settings sections). */
function openSection(section: string) {
  document.getElementById(`settings-${section}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
}

const SEVERITY_LABEL: Record<Severity, string> = {
  critical: tx("Fix now"),
  warning: tx("Should fix"),
  notice: tx("Worth a look"),
  info: tx("Good to know"),
};

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <tr>
      <th scope="row" className="muted small">{label}</th>
      <td>{children}</td>
    </tr>
  );
}

const yesNo = (on: boolean) => (on ? t("On") : t("Off"));

function Findings({ findings, busy, onFix, onOpen }: { findings: Finding[]; busy: string | null; onFix: (f: Finding) => void; onOpen: (section: string) => void }) {
  if (findings.length === 0) return <p className="muted small">{t("No problems found.")}</p>;
  return (
    <>
      {groupFindings(findings).map((g) => (
        <div key={g.severity} className="sec-group">
          <h3>{t(SEVERITY_LABEL[g.severity])}</h3>
          <ul className="teach-list sec-findings">
            {g.items.map((f) => {
              const text = findingText(f);
              return (
                <li key={f.id} className={`sec-finding ${f.severity}`}>
                  <span>
                    <strong>{text.title}</strong>
                    <span className="muted small block">{text.detail}</span>
                  </span>
                  <span className="row">
                    {f.fix && (
                      <button className="btn small" disabled={busy !== null} onClick={() => onFix(f)}>
                        {busy === f.id ? t("Working…") : t(f.fix.label, f.args)}
                      </button>
                    )}
                    {!f.fix && f.open && (
                      <button className="btn ghost small" onClick={() => onOpen(f.open!)}>{t("Show the setting")}</button>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </>
  );
}

/** The overview and findings, with no state of its own. */
export function SecurityPageView({ report, busy, onFix, onOpen }: { report: SecurityReport; busy: string | null; onFix: (f: Finding) => void; onOpen: (section: string) => void }) {
  const o: SecurityOverview = report.overview;
  const p = o.protection;
  return (
    <>
      <Findings findings={report.findings} busy={busy} onFix={onFix} onOpen={onOpen} />

      <h3>{t("Protection")}</h3>
      <table className="sec-table">
        <tbody>
          <Row label={t("Version")}>{o.version}</Row>
          <Row label={t("Data encryption")}>{yesNo(p.encryption)}</Row>
          <Row label={t("App lock")}>{yesNo(p.lock_enabled)}</Row>
          {p.lock_enabled && <Row label={t("Locks after")}>{p.auto_lock_minutes === 0 ? t("Never") : t("{minutes} minutes", { minutes: p.auto_lock_minutes })}</Row>}
          {p.lock_enabled && <Row label={t("Windows Hello")}>{yesNo(p.hello_enabled)}</Row>}
        </tbody>
      </table>

      <h3>{t("Phones")}</h3>
      <p className="muted small">
        {o.phones.page_on ? t("The phone page is on.") : t("The phone page is off.")} {t("Signed-in phones: {n}", { n: o.phones.open_sessions })}
      </p>
      {o.phones.devices.length > 0 && (
        <ul className="teach-list small">
          {o.phones.devices.map((d) => (
            <li key={d.id}>
              <span>{d.name}</span>
              <span className="muted">{lastUsed(d.last_seen, o.now)}</span>
            </li>
          ))}
        </ul>
      )}

      <h3>{t("Servers on this PC")}</h3>
      <table className="sec-table">
        <tbody>
          {o.listeners.map((l) => (
            <tr key={l.id}>
              <th scope="row" className="small">{t(l.name)}</th>
              <td className="muted small">
                {!l.enabled ? t("Off") : l.running ? t("On") : t("Not running")}
                {l.enabled && l.port != null && ` · ${t("port {port}", { port: l.port })}`}
                {l.enabled && ` · ${l.local_only ? t("this PC only") : t("local network")}`}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h3>{t("Add-ons")}</h3>
      {o.addons.length === 0 ? (
        <p className="muted small">{t("None added.")}</p>
      ) : (
        <ul className="teach-list small">
          {o.addons.map((a) => (
            <li key={a.id}>
              <span>
                {a.name}
                {!a.enabled && <span className="tag">{t("Off")}</span>}
                <span className="muted block">
                  {a.host ? t("Talks to {host}", { host: a.host }) : t("Runs on this PC")} · {t("{n} tools", { n: a.tools })}
                </span>
              </span>
              <span className="muted">
                {t("{n} run unasked", { n: a.auto_run.length })} · {t("{n} ask first", { n: a.ask })} · {t("{n} off", { n: a.off })}
              </span>
            </li>
          ))}
        </ul>
      )}

      <h3>{t("Access keys")}</h3>
      {o.keys.length === 0 ? (
        <p className="muted small">{t("No keys saved.")}</p>
      ) : (
        <ul className="teach-list small">
          {o.keys.map((k) => (
            <li key={`${k.kind}:${k.host ?? ""}`}>
              <span>{t(k.label)}{k.host && <span className="muted block">{k.host}</span>}</span>
              <span className="muted">{keyAge(k.age_days)}</span>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/** Settings → Security overview: what protects this PC's data, what can reach
 * it, and one-click fixes. Nothing changes without a click and a yes. */
export function SecurityPage({ toast }: { toast: PushToast }) {
  const [report, setReport] = useState<SecurityReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<SecurityNotice | null>(null);
  const [noticeBusy, setNoticeBusy] = useState(false);

  const load = useCallback(() => {
    securityApi.overview().then(setReport).catch((e) => toast(errorText(e), "error"));
  }, [toast]);
  useEffect(load, [load]);
  useEffect(() => {
    securityApi.notice().then(setNotice).catch(() => {});
  }, []);

  const checkNotice = async () => {
    setNoticeBusy(true);
    try {
      setNotice(await securityApi.checkNotice());
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setNoticeBusy(false);
    }
  };

  const fix = async (f: Finding) => {
    setBusy(f.id);
    try {
      const next = await fixAfterAsking(f, confirmed, securityApi.applyFix);
      if (next) setReport(next);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  if (!report) return null;
  return (
    <section className="card security-page" id="settings-security-check">
      <div className="adv-head">
        <h2>{t("Security check")}</h2>
        <button className="btn ghost small" onClick={load}>{t("Check again")}</button>
      </div>
      <p className="muted small">{t("What protects your data on this PC and what can reach it. A fix changes one setting, and only after you say yes.")}</p>
      {notice && <SecurityNoticesView notice={notice} busy={noticeBusy} onCheck={checkNotice} />}
      <SecurityPageView report={report} busy={busy} onFix={fix} onOpen={openSection} />
    </section>
  );
}
