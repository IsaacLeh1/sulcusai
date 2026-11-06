// SPDX-License-Identifier: AGPL-3.0-only
// Networks in Social: virtual LANs for games and anything else that looks
// for other PCs on the local network. A list of networks, each with its
// members, their 10.77 addresses, online state and ping, like Radmin.
import { Fragment, useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { confirmed } from "../../confirm";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";

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
  if (!v) return null;
  if (!v.lan_on) return <p className="small muted">{t("Networks use the virtual LAN. Turn on Virtual LAN in Settings > Sharing first.")}</p>;
  const mine = v.mine.filter((n) => !group || n.group === group);
  const openCards = v.open.filter((c) => !group || c.group === group);
  return (
    <div className="nets">
      <div className="row nets-top">
        <strong>{t("Networks")}</strong>
        {v.my_address && <span className="small muted">{t("Your address: {a}", { a: v.my_address })}</span>}
        <button onClick={() => setMaking((m) => !m)}>{t("New network")}</button>
      </div>
      {helper && !helper.elevated && !helper.set_up && (
        <div className="note small">
          {t("The first time, SulcusAI downloads Wintun (from wintun.net) and makes the LAN adapter. Windows asks for admin rights once; after that networks turn on without asking.")}{" "}
          <button disabled={busy} onClick={() => run(() => nets.setUp().then(setHelper))}>
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
      <table className="nets-table">
        <tbody>
          {mine.map((n) => (
            <Fragment key={n.id}>
              <tr className={n.connected ? "net-row on" : "net-row"} onClick={() => setOpen((o) => (o === n.id ? null : n.id))}>
                <td>
                  <button
                    className={n.connected ? "net-power on" : "net-power"}
                    aria-pressed={n.connected}
                    title={n.connected ? t("Turn this network off") : t("Turn this network on")}
                    disabled={busy}
                    onClick={(e) => {
                      e.stopPropagation();
                      run(() => nets.connect(n.id, !n.connected));
                    }}
                  >
                    <PowerIcon />
                  </button>
                </td>
                <td>
                  <strong>{n.name}</strong>
                  {n.has_password && <span className="small muted"> {t("password")}</span>}
                  {n.approval && <span className="small muted"> {t("approval")}</span>}
                </td>
                <td className="small muted">
                  {t("{on} of {n} online", { on: n.members.filter((m) => m.online).length, n: n.members.length })}
                  {n.waiting.length > 0 && <span className="social-badge">{n.waiting.length}</span>}
                </td>
                <td className="small">{n.connected ? t("On") : t("Off")}</td>
              </tr>
              {open === n.id && (
                <tr>
                  <td colSpan={4}>
                    <NetDetail n={n} toast={toast} run={run} busy={busy} actions={actions} />
                  </td>
                </tr>
              )}
            </Fragment>
          ))}
          {openCards.map((c) => (
            <tr key={c.id} className="net-row joinable">
              <td />
              <td>
                <strong>{c.name}</strong>
                {c.has_password && <span className="small muted"> {t("password")}</span>}
              </td>
              <td className="small muted">{t("{n} members", { n: c.members })}</td>
              <td>
                <button
                  disabled={busy}
                  onClick={() => {
                    const pw = c.has_password ? window.prompt(t("Password for {name}", { name: c.name })) : "";
                    if (pw === null) return;
                    run(() => nets.join(c.id, pw || null).then(toast));
                  }}
                >
                  {t("Join")}
                </button>
              </td>
            </tr>
          ))}
          {v.invites.map(([c]) => (
            <tr key={"i" + c.id} className="net-row joinable">
              <td />
              <td>
                <strong>{c.name}</strong> <span className="small muted">{t("invited you")}</span>
              </td>
              <td className="small muted">{t("{n} members", { n: c.members })}</td>
              <td>
                <button disabled={busy} onClick={() => run(() => nets.join(c.id, null).then(toast))}>
                  {t("Accept")}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="row nets-join">
        <input value={joinBy} onChange={(e) => setJoinBy(e.target.value)} placeholder={t("Invite link or network name")} aria-label={t("Invite link or network name")} />
        <input type="password" value={joinPw} onChange={(e) => setJoinPw(e.target.value)} placeholder={t("Password (if it has one)")} aria-label={t("Password")} />
        <button
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
          className="primary"
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
        <button onClick={() => onDone()}>{t("Cancel")}</button>
      </div>
    </div>
  );
}

function NetDetail({ n, toast, run, busy, actions }: { n: NetView; toast: PushToast; run: (f: () => Promise<unknown>) => Promise<void>; busy: boolean; actions: MemberActions }) {
  const m = (op: Manage) => run(() => nets.manage(n.id, op));
  const copy = (s: string) =>
    navigator.clipboard
      .writeText(s)
      .then(() => toast(t("Copied {s}", { s })))
      .catch(() => {});
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
          {n.members.map((p) => (
            <tr key={p.peer}>
              <td>
                <Dot on={p.online} />
              </td>
              <td>
                {p.name}
                {p.me && <span className="small muted"> {t("(you)")}</span>}
                {p.admin && <span className="small muted"> {t("admin")}</span>}
              </td>
              <td>
                <code>{p.address}</code>
              </td>
              <td className="small">{p.me ? "" : p.ping !== null ? `${p.ping} ms` : p.online ? "..." : t("offline")}</td>
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
          ))}
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
        {n.admin && (
          <>
            <button
              onClick={() => {
                const name = window.prompt(t("New name"), n.name);
                if (name) m({ op: "rename", name });
              }}
            >
              {t("Rename")}
            </button>
            <button
              onClick={() => {
                const pw = window.prompt(t("New password (leave empty for none)"), "");
                if (pw !== null) m({ op: "rules", password: pw, approval: n.approval, limit: n.limit });
              }}
            >
              {n.has_password ? t("Change password") : t("Set a password")}
            </button>
            <label className="row small">
              <input type="checkbox" checked={n.approval} onChange={(e) => m({ op: "rules", password: null, approval: e.target.checked, limit: n.limit })} />
              {t("Approve each join")}
            </label>
            <label className="row small">
              {t("Limit")}
              <input
                type="number"
                min={0}
                max={250}
                defaultValue={n.limit}
                onBlur={(e) => Number(e.target.value) !== n.limit && m({ op: "rules", password: null, approval: n.approval, limit: Number(e.target.value) || 0 })}
              />
            </label>
          </>
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

