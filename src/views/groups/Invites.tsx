// SPDX-License-Identifier: AGPL-3.0-only
// Invites that travel as direct messages: a person is invited from their
// profile or DM, the invite waits in Pending invites until answered, and the
// first visit to a joined group shows a short welcome.
import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import { confirmed } from "../../confirm";
import type { PushToast } from "../../components/Toasts";
import { social, type Dm, type GroupView } from "./socialApi";

// Plain words first so an app without this view still shows something useful.
const MARK = "sulcus-invite:";

export function inviteText(g: GroupView, code: string): string {
  return `Invite to ${g.name} (${g.members.length} members). ${MARK}${g.hub}:${code}:${encodeURIComponent(g.name)}:${g.members.length}`;
}

export type ParsedInvite = { hub: string; code: string; name: string; members: number };

export function parseInvite(text: string): ParsedInvite | null {
  const m = new RegExp(`${MARK}([^\\s:]+):([^\\s:]+):([^\\s:]*):(\\d+)`).exec(text);
  if (!m) return null;
  let name = m[3];
  try {
    name = decodeURIComponent(m[3]);
  } catch {
    // A damaged name still leaves a usable invite.
  }
  return { hub: m[1], code: m[2], name, members: Number(m[4]) };
}

const answeredKey = "social.invites.answered";
function answered(): string[] {
  try {
    return JSON.parse(localStorage.getItem(answeredKey) ?? "[]");
  } catch {
    return [];
  }
}
function markAnswered(code: string) {
  try {
    localStorage.setItem(answeredKey, JSON.stringify([...answered(), code].slice(-200)));
  } catch {
    // Without storage it shows again next time, which is harmless.
  }
}

const welcomeKey = (group: string) => `social.welcome.${group}`;
/** True the first time a group is opened after joining it from an invite here. */
export function takeWelcome(group: string): boolean {
  try {
    if (localStorage.getItem(welcomeKey(group)) !== "due") return false;
    localStorage.setItem(welcomeKey(group), "shown");
    return true;
  } catch {
    return false;
  }
}

/** Pick a group to invite someone to, confirm, then the invite goes to them as a direct message. */
export function InviteToGroup({
  groups,
  person,
  personName,
  via,
  toast,
  onClose,
}: {
  groups: GroupView[];
  person: string;
  personName: string;
  via?: string;
  toast: PushToast;
  onClose: () => void;
}) {
  // Only groups this person isn't in, where an invite can be made.
  const choices = groups.filter((g) => (g.role === "owner" || g.role === "moderator") && !g.members.some((m) => m.id === person));
  const [pick, setPick] = useState(choices[0]?.id ?? "");
  const shared = via ?? groups.find((g) => g.members.some((m) => m.id === person))?.id;
  const send = async () => {
    const g = choices.find((x) => x.id === pick);
    if (!g || !shared) return;
    if (!(await confirmed(t("Invite {name} to {group}?", { name: personName, group: g.name })))) return;
    try {
      const code = await social.invite(g.id, 1, 168);
      await social.dmSend(shared, person, inviteText(g, code));
      toast(t("Invite sent to {name}.", { name: personName }));
      onClose();
    } catch (e) {
      toast(errorText(e));
    }
  };
  return createPortal(
    <div className="social-modal" role="dialog" aria-label={t("Invite to group")} onClick={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="social-modal-box" onClick={(e) => e.stopPropagation()}>
        <h3>{t("Invite {name} to a group", { name: personName })}</h3>
        {choices.length === 0 ? (
          <p className="small muted">{t("There's no group you can invite them to: they're in all of yours, or you don't moderate any.")}</p>
        ) : !shared ? (
          <p className="small muted">{t("You need a group in common to send them an invite.")}</p>
        ) : (
          <label>
            {t("Group")}
            <select value={pick} onChange={(e) => setPick(e.target.value)}>
              {choices.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <div className="row">
          <button className="btn-discord" disabled={!pick || !shared} onClick={send}>
            {t("Send invite")}
          </button>
          <button className="btn-discord secondary" onClick={onClose}>
            {t("Cancel")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** Every invite that came in a direct message and hasn't been answered, with Accept and Decline. */
export function PendingInvites({ groups, myName, toast, onJoined }: { groups: GroupView[]; myName: string; toast: PushToast; onJoined: () => void }) {
  const [dms, setDms] = useState<Dm[]>([]);
  const [done, setDone] = useState<string[]>(answered);
  const load = useCallback(() => {
    social.dms().then(setDms).catch(() => setDms([]));
  }, []);
  useEffect(() => {
    load();
    const un = listen("social:dm", load);
    return () => {
      un.then((f) => f());
    };
  }, [load]);
  const pending = dms
    .filter((d) => !d.from_me)
    .map((d) => ({ d, inv: parseInvite(d.text) }))
    .filter((x): x is { d: Dm; inv: ParsedInvite } => !!x.inv && !done.includes(x.inv.code) && !groups.some((g) => x.inv!.code.startsWith(g.id + ".")));
  if (pending.length === 0) return null;
  const answer = (code: string) => {
    markAnswered(code);
    setDone((x) => [...x, code]);
  };
  return (
    <section className="social-invites" aria-label={t("Pending invites")}>
      <h4>{t("Pending invites")}</h4>
      {pending.map(({ d, inv }) => (
        <div key={inv.code} className="social-invite-card">
          <div>
            <strong>{inv.name}</strong>
            <div className="small muted">
              {inv.members === 1 ? t("1 member") : t("{n} members", { n: inv.members })} · {t("from {name}", { name: d.with_name })}
            </div>
          </div>
          <div className="row">
            <button
              className="btn-discord"
              onClick={async () => {
                try {
                  await social.join(inv.hub, inv.code, myName);
                  try {
                    localStorage.setItem(welcomeKey(inv.code.split(".")[0]), "due");
                  } catch {
                    // The welcome is a nicety; joining still works without it.
                  }
                  answer(inv.code);
                  toast(t("Asked to join. The group appears once they let you in."));
                  onJoined();
                } catch (e) {
                  toast(errorText(e));
                }
              }}
            >
              {t("Accept")}
            </button>
            <button className="btn-discord secondary" onClick={() => answer(inv.code)}>
              {t("Decline")}
            </button>
          </div>
        </div>
      ))}
    </section>
  );
}

/** Shown once on a group's first visit after joining: its rules and where to start. */
export function Welcome({ g, onPick, onClose }: { g: GroupView; onPick: (channel: string) => void; onClose: () => void }) {
  const text = g.channels.filter((c) => c.kind !== "voice").slice(0, 4);
  const rules = [g.rules.block_links && t("No links."), g.rules.blocked_words.length > 0 && t("Some words aren't allowed.")].filter(Boolean) as string[];
  return (
    <div className="note social-welcome">
      <strong>{t("Welcome to {group}", { group: g.name })}</strong>
      {rules.length > 0 && <p className="small">{t("Rules here: {rules}", { rules: rules.join(" ") })}</p>}
      <div className="row small">
        {t("Start in:")}
        {text.map((c) => (
          <button key={c.id} className="chip" onClick={() => (onPick(c.id), onClose())}>
            #{c.name}
          </button>
        ))}
      </div>
      <button className="link small" onClick={onClose}>
        {t("Close")}
      </button>
    </div>
  );
}
