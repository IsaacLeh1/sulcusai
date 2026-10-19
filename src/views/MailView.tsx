// SPDX-License-Identifier: AGPL-3.0-only
// Mail: your accounts' email, copied to this PC (encrypted) so reading and
// searching work Offline. Syncing and sending need Local AI + Web. Laid out
// like a webmail inbox: folders on the left, a list of rows, and the open
// message in place of the list; writing happens in a panel at the bottom right.
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  api,
  errorText,
  on,
  type CalAccount,
  type EventDraft,
  type MailAccount,
  type MailCategory,
  type MailConfig,
  type MailItem,
  type MailSecurity,
  type Subscription,
  type TriageItem,
  type TriageReport,
  type UnsubscribeResult,
} from "../api";
import { SignInButtons } from "../components/SignIn";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";
import { calendarName, NewEventDialog } from "./CalendarView";

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

type Folder = "inbox" | "unread" | "marked" | "sent" | "all" | "subscriptions" | "report";
const FOLDERS: { id: Folder; icon: string; label: string }[] = [
  { id: "inbox", icon: "📥", label: tx("Inbox") },
  { id: "unread", icon: "✉", label: tx("Unread") },
  { id: "marked", icon: "✓", label: tx("Marked read") },
  { id: "sent", icon: "➤", label: tx("Sent") },
  { id: "all", icon: "🗂", label: tx("All mail") },
  { id: "subscriptions", icon: "📰", label: tx("Subscriptions") },
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
// The report's categories, in the order it groups them.
const CATEGORIES: { id: MailCategory; label: string }[] = [
  { id: "needs reply", label: tx("Needs reply") },
  { id: "to-do", label: tx("To-do") },
  { id: "work", label: tx("Work") },
  { id: "personal", label: tx("Personal") },
  { id: "bills", label: tx("Bills") },
  { id: "travel", label: tx("Travel") },
  { id: "updates", label: tx("Updates") },
  { id: "newsletters", label: tx("Newsletters") },
  { id: "promotions", label: tx("Promotions") },
  { id: "social", label: tx("Social") },
];
const categoryLabel = (c: MailCategory) => t(CATEGORIES.find((x) => x.id === c)?.label ?? "Updates");
const categoryClass = (c: MailCategory) => `cat-chip cat-${c.replace(/\s/g, "-")}`;
const isInbox = (m: { folder: string }) => m.folder.toUpperCase() === "INBOX";
const isArchived = (m: { folder: string }) => m.folder === "Archive";
const isSent = (m: { folder: string }) => !isInbox(m) && !isArchived(m);
/** Marked read with the button: under Marked read, not in the inbox list. */
const isMarked = (m: MailItem) => isInbox(m) && m.marked_at != null;

interface Compose {
  to: string;
  cc: string;
  subject: string;
  body: string;
  reply_to: string | null;
  /** The account it goes from (a reply: the one the message came to). */
  account_id: string | null;
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
  // The last mark-read click, for Undo.
  const [lastMarked, setLastMarked] = useState<{ id: string; subject: string } | null>(null);
  // A message the report or the message view is fetching or reading for an event.
  const [busy, setBusy] = useState<string | null>(null);
  const [event, setEvent] = useState<{ draft: EventDraft; calendars: CalAccount[] } | null>(null);
  const [subsVersion, setSubsVersion] = useState(0);

  const loadAccounts = () => api.mailAccounts().then(setAccounts).catch((e) => toast(errorText(e), "error"));
  // The search as typed now: the event handlers set up once would otherwise reload with the first one.
  const queryRef = useRef(query);
  queryRef.current = query;
  const loadItems = (q = queryRef.current) => api.mailList(q).then(setItems).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    loadAccounts();
    loadItems("");
    api.mailTriageReport().then(setReport).catch(() => {});
    const subs = [
      on("mail:synced", () => {
        loadItems();
        setSubsVersion((v) => v + 1);
      }),
      on("mail:triage", (p) => setSorting(p)),
      // New mail was sorted in the background.
      on("mail:triaged", () => {
        loadItems();
        api.mailTriageReport().then(setReport).catch(() => {});
      }),
    ];
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

  // Archive (out of the inbox) or Delete (to the trash), here and on the server.
  const move = async (ids: string[], action: "archive" | "trash") => {
    setItems((all) => (action === "trash" ? all.filter((x) => !ids.includes(x.id)) : all.map((x) => (ids.includes(x.id) ? { ...x, folder: "Archive", seen: true } : x))));
    if (open && ids.includes(open.id)) setOpenId(null);
    try {
      await api.mailAction(ids, action);
      toast(action === "trash" ? t("Moved to the trash.") : t("Archived."), "success");
    } catch (e) {
      toast(errorText(e), "error");
      loadItems();
    }
  };
  const setAuto = async (on: boolean) => {
    setReport((r) => (r ? { ...r, auto: on } : r));
    try {
      await api.setMailAutoSort(on);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  // Read state here at once (the list and the report), for the server to follow.
  const showSeen = (ids: string[], seen: boolean, marked_at: number | null) => {
    setItems((all) => all.map((x) => (ids.includes(x.id) ? { ...x, seen, marked_at } : x)));
    setReport((r) => (r ? { ...r, items: r.items.map((x) => (ids.includes(x.id) ? { ...x, seen } : x)) } : r));
  };
  // The mark-read button: the row moves to Marked read at once, so the next
  // one comes up under the pointer. The server hears in one batch.
  const markRead = async (m: { id: string; subject: string }) => {
    showSeen([m.id], true, Date.now());
    setLastMarked({ id: m.id, subject: m.subject });
    try {
      await api.mailAction([m.id], "mark_read");
    } catch (e) {
      showSeen([m.id], false, null);
      setLastMarked((l) => (l?.id === m.id ? null : l));
      toast(errorText(e), "error");
    }
  };
  // Unread again: out of Marked read, back in the inbox.
  const markUnread = async (ids: string[]) => {
    const before = items.filter((x) => ids.includes(x.id));
    showSeen(ids, false, null);
    setLastMarked((l) => (l && ids.includes(l.id) ? null : l));
    try {
      await api.mailAction(ids, "unseen");
    } catch (e) {
      for (const b of before) showSeen([b.id], b.seen, b.marked_at);
      toast(errorText(e), "error");
    }
  };
  useEffect(() => {
    if (!lastMarked) return;
    const timer = setTimeout(() => setLastMarked(null), 8000);
    return () => clearTimeout(timer);
  }, [lastMarked]);

  const mine = useMemo(() => items.filter((m) => !account || m.account_id === account), [items, account]);
  const shown = useMemo(() => {
    const list = mine.filter((m) =>
      folder === "inbox" ? isInbox(m) && !isMarked(m) : folder === "unread" ? isInbox(m) && !m.seen && !isMarked(m) : folder === "marked" ? isMarked(m) : folder === "sent" ? isSent(m) : true,
    );
    // Newest mark first.
    return folder === "marked" ? [...list].sort((a, b) => (b.marked_at ?? 0) - (a.marked_at ?? 0)) : list;
  }, [mine, folder]);
  const unread = mine.filter((m) => isInbox(m) && !m.seen).length;
  const waiting = report?.items.filter((i) => i.important && !i.seen).length ?? 0;

  const reply = (m: MailItem) =>
    setCompose({
      to: isInbox(m) ? m.from : m.to.join(", "),
      cc: "",
      subject: m.subject.toLowerCase().startsWith("re:") ? m.subject : `Re: ${m.subject}`,
      body: `\n\nOn ${new Date(m.date).toLocaleString()}, ${m.from_name || m.from} wrote:\n${m.body.split("\n").map((l) => `> ${l}`).join("\n")}`,
      reply_to: m.id,
      account_id: m.account_id,
    });
  const forward = (m: MailItem) =>
    setCompose({
      to: "",
      cc: "",
      subject: m.subject.toLowerCase().startsWith("fwd:") ? m.subject : `Fwd: ${m.subject}`,
      body: `\n\n---------- ${t("Forwarded message")} ----------\n${t("From")}: ${m.from_name ? `${m.from_name} <${m.from}>` : m.from}\n${t("Date")}: ${new Date(m.date).toLocaleString()}\n${t("Subject")}: ${m.subject}\n${t("To")}: ${m.to.join(", ")}\n\n${m.body}`,
      reply_to: null,
      account_id: m.account_id,
    });

  // Reply from the report: the whole message first (it opens read), from the account it came to.
  const replyTo = async (id: string) => {
    setBusy(id);
    try {
      const m = await api.mailGet(id);
      showSeen([id], true, m.marked_at);
      reply(m);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  // Add to calendar: the AI reads the event, the form shows it to correct, then it's saved.
  const addToCalendar = async (id: string) => {
    setBusy(id);
    try {
      const [draft, calendars] = await Promise.all([api.mailEventDraft(id), api.calendarAccounts().catch(() => [] as CalAccount[])]);
      setEvent({ draft, calendars });
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  // Unsubscribe the most automatic way the list allows; a page opens in the browser.
  // `quiet`: one of several (the caller sums up); pages still open.
  const unsubscribe = async (key: string, name: string, quiet = false): Promise<UnsubscribeResult | null> => {
    try {
      const r = await api.mailUnsubscribe(key);
      if (r.method === "link") {
        await api.openWeb(r.url).catch((e) => toast(errorText(e), "error"));
        if (!quiet) {
          toast(
            r.problem
              ? t("One-click unsubscribe didn't work, so the unsubscribe page for {name} is open in your browser. The site may ask you to confirm.", { name })
              : t("The unsubscribe page for {name} is open in your browser. The site may ask you to confirm.", { name }),
            "info",
          );
        }
      } else if (quiet) {
        // Summed up by the caller.
      } else if (r.method === "email") {
        toast(t("Sent “{subject}” to {to} to unsubscribe from {name}.", { subject: r.subject, to: r.to, name }), "success");
      } else {
        toast(t("Unsubscribed from {name}.", { name }), "success");
      }
      api.mailTriageReport().then(setReport).catch(() => {});
      return r;
    } catch (e) {
      toast(errorText(e), "error");
      return null;
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
              await loadItems();
            }}
          />
        </div>
      </div>
    );
  }

  const fromAccount = accounts.find((a) => a.id === compose?.account_id) ?? accounts.find((a) => a.id === (open?.account_id ?? account)) ?? accounts[0];
  return (
    <div className="inbox">
      <aside className="inbox-rail">
        <button className="compose-btn" onClick={() => setCompose({ to: "", cc: "", subject: "", body: "", reply_to: null, account_id: account })}>
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
                      await loadItems();
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
          <TriageView
            report={report}
            sorting={!!sorting}
            unread={unread}
            busy={busy}
            onSort={sortUnread}
            onOpen={setOpenId}
            onRead={markRead}
            onUndo={undo}
            onAuto={setAuto}
            onReply={(i) => replyTo(i.id)}
            onCalendar={(i) => addToCalendar(i.id)}
            onUnsubscribe={(i) => i.unsubscribe && unsubscribe(i.unsubscribe.key, i.from_name || i.from)}
          />
        ) : !open && folder === "subscriptions" ? (
          <SubscriptionsView
            toast={toast}
            account={account}
            version={subsVersion}
            showAccount={accounts.length > 1 && !account}
            accounts={accounts}
            onUnsubscribe={unsubscribe}
            onArchive={async (ids) => {
              await move(ids, "archive");
            }}
          />
        ) : open ? (
          <article className="message">
            <div className="message-bar">
              <button className="icon-btn round" onClick={() => setOpenId(null)} title={t("Back")} aria-label={t("Back")}>←</button>
              <span className="spacer" />
              {!isArchived(open) && <button className="btn ghost small" onClick={() => move([open.id], "archive")} title={t("Take it out of the inbox (it stays in All mail)")}>🗄 {t("Archive")}</button>}
              <button className="btn ghost small" onClick={() => move([open.id], "trash")} title={t("Move it to the trash")}>🗑 {t("Delete")}</button>
              <button className="btn ghost small" onClick={() => { markUnread([open.id]); setOpenId(null); }}>{t("Mark as unread")}</button>
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
              <button className="btn" onClick={() => addToCalendar(open.id)} disabled={busy === open.id} title={t("Your AI reads the date, time and place, and you check them before it's saved.")}>
                📅 {busy === open.id ? t("Reading the email…") : t("Add to calendar")}
              </button>
            </div>
          </article>
        ) : (
          <>
            <div className="list-bar small muted">
              <span>{t(FOLDERS.find((f) => f.id === folder)!.label)}</span>
              {lastMarked && (
                <span className="undo-mark" role="status">
                  <span className="ellipsis">{t("Marked read: {subject}", { subject: lastMarked.subject || t("(no subject)") })}</span>
                  <button className="link small" onClick={() => markUnread([lastMarked.id])}>{t("Undo")}</button>
                </span>
              )}
              <span className="spacer" />
              {shown.length > 0 && <span>{t("{n} messages", { n: shown.length })}</span>}
            </div>
            <div className="mail-list" role="list">
              {shown.length === 0 && (
                <p className="muted center pad-lg">
                  {query
                    ? t("Nothing matches.")
                    : folder === "unread"
                      ? t("You're all caught up.")
                      : folder === "marked"
                        ? t("Mail you mark read with ✓ moves here, newest first. Mark unread puts it back in the inbox.")
                        : t("No mail copied yet. Click Sync.")}
                </p>
              )}
              {shown.map((m) => {
                const who = !isSent(m) ? m.from_name || m.from : t("To: {names}", { names: m.to.join(", ") });
                return (
                  <div key={m.id} role="listitem" className={`mail-row ${m.seen ? "read" : "unread"}`} onClick={() => setOpenId(m.id)} onKeyDown={(e) => e.key === "Enter" && setOpenId(m.id)} tabIndex={0}>
                    <Avatar name={!isSent(m) ? m.from_name : m.to[0] ?? ""} email={!isSent(m) ? m.from : m.to[0] ?? ""} size={30} />
                    <span className="mail-from ellipsis">{who}</span>
                    <span className="mail-line ellipsis">
                      <span className="mail-subject">{m.subject || t("(no subject)")}</span>
                      {m.snippet && <span className="mail-snippet"> — {m.snippet}</span>}
                    </span>
                    {m.attachments.length > 0 ? <span className="mail-clip" title={m.attachments.join(", ")} aria-label={t("Has attachments")}>📎</span> : <span className="mail-clip" />}
                    <span className="mail-date">{when(m.date)}</span>
                    {/* Always in the same place, so marking one after another is quick. */}
                    <span className="mail-mark" onClick={(e) => e.stopPropagation()}>
                      {!m.seen && isInbox(m) ? (
                        <button className="mark-btn" title={t("Mark as read")} aria-label={t("Mark as read")} onClick={() => markRead(m)}>✓</button>
                      ) : isMarked(m) ? (
                        <button className="mark-btn" title={t("Mark unread")} aria-label={t("Mark unread")} onClick={() => markUnread([m.id])}>↩</button>
                      ) : null}
                    </span>
                    <span className="mail-hover" onClick={(e) => e.stopPropagation()}>
                      {!isArchived(m) && (
                        <button className="icon-btn" title={t("Archive")} aria-label={t("Archive")} onClick={() => move([m.id], "archive")}>🗄</button>
                      )}
                      <button className="icon-btn" title={t("Delete")} aria-label={t("Delete")} onClick={() => move([m.id], "trash")}>🗑</button>
                      {m.seen && !isMarked(m) && (
                        <button className="icon-btn" title={t("Mark as unread")} aria-label={t("Mark as unread")} onClick={() => markUnread([m.id])}>✉</button>
                      )}
                    </span>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </section>
      {compose && <ComposePanel draft={compose} from={fromAccount} onClose={() => setCompose(null)} toast={toast} onSent={() => loadItems()} />}
      {event && (
        <NewEventDialog
          day={Date.now()}
          heading={t("Add to calendar")}
          note={event.draft.source === "ai" ? undefined : event.draft.source === "text" ? t("Your AI couldn't read this one, so this is the first date in the email. Check it before adding.") : t("No date was found in the email. Pick the day and time.")}
          initial={{
            title: event.draft.title,
            date: event.draft.date,
            start: event.draft.all_day ? "" : event.draft.start,
            end: event.draft.all_day ? "" : event.draft.end,
            allDay: event.draft.all_day,
            location: event.draft.location,
            notes: event.draft.notes,
            // The first connected calendar, else this PC.
            calendar: event.calendars[0]?.calendars[0]?.href ?? "",
          }}
          accounts={event.calendars}
          onClose={() => setEvent(null)}
          onCreated={(e) => {
            const where = calendarName(e, event.calendars);
            setEvent(null);
            toast(t("Added “{title}” to {calendar}.", { title: e.title, calendar: where }), "success");
          }}
          toast={toast}
        />
      )}
    </div>
  );
}

/** Mailing lists and bulk senders in the inbox, and leaving them. */
function SubscriptionsView({ toast, account, version, showAccount, accounts, onUnsubscribe, onArchive }: {
  toast: PushToast;
  account: string | null;
  version: number;
  showAccount: boolean;
  accounts: MailAccount[];
  onUnsubscribe: (key: string, name: string, quiet?: boolean) => Promise<UnsubscribeResult | null>;
  onArchive: (ids: string[]) => Promise<void>;
}) {
  const [subs, setSubs] = useState<Subscription[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [working, setWorking] = useState<Set<string>>(new Set());
  const load = () => api.mailSubscriptions().then(setSubs).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
  }, [version]); // eslint-disable-line react-hooks/exhaustive-deps
  const mine = (subs ?? []).filter((s) => !account || s.account_id === account);
  const can = (s: Subscription) => !s.unsubscribed && s.method !== "none";
  const leave = async (list: Subscription[]) => {
    setWorking((w) => new Set([...w, ...list.map((s) => s.key)]));
    const quiet = list.length > 1;
    let left = 0;
    let pages = 0;
    for (const s of list) {
      const r = await onUnsubscribe(s.key, s.name, quiet);
      if (r?.method === "link") pages++;
      else if (r) left++;
      setWorking((w) => {
        const n = new Set(w);
        n.delete(s.key);
        return n;
      });
    }
    setPicked(new Set());
    if (quiet) {
      if (left > 0) toast(t("Unsubscribed from {n} of {total}.", { n: left, total: list.length }), left === list.length ? "success" : "info");
      if (pages > 0) toast(t("{n} unsubscribe pages are open in your browser. The sites may ask you to confirm.", { n: pages }), "info");
    }
    load();
  };
  const how = (s: Subscription) =>
    s.method === "one_click"
      ? t("One click: done for you")
      : s.method === "email"
        ? t("Sends an email to {to} with the subject “{subject}”", { to: s.email_to, subject: s.email_subject })
        : s.method === "link"
          ? t("Opens its unsubscribe page in your browser")
          : t("No unsubscribe link in its mail");
  const selected = mine.filter((s) => picked.has(s.key) && can(s));
  if (!subs) return <p className="muted center pad-lg">{t("Loading…")}</p>;
  return (
    <div className="subs">
      <div className="subs-head">
        <div>
          <h2>{t("Subscriptions")}</h2>
          <p className="muted small">{t("Newsletters, stores and other mailing lists in your inbox, from their unsubscribe headers. Unsubscribe does it for you when the list allows it.")}</p>
        </div>
        <button className="btn" onClick={() => leave(selected)} disabled={selected.length === 0 || working.size > 0}>
          {t("Unsubscribe from selected ({n})", { n: selected.length })}
        </button>
      </div>
      {mine.length === 0 && <p className="muted center pad-lg">{t("No subscriptions found in your inbox yet.")}</p>}
      {mine.map((s) => {
        const done = s.unsubscribed;
        return (
          <div key={s.key} className={`subs-row ${done ? "done" : ""}`}>
            <input
              type="checkbox"
              aria-label={t("Select {name}", { name: s.name })}
              checked={picked.has(s.key)}
              disabled={!can(s)}
              onChange={(e) => setPicked((p) => {
                const n = new Set(p);
                if (e.target.checked) n.add(s.key);
                else n.delete(s.key);
                return n;
              })}
            />
            <Avatar name={s.name} email={s.address} size={32} />
            <div className="grow subs-who">
              <div className="ellipsis"><strong>{s.name}</strong> <span className="muted small">{s.address}</span></div>
              <div className="muted small ellipsis">
                {s.count === 1 ? t("1 email") : t("{n} emails", { n: s.count })} · {t("latest {date}", { date: when(s.latest) })}
                {showAccount && ` · ${accounts.find((a) => a.id === s.account_id)?.email ?? ""}`}
                {!done && ` · ${how(s)}`}
              </div>
            </div>
            <div className="subs-actions">
              {done ? (
                <>
                  <span className="muted small nowrap">
                    {done.method === "link" ? t("Unsubscribe page opened {date}", { date: when(done.at) }) : t("Unsubscribed {date}", { date: when(done.at) })}
                  </span>
                  {s.inbox_ids.length > 0 && (
                    <button className="btn small" onClick={async () => { await onArchive(s.inbox_ids); load(); }}>
                      {s.inbox_ids.length === 1 ? t("Archive its email") : t("Archive its {n} emails", { n: s.inbox_ids.length })}
                    </button>
                  )}
                </>
              ) : (
                <button className="btn small" onClick={() => leave([s])} disabled={!can(s) || working.has(s.key)} title={how(s)}>
                  {working.has(s.key) ? t("Unsubscribing…") : t("Unsubscribe")}
                </button>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** What the AI did with the unread mail: what it left for you and why, and
 * what it marked read (each can be put back), grouped by kind of mail. */
function TriageView({ report, sorting, unread, busy, onSort, onOpen, onRead, onUndo, onAuto, onReply, onCalendar, onUnsubscribe }: {
  report: TriageReport | null;
  sorting: boolean;
  unread: number;
  /** A message being fetched for a reply or read for an event. */
  busy: string | null;
  onSort: () => void;
  onOpen: (id: string) => void;
  onRead: (item: TriageItem) => void;
  onUndo: (ids: string[]) => void;
  onAuto: (on: boolean) => void;
  onReply: (item: TriageItem) => void;
  onCalendar: (item: TriageItem) => void;
  onUnsubscribe: (item: TriageItem) => void;
}) {
  const [filter, setFilter] = useState<MailCategory | "all">("all");
  const auto = (
    <label className="auto-sort small">
      <input type="checkbox" checked={!!report?.auto} onChange={(e) => onAuto(e.target.checked)} />
      <span>
        <strong>{t("Sort new mail automatically")}</strong>
        <span className="muted block">{t("When new mail arrives, your AI sorts it and lets you know if something needs you.")}</span>
      </span>
    </label>
  );
  if (!report?.run_at) {
    return (
      <div className="triage-empty">
        <span className="triage-icon" aria-hidden>✨</span>
        <h2>{t("Let your AI sort your unread mail")}</h2>
        <p className="muted">{t("It reads each unread email, keeps the ones that need you, and marks newsletters, promotions and notices that need nothing as read. This report shows what it did and why, and anything can be put back.")}</p>
        <button className="btn primary" onClick={onSort} disabled={sorting || unread === 0}>{unread === 0 ? t("No unread mail") : t("Sort {n} unread emails", { n: unread })}</button>
        {auto}
      </div>
    );
  }
  const counts = CATEGORIES.map((c) => ({ ...c, n: report.items.filter((i) => i.category === c.id).length })).filter((c) => c.n > 0);
  const inFilter = (i: TriageItem) => filter === "all" || i.category === filter;
  const attention = report.items.filter((i) => i.important && inFilter(i));
  const cleared = report.items.filter((i) => !i.important && inFilter(i));
  const undoable = cleared.filter((i) => i.marked_read && i.seen).map((i) => i.id);
  // By category in CATEGORIES' order; missed replies first, then newest.
  const grouped = (list: TriageItem[]) =>
    CATEGORIES.map((c) => ({ ...c, items: list.filter((i) => i.category === c.id).sort((a, b) => Number(b.missed) - Number(a.missed) || b.date - a.date) })).filter((g) => g.items.length > 0);
  // What to do about it, as a button that does it: Reply writes the reply,
  // Add to calendar reads the event, the others open the message.
  const next = (i: TriageItem) => {
    if (!i.important || i.next === "none") return null;
    const act = () => (i.next === "reply" ? onReply(i) : i.next === "calendar" ? onCalendar(i) : onOpen(i.id));
    return (
      <button className={`next-chip next-${i.next.replace(" ", "-")}`} onClick={act} disabled={busy === i.id}>
        {busy === i.id ? (i.next === "calendar" ? t("Reading the email…") : t("Opening…")) : t(NEXT_LABEL[i.next])}
      </button>
    );
  };
  const leave = (i: TriageItem) => {
    const u = i.unsubscribe;
    if (!u || !(i.category === "newsletters" || i.category === "promotions" || !i.important)) return null;
    if (u.unsubscribed) return <span className="muted small nowrap">{t("Unsubscribed")}</span>;
    const how = u.method === "email" ? t("Sends an email to {to} with the subject “{subject}”", { to: u.email_to, subject: u.email_subject }) : u.method === "link" ? t("Opens its unsubscribe page in your browser") : t("One click: done for you");
    return <button className="btn ghost small" onClick={() => onUnsubscribe(i)} title={how}>{t("Unsubscribe")}</button>;
  };
  const row = (i: TriageItem, actions: ReactNode) => (
    <div key={i.id} className={`triage-row ${i.seen && i.important ? "done" : ""} ${i.missed ? "missed" : ""}`}>
      <Avatar name={i.from_name} email={i.from} size={32} />
      <div className="grow">
        <div className="row between">
          <span className="ellipsis"><strong>{i.from_name || i.from}</strong> <span className="muted">· {i.subject || t("(no subject)")}</span></span>
          <span className="muted small nowrap">{when(i.date)}</span>
        </div>
        <div className="triage-reason">
          <span className={categoryClass(i.category)}>{categoryLabel(i.category)}</span>
          {i.missed && <span className="missed-chip" title={t("It needs your reply, and none was sent in over a day.")}>{t("Missed")}</span>}
          {t(i.reason)}
        </div>
      </div>
      <div className="triage-actions">
        {next(i)}
        {leave(i)}
        {actions}
      </div>
    </div>
  );
  const groups = (list: TriageItem[], actions: (i: TriageItem) => ReactNode) =>
    grouped(list).map((g) => {
      const missed = g.items.filter((i) => i.missed).length;
      return (
        <div key={g.id} className="triage-group">
          <h4 className="triage-group-head">
            <span className={categoryClass(g.id)}>{t(g.label)}</span> <span className="muted">{g.items.length}</span>
            {missed > 0 && <span className="missed-chip">{t("{n} missed", { n: missed })}</span>}
          </h4>
          {g.items.map((i) => row(i, actions(i)))}
        </div>
      );
    });
  return (
    <div className="triage">
      <div className="triage-head">
        <div>
          <h2>{t("AI report")}</h2>
          <p className="muted small">
            {t("Last sorted {n} unread emails {time} with {model}. This shows the past week.", { n: report.looked_at, time: new Date(report.run_at).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }), model: report.model || t("your model") })}
          </p>
        </div>
        <button className="btn" onClick={onSort} disabled={sorting || unread === 0}>{t("Sort again")}</button>
      </div>
      {auto}

      <div className="cat-filters" role="group" aria-label={t("Show")}>
        <button className={filter === "all" ? "active" : ""} aria-pressed={filter === "all"} onClick={() => setFilter("all")}>
          {t("All")} <span className="muted">{report.items.length}</span>
        </button>
        {counts.map((c) => (
          <button key={c.id} className={filter === c.id ? "active" : ""} aria-pressed={filter === c.id} onClick={() => setFilter(filter === c.id ? "all" : c.id)}>
            <i className={`cat-dot ${categoryClass(c.id)}`} aria-hidden />
            {t(c.label)} <span className="muted">{c.n}</span>
          </button>
        ))}
      </div>

      <section className="triage-section">
        <h3>{t("Needs your attention")} <span className="muted">({attention.filter((i) => !i.seen).length})</span></h3>
        {attention.length === 0 && <p className="muted small">{t("Nothing needed you.")}</p>}
        {groups(attention, (i) => (
          <>
            <button className="btn small" onClick={() => onOpen(i.id)}>{t("Open")}</button>
            {i.seen ? <span className="muted small">✓ {t("Read")}</span> : <button className="btn small" onClick={() => onRead(i)}>{t("Mark as read")}</button>}
          </>
        ))}
      </section>

      <section className="triage-section">
        <div className="row between">
          <h3>{t("Marked as read by the AI")} <span className="muted">({cleared.length})</span></h3>
          {undoable.length > 1 && <button className="btn ghost small" onClick={() => onUndo(undoable)}>{t("Undo all")}</button>}
        </div>
        {cleared.length === 0 && <p className="muted small">{t("Nothing was marked as read.")}</p>}
        {groups(cleared, (i) => (
          <>
            <button className="btn ghost small" onClick={() => onOpen(i.id)}>{t("Open")}</button>
            {i.marked_read && i.seen ? <button className="btn small" onClick={() => onUndo([i.id])}>{t("Mark unread")}</button> : <span className="muted small">{t("Back in your inbox")}</span>}
          </>
        ))}
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
