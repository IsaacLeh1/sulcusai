// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api } from "../../api";
import { t } from "../../i18n";
import { confirmed } from "../../confirm";
import { social, type GroupView } from "./socialApi";
import type { PushToast } from "../../components/Toasts";

export type Contact = { id: string; name: string };

interface GroupMembersProps {
  g: GroupView;
  me: string;
  run: (f: () => Promise<unknown>) => Promise<void>;
  contacts?: Contact[];
  toast?: PushToast;
}

/** Members panel for a group: invite direct message friends with 1 click, manage roles, and review membership. */
export function GroupMembers({ g, me, run, contacts = [], toast }: GroupMembersProps) {
  const [invite, setInvite] = useState<string | null>(null);
  const [invitingId, setInvitingId] = useState<string | null>(null);
  const [invitedIds, setInvitedIds] = useState<Set<string>>(new Set());
  const [friendQuery, setFriendQuery] = useState("");

  const owner = g.role === "owner";
  const mod = owner || g.role === "moderator";

  const nonMembers = contacts.filter((c) => !g.members.some((m) => m.id === c.id));
  const filteredFriends = nonMembers.filter((c) =>
    friendQuery.trim() ? c.name.toLowerCase().includes(friendQuery.trim().toLowerCase()) : true
  );

  const inviteFriend = async (c: Contact) => {
    try {
      setInvitingId(c.id);
      const code = await social.invite(g.id, 1, 24);
      const messageText = t("Join my group '{name}' with invite code: {code}", { name: g.name, code });
      await api.peopleSend(c.id, messageText, null);
      setInvitedIds((prev) => new Set(prev).add(c.id));
      if (toast) toast(t("Invite sent to {name}", { name: c.name }));
    } catch (e) {
      if (toast) toast(String(e));
    } finally {
      setInvitingId(null);
    }
  };

  return (
    <div className="group-members-pane" style={{ padding: "16px 20px", maxWidth: 720 }}>
      {/* 1-Click direct message invite section */}
      {mod && nonMembers.length > 0 && (
        <div className="social-invite-card" style={{ marginBottom: 24, padding: "14px 16px", borderRadius: 8, background: "var(--social-side, var(--panel))", border: "1px solid var(--social-sel, var(--border))" }}>
          <h4 style={{ margin: "0 0 4px", fontSize: 16 }}>{t("Invite friends to {name}", { name: g.name })}</h4>
          <p className="small muted" style={{ margin: "0 0 12px" }}>
            {t("Send a direct message invite to friends you are already connected with.")}
          </p>

          {nonMembers.length > 5 && (
            <input
              type="search"
              placeholder={t("Filter contacts...")}
              value={friendQuery}
              onChange={(e) => setFriendQuery(e.target.value)}
              style={{ width: "100%", marginBottom: 10, padding: "6px 10px" }}
            />
          )}

          <div className="social-friend-invites" style={{ display: "flex", flexDirection: "column", gap: 6, maxHeight: 220, overflow: "auto" }}>
            {filteredFriends.map((c) => {
              const already = invitedIds.has(c.id);
              const busy = invitingId === c.id;
              return (
                <div key={c.id} style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "6px 8px", borderRadius: 6, background: "rgba(0, 0, 0, 0.15)" }}>
                  <span style={{ fontWeight: 500, fontSize: 14 }}>{c.name}</span>
                  <button
                    type="button"
                    className={already ? "btn-discord secondary" : "btn-discord"}
                    style={{ minWidth: 80, padding: "4px 12px", fontSize: 13 }}
                    disabled={already || busy}
                    onClick={() => inviteFriend(c)}
                  >
                    {busy ? t("Sending...") : already ? t("Invited ✓") : t("Invite")}
                  </button>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Manual invite code generator */}
      {mod && (
        <div style={{ marginBottom: 20 }}>
          <div className="row" style={{ gap: 8, alignItems: "center" }}>
            <button
              type="button"
              className="btn-discord secondary"
              onClick={() => run(async () => setInvite(await social.invite(g.id, 1, 24)))}
            >
              {t("Make an invite code (1 person, 24 hours)")}
            </button>
            {invite && <code className="selectable" style={{ padding: "4px 8px", borderRadius: 4, background: "var(--code-bg)" }}>{invite}</code>}
          </div>
          {invite && <p className="small muted" style={{ marginTop: 6 }}>{t("Give this code to someone linked in People. It works once and is valid for 24 hours.")}</p>}
        </div>
      )}

      {/* Existing members */}
      <h4 style={{ margin: "0 0 8px", fontSize: 15 }}>{t("Members ({n})", { n: g.members.length })}</h4>
      <ul className="plain" style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 24 }}>
        {g.members.map((m) => (
          <li key={m.id} className="row" style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "6px 8px", borderRadius: 6, background: "var(--social-side, var(--panel))" }}>
            <span>
              <strong>{m.name}</strong>
              {m.assistant && <span className="small muted"> ({t("assistant")})</span>}
              <span className="small muted"> · {m.role === "owner" ? t("Owner") : m.role === "moderator" ? t("Moderator") : t("Member")}</span>
              {m.muted && <span className="small muted"> · {t("Muted")}</span>}
            </span>
            {m.id !== me && (
              <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                {!m.assistant && !m.blocked && (
                  <button
                    className="link small"
                    onClick={() => {
                      const text = window.prompt(t("Message to {name}", { name: m.name }));
                      if (text) run(() => social.dmSend(g.id, m.id, text));
                    }}
                  >
                    {t("Message")}
                  </button>
                )}
                <button className="link small" onClick={() => run(() => social.block(m.id, !m.blocked, false))}>
                  {m.blocked ? t("Unblock") : t("Block")}
                </button>
                {owner && m.role !== "owner" && (
                  <button className="link small" onClick={() => run(() => social.act(g.id, { op: "set_role", member: m.id, role: m.role === "moderator" ? "member" : "moderator" }))}>
                    {m.role === "moderator" ? t("Make member") : t("Make moderator")}
                  </button>
                )}
                {mod && m.role === "member" && (
                  <>
                    <button className="link small" onClick={() => run(() => social.act(g.id, { op: "mute", member: m.id, until: m.muted ? 0 : Date.now() + 3600_000 }))}>
                      {m.muted ? t("Unmute") : t("Mute for an hour")}
                    </button>
                    <button className="link small" onClick={async () => (await confirmed(t("Remove {name} from the group?", { name: m.name }))) && run(() => social.act(g.id, { op: "kick", member: m.id }))}>
                      {t("Remove")}
                    </button>
                    <button className="link small danger" onClick={async () => (await confirmed(t("Ban {name}? They can't join again.", { name: m.name }))) && run(() => social.act(g.id, { op: "ban", member: m.id }))}>
                      {t("Ban")}
                    </button>
                  </>
                )}
              </div>
            )}
          </li>
        ))}
      </ul>

      <div style={{ display: "flex", gap: 10, marginTop: 16 }}>
        {mod && (
          <button
            type="button"
            className="btn-discord secondary"
            onClick={() => {
              const name = window.prompt(t("Assistant name"));
              if (name) run(() => social.addAssistant(g.id, name, g.channels[0]?.id ?? "general"));
            }}
          >
            {t("Add an assistant (answers when named)")}
          </button>
        )}
        <button
          type="button"
          className="btn-discord danger"
          onClick={async () => (await confirmed(t("Leave {name}?", { name: g.name }))) && run(() => social.leave(g.id))}
        >
          {t("Leave group")}
        </button>
      </div>
    </div>
  );
}
