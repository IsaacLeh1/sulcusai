// SPDX-License-Identifier: AGPL-3.0-only
// Access keys for programs using the local API server: one per program, each
// allowed only what was ticked for it (src-tauri/src/access_keys).
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { confirmed } from "../confirm";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

type Scope = "use_models" | "use_cloud_models" | "use_assistant" | string;

export interface ScopeInfo {
  scope: Scope;
  id: string;
  title: string;
  description: string;
  risk: "low" | "medium" | "high";
}

export interface AccessKey {
  id: string;
  name: string;
  hint: string;
  scopes: ScopeInfo[];
  created_at: number;
  expires_at: number | null;
  last_used_at: number | null;
  status: "active" | "expired" | "revoked";
  legacy: boolean;
}

export interface AuditEntry {
  at: number;
  app: string;
  scope: Scope | null;
  outcome: { kind: "allowed" | "created" | "changed" | "revoked" } | { kind: "denied"; reason: string };
}

export interface AccessKeysView {
  keys: AccessKey[];
  scopes: ScopeInfo[];
  log: AuditEntry[];
}

/** What the local API server checks today; the rest come with the servers that use them. */
export const SERVER_SCOPES: Scope[] = ["use_models", "use_cloud_models", "use_assistant"];

const keysApi = {
  view: () => invoke<AccessKeysView>("access_keys_view"),
  create: (name: string, scopes: Scope[]) => invoke<[string, AccessKeysView]>("access_key_create", { new: { name, scopes } }),
  revoke: (id: string) => invoke<AccessKeysView>("access_key_revoke", { id }),
};

const when = (ms: number | null) => (ms ? new Date(ms).toLocaleString() : t("never"));

export function AccessKeys({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<AccessKeysView | null>(null);
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<Scope[]>([]);
  // The new key's secret: shown once, gone when the person closes it.
  const [secret, setSecret] = useState<string | null>(null);
  useEffect(() => {
    keysApi.view().then(setV, () => {});
  }, []);
  if (!v) return null;
  const offered = offeredScopes(v.scopes);

  const make = async () => {
    try {
      const [made, next] = await keysApi.create(name, picked);
      setSecret(made);
      setV(next);
      setName("");
      setPicked([]);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const revoke = async (k: AccessKey) => {
    if (!(await confirmed(t("Turn off the key for {name}? That program stops working until it gets a new key.", { name: k.name })))) return;
    try {
      setV(await keysApi.revoke(k.id));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <AccessKeysBody
      v={v}
      offered={offered}
      secret={secret}
      name={name}
      picked={picked}
      onName={setName}
      onPick={(id, on) => setPicked(on ? [...picked, id] : picked.filter((x) => x !== id))}
      onMake={make}
      onRevoke={revoke}
      onCopy={(s) => navigator.clipboard.writeText(s).then(() => toast(t("Copied the key."), "success"))}
      onDone={() => setSecret(null)}
    />
  );
}

/** The scopes a new key can be given today, as the server checks them. */
export function offeredScopes(all: ScopeInfo[]): ScopeInfo[] {
  return all.filter((s) => SERVER_SCOPES.includes(s.id));
}

export function AccessKeysBody({ v, offered, secret, name, picked, onName, onPick, onMake, onRevoke, onCopy, onDone }: {
  v: AccessKeysView;
  offered: ScopeInfo[];
  secret: string | null;
  name: string;
  picked: Scope[];
  onName: (n: string) => void;
  onPick: (id: Scope, on: boolean) => void;
  onMake: () => void;
  onRevoke: (k: AccessKey) => void;
  onCopy: (secret: string) => void;
  onDone: () => void;
}) {
  const denials = v.log.filter((e) => e.outcome.kind === "denied").slice(0, 10);
  return (
    <div className="access-keys">
      <h3>{t("Access keys")}</h3>
      <p className="muted small">{t("A key per program, allowed only what you tick. The API key above still works for everything, for programs set up before access keys.")}</p>
      {secret && (
        <div className="access-key-new">
          <p className="small">{t("Copy this key into the program now. SulcusAI keeps only a fingerprint of it and can't show it again.")}</p>
          <div className="row wrap">
            <code className="api-value">{secret}</code>
            <button className="btn ghost small" onClick={() => onCopy(secret)}>
              {t("Copy")}
            </button>
            <button className="btn ghost small" onClick={onDone}>
              {t("Done")}
            </button>
          </div>
        </div>
      )}
      {v.keys.length > 0 && (
        <ul className="access-key-list">
          {v.keys.map((k) => (
            <li key={k.id} className={k.status === "active" ? "" : "muted"}>
              <strong>{k.name}</strong> <span className="small muted">{k.hint}</span>
              <span className="small block">{k.scopes.map((s) => t(s.title)).join(", ") || t("Nothing allowed yet")}</span>
              <span className="small muted block">
                {k.status === "active" ? t("Last used: {when}", { when: when(k.last_used_at) }) : k.status === "revoked" ? t("Turned off") : t("Ran out")}
              </span>
              {k.status === "active" && (
                <button className="btn ghost small" onClick={() => onRevoke(k)}>
                  {t("Turn off")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="access-key-form">
        <label className="adv-field">
          <span>{t("Program's name")}</span>
          <input className="input" value={name} onChange={(e) => onName(e.target.value)} placeholder={t("For example: my code editor")} />
        </label>
        {offered.map((s) => (
          <label key={s.id} className="check">
            <input type="checkbox" checked={picked.includes(s.id)} onChange={(e) => onPick(s.id, e.target.checked)} />
            <span>
              <strong>{t(s.title)}</strong>
              <span className="small muted block">{t(s.description)}</span>
            </span>
          </label>
        ))}
        <button className="btn small" disabled={!name.trim()} onClick={onMake}>
          {t("Make a key")}
        </button>
      </div>
      {denials.length > 0 && (
        <details className="small">
          <summary>{t("Refused lately ({count})", { count: denials.length })}</summary>
          <ul>
            {denials.map((e, i) => (
              <li key={i}>
                {when(e.at)}: {e.app || t("a program without a known key")}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
