// SPDX-License-Identifier: AGPL-3.0-only
// Mail: your accounts' email, copied to this PC (encrypted) so reading and
// searching work Offline. Syncing and sending need Local AI + Web.
import { useEffect, useMemo, useState } from "react";
import { api, errorText, on, type MailAccount, type MailConfig, type MailItem, type MailSecurity } from "../api";
import { Modal } from "../components/Modal";
import { SignInButtons } from "../components/SignIn";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

const when = (ms: number) => {
  const d = new Date(ms);
  const today = new Date();
  return d.toDateString() === today.toDateString() ? d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : d.toLocaleDateString([], { month: "short", day: "numeric" });
};

interface Compose {
  to: string;
  cc: string;
  subject: string;
  body: string;
  reply_to: string | null;
}

export function MailView({ toast }: { toast: PushToast }) {
  const [accounts, setAccounts] = useState<MailAccount[] | null>(null);
  const [items, setItems] = useState<MailItem[]>([]);
  const [query, setQuery] = useState("");
  const [openId, setOpenId] = useState<string | null>(null);
  const [open, setOpen] = useState<MailItem | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [compose, setCompose] = useState<Compose | null>(null);
  const [adding, setAdding] = useState(false);

  const loadAccounts = () => api.mailAccounts().then(setAccounts).catch((e) => toast(errorText(e), "error"));
  const loadItems = (q = query) => api.mailList(q).then(setItems).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    loadAccounts();
    loadItems("");
    const sub = on("mail:synced", () => loadItems());
    return () => {
      sub.then((u) => u());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const timer = setTimeout(() => loadItems(query), 200);
    return () => clearTimeout(timer);
  }, [query]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!openId) return setOpen(null);
    api.mailGet(openId).then((m) => {
      setOpen(m);
      setItems((all) => all.map((x) => (x.id === m.id ? { ...x, seen: true } : x)));
    }).catch((e) => toast(errorText(e), "error"));
  }, [openId]); // eslint-disable-line react-hooks/exhaustive-deps

  const sync = async () => {
    setSyncing(true);
    try {
      const n = await api.syncMail();
      await loadItems();
      await loadAccounts();
      toast(n === 0 ? t("No new mail.") : n === 1 ? t("1 new message.") : t("{n} new messages.", { n }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSyncing(false);
    }
  };

  if (!accounts) return <div className="page"><p className="muted">{t("Loading…")}</p></div>;
  if (accounts.length === 0 || adding) {
    return (
      <div className="page">
        <div className="narrow">
          <header className="page-head"><h1>{t("Mail")}</h1></header>
          <AddAccount
            toast={toast}
            onCancel={accounts.length > 0 ? () => setAdding(false) : undefined}
            onAdded={async () => {
              setAdding(false);
              await loadAccounts();
              await loadItems("");
            }}
          />
        </div>
      </div>
    );
  }

  const reply = (m: MailItem) =>
    setCompose({
      to: m.from,
      cc: "",
      subject: m.subject.toLowerCase().startsWith("re:") ? m.subject : `Re: ${m.subject}`,
      body: `\n\nOn ${new Date(m.date).toLocaleString()}, ${m.from_name || m.from} wrote:\n${m.body.split("\n").map((l) => `> ${l}`).join("\n")}`,
      reply_to: m.id,
    });

  return (
    <div className="notes-page mail-page">
      <aside className="notes-list">
        <div className="row">
          <input className="input" placeholder={t("Search mail")} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t("Search mail")} />
          <button className="btn primary small" onClick={() => setCompose({ to: "", cc: "", subject: "", body: "", reply_to: null })}>{t("Write")}</button>
        </div>
        <div className="row small muted">
          <button className="btn ghost small" onClick={sync} disabled={syncing}>{syncing ? t("Syncing…") : t("↻ Sync")}</button>
          <span className="ellipsis">{accounts.map((a) => a.email).join(", ")}</span>
        </div>
        {items.length === 0 && <p className="muted small">{query ? t("Nothing matches.") : t("No mail copied yet. Click Sync.")}</p>}
        {items.map((m) => (
          <button key={m.id} className={`note-item mail-item ${openId === m.id ? "active" : ""} ${m.seen ? "" : "unread"}`} onClick={() => setOpenId(m.id)}>
            <span className="row between">
              <strong className="ellipsis">{m.folder === "INBOX" ? m.from_name || m.from : t("To: {names}", { names: m.to.join(", ") })}</strong>
              <span className="small muted">{when(m.date)}</span>
            </span>
            <span className="ellipsis">{m.subject || t("(no subject)")}</span>
            <span className="small muted ellipsis">{m.snippet}</span>
          </button>
        ))}
        <details className="mail-accounts small">
          <summary>{t("Accounts")}</summary>
          {accounts.map((a) => (
            <div key={a.id} className="row between">
              <span className="ellipsis">{a.email}</span>
              <button
                className="btn ghost small danger"
                onClick={async () => {
                  try {
                    await api.removeMailAccount(a.id);
                    setOpenId(null);
                    await loadAccounts();
                    await loadItems("");
                  } catch (e) {
                    toast(errorText(e), "error");
                  }
                }}
              >
                {t("Remove")}
              </button>
            </div>
          ))}
          <button className="btn small" onClick={() => setAdding(true)}>{t("+ Add account")}</button>
        </details>
      </aside>
      <section className="mail-read">
        {!open && <p className="muted pad">{t("Pick a message to read it. The assistant can also search and summarize your mail.")}</p>}
        {open && (
          <article>
            <header className="mail-head">
              <h2>{open.subject || t("(no subject)")}</h2>
              <p className="small">
                <strong>{open.from_name || open.from}</strong> <span className="muted">&lt;{open.from}&gt;</span>
                <br />
                <span className="muted">{t("To {names}", { names: open.to.join(", ") })}{open.cc.length > 0 ? ` · ${t("Cc {names}", { names: open.cc.join(", ") })}` : ""} · {new Date(open.date).toLocaleString()}</span>
              </p>
              {open.attachments.length > 0 && <p className="small muted">{t("📎 {files} (left on the server)", { files: open.attachments.join(", ") })}</p>}
              <div className="row">
                <button className="btn small" onClick={() => reply(open)}>{t("Reply")}</button>
              </div>
            </header>
            <pre className="mail-body">{open.body || (open.partial ? t("This message is large, so only its headers were copied.") : "")}</pre>
          </article>
        )}
      </section>
      {compose && <ComposeDialog draft={compose} from={accounts[0]} onClose={() => setCompose(null)} toast={toast} />}
    </div>
  );
}

function ComposeDialog({ draft, from, onClose, toast }: { draft: Compose; from: MailAccount; onClose: () => void; toast: PushToast }) {
  const [d, setD] = useState(draft);
  const [sending, setSending] = useState(false);
  const split = (s: string) => s.split(/[,;]/).map((x) => x.trim()).filter(Boolean);
  const send = async () => {
    setSending(true);
    try {
      await api.sendMail(from.id, { to: split(d.to), cc: split(d.cc), subject: d.subject, body: d.body, reply_to: d.reply_to });
      toast(t("Sent."), "success");
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
      setSending(false);
    }
  };
  return (
    <Modal title={d.reply_to ? t("Reply") : t("New email")} onClose={onClose}>
      <div className="form">
        <span className="small muted">{t("From {email}", { email: from.email })}</span>
        <label>{t("To")}<input className="input" value={d.to} onChange={(e) => setD({ ...d, to: e.target.value })} placeholder="name@example.com" autoFocus={!d.reply_to} /></label>
        <label>{t("Cc")}<input className="input" value={d.cc} onChange={(e) => setD({ ...d, cc: e.target.value })} /></label>
        <label>{t("Subject")}<input className="input" value={d.subject} onChange={(e) => setD({ ...d, subject: e.target.value })} /></label>
        <textarea className="input" rows={12} value={d.body} onChange={(e) => setD({ ...d, body: e.target.value })} aria-label={t("Message")} autoFocus={!!d.reply_to} />
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={send} disabled={sending || !d.to.trim()}>{sending ? t("Sending…") : t("Send")}</button>
      </div>
    </Modal>
  );
}

const SECURITY: { id: MailSecurity; label: string }[] = [
  { id: "tls", label: "SSL/TLS" },
  { id: "starttls", label: "STARTTLS" },
  { id: "plain", label: tx("None (this PC only)") },
];

function AddAccount({ toast, onAdded, onCancel }: { toast: PushToast; onAdded: () => void; onCancel?: () => void }) {
  const [cfg, setCfg] = useState<MailConfig | null>(null);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const valid = useMemo(() => /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email.trim()), [email]);

  const fill = async () => {
    if (!valid) return;
    const p = await api.mailPreset(email.trim());
    setCfg(p.config);
    setNote(p.note);
  };
  const connect = async () => {
    setBusy(true);
    try {
      const base = cfg ?? (await api.mailPreset(email.trim())).config;
      await api.addMailAccount({ ...base, email: email.trim(), name, password });
      toast(t("Connected. Your recent mail is copied to this PC."), "success");
      onAdded();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const field = (label: string, key: "imap_host" | "smtp_host") => (
    <label>{label}<input className="input" value={cfg?.[key] ?? ""} onChange={(e) => cfg && setCfg({ ...cfg, [key]: e.target.value })} /></label>
  );
  const port = (label: string, key: "imap_port" | "smtp_port") => (
    <label>{label}<input className="input" type="number" value={cfg?.[key] ?? 0} onChange={(e) => cfg && setCfg({ ...cfg, [key]: Number(e.target.value) })} /></label>
  );
  const sec = (label: string, key: "imap_security" | "smtp_security") => (
    <label>{label}
      <select value={cfg?.[key] ?? "tls"} onChange={(e) => cfg && setCfg({ ...cfg, [key]: e.target.value as MailSecurity })}>
        {SECURITY.map((s) => <option key={s.id} value={s.id}>{t(s.label)}</option>)}
      </select>
    </label>
  );

  return (
    <section className="card">
      <h2>{t("Connect an email account")}</h2>
      <p className="muted small">{t("Your mail is copied to this PC and encrypted, so you and the assistant can read and search it, even Offline. Syncing and sending need Local AI + Web. Sending always asks you first.")}</p>
      <SignInButtons
        toast={toast}
        start={(p) => api.addMailAccountOAuth(p, name.trim() || null)}
        done={(p) => {
          toast(t("Connected with {provider}. Your recent mail is copied to this PC.", { provider: p === "microsoft" ? "Microsoft" : "Google" }), "success");
          onAdded();
        }}
      />
      <p className="small muted or-line">{t("or use a password (other providers, or an app password)")}</p>
      <div className="form">
        <label>{t("Email address")}<input className="input" value={email} onChange={(e) => setEmail(e.target.value)} onBlur={fill} placeholder="you@example.com" autoFocus /></label>
        <label>{t("Your name (shown to people you write to)")}<input className="input" value={name} onChange={(e) => setName(e.target.value)} /></label>
        <label>{t("Password or app password")}<input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="off" /></label>
        {note && <p className="small hint-box">{note}</p>}
        {cfg && (
          <button className="link small left" onClick={() => setAdvanced(!advanced)}>{advanced ? t("Hide server settings") : t("Server settings ({host})", { host: cfg.imap_host })}</button>
        )}
        {cfg && advanced && (
          <div className="grid-2">
            {field(t("Incoming (IMAP) server"), "imap_host")}
            {port(t("Port"), "imap_port")}
            {sec(t("Security"), "imap_security")}
            <label>{t("User name")}<input className="input" value={cfg.username} onChange={(e) => setCfg({ ...cfg, username: e.target.value })} /></label>
            {field(t("Outgoing (SMTP) server"), "smtp_host")}
            {port(t("Port"), "smtp_port")}
            {sec(t("Security"), "smtp_security")}
          </div>
        )}
        <div className="row">
          <button className="btn primary" onClick={connect} disabled={busy || !valid || !password}>{busy ? t("Connecting…") : t("Connect")}</button>
          {onCancel && <button className="btn" onClick={onCancel}>{t("Cancel")}</button>}
        </div>
      </div>
    </section>
  );
}
