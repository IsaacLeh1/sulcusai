// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { api, errorText, type CrashReport } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Top of the window: report a bug or send an idea to SulcusAI's developer. */
export function FeedbackButton({ toast }: { toast: PushToast }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button className="btn ghost small feedback-btn" onClick={() => setOpen(true)} title={t("Report a bug or send an idea to SulcusAI's developer")}>
        {t("Bug / Feedback")}
      </button>
      {open && <FeedbackDialog onClose={() => setOpen(false)} toast={toast} />}
    </>
  );
}

function FeedbackDialog({ onClose, toast }: { onClose: () => void; toast: PushToast }) {
  const [kind, setKind] = useState<"bug" | "idea">("bug");
  const [text, setText] = useState("");
  const [contact, setContact] = useState("");
  const [version, setVersion] = useState(true);
  const [number, setNumber] = useState("");
  useEffect(() => {
    getVersion().then(setNumber).catch(() => {});
  }, []);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const send = async () => {
    setSending(true);
    setError(null);
    try {
      await api.sendFeedback(kind, text, contact.trim() || null, version);
      toast(t("Sent. Thank you!"), "success");
      onClose();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSending(false);
    }
  };
  return (
    <Modal title={t("Report a bug or send feedback")} onClose={onClose}>
      <div className="mode-switch feedback-kind" role="radiogroup" aria-label={t("What it is")}>
        <button role="radio" aria-checked={kind === "bug"} className={`mode ${kind === "bug" ? "active" : ""}`} onClick={() => setKind("bug")}>
          {t("A bug")}
        </button>
        <button role="radio" aria-checked={kind === "idea"} className={`mode ${kind === "idea" ? "active" : ""}`} onClick={() => setKind("idea")}>
          {t("An idea or feedback")}
        </button>
      </div>
      <textarea
        className="input feedback-text"
        rows={7}
        value={text}
        maxLength={8000}
        autoFocus
        onChange={(e) => setText(e.target.value)}
        placeholder={kind === "bug" ? t("What happened, and what did you expect? The steps to make it happen again help a lot.") : t("What would make SulcusAI better for you?")}
        aria-label={t("Your message")}
      />
      <input className="input" value={contact} onChange={(e) => setContact(e.target.value)} placeholder={t("Your email, if you'd like a reply (optional)")} aria-label={t("Reply address (optional)")} />
      <label className="check small">
        <input type="checkbox" checked={version} onChange={(e) => setVersion(e.target.checked)} />
        <span>{t("Also send the version number ({version}), so the developer knows which one you have", { version: number })}</span>
      </label>
      <div className="feedback-privacy">
        <strong>{t("Only what you write here is sent")}</strong>
        <span className="small">
          {t("It goes straight to SulcusAI's developer, and nothing else goes with it: not your name, your chats, your files, your models or anything about your PC. Your internet address isn't saved. Leave out passwords and private details.")}
        </span>
      </div>
      {error && <p className="error-text">{error}</p>}
      <div className="modal-actions">
        <span className="spacer" />
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={send} disabled={sending || !text.trim()}>
          {sending ? t("Sending…") : t("Send")}
        </button>
      </div>
    </Modal>
  );
}

/** After a crash (with crash reports on): the report, to read, then send or not. */
export function CrashReportPrompt({ toast }: { toast: PushToast }) {
  const [reports, setReports] = useState<CrashReport[]>([]);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.crashReportsView().then((v) => setReports(v.pending)).catch(() => {});
  }, []);
  const r = reports[0];
  if (!r) return null;
  const next = () => setReports((all) => all.slice(1));
  return (
    <Modal title={r.kind === "closed" ? t("SulcusAI closed unexpectedly last time") : t("Something in SulcusAI went wrong")} onClose={() => api.dismissCrashReports().catch(() => {}).finally(() => setReports([]))}>
      <p className="small">{t("Sending this report helps fix it. This is everything it contains; nothing else goes with it.")}</p>
      <pre className="crash-text">{r.text}</pre>
      <div className="modal-actions">
        <button
          className="btn ghost"
          onClick={() => {
            navigator.clipboard.writeText(r.text).then(() => toast(t("Copied."), "success"));
          }}
        >
          {t("Copy")}
        </button>
        <span className="spacer" />
        <button className="btn" onClick={() => api.dismissCrashReports().catch(() => {}).finally(() => setReports([]))}>
          {t("Don't send")}
        </button>
        <button
          className="btn primary"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              await api.sendCrashReport(r.id);
              toast(t("Sent. Thank you!"), "success");
              next();
            } catch (e) {
              toast(errorText(e), "error");
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? t("Sending…") : t("Send report")}
        </button>
      </div>
    </Modal>
  );
}

/** Settings: whether to keep crash reports to offer after a crash. */
export function CrashReportsSetting({ toast }: { toast: PushToast }) {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    api.crashReportsView().then((v) => setOn(v.on)).catch(() => {});
  }, []);
  if (on === null) return null;
  return (
    <label className="check">
      <input
        type="checkbox"
        checked={on}
        onChange={async (e) => {
          try {
            setOn((await api.setCrashReports(e.target.checked)).on);
          } catch (err) {
            toast(errorText(err), "error");
          }
        }}
      />
      <span>
        {t("Offer to send crash reports")}
        <span className="muted small block">
          {t("If SulcusAI crashes or is closed unexpectedly, it keeps a short report (the version, Windows, the processor, memory and graphics card, and where it failed; never your chats or files) and shows it to you next time. Nothing is sent unless you choose Send.")}
        </span>
      </span>
    </label>
  );
}
