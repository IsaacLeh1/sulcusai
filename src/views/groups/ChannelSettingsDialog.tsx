// SPDX-License-Identifier: AGPL-3.0-only
// Channel settings modal and settings launcher gear.
import { useRef, useState } from "react";
import { useDismiss } from "../../useDismiss";
import { t } from "../../i18n";
import type { ChannelSettings, ChannelView, GroupView, Perms } from "./socialApi";

export function GearIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
      <path
        fill="currentColor"
        d="M9.4 1l.3 1.8 1.2.5 1.5-1 1.3 1.3-1 1.5.5 1.2 1.8.3v1.8l-1.8.3-.5 1.2 1 1.5-1.3 1.3-1.5-1-1.2.5-.3 1.8H7.6l-.3-1.8-1.2-.5-1.5 1-1.3-1.3 1-1.5-.5-1.2L1 9.4V7.6l1.8-.3.5-1.2-1-1.5 1.3-1.3 1.5 1 1.2-.5L6.6 1zM8 5.5a2.5 2.5 0 100 5 2.5 2.5 0 000-5z"
      />
    </svg>
  );
}

/** The gear on a channel row (moderators): opens its settings without opening the channel. */
export function ChannelGear({ onOpen }: { onOpen: () => void }) {
  return (
    <span
      role="button"
      tabIndex={0}
      className="social-ch-gear"
      title={t("Edit channel")}
      aria-label={t("Edit channel")}
      onClick={(e) => {
        e.stopPropagation();
        onOpen();
      }}
      onKeyDown={(e) => {
        if (e.key !== "Enter") return;
        e.stopPropagation();
        onOpen();
      }}
    >
      <GearIcon />
    </span>
  );
}

const PERM_NAMES = [
  ["view", "See the channel"],
  ["send", "Send messages"],
  ["connect", "Join the call"],
  ["speak", "Speak"],
] as const;

/** A channel's settings, like Discord's: overview, permissions, and delete. */
export function ChannelSettingsDialog({ g, c, onClose, onSave, onDelete }: { g: GroupView; c: ChannelView; onClose: () => void; onSave: (s: ChannelSettings) => void; onDelete: () => void }) {
  const [tab, setTab] = useState<"overview" | "perms">("overview");
  const [s, setS] = useState<ChannelSettings>({ name: c.name, topic: c.topic, slow_secs: c.slow_secs, user_limit: c.user_limit, bitrate: c.bitrate, mark_voice_changes: !!c.mark_voice_changes, adults_only: c.adults_only, private: c.private, perms: c.perms ?? {} });
  const [adding, setAdding] = useState("");
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box);
  const voice = c.kind === "voice";
  const label = (key: string) => (key === "role:member" ? t("Members (role)") : (g.members.find((m) => m.id === key)?.name ?? key));
  const setPerm = (key: string, p: keyof Perms, v: boolean | null) => setS((x) => ({ ...x, perms: { ...x.perms, [key]: { ...x.perms[key], [p]: v } } }));
  const targets = [...new Set(["role:member", ...Object.keys(s.perms)])];
  const perms = PERM_NAMES.filter(([k]) => (voice ? k !== "send" : k !== "connect" && k !== "speak"));
  return (
    <div className="social-modal" role="dialog" aria-label={t("Channel settings")}>
      <div className="social-modal-box channel-settings" ref={box}>
        <div className="row">
          <h3 className="grow">{voice ? c.name : `#${c.name}`}</h3>
          <button className={tab === "overview" ? "active" : ""} onClick={() => setTab("overview")}>
            {t("Overview")}
          </button>
          <button className={tab === "perms" ? "active" : ""} onClick={() => setTab("perms")}>
            {t("Permissions")}
          </button>
        </div>
        {tab === "overview" ? (
          <>
            <label>
              {t("Channel name")}
              <input value={s.name} maxLength={100} onChange={(e) => setS({ ...s, name: e.target.value })} />
            </label>
            {!voice && (
              <label>
                {t("Topic")}
                <textarea value={s.topic} maxLength={1024} rows={3} onChange={(e) => setS({ ...s, topic: e.target.value })} />
              </label>
            )}
            {!voice && (
              <label>
                {t("Slow mode")}
                <select value={s.slow_secs} onChange={(e) => setS({ ...s, slow_secs: Number(e.target.value) })}>
                  {[0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600].map((n) => (
                    <option key={n} value={n}>
                      {n === 0 ? t("Off") : n < 60 ? t("{n} seconds", { n }) : n < 3600 ? t("{n} minutes", { n: n / 60 }) : t("{n} hours", { n: n / 3600 })}
                    </option>
                  ))}
                </select>
              </label>
            )}
            {voice && (
              <label>
                {t("User limit: {n}", { n: s.user_limit === 0 ? t("no limit") : s.user_limit })}
                <input type="range" min={0} max={99} value={s.user_limit} onChange={(e) => setS({ ...s, user_limit: Number(e.target.value) })} />
              </label>
            )}
            {voice && (
              <label>
                {t("Bitrate: {n} kbps", { n: s.bitrate || 48 })}
                <input type="range" min={8} max={384} step={8} value={s.bitrate || 48} onChange={(e) => setS({ ...s, bitrate: Number(e.target.value) })} />
              </label>
            )}
            {voice && (
              <label className="row">
                <input type="checkbox" checked={!!s.mark_voice_changes} onChange={(e) => setS({ ...s, mark_voice_changes: e.target.checked })} />
                {t("Always mark a changed voice so everyone knows")}
              </label>
            )}
            <label className="row">
              <input type="checkbox" checked={s.adults_only} onChange={(e) => setS({ ...s, adults_only: e.target.checked })} />
              {t("Age-restricted (hidden from teen members)")}
            </label>
            <label className="row">
              <input type="checkbox" checked={s.private} onChange={(e) => setS({ ...s, private: e.target.checked })} />
              {t("Private channel: only moderators and the people or roles you allow on Permissions")}
            </label>
          </>
        ) : (
          <>
            <p className="small muted">{t("Moderators can always see and use every channel. A person's own setting wins over their role's.")}</p>
            <table className="channel-perms">
              <thead>
                <tr>
                  <th />
                  {perms.map(([k, name]) => (
                    <th key={k}>{t(name)}</th>
                  ))}
                  <th />
                </tr>
              </thead>
              <tbody>
                {targets.map((key) => (
                  <tr key={key}>
                    <td>{label(key)}</td>
                    {perms.map(([k, name]) => {
                      const v = s.perms[key]?.[k] ?? null;
                      return (
                        <td key={k}>
                          <select value={v === null ? "" : v ? "allow" : "deny"} aria-label={`${label(key)}: ${t(name)}`} onChange={(e) => setPerm(key, k, e.target.value === "" ? null : e.target.value === "allow")}>
                            <option value="">{t("Default")}</option>
                            <option value="allow">{t("Allow")}</option>
                            <option value="deny">{t("Deny")}</option>
                          </select>
                        </td>
                      );
                    })}
                    <td>
                      {key !== "role:member" && (
                        <button
                          className="link"
                          onClick={() =>
                            setS((x) => {
                              const next = { ...x.perms };
                              delete next[key];
                              return { ...x, perms: next };
                            })
                          }
                        >
                          {t("Remove")}
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="row">
              <select value={adding} onChange={(e) => setAdding(e.target.value)} aria-label={t("Add a person")}>
                <option value="">{t("Add a person...")}</option>
                {g.members
                  .filter((m) => m.role === "member" && !s.perms[m.id])
                  .map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
              </select>
              <button
                disabled={!adding}
                onClick={() => {
                  setPerm(adding, "view", true);
                  setAdding("");
                }}
              >
                {t("Add")}
              </button>
            </div>
          </>
        )}
        <div className="row">
          <button className="danger" onClick={onDelete}>
            {t("Delete channel")}
          </button>
          <span className="grow" />
          <button onClick={onClose}>{t("Cancel")}</button>
          <button className="primary" disabled={!s.name.trim()} onClick={() => onSave(s)}>
            {t("Save changes")}
          </button>
        </div>
      </div>
    </div>
  );
}
