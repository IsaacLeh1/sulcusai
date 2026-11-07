// SPDX-License-Identifier: AGPL-3.0-only
// Networks in Social: virtual LANs for games and anything else that looks
// for other PCs on the local network. A list of networks, each with its
// members, their 10.77 addresses, online state and ping, like Radmin.
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { setSharing, sharingSettings } from "../../api/sharing";
import { confirmed } from "../../confirm";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { ChevronDownIcon, DocumentIcon } from "./SocialIcons";

export type MemberRow = { peer: string; name: string; address: string; admin: boolean; me: boolean; online: boolean; ping: number | null; device: string | null };
export type NetView = {
  id: string;
  name: string;
  group: string | null;
  admin: boolean;
  owner: boolean;
  connected: boolean;
  has_password: boolean;
  approval: boolean;
  limit: number;
  members: MemberRow[];
  waiting: [string, string][];
  banned: [string, string][];
};
export type Card = { id: string; name: string; group: string; has_password: boolean; approval: boolean; members: number; admins: string[] };
export type NetsView = { lan_on: boolean; mine: NetView[]; open: Card[]; invites: [Card, string][]; my_address: string | null };
type HelperStatus = { elevated: boolean; set_up: boolean; dll: boolean };
type Manage =
  | { op: "rename"; name: string }
  | { op: "rules"; password: string | null; approval: boolean; limit: number }
  | { op: "kick" | "ban" | "unban" | "admit" | "deny"; peer: string }
  | { op: "admin"; peer: string; on: boolean }
  | { op: "leave" | "delete" };

export const nets = {
  view: () => invoke<NetsView>("social_networks"),
  create: (group: string | null, name: string, password: string, approval: boolean, limit: number) => invoke<NetsView>("social_net_create", { group, name, password, approval, limit }),
  manage: (id: string, op: Manage) => invoke<NetsView>("social_net_manage", { id, op }),
  invite: (id: string, to: string | null) => invoke<string>("social_net_invite", { id, to }),
  join: (by: string, password: string | null) => invoke<string>("social_net_join", { by, password }),
  connect: (id: string, on: boolean) => invoke<NetsView>("social_net_connect", { id, on }),
  helper: () => invoke<HelperStatus>("lan_helper_status"),
  setUp: () => invoke<HelperStatus>("lan_helper_setup"),
  remove: () => invoke<HelperStatus>("lan_helper_remove"),
};

export type MemberActions = {
  /** Their Social device: start a direct message. */
  chat?: (device: string, name: string) => void;
  call?: (device: string, name: string) => void;
  block?: (device: string) => void;
};

function Dot({ on }: { on: boolean }) {
  return <span className={on ? "net-dot on" : "net-dot"} aria-label={on ? t("Online") : t("Offline")} />;
}

/** Networks, all of them or one group's, with joining and managing. */
export function Networks({ group, toast, actions = {} }: { group?: string; toast: PushToast; actions?: MemberActions }) {
  const [v, setV] = useState<NetsView | null>(null);
  const [helper, setHelper] = useState<HelperStatus | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [making, setMaking] = useState(false);
  const [joinBy, setJoinBy] = useState("");
  const [joinPw, setJoinPw] = useState("");
  const [busy, setBusy] = useState(false);
  const [editingNet, setEditingNet] = useState<NetView | null>(null);
  const load = useCallback(() => {
    nets.view().then(setV).catch((e) => toast(errorText(e)));
    nets.helper().then(setHelper).catch(() => {});
  }, [toast]);
  useEffect(() => {
    load();
    const un = [listen("social:networks", load), listen<{ text: string }>("social:network_answer", (e) => toast(e.payload.text))];
    // Online state and ping change on their own.
    const id = window.setInterval(load, 5000);
    return () => {
      window.clearInterval(id);
      un.forEach((p) => p.then((f) => f()));
    };
  }, [load, toast]);
  const run = async (f: () => Promise<NetsView | unknown>) => {
    setBusy(true);
    try {
      const r = await f();
      if (r && typeof r === "object" && "mine" in (r as object)) setV(r as NetsView);
      else load();
    } catch (e) {
      toast(errorText(e));
    }
    setBusy(false);
  };
  const toggleConnect = async (id: string, on: boolean) => {
    setBusy(true);
    try {
      const r = await nets.connect(id, on);
      if (r && typeof r === "object" && "mine" in (r as object)) setV(r as NetsView);
      else await load();
      toast(on ? t("Network connected.") : t("Network disconnected."));
    } catch (e) {
      toast(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const enableLan = async () => {
    try {
      setBusy(true);
      const cur = await sharingSettings();
      await setSharing({ ...cur, virtualLan: true });
      await load();
      toast(t("Virtual LAN enabled."));
    } catch (e) {
      toast(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  if (!v) return null;
  const mine = v.mine.filter((n) => !group || n.group === group);
  const openCards = v.open.filter((c) => !group || c.group === group);
  return (
    <div className="nets">
      <div className="radmin-header">
        <div className="radmin-status-card">
          <button
            type="button"
            className={`radmin-power-btn ${v.lan_on ? "on" : "off"}`}
            title={v.lan_on ? t("Virtual LAN active") : t("Turn on Virtual LAN")}
            disabled={busy}
            onClick={() => {
              if (!v.lan_on) {
                enableLan();
                return;
              }
              if (mine.length > 0) {
                const anyOn = mine.some((m) => m.connected);
                toggleConnect(mine[0].id, !anyOn);
              }
            }}
          >
            <PowerIcon />
          </button>
          <div className="radmin-ip-col">
            <span className="small muted">{t("Virtual LAN IPv4")}</span>
            <div className="radmin-ip-display">
              <span className="radmin-ip-text">{v.lan_on ? v.my_address || t("0.0.0.0") : t("Disabled")}</span>
              {v.lan_on && v.my_address && (
                <button
                  type="button"
                  className="icon-ghost small"
                  title={t("Copy IPv4 address")}
                  onClick={() => {
                    navigator.clipboard.writeText(v.my_address!).then(() => toast(t("Copied {s}", { s: v.my_address! })));
                  }}
                >
                  <DocumentIcon size={14} />
                </button>
              )}
            </div>
          </div>
        </div>
        <div className="radmin-actions">
          <button type="button" className="btn-discord" disabled={!v.lan_on} onClick={() => setMaking((m) => !m)}>
            + {t("Create Network")}
          </button>
        </div>
      </div>

      {!v.lan_on && (
        <div className="radmin-enable-card">
          <div className="radmin-enable-info">
            <span className="radmin-enable-title">{t("Virtual LAN is turned off")}</span>
            <span className="small muted">
              {t("Turn on Virtual LAN to create and connect to private networks.")}
            </span>
          </div>
          <button type="button" className="btn-discord" disabled={busy} onClick={enableLan}>
            {t("Enable Virtual LAN")}
          </button>
        </div>
      )}

      {helper && !helper.elevated && !helper.set_up && (
        <div className="note small">
          {t("The first time, SulcusAI downloads Wintun (from wintun.net) and makes the LAN adapter. Windows asks for admin rights once; after that networks turn on without asking.")}{" "}
          <button className="btn-discord secondary" disabled={busy} onClick={() => run(() => nets.setUp().then(setHelper))}>
            {t("Set up the adapter")}
          </button>
        </div>
      )}
      {helper && !helper.elevated && helper.set_up && (
        <details className="small muted">
          <summary>{t("LAN adapter")}</summary>
          {t("Set up: networks turn on without an admin prompt.")}{" "}
          <button className="link small" disabled={busy} onClick={async () => (await confirmed(t("Remove the LAN helper? Turning a network on will ask for admin rights again."))) && run(() => nets.remove().then(setHelper))}>
            {t("Remove the LAN helper")}
          </button>
        </details>
      )}
      {making && <NewNetwork group={group ?? null} onDone={(x) => (x && setV(x), setMaking(false))} toast={toast} />}
      {mine.length === 0 && openCards.length === 0 && <p className="small muted">{t("No networks yet. Make one, or join with an invite or a network's name and password.")}</p>}

      <div className="radmin-tree">
        {mine.map((n) => {
          const isOpen = open === n.id || (open === null && mine.length === 1);
          const onlinePeers = n.members.filter((m) => m.online && !m.me);
          const onlineCount = n.members.filter((m) => m.online).length;
          return (
            <div key={n.id} className="radmin-net-card">
              <div
                className="radmin-net-header"
                onClick={() => setOpen((o) => (o === n.id ? "" : n.id))}
                role="button"
                tabIndex={0}
              >
                <ChevronDownIcon open={isOpen} size={14} />
                <span className="radmin-net-title">{n.name}</span>
                {n.has_password && <span className="small muted"> ({t("password")})</span>}
                {n.approval && <span className="small muted"> ({t("approval")})</span>}
                <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 8 }}>
                  {onlinePeers.length > 0 ? (
                    <div
                      className="radmin-online-chip"
                      title={t("Online: {names}", { names: onlinePeers.map((p) => `${p.name} (${p.address})`).join(", ") })}
                    >
                      <span className="net-dot on" style={{ width: 6, height: 6 }} />
                      <span className="radmin-online-names">
                        {onlinePeers.map((p) => p.name).slice(0, 3).join(", ")}
                        {onlinePeers.length > 3 ? ` +${onlinePeers.length - 3}` : ""}
                      </span>
                    </div>
                  ) : (
                    <span className="small muted">
                      {t("{on} of {n} online", { on: onlineCount, n: n.members.length })}
                    </span>
                  )}
                </div>
                {n.waiting.length > 0 && <span className="social-badge">{n.waiting.length}</span>}
                {n.admin && (
                  <button
                    type="button"
                    className="icon-ghost"
                    title={t("Edit network settings")}
                    disabled={busy}
                    onClick={(e) => {
                      e.stopPropagation();
                      setEditingNet(n);
                    }}
                    style={{ marginLeft: 6, padding: 4 }}
                    aria-label={t("Edit network settings")}
                  >
                    <GearIcon size={14} />
                  </button>
                )}
                <button
                  type="button"
                  className={n.connected ? "net-power on" : "net-power"}
                  aria-pressed={n.connected}
                  title={n.connected ? t("Turn this network off") : t("Turn this network on")}
                  disabled={busy}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleConnect(n.id, !n.connected);
                  }}
                  style={{ marginLeft: 6 }}
                >
                  <PowerIcon />
                </button>
              </div>
              {isOpen && (
                <div style={{ padding: "0 8px 8px 8px" }}>
                  <NetDetail n={n} toast={toast} run={run} busy={busy} actions={actions} onEdit={setEditingNet} />
                </div>
              )}
            </div>
          );
        })}

        {editingNet && (
          <EditNetworkDialog
            n={editingNet}
            toast={toast}
            onClose={() => setEditingNet(null)}
            onSaved={load}
          />
        )}

        {openCards.map((c) => (
          <div key={c.id} className="radmin-net-card" style={{ padding: "8px 12px", display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <div>
              <strong>{c.name}</strong>
              {c.has_password && <span className="small muted"> ({t("password")})</span>}
              <div className="small muted">{t("{n} members", { n: c.members })}</div>
            </div>
            <button
              type="button"
              className="btn-discord"
              disabled={busy}
              onClick={() => {
                const pw = c.has_password ? window.prompt(t("Password for {name}", { name: c.name })) : "";
                if (pw === null) return;
                run(() => nets.join(c.id, pw || null).then(toast));
              }}
            >
              {t("Join")}
            </button>
          </div>
        ))}

        {v.invites.map(([c]) => (
          <div key={"i" + c.id} className="radmin-net-card" style={{ padding: "8px 12px", display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <div>
              <strong>{c.name}</strong> <span className="small muted">{t("invited you")}</span>
              <div className="small muted">{t("{n} members", { n: c.members })}</div>
            </div>
            <button type="button" className="btn-discord" disabled={busy} onClick={() => run(() => nets.join(c.id, null).then(toast))}>
              {t("Accept")}
            </button>
          </div>
        ))}
      </div>

      <div className="row nets-join" style={{ marginTop: 12 }}>
        <input value={joinBy} onChange={(e) => setJoinBy(e.target.value)} placeholder={t("Invite link or network name")} aria-label={t("Invite link or network name")} />
        <input type="password" value={joinPw} onChange={(e) => setJoinPw(e.target.value)} placeholder={t("Password (if it has one)")} aria-label={t("Password")} />
        <button
          type="button"
          className="btn-discord secondary"
          disabled={busy || !joinBy.trim()}
          onClick={() =>
            run(() =>
              nets.join(joinBy.trim(), joinPw || null).then((m) => {
                toast(m);
                setJoinBy("");
                setJoinPw("");
              }),
            )
          }
        >
          {t("Join")}
        </button>
      </div>
    </div>
  );
}

function NewNetwork({ group, onDone, toast }: { group: string | null; onDone: (v?: NetsView) => void; toast: PushToast }) {
  const [name, setName] = useState("");
  const [pw, setPw] = useState("");
  const [approval, setApproval] = useState(false);
  const [limit, setLimit] = useState(0);
  return (
    <div className="nets-form">
      <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder={t("Network name")} maxLength={60} />
      <input type="password" value={pw} onChange={(e) => setPw(e.target.value)} placeholder={t("Password to join (optional)")} />
      <label className="row small">
        <input type="checkbox" checked={approval} onChange={(e) => setApproval(e.target.checked)} />
        {t("An admin lets each person in")}
      </label>
      <label className="row small">
        {t("Most members (0 for no limit)")}
        <input type="number" min={0} max={250} value={limit} onChange={(e) => setLimit(Number(e.target.value) || 0)} />
      </label>
      <div className="row">
        <button
          type="button"
          className="btn-discord"
          disabled={!name.trim()}
          onClick={() =>
            nets
              .create(group, name, pw, approval, limit)
              .then(onDone)
              .catch((e) => toast(errorText(e)))
          }
        >
          {t("Make network")}
        </button>
        <button type="button" className="btn-discord secondary" onClick={() => onDone()}>{t("Cancel")}</button>
      </div>
    </div>
  );
}

function NetDetail({ n, toast, run, busy, actions, onEdit }: { n: NetView; toast: PushToast; run: (f: () => Promise<unknown>) => Promise<void>; busy: boolean; actions: MemberActions; onEdit?: (n: NetView) => void }) {
  const m = (op: Manage) => run(() => nets.manage(n.id, op));
  const copy = (s: string) =>
    navigator.clipboard
      .writeText(s)
      .then(() => toast(t("Copied {s}", { s })))
      .catch(() => {});

  const onlineMembers = n.members.filter((p) => p.online);
  const offlineMembers = n.members.filter((p) => !p.online);

  const renderMemberRow = (p: MemberRow, isOnline: boolean) => (
    <tr key={p.peer} className={isOnline ? "radmin-row-online" : ""}>
      <td>
        <Dot on={p.online} />
      </td>
      <td>
        <strong>{p.name}</strong>
        {p.me && <span className="small muted"> {t("(you)")}</span>}
        {p.admin && <span className="small muted"> {t("admin")}</span>}
        {isOnline && <span className="radmin-status-badge online" style={{ marginLeft: 6 }}>{t("Connected")}</span>}
      </td>
      <td>
        <code>{p.address}</code>
      </td>
      <td className="small">
        {p.me ? "" : p.ping !== null ? (
          <span className={`radmin-ping-badge ${p.ping < 50 ? "fast" : p.ping < 150 ? "medium" : "slow"}`}>
            {p.ping} ms
          </span>
        ) : p.online ? "..." : t("offline")}
      </td>
      <td className="net-actions">
        <button className="link small" onClick={() => copy(p.address)}>
          {t("Copy address")}
        </button>
        {!p.me && (
          <>
            <button className="link small" title={t("Measure the ping again")} onClick={() => run(() => nets.view())}>
              {t("Ping")}
            </button>
            {p.device && actions.chat && (
              <button className="link small" onClick={() => actions.chat?.(p.device!, p.name)}>
                {t("Chat")}
              </button>
            )}
            {p.device && actions.call && (
              <button className="link small" onClick={() => actions.call?.(p.device!, p.name)}>
                {t("Call")}
              </button>
            )}
            {p.device && actions.block && (
              <button className="link small" onClick={async () => (await confirmed(t("Block {name}? Their messages are hidden from you.", { name: p.name }))) && actions.block?.(p.device!)}>
                {t("Block")}
              </button>
            )}
            {n.admin && (
              <>
                <button className="link small" disabled={busy} onClick={() => m({ op: "admin", peer: p.peer, on: !p.admin })}>
                  {p.admin ? t("Remove admin") : t("Make admin")}
                </button>
                <button className="link small" disabled={busy} onClick={() => m({ op: "kick", peer: p.peer })}>
                  {t("Kick")}
                </button>
                <button className="link small danger" disabled={busy} onClick={async () => (await confirmed(t("Ban {name} from {net}?", { name: p.name, net: n.name }))) && m({ op: "ban", peer: p.peer })}>
                  {t("Ban")}
                </button>
              </>
            )}
          </>
        )}
      </td>
    </tr>
  );

  return (
    <div className="net-detail">
      <table className="nets-members">
        <thead>
          <tr>
            <th />
            <th>{t("Name")}</th>
            <th>{t("Address")}</th>
            <th>{t("Ping")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {onlineMembers.length > 0 && (
            <tr className="radmin-section-row">
              <td colSpan={5} className="radmin-section-divider online">
                ● {t("Online & Connected ({n})", { n: onlineMembers.length })}
              </td>
            </tr>
          )}
          {onlineMembers.map((p) => renderMemberRow(p, true))}
          {onlineMembers.length === 1 && onlineMembers[0].me && (
            <tr>
              <td colSpan={5} className="small muted" style={{ padding: "4px 8px 10px 8px", fontStyle: "italic", fontSize: 11 }}>
                {t("You are connected. Waiting for peers to come online...")}
              </td>
            </tr>
          )}
          {offlineMembers.length > 0 && (
            <tr className="radmin-section-row">
              <td colSpan={5} className="radmin-section-divider offline">
                ○ {t("Offline ({n})", { n: offlineMembers.length })}
              </td>
            </tr>
          )}
          {offlineMembers.map((p) => renderMemberRow(p, false))}
          {n.members.length === 0 && (
            <tr>
              <td colSpan={5} className="small muted" style={{ padding: "8px 6px", fontStyle: "italic" }}>
                {t("No members in this network yet.")}
              </td>
            </tr>
          )}
        </tbody>
      </table>
      {n.admin && n.waiting.length > 0 && (
        <div className="small">
          <strong>{t("Asking to join")}</strong>
          {n.waiting.map(([peer, name]) => (
            <div key={peer} className="row">
              {name}
              <button disabled={busy} onClick={() => m({ op: "admit", peer })}>
                {t("Let in")}
              </button>
              <button disabled={busy} onClick={() => m({ op: "deny", peer })}>
                {t("Turn down")}
              </button>
            </div>
          ))}
        </div>
      )}
      <div className="row net-manage">
        <button
          onClick={() =>
            nets
              .invite(n.id, null)
              .then((link) => copy(link))
              .catch((e) => toast(errorText(e)))
          }
          disabled={!n.admin}
          title={n.admin ? undefined : t("Only admins can invite")}
        >
          {t("Copy invite link")}
        </button>
        {n.admin && onEdit && (
          <button type="button" onClick={() => onEdit(n)}>
            {t("Edit network settings")}
          </button>
        )}
        <button onClick={async () => (await confirmed(t("Leave {net}?", { net: n.name }))) && m({ op: "leave" })}>{t("Leave")}</button>
        {n.owner && (
          <button className="danger" onClick={async () => (await confirmed(t("Delete {net} for everyone?", { net: n.name }))) && m({ op: "delete" })}>
            {t("Delete")}
          </button>
        )}
      </div>
      {n.admin && n.banned.length > 0 && (
        <div className="small muted">
          {t("Banned:")}{" "}
          {n.banned.map(([peer, name]) => (
            <button key={peer} className="link small" title={t("Unban")} onClick={() => m({ op: "unban", peer })}>
              {name}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function PowerIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      <path d="M8 1.5v6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
      <path d="M4.2 4a5.5 5.5 0 1 0 7.6 0" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}

function GearIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <path d="M9.1 1a1 1 0 0 1 .98.8l.2 1.1a5 5 0 0 1 1.4.8l1-.5a1 1 0 0 1 1.25.32l1 1.73a1 1 0 0 1-.26 1.27l-.9.7a5.2 5.2 0 0 1 0 1.6l.9.7a1 1 0 0 1 .26 1.27l-1 1.73a1 1 0 0 1-1.25.32l-1-.5a5 5 0 0 1-1.4.8l-.2 1.1a1 1 0 0 1-.98.8H6.9a1 1 0 0 1-.98-.8l-.2-1.1a5 5 0 0 1-1.4-.8l-1 .5a1 1 0 0 1-1.25-.32l-1-1.73a1 1 0 0 1 .26-1.27l.9-.7a5.2 5.2 0 0 1 0-1.6l-.9-.7a1 1 0 0 1-.26-1.27l1-1.73a1 1 0 0 1 1.25-.32l1 .5a5 5 0 0 1 1.4-.8l.2-1.1A1 1 0 0 1 6.9 1h2.2zM8 5.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z" />
    </svg>
  );
}

function EditNetworkDialog({
  n,
  toast,
  onClose,
  onSaved,
}: {
  n: NetView;
  toast: PushToast;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [name, setName] = useState(n.name);
  const [changePassword, setChangePassword] = useState(false);
  const [password, setPassword] = useState("");
  const [removePassword, setRemovePassword] = useState(false);
  const [approval, setApproval] = useState(n.approval);
  const [limit, setLimit] = useState(n.limit);
  const [saving, setSaving] = useState(false);

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setSaving(true);
    try {
      if (name.trim() !== n.name) {
        await nets.manage(n.id, { op: "rename", name: name.trim() });
      }
      let newPw: string | null = null;
      if (removePassword) {
        newPw = "";
      } else if (changePassword && password) {
        newPw = password;
      }
      if (newPw !== null || approval !== n.approval || limit !== n.limit) {
        await nets.manage(n.id, { op: "rules", password: newPw, approval, limit });
      }
      toast(t("Network settings saved."));
      onSaved();
      onClose();
    } catch (err) {
      toast(errorText(err));
    } finally {
      setSaving(false);
    }
  };

  const deleteNetwork = async () => {
    const ok = await confirmed(t("Delete {name} permanently for everyone?", { name: n.name }));
    if (!ok) return;
    try {
      setSaving(true);
      await nets.manage(n.id, { op: "delete" });
      toast(t("Network deleted."));
      onSaved();
      onClose();
    } catch (err) {
      toast(errorText(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="chat-viewer" style={{ zIndex: 1200 }} onClick={onClose} role="dialog" aria-modal="true">
      <div
        className="channel-settings-dialog"
        onClick={(e) => e.stopPropagation()}
        style={{ width: 440, padding: 24, borderRadius: 10, background: "var(--social-chat, var(--panel))", border: "1px solid var(--border)", boxShadow: "0 8px 32px rgba(0,0,0,0.5)" }}
      >
        <h3 style={{ margin: "0 0 16px", fontSize: 18 }}>{t("Edit Network: {name}", { name: n.name })}</h3>
        <form onSubmit={save} style={{ display: "flex", flexDirection: "column", gap: 14 }}>
          <div>
            <label className="small muted" style={{ display: "block", marginBottom: 4 }}>{t("Network Name")}</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              style={{ width: "100%", padding: "8px 10px", borderRadius: 6 }}
            />
          </div>

          <div>
            <label className="small muted" style={{ display: "block", marginBottom: 4 }}>
              {t("Password")} {n.has_password && !changePassword && !removePassword && `(${t("Protected")})`}
            </label>
            {!changePassword && !removePassword ? (
              <div style={{ display: "flex", gap: 8 }}>
                <button
                  type="button"
                  className="btn-discord secondary"
                  style={{ fontSize: 13, padding: "4px 10px" }}
                  onClick={() => setChangePassword(true)}
                >
                  {n.has_password ? t("Change password") : t("Set password")}
                </button>
                {n.has_password && (
                  <button
                    type="button"
                    className="btn-discord secondary"
                    style={{ fontSize: 13, padding: "4px 10px" }}
                    onClick={() => setRemovePassword(true)}
                  >
                    {t("Remove password")}
                  </button>
                )}
              </div>
            ) : changePassword ? (
              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <input
                  type="password"
                  placeholder={t("New password")}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  style={{ flex: 1, padding: "8px 10px", borderRadius: 6 }}
                />
                <button type="button" className="link small" onClick={() => { setChangePassword(false); setPassword(""); }}>
                  {t("Cancel")}
                </button>
              </div>
            ) : (
              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <span className="small muted">{t("Password will be removed.")}</span>
                <button type="button" className="link small" onClick={() => setRemovePassword(false)}>
                  {t("Cancel")}
                </button>
              </div>
            )}
          </div>

          <label className="row small" style={{ display: "flex", gap: 8, alignItems: "center", cursor: "pointer" }}>
            <input type="checkbox" checked={approval} onChange={(e) => setApproval(e.target.checked)} />
            <span>{t("Require admin approval for each new member")}</span>
          </label>

          <div>
            <label className="small muted" style={{ display: "block", marginBottom: 4 }}>
              {t("Member limit (0 for unlimited, up to 250)")}
            </label>
            <input
              type="number"
              min={0}
              max={250}
              value={limit}
              onChange={(e) => setLimit(Math.max(0, Math.min(250, Number(e.target.value) || 0)))}
              style={{ width: 100, padding: "6px 10px", borderRadius: 6 }}
            />
          </div>

          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginTop: 12, paddingTop: 12, borderTop: "1px solid var(--border)" }}>
            {n.owner ? (
              <button
                type="button"
                className="btn-discord danger"
                style={{ fontSize: 13, padding: "6px 12px" }}
                disabled={saving}
                onClick={deleteNetwork}
              >
                {t("Delete Network")}
              </button>
            ) : <span />}

            <div style={{ display: "flex", gap: 8 }}>
              <button
                type="button"
                className="btn-discord secondary"
                onClick={onClose}
                disabled={saving}
              >
                {t("Cancel")}
              </button>
              <button
                type="submit"
                className="btn-discord"
                disabled={saving || !name.trim()}
              >
                {saving ? t("Saving...") : t("Save Changes")}
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  );
}

