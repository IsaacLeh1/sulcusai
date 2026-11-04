// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { confirmed } from "../confirm";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

interface Pending {
  peer: string;
  name: string;
  kind: string;
  key_label: string;
  at: number;
}

type Reply =
  | { is: "state"; pending: Pending[]; members: string[]; keys: [string, string, boolean][] }
  | { is: "done" }
  | { is: "key"; id: string; secret: string }
  | { is: "unknown" };

/** For a relay this PC owns: who's asking to join, who's in, and invite keys. */
export function YourRelay({ toast }: { toast: PushToast }) {
  const [address, setAddress] = useState("");
  const [state, setState] = useState<Extract<Reply, { is: "state" }> | null>(null);
  const [key, setKey] = useState<{ id: string; secret: string } | null>(null);
  const [label, setLabel] = useState("Family");
  const ask = (request: Record<string, unknown>) => invoke<Reply>("p2p_relay_admin", { address: address.trim(), request });
  const load = () =>
    ask({ do: "list" })
      .then((r) => r.is === "state" && setState(r))
      .catch((e) => toast(errorText(e), "error"));
  const act = async (request: Record<string, unknown>, question?: string) => {
    if (question && !(await confirmed(question))) return;
    try {
      await ask(request);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <details className="small your-relay">
      <summary>{t("Your relay")}</summary>
      <p className="muted">{t("For a relay you run (sulcus-relay) with this PC as its owner: approve who may use it, remove people, make invite keys.")}</p>
      <div className="row">
        <input className="input" style={{ flex: 1 }} value={address} onChange={(e) => setAddress(e.target.value)} placeholder={t("Your relay's address (/ip4/.../p2p/...)")} aria-label={t("Your relay's address")} />
        <button className="btn small" disabled={!address.trim()} onClick={load}>{t("Open")}</button>
      </div>
      {state && (
        <>
          <h4>{t("Asking to join")}</h4>
          {state.pending.length === 0 && <p className="muted">{t("Nobody is waiting.")}</p>}
          {state.pending.map((p) => (
            <p key={p.peer}>
              {t("{name} ({kind}, key: {label})", { name: p.name, kind: p.kind, label: p.key_label })}{" "}
              <button className="btn small" onClick={() => act({ do: "decide", peer: p.peer, approve: true }, t("Let {name} use your relay? Their traffic will pass through it and leave from its address.", { name: p.name }))}>{t("Approve")}</button>{" "}
              <button className="btn small ghost" onClick={() => act({ do: "decide", peer: p.peer, approve: false })}>{t("Refuse")}</button>
            </p>
          ))}
          <h4>{t("Members")}</h4>
          {state.members.length === 0 && <p className="muted">{t("No members yet.")}</p>}
          {state.members.map((m) => (
            <p key={m}>
              <code>{m.slice(0, 16)}…</code>{" "}
              <button className="link" onClick={() => act({ do: "remove", peer: m, block: false }, t("Remove this member? Their connections close at once."))}>{t("Remove")}</button>{" "}
              <button className="link danger" onClick={() => act({ do: "remove", peer: m, block: true }, t("Block this PC from asking again?"))}>{t("Block")}</button>
            </p>
          ))}
          <h4>{t("Invite keys")}</h4>
          {state.keys.map(([id, l, off]) => (
            <p key={id} className="muted">{t("{label}: key {id}{off}", { label: l, id, off: off ? ` (${t("stopped")})` : "" })}</p>
          ))}
          <div className="row">
            <input className="input" value={label} onChange={(e) => setLabel(e.target.value)} aria-label={t("Circle")} />
            <button
              className="btn small"
              onClick={() =>
                ask({ do: "new_key", label })
                  .then((r) => (r.is === "key" && setKey({ id: r.id, secret: r.secret }), load()))
                  .catch((e) => toast(errorText(e), "error"))
              }
            >
              {t("New key")}
            </button>
          </div>
          {key && <p className="warn-box">{t("Give these to the people in this circle, privately: key id {id}, secret {secret}. Older keys for the circle stop admitting new people.", key)}</p>}
        </>
      )}
    </details>
  );
}
