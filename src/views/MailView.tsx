// SPDX-License-Identifier: AGPL-3.0-only
// Mail: your accounts' email, copied to this PC (encrypted) so reading and
// searching work Offline. Syncing and sending need Local AI + Web. Laid out
// like a webmail inbox: folders on the left, a list of rows, and the open
// message in place of the list; writing happens in a panel at the bottom right.
import { Fragment, useEffect, useMemo, useState, type ReactNode } from "react";
import { api, errorText, on, type MailAccount, type MailConfig, type MailItem, type MailSecurity, type TriageItem, type TriageReport } from "../api";
import { SignInButtons } from "../components/SignIn";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

/** Today: the time. This year: "Oct 12". Older: the full date. */
const when = (ms: number) => {
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (d.getFullYear() === now.getFullYear()) return d.toLocaleDateString([], { month: "short", day: "numeric" });
  return d.toLocaleDateString();
};
const fullDate = (ms: number) => new Date(ms).toLocaleString([], { weekday: "short", month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit" });

// A steady color per sender, from the logo's hues.
const AVATAR = ["#d02f8c", "#8d44cf", "#4b5df0", "#2d89ff", "#0f8b8d", "#c2410c", "#6447e8", "#be185d"];
const avatarColor = (key: string) => AVATAR[[...key].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % AVATAR.length];
function Avatar({ name, email, size = 32 }: { name: string; email: string; size?: number }) {
  const label = (name || email).trim();
  const initial = (label.match(/[\p{L}\p{N}]/u)?.[0] ?? "?").toUpperCase();
  return (
    <span className="mail-avatar" style={{ background: avatarColor(email || name), width: size, height: size, fontSize: size * 0.42 }} aria-hidden>
      {initial}
    </span>
  );
}

type Folder = "inbox" | "unread" | "sent" | "all" | "report";
const FOLDERS: { id: Folder; icon: string; label: string }[] = [
  { id: "inbox", icon: "📥", label: tx("Inbox") },
  { id: "unread", icon: "✉", label: tx("Unread") },
  { id: "sent", icon: "➤", label: tx("Sent") },
  { id: "all", icon: "🗂", label: tx("All mail") },
  { id: "report", icon: "✨", label: tx("AI report") },
];
const NEXT_LABEL: Record<TriageItem["next"], string> = {
  reply: tx("Reply"),
  calendar: tx("Add to calendar"),
  pay: tx("Pay"),
  review: tx("Review"),
  "read later": tx("Read later"),
  none: tx("Nothing to do"),
};
const isInbox = (m: MailItem) => m.folder.toUpperCase() === "INBOX";

interface Compose {
  to: string;
  cc: string;
  subject: string;
  body: string;
  reply_to: string | null;
}

/** Web links in a message, clickable (they open in the browser). */
function linkify(text: string): ReactNode[] {
  const parts = text.split(/(https?:\/\/[^\s<>"')\]]+)/g);
  return parts.map((p, i) =>
    i % 2 === 1 ? (
      <a key={i} href={p} onClick={(e) => { e.preventDefault(); api.openWeb(p).catch(() => {}); }}>{p}</a>
    ) : (
      <Fragment key={i}>{p}</Fragment>
    ),
  );
}

export function MailView({ toast }: { toast: PushToast }) {
  const [accounts, setAccounts] = useState<MailAccount[] | null>(null);
  const [items, setItems] = useState<MailItem[]>([]);
  const [query, setQuery] = useState("");
  const [folder, setFolder] = useState<Folder>("inbox");
  const [account, setAccount] = useState<string | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [open, setOpen] = useState<MailItem | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [compose, setCompose] = useState<Compose | null>(null);
  const [adding, setAdding] = useState(false);
  const [managing, setManaging] = useState(false);
  const [report, setReport] = useState<TriageReport | null>(null);
  const [sorting, setSorting] = useState<{ done: number; total: number } | null>(null);

  const loadAccounts = () => api.mailAccounts().then(setAccounts).catch((e) => toast(errorText(e), "error"));
  const loadItems = (q = query) => api.mailList(q).then(setItems).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    loadAccounts();
    loadItems("");
    api.mailTriageReport().then(setReport).catch(() => {});
    const subs = [on("mail:synced", () => loadItems()), on("mail:triage", (p) => setSorting(p))];
    return () => {
      subs.forEach((s) => s.then((u) => u()));
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

  // The AI reads the unread inbox and marks what doesn't need you as read.
  const sortUnread = async () => {
    setSorting({ done: 0, total: 0 });
    try {
      const r = await api.mailTriage();
      setReport(r);
      await loadItems();
      setOpenId(null);
      setFolder("report");
      const cleared = r.items.filter((i) => i.marked_read).length;
      toast(r.looked_at === 0 ? t("No unread mail to sort.") : t("Sorted {n} unread emails: {kept} need you, {cleared} marked as read.", { n: r.looked_at, kept: r.looked_at - cleared, cleared }), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSorting(null);
    }
  };
  const undo = async (ids: string[]) => {
    try {
      await api.mailTriageUndo(ids);
      setReport(await api.mailTriageReport());
      await loadItems();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const setSeen = async (m: { id: string }, seen: boolean) => {
    setItems((all) => all.map((x) => (x.id === m.id ? { ...x, seen } : x)));
    setReport((r) => (r ? { ...r, items: r.items.map((x) => (x.id === m.id ? { ...x, seen } : x)) } : r));
    try {
      await api.mailSetSeen(m.id, seen);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const mine = useMemo(() => items.filter((m) => !account || m.account_id === account), [items, account]);
  const shown = useMemo(
    () => mine.filter((m) => (folder === "inbox" ? isInbox(m) : folder === "unread" ? isInbox(m) && !m.seen : folder === "sent" ? !isInbox(m) : true)),
    [mine, folder],
  );
  const unread = mine.filter((m) => isInbox(m) && !m.seen).length;
  const waiting = report?.items.filter((i) => i.important && !i.seen).length ?? 0;

  const reply = (m: MailItem) =>
    setCompose({
      to: isInbox(m) ? m.from : m.to.join(", "),
      cc: "",
      subject: m.subject.toLowerCase().startsWith("re:") ? m.subject : `Re: ${m.subject}`,
      body: `\n\nOn ${new Date(m.date).toLocaleString()}, ${m.from_name || m.from} wrote:\n${m.body.split("\n").map((l) => `> ${l}`).join("\n")}`,
      reply_to: m.id,
    });
  const forward = (m: MailItem) =>
    setCompose({
      to: "",
      cc: "",
      subject: m.subject.toLowerCase().startsWith("fwd:") ? m.subject : `Fwd: ${m.subject}`,
      body: `\n\n---------- ${t("Forwarded message")} ----------\n${t("From")}: ${m.from_name ? `${m.from_name} <${m.from}>` : m.from}\n${t("Date")}: ${new Date(m.date).toLocaleString()}\n${t("Subject")}: ${m.subject}\n${t("To")}: ${m.to.join(", ")}\n\n${m.body}`,
      reply_to: null,
    });

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

  const fromAccount = accounts.find((a) => a.id === (open?.account_id ?? account)) ?? accounts[0];
  return (
    <div className="inbox">
      <aside className="inbox-rail">
        <button className="compose-btn" onClick={() => setCompose({ to: "", cc: "", subject: "", body: "", reply_to: null })}>
          <span aria-hidden>✎</span> {t("Compose")}
        </button>
        <nav className="inbox-folders" aria-label={t("Folders")}>
          {FOLDERS.map((f) => (
            <button key={f.id} className={folder === f.id && !open ? "active" : ""} onClick={() => { setFolder(f.id); setOpenId(null); }}>
              <span className="folder-icon" aria-hidden>{f.icon}</span>
              <span className="grow">{t(f.label)}</span>
              {(f.id === "inbox" || f.id === "unread") && unread > 0 && <span className="folder-count">{unread}</span>}
              {f.id === "report" && waiting > 0 && <span className="folder-count">{waiting}</span>}
            </button>
          ))}
        </nav>
        <div className="inbox-accounts">
          <span className="rail-label">{t("Accounts")}</span>
          {accounts.length > 1 && (
            <button className={!account ? "active" : ""} onClick={() => setAccount(null)}>
              <span className="grow">{t("All accounts")}</span>
            </button>
          )}
          {accounts.map((a) => (
            <button key={a.id} className={account === a.id || accounts.length === 1 ? "active" : ""} onClick={() => setAccount(accounts.length > 1 ? a.id : null)} title={a.email}>
              <Avatar name={a.name} email={a.email} size={20} />
              <span className="grow ellipsis">{a.email}</span>
            </button>
          ))}
          <button className="rail-link" onClick={() => setAdding(true)}>{t("+ Add account")}</button>
          <button className="rail-link" onClick={() => setManaging(!managing)}>{managing ? t("Done") : t("Manage accounts")}</button>
          {managing &&
            accounts.map((a) => (
              <div key={a.id} className="row between small">
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
        </div>
      </aside>

      <section className="inbox-main">
        <header className="inbox-top">
          <label className="mail-search">
            <span aria-hidden>🔍</span>
            <input value={query} onChange={(e) => { setQuery(e.target.value); setOpenId(null); }} placeholder={t("Search mail")} aria-label={t("Search mail")} />
            {query && <button className="icon-btn" onClick={() => setQuery("")} aria-label={t("Clear search")}>×</button>}
          </label>
          <button className={`icon-btn round ${syncing ? "spin" : ""}`} onClick={sync} disabled={syncing} title={syncing ? t("Syncing…") : t("Check for new mail")} aria-label={t("Check for new mail")}>↻</button>
          <button className="btn" onClick={sortUnread} disabled={!!sorting || unread === 0} title={t("Your AI reads your unread mail and marks what doesn't need you as read. You can undo it in the AI report.")}>
            {sorting ? (sorting.total ? t("Sorting {done} of {total}…", sorting) : t("Starting…")) : `✨ ${t("Sort unread")}`}
          </button>
        </header>
        {sorting && sorting.total > 0 && <div className="sort-progress"><span style={{ width: `${(sorting.done / sorting.total) * 100}%` }} /></div>}

        {!open && folder === "report" ? (
          <TriageView report={report} sorting={!!sorting} unread={unread} onSort={sortUnread} onOpen={setOpenId} onRead={(i) => setSeen(i, true)} onUndo={undo} />
        ) : open ? (
          <article className="message">
            <div className="message-bar">
              <button className="icon-btn round" onClick={() => setOpenId(null)} title={t("Back")} aria-label={t("Back")}>←</button>
              <span className="spacer" />
              <button className="btn ghost small" onClick={() => { setSeen(open, false); setOpenId(null); }}>{t("Mark as unread")}</button>
            </div>
            <h1 className="message-subject">{open.subject || t("(no subject)")}{isInbox(open) && <span className="badge subtle">{t("Inbox")}</span>}</h1>
            <div className="message-head">
              <Avatar name={open.from_name} email={open.from} size={40} />
              <div className="grow">
                <div><strong>{open.from_name || open.from}</strong> <span className="muted small">&lt;{open.from}&gt;</span></div>
                <div className="muted small">{t("To {names}", { names: open.to.join(", ") })}{open.cc.length > 0 ? ` · ${t("Cc {names}", { names: open.cc.join(", ") })}` : ""}</div>
              </div>
              <span className="muted small nowrap">{fullDate(open.date)}</span>
            </div>
            {open.attachments.length > 0 && (
              <div className="message-files">
                {open.attachments.map((f) => <span key={f} className="file-chip" title={t("Left on the server")}>📎 {f}</span>)}
              </div>
            )}
            <div className="message-body">{open.body ? linkify(open.body) : open.partial ? t("This message is large, so only its headers were copied.") : ""}</div>
            <div className="message-actions">
              <button className="btn" onClick={() => reply(open)}>↩ {t("Reply")}</button>
              <button className="btn" onClick={() => forward(open)}>↪ {t("Forward")}</button>
            </div>
          </article>
        ) : (
          <>
            <div className="list-bar small muted">
              <span>{t(FOLDERS.find((f) => f.id === folder)!.label)}</span>
              <span className="spacer" />
              {shown.length > 0 && <span>{t("{n} messages", { n: shown.length })}</span>}
            </div>
            <div className="mail-list" role="list">
              {shown.length === 0 && (
                <p className="muted center pad-lg">{query ? t("Nothing matches.") : folder === "unread" ? t("You're all caught up.") : t("No mail copied yet. Click Sync.")}</p>
              )}
              {shown.map((m) => {
                const who = isInbox(m) ? m.from_name || m.from : t("To: {names}", { names: m.to.join(", ") });
                return (
                  <div key={m.id} role="listitem" className={`mail-row ${m.seen ? "read" : "unread"}`} onClick={() => setOpenId(m.id)} onKeyDown={(e) => e.key === "Enter" && setOpenId(m.id)} tabIndex={0}>
                    <Avatar name={isInbox(m) ? m.from_name : m.to[0] ?? ""} email={isInbox(m) ? m.from : m.to[0] ?? ""} size={30} />
                    <span className="mail-from ellipsis">{who}</span>
                    <span className="mail-line ellipsis">
                      <span className="mail-subject">{m.subject || t("(no subject)")}</span>
                      {m.snippet && <span className="mail-snippet"> — {m.snippet}</span>}
                    </span>
                    {m.attachments.length > 0 && <span className="mail-clip" title={m.attachments.join(", ")} aria-label={t("Has attachments")}>📎</span>}
                    <span className="mail-date">{when(m.date)}</span>
                    <span className="mail-hover" onClick={(e) => e.stopPropagation()}>
                      <button className="icon-btn" title={m.seen ? t("Mark as unread") : t("Mark as read")} aria-label={m.seen ? t("Mark as unread") : t("Mark as read")} onClick={() => setSeen(m, !m.seen)}>
                        {m.seen ? "✉" : "✓"}
                      </button>
                    </span>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </section>
      {compose && <ComposePanel draft={compose} from={fromAccount} onClose={() => setCompose(null)} toast={toast} onSent={() => loadItems()} />}
    </div>
  );
}

/** What the AI did with the unread mail: what it left for you and why, and
 * what it marked read (each can be put back). */
function TriageView({ report, sorting, unread, onSort, onOpen, onRead, onUndo }: {
  report: TriageReport | null;
  sorting: boolean;
  unread: number;
  onSort: () => void;
  onOpen: (id: string) => void;
  onRead: (item: TriageItem) => void;
  onUndo: (ids: string[]) => void;
}) {
  if (!report?.run_at) {
    return (
      <div className="triage-empty">
        <span className="triage-icon" aria-hidden>✨</span>
        <h2>{t("Let your AI sort your unread mail")}</h2>
        <p className="muted">{t("It reads each unread email, keeps the ones that need you, and marks newsletters, promotions and notices that need nothing as read. This report shows what it did and why, and anything can be put back.")}</p>
        <button className="btn primary" onClick={onSort} disabled={sorting || unread === 0}>{unread === 0 ? t("No unread mail") : t("Sort {n} unread emails", { n: unread })}</button>
      </div>
    );
  }
  const attention = report.items.filter((i) => i.important);
  const cleared = report.items.filter((i) => !i.important);
  const undoable = cleared.filter((i) => i.marked_read && i.seen).map((i) => i.id);
  const row = (i: TriageItem, actions: React.ReactNode) => (
    <div key={i.id} className={`triage-row ${i.seen && i.important ? "done" : ""}`}>
      <Avatar name={i.from_name} email={i.from} size={32} />
      <div className="grow">
        <div className="row between">
          <span className="ellipsis"><strong>{i.from_name || i.from}</strong> <span className="muted">· {i.subject || t("(no subject)")}</span></span>
          <span className="muted small nowrap">{when(i.date)}</span>
        </div>
        <div className="triage-reason">{t(i.reason)}</div>
      </div>
      <div className="triage-actions">
        {i.important && <span className={`next-chip next-${i.next.replace(" ", "-")}`}>{t(NEXT_LABEL[i.next])}</span>}
        {actions}
      </div>
    </div>
  );
  return (
    <div className="triage">
      <div className="triage-head">
        <div>
          <h2>{t("AI report")}</h2>
          <p className="muted small">
            {t("Sorted {n} unread emails {time} with {model}.", { n: report.looked_at, time: new Date(report.run_at).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }), model: report.model || t("your model") })}
          </p>
        </div>
        <button className="btn" onClick={onSort} disabled={sorting || unread === 0}>{t("Sort again")}</button>
      </div>

      <section className="triage-section">
        <h3>{t("Needs your attention")} <span className="muted">({attention.filter((i) => !i.seen).length})</span></h3>
        {attention.length === 0 && <p className="muted small">{t("Nothing needed you.")}</p>}
        {attention.map((i) =>
          row(i, (
            <>
              <button className="btn small" onClick={() => onOpen(i.id)}>{t("Open")}</button>
              {i.seen ? <span className="muted small">✓ {t("Read")}</span> : <button className="btn small" onClick={() => onRead(i)}>{t("Mark as read")}</button>}
            </>
          )),
        )}
      </section>

      <section className="triage-section">
        <div className="row between">
          <h3>{t("Marked as read by the AI")} <span className="muted">({cleared.length})</span></h3>
          {undoable.length > 1 && <button className="btn ghost small" onClick={() => onUndo(undoable)}>{t("Undo all")}</button>}
        </div>
        {cleared.length === 0 && <p className="muted small">{t("Nothing was marked as read.")}</p>}
        {cleared.map((i) =>
          row(i, (
            <>
              <button className="btn ghost small" onClick={() => onOpen(i.id)}>{t("Open")}</button>
              {i.marked_read && i.seen ? <button className="btn small" onClick={() => onUndo([i.id])}>{t("Mark unread")}</button> : <span className="muted small">{t("Back in your inbox")}</span>}
            </>
          )),
        )}
      </section>
    </div>
  );
}

/** Writing a message: a panel at the bottom right, like webmail. */
function ComposePanel({ draft, from, onClose, toast, onSent }: { draft: Compose; from: MailAccount; onClose: () => void; toast: PushToast; onSent: () => void }) {
  const [d, setD] = useState(draft);
  const [sending, setSending] = useState(false);
  const [small, setSmall] = useState(false);
  const [showCc, setShowCc] = useState(!!draft.cc);
  useEffect(() => setD(draft), [draft]);
  const split = (s: string) => s.split(/[,;]/).map((x) => x.trim()).filter(Boolean);
  const send = async () => {
    setSending(true);
    try {
      await api.sendMail(from.id, { to: split(d.to), cc: split(d.cc), subject: d.subject, body: d.body, reply_to: d.reply_to });
      toast(t("Sent."), "success");
      onSent();
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
      setSending(false);
    }
  };
  const title = d.subject.trim() || (d.reply_to ? t("Reply") : t("New message"));
  return (
    <div className={`compose ${small ? "small" : ""}`} role="dialog" aria-label={title}>
      <header className="compose-head" onClick={() => small && setSmall(false)}>
        <span className="ellipsis grow">{title}</span>
        <button className="icon-btn" onClick={(e) => { e.stopPropagation(); setSmall(!small); }} aria-label={small ? t("Expand") : t("Minimize")} title={small ? t("Expand") : t("Minimize")}>{small ? "▴" : "▾"}</button>
        <button className="icon-btn" onClick={onClose} aria-label={t("Close")} title={t("Close")}>×</button>
      </header>
      {!small && (
        <>
          <div className="compose-field muted small">{t("From {email}", { email: from.email })}</div>
          <label className="compose-field">
            <span className="muted">{t("To")}</span>
            <input value={d.to} onChange={(e) => setD({ ...d, to: e.target.value })} placeholder="name@example.com" autoFocus={!d.reply_to} />
            {!showCc && <button className="link small" onClick={() => setShowCc(true)}>{t("Cc")}</button>}
          </label>
          {showCc && (
            <label className="compose-field">
              <span className="muted">{t("Cc")}</span>
              <input value={d.cc} onChange={(e) => setD({ ...d, cc: e.target.value })} />
            </label>
          )}
          <label className="compose-field">
            <input value={d.subject} onChange={(e) => setD({ ...d, subject: e.target.value })} placeholder={t("Subject")} aria-label={t("Subject")} />
          </label>
          <textarea className="compose-body" value={d.body} onChange={(e) => setD({ ...d, body: e.target.value })} aria-label={t("Message")} autoFocus={!!d.reply_to} />
          <footer className="compose-foot">
            <button className="send-btn" onClick={send} disabled={sending || !d.to.trim()}>{sending ? t("Sending…") : t("Send")}</button>
            <span className="spacer" />
            <button className="icon-btn" onClick={onClose} title={t("Discard")} aria-label={t("Discard")}>🗑</button>
          </footer>
        </>
      )}
    </div>
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
