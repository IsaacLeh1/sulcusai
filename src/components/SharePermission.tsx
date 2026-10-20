// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Chat, type PrivacyMode, type SentEntry, type ShareReview } from "../api";
import { useWaitingApprovals } from "../approvals";
import { t, tx } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** The scale's names, 1 to 5, as the core gives them. */
export const LEVELS = [
  tx("No personal information"),
  tx("A first or last name"),
  tx("Your interests and searches"),
  tx("Identifying information"),
  tx("Sensitive information"),
];

/** Texts the core sends in a review, listed so they're translated. */
export const REVIEW_TEXTS = [
  tx("Search words"),
  tx("Description"),
  tx("Picture"),
  tx("Title"),
  tx("Notes"),
  tx("Invites"),
  tx("Question"),
  tx("Web address"),
  tx("Typed into the page"),
  tx("Place"),
  tx("Your location (exact coordinates)"),
  tx("Your town (from your profile)"),
  tx("Looked up"),
  tx("Symbol or name"),
  tx("Looking for"),
  tx("Near"),
  tx("To"),
  tx("Cc"),
  tx("Subject"),
  tx("Message"),
  tx("Your name and email address"),
  tx("Sent"),
  tx("Secret"),
  tx("Command"),
  tx("What to work out"),
  tx("What it's given"),
  tx("what looks like a password, key or secret"),
  tx("a card number"),
  tx("an ID number"),
  tx("a phone number"),
  tx("your email address"),
  tx("an email address"),
  tx("a street address"),
  tx("your full name"),
  tx("your first name"),
  tx("your last name"),
  tx("where you live (your town)"),
  tx("your location (exact coordinates)"),
  tx("your name and email address"),
  tx("A search engine can keep your searches with your internet address and the time, and use them to build a profile of your interests for ads, or hand them over if a court asks."),
  tx("SulcusAI also opens the top results to read them: each of those sites sees your internet address and which page was read."),
  tx("The site sees your internet address and the page asked for, can keep both in its logs, and may share them with ad and analytics companies. Anything written into the address goes to it too."),
  tx("Open-Meteo learns which place you want the weather for, and from coordinates roughly where you are. It can keep this with your internet address."),
  tx("Wikipedia keeps server logs with internet addresses for a short time. What you look up isn't made public."),
  tx("Yahoo Finance can log which stocks, funds and coins you look up, with your internet address, and use that for ads."),
  tx("OpenStreetMap's servers receive what you're looking for and where. They keep logs with internet addresses."),
  tx("The site sees everything opened, typed and clicked. With Browser control on, this happens in your own browser where you're signed in, so the site knows who you are."),
  tx("They receive it on their PC and can read, keep and share it. Their assistant reads it too."),
  tx("The people it's sent to get your name and email address with the message and can keep or forward it. Your mail provider keeps a copy."),
  tx("Connectors are programs on this PC. SulcusAI can't see where a connector sends what it's given: it may pass it on to an online service."),
  tx("Your private connection is on, so these services see the proxy's or Tor's address instead of yours."),
  tx("Your message"),
  tx("Earlier messages in this chat"),
  tx("Pictures in this chat"),
  tx("About you (from your profile)"),
  tx("What it remembers about you"),
  tx("Your connected accounts"),
  tx("AI companies keep what they're sent for a while (often about 30 days) to check for misuse. Depending on your account's settings, some use it to improve their models, and their staff may read conversations that get flagged."),
  tx("Emails, phone numbers, card and ID numbers, keys, street addresses and your name are swapped for placeholders before sending, and put back in the reply."),
  tx("Cloud redaction is off (Settings, Cloud models), so everything goes as written."),
];

/** A level's number and name, coloured from green (1) to red (5). */
export function LevelBadge({ level }: { level: number }) {
  return (
    <span className={`share-level share-level-${level}`}>
      <span className="share-level-n">{level}</span>
      {t(LEVELS[level - 1] ?? LEVELS[4])}
    </span>
  );
}

/** Privacy mode: before a reply sends something off this PC, a popup with
 * what goes, who gets it, what they could do with it and how personal it is
 * (1 to 5). The person shares it, rewords or leaves out parts first, or
 * doesn't share it; the reply carries on either way. */
export function SharePermission({ chats, onOpenChat }: { chats: Chat[]; onOpenChat: (id: string) => void }) {
  const [asks, setAsks] = useWaitingApprovals(chats, (p) => p.preview.kind === "privacy");
  const [values, setValues] = useState<Record<string, string>>({});
  const [left, setLeft] = useState<Record<string, boolean>>({});
  const [forChat, setForChat] = useState(false);
  const p = asks[0];
  if (!p) return null;
  let r: ShareReview | null = null;
  try {
    r = p.preview.detail ? (JSON.parse(p.preview.detail) as ShareReview) : null;
  } catch {
    r = null;
  }
  const chat = chats.find((c) => c.id === p.chat_id);
  const reset = () => {
    setValues({});
    setLeft({});
    setForChat(false);
    setAsks((all) => all.filter((x) => x.call_id !== p.call_id));
  };
  const edits: { key: string; value: string | null }[] = (r?.items ?? [])
    .filter((i) => i.key)
    .flatMap((i): { key: string; value: string | null }[] => {
      const key = i.key!;
      if (left[key]) return [{ key, value: null }];
      const v = values[key];
      return v !== undefined && v !== i.value ? [{ key, value: v }] : [];
    });
  const blank = (r?.items ?? []).some((i) => i.key && !left[i.key] && values[i.key] !== undefined && values[i.key].trim() === "");
  const answer = async (d: "allow" | "deny") => {
    const callId = p.call_id;
    const decision = d === "allow" && forChat && edits.length === 0 ? "always" : d;
    reset();
    if (d === "allow" && edits.length) await api.privacyShare(callId, edits).catch(() => {});
    await api.answerApproval(callId, decision).catch(() => {});
  };
  if (!r) {
    return (
      <Modal title={t(p.preview.title)} onClose={() => answer("deny")}>
        <p>{t("The assistant wants to send something off this PC.")}</p>
        <div className="modal-actions">
          <button className="btn ghost" onClick={() => answer("deny")}>
            {t("Don't share")}
          </button>
          <button className="btn primary" onClick={() => answer("allow")}>
            {t("Share")}
          </button>
        </div>
      </Modal>
    );
  }
  return (
    <Modal title={t("Send this to {to}?", { to: r.to })} onClose={() => answer("deny")}>
      <p>
        {chat
          ? t("Privacy mode: to answer in “{title}”, the assistant wants to send this off your PC.", { title: chat.title })
          : t("Privacy mode: the assistant wants to send this off your PC.")}
      </p>
      <div className="share-head">
        <span className="muted small">{t("How personal (1 to 5)")}</span>
        <div className="share-meter" aria-hidden>
          {[1, 2, 3, 4, 5].map((n) => (
            <span key={n} className={n <= r!.level ? `on share-level-${r!.level}` : ""} />
          ))}
        </div>
        <LevelBadge level={r.level} />
      </div>
      {r.found.length > 0 && (
        <p className="small">
          {t("Why:")} {r.found.map((f) => t(f.what)).join(", ")}
        </p>
      )}
      <h3 className="share-sub">{t("What they receive")}</h3>
      <ul className="share-items">
        {r.items.map((i, n) => {
          const key = i.key ?? `fixed-${n}`;
          const out = !!(i.key && left[i.key]);
          return (
            <li key={key} className={out ? "left-out" : ""}>
              <div className="share-item-head">
                <span>{t(i.label)}</span>
                <span className={`share-dot share-level-${i.level}`} title={t(LEVELS[i.level - 1] ?? "")}>
                  {i.level}
                </span>
                {i.removable && i.key && (
                  <label className="share-send">
                    <input type="checkbox" checked={!out} onChange={(e) => setLeft((l) => ({ ...l, [i.key!]: !e.target.checked }))} />
                    {t("Send")}
                  </label>
                )}
              </div>
              {i.editable && i.key && !out ? (
                <textarea
                  className="input"
                  rows={Math.min(4, Math.max(1, Math.ceil(i.value.length / 60)))}
                  value={values[i.key] ?? i.value}
                  onChange={(e) => setValues((v) => ({ ...v, [i.key!]: e.target.value }))}
                  aria-label={t(i.label)}
                />
              ) : (
                <div className="share-value">{out ? t("Left out") : i.value || "—"}</div>
              )}
            </li>
          );
        })}
      </ul>
      {r.items.some((i) => i.editable) && <p className="muted small">{t("You can reword what's sent, or untick parts to leave them out.")}</p>}
      <h3 className="share-sub">{t("What they could do with it")}</h3>
      <ul className="small">
        {r.could.map((c) => (
          <li key={c}>{t(c)}</li>
        ))}
      </ul>
      {r.note && <p className="muted small">{t(r.note)}</p>}
      {r.kind === "cloud" && <p className="muted small">{t("Asked once for this chat, and again only if a later message is more personal.")}</p>}
      {edits.length === 0 && r.kind !== "cloud" && (
        <label className="share-forchat small">
          <input type="checkbox" checked={forChat} onChange={(e) => setForChat(e.target.checked)} />
          {t("Don't ask again in this chat for level {level} or lower", { level: r.level })}
        </label>
      )}
      <div className="modal-actions">
        {chat && (
          <button className="btn ghost" onClick={() => onOpenChat(p.chat_id)}>
            {t("Show the chat")}
          </button>
        )}
        <button className="btn ghost" onClick={() => answer("deny")}>
          {t("Don't share")}
        </button>
        <button className="btn primary" disabled={blank} onClick={() => answer("allow")}>
          {edits.length ? t("Share my version") : t("Share")}
        </button>
      </div>
    </Modal>
  );
}

/** Settings: privacy mode on or off, and the level from which it asks. */
export function PrivacyModeSettings({ toast }: { toast: PushToast }) {
  const [mode, setMode] = useState<PrivacyMode | null>(null);
  useEffect(() => {
    api.privacyMode().then(setMode).catch(() => setMode({ on: false, ask_from: 1 }));
  }, []);
  const save = async (next: PrivacyMode) => {
    setMode(next);
    try {
      await api.setPrivacyMode(next);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  if (!mode) return null;
  return (
    <section className="card">
      <h2>{t("Privacy mode")}</h2>
      <p className="muted">
        {t("Before the assistant sends anything off your PC (a search, a web page, a message), it shows you what goes, who gets it, what they could do with it, and how personal it is. You share it, change it first, or don't share it.")}
      </p>
      <div className="toggle-row">
        <div>
          <strong>{t("Show me what's sent before it goes")}</strong>
          <span className="small muted block">{mode.on ? t("On. Each search, page or message off this PC is shown first.") : t("Off. The assistant sends what it needs without asking.")}</span>
        </div>
        <label className="switch" title={mode.on ? t("On") : t("Off")}>
          <input type="checkbox" checked={mode.on} onChange={(e) => save({ ...mode, on: e.target.checked })} aria-label={t("Privacy mode")} />
          <span />
        </label>
      </div>
      {mode.on && (
        <label className="form small">
          {t("Ask me before sending")}
          <select className="input" value={mode.ask_from} onChange={(e) => save({ ...mode, ask_from: Number(e.target.value) })}>
            <option value={1}>{t("Everything")}</option>
            <option value={2}>{t("Anything with a name in it, or more personal (level 2 and up)")}</option>
            <option value={3}>{t("My interests and searches, or more personal (level 3 and up)")}</option>
            <option value={4}>{t("Identifying or sensitive information (level 4 and up)")}</option>
            <option value={5}>{t("Only sensitive information (level 5)")}</option>
          </select>
        </label>
      )}
      <ol className="share-scale small">
        {LEVELS.map((l, i) => (
          <li key={l}>
            <LevelBadge level={i + 1} />{" "}
            <span className="muted">
              {t(
                [
                  "Nothing about you, like a stock price or the weather in a city you name.",
                  "Just your first or your last name.",
                  "What you're interested in: what you search for, read or ask about.",
                  "What identifies you: your full name or an email address.",
                  "Keys, passwords and secrets, addresses, phone numbers, card and ID numbers, your exact location.",
                ][i],
              )}
            </span>
          </li>
        ))}
      </ol>
      <p className="muted small">{t("Scheduled tasks run when nobody's there to ask, so in privacy mode they only send what's below the level you're asked from.")}</p>
    </section>
  );
}

/** Settings: everything sent off this PC lately (searches, pages, emails,
 * turns sent to online models), with how personal each was. Kept on this
 * PC, encrypted, for 30 days. */
export function SentLog({ toast }: { toast: PushToast }) {
  const [list, setList] = useState<SentEntry[] | null>(null);
  const [days, setDays] = useState(7);
  const [personal, setPersonal] = useState(false);
  const [all, setAll] = useState(false);
  const load = () => api.privacySent().then(setList).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  if (list === null) return null;
  const since = Date.now() - days * 86_400_000;
  const shown = list.filter((s) => s.at >= since && (!personal || s.level >= 3));
  const visible = all ? shown : shown.slice(0, 25);
  const clear = async () => {
    try {
      await api.privacySentClear();
      setList([]);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <section className="card">
      <h2>{t("What left this PC")}</h2>
      <p className="muted">
        {t("Everything the assistant sent off your PC lately: searches, pages, emails and messages to online AI models, with how personal each was. This list stays on your PC, encrypted, for 30 days.")}
      </p>
      <div className="sent-filters">
        <select className="input" value={days} onChange={(e) => setDays(Number(e.target.value))} aria-label={t("How far back")}>
          <option value={1}>{t("Today")}</option>
          <option value={7}>{t("Past week")}</option>
          <option value={30}>{t("Past 30 days")}</option>
        </select>
        <label className="row small">
          <input type="checkbox" checked={personal} onChange={(e) => setPersonal(e.target.checked)} />
          {t("Level 3 and up only")}
        </label>
        <button className="btn ghost small" onClick={load}>
          {t("Refresh")}
        </button>
        {list.length > 0 && (
          <button className="btn ghost small danger" onClick={clear}>
            {t("Clear the list")}
          </button>
        )}
      </div>
      {shown.length === 0 ? (
        <p className="muted small">{t("Nothing in this time.")}</p>
      ) : (
        <ul className="sent-list">
          {visible.map((s, i) => (
            <li key={`${s.at}-${i}`} className={s.sent ? "" : "not-sent"}>
              <span className={`share-dot share-level-${s.level}`} title={t(LEVELS[s.level - 1] ?? "")}>
                {s.level}
              </span>
              <div className="sent-body">
                <div className="sent-head">
                  <strong>{s.to}</strong>
                  <span className="muted small">{new Date(s.at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}</span>
                  {!s.sent && <span className="sent-chip">{t("Not sent")}</span>}
                </div>
                <div className="sent-what small">{s.what}</div>
              </div>
            </li>
          ))}
        </ul>
      )}
      {!all && shown.length > visible.length && (
        <button className="btn ghost small" onClick={() => setAll(true)}>
          {t("Show all {n}", { n: shown.length })}
        </button>
      )}
    </section>
  );
}

/** The scale's descriptions, listed so they're translated. */
export const SCALE_TEXTS = [
  tx("Nothing about you, like a stock price or the weather in a city you name."),
  tx("Just your first or your last name."),
  tx("What you're interested in: what you search for, read or ask about."),
  tx("What identifies you: your full name or an email address."),
  tx("Keys, passwords and secrets, addresses, phone numbers, card and ID numbers, your exact location."),
];
