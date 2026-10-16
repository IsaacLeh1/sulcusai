// SPDX-License-Identifier: AGPL-3.0-only
// People › Connect: this user's codes (they don't expire; each works for as
// many people as it allows, and lists who linked with it, to remove or
// block), Windows Firewall's say on each network (with a one-click fix), and
// typing someone else's code, step by step, with why it failed if it does.
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type PeopleFirewall, type PeopleView, type PersonCode } from "../api";
import { clock } from "../format";
import { t, tx } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

type Attempt = { name: string; result: string; at: number };

const MAXES = [1, 2, 3, 5, 10, 25, 100];
/** Why a code isn't taking links (as the core says it). */
const WHY: Record<string, string> = {
  "Turned off.": tx("Turned off"),
  "Used up.": tx("Used up"),
  "Stopped after too many wrong tries. Make a new code.": tx("Stopped after too many wrong tries. Make a new code."),
};
const KIND: Record<string, string> = { Public: tx("Public"), Private: tx("Private"), Domain: tx("Domain") };

function when(ms: number): string {
  return new Date(ms).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}

export function PeopleConnect({
  v,
  onView,
  onClose,
  onLinked,
  toast,
}: {
  v: PeopleView;
  onView: (v: PeopleView) => void;
  onClose: () => void;
  onLinked: (id: string | null) => void;
  toast: PushToast;
}) {
  const [tab, setTab] = useState<"codes" | "enter">("codes");
  return (
    <Modal title={t("Connect with someone")} onClose={onClose}>
      <div className="seg" role="tablist">
        <button role="tab" aria-selected={tab === "codes"} className={tab === "codes" ? "active" : ""} onClick={() => setTab("codes")}>
          {t("Your codes")}
        </button>
        <button role="tab" aria-selected={tab === "enter"} className={tab === "enter" ? "active" : ""} onClick={() => setTab("enter")}>
          {t("Enter their code")}
        </button>
      </div>
      {tab === "codes" ? (
        <YourCodes v={v} onView={onView} toast={toast} />
      ) : (
        <EnterCode v={v} onLinked={onLinked} onSwap={() => setTab("codes")} toast={toast} />
      )}
      <Help />
    </Modal>
  );
}

// ---------- your codes ----------

function YourCodes({ v, onView, toast }: { v: PeopleView; onView: (v: PeopleView) => void; toast: PushToast }) {
  const [max, setMax] = useState(1);
  const [label, setLabel] = useState("");
  const [attempts, setAttempts] = useState<Record<string, Attempt>>({});
  useEffect(() => {
    const sub = on("people:attempt", (a) => {
      setAttempts((all) => ({ ...all, [a.invite_id]: { name: a.name, result: a.result, at: a.at } }));
      if (a.result === "linked") toast(t("{name} linked with you.", { name: a.name }), "success");
    });
    return () => {
      sub.then((un) => un());
    };
  }, [toast]);
  const run = async (f: () => Promise<PeopleView>) => {
    try {
      onView(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const make = () => {
    run(() => api.peopleNewCode(max, label.trim() || null));
    setLabel("");
  };
  return (
    <div className="form">
      <Reachable v={v} toast={toast} />
      <div className="row wrap new-code">
        <label>
          {t("Good for")}
          <select className="input" value={max} onChange={(e) => setMax(Number(e.target.value))}>
            {MAXES.map((n) => (
              <option key={n} value={n}>
                {n === 1 ? t("1 person (one-time)") : t("{n} people", { n })}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("Name (optional)")}
          <input className="input" value={label} maxLength={40} placeholder={t("e.g. For Sam")} onChange={(e) => setLabel(e.target.value)} onKeyDown={(e) => e.key === "Enter" && make()} />
        </label>
        <button className="btn primary" onClick={make}>
          {t("+ New code")}
        </button>
      </div>
      {v.codes.length === 0 && <p className="muted small">{t("No codes yet. Make one and give it to the person you want to connect with: they type it under Enter their code. Codes don't expire.")}</p>}
      <div className="code-list">
        {v.codes.map((c) => (
          <CodeCard key={c.id} c={c} attempt={attempts[c.id]} run={run} />
        ))}
      </div>
      {v.blocked.length > 0 && (
        <div>
          <strong className="small">{t("Blocked")}</strong>
          <ul className="people-addresses small">
            {v.blocked.map((b) => (
              <li key={b.id} className="row">
                <span className="ellipsis">{b.name}</span>
                <span className="muted">{when(b.at)}</span>
                <button className="link small push-right" onClick={() => run(() => api.peopleBlock(b.id, b.name, false))}>
                  {t("Unblock")}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function CodeCard({ c, attempt, run }: { c: PersonCode; attempt?: Attempt; run: (f: () => Promise<PeopleView>) => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(c.code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // The code is on screen to type.
    }
  };
  const tried =
    attempt &&
    {
      checking: t("{name} is connecting…", { name: attempt.name }),
      wrong: t("{name} typed a wrong code.", { name: attempt.name }),
      linked: t("{name} linked.", { name: attempt.name }),
      refused: t("{name} was turned away.", { name: attempt.name }),
    }[attempt.result];
  const maxes = MAXES.includes(c.max) ? MAXES : [...MAXES, c.max].sort((a, b) => a - b);
  const off = c.why === "Turned off.";
  return (
    <div className={`code-card ${c.open ? "" : "closed"}`}>
      <div className="row code-head">
        <strong className="code">{c.code}</strong>
        <button className="link small" onClick={copy}>
          {copied ? t("Copied") : t("Copy")}
        </button>
        <span className={`badge ${c.open ? "" : "warn"} push-right`}>{c.open ? t("Open") : t(WHY[c.why ?? ""] ?? c.why ?? "Closed")}</span>
      </div>
      {c.label && <div className="small">{c.label}</div>}
      <div className="row wrap small muted">
        <span>{t("{used} of {max} used", { used: c.used.length, max: c.max })}</span>
        {c.wrong > 0 && <span>· {t("{n} wrong tries", { n: c.wrong })}</span>}
        <span>· {t("made {when}", { when: when(c.created_at) })}</span>
      </div>
      {tried && <div className={`small ${attempt!.result === "wrong" || attempt!.result === "refused" ? "warn-text" : attempt!.result === "checking" ? "live-dot" : ""}`}>{tried}</div>}
      {c.used.length > 0 && (
        <ul className="code-uses small">
          {c.used.map((u) => (
            <li key={u.id} className="row">
              <span className="ellipsis">
                <strong>{u.name}</strong> <span className="muted">· {when(u.at)}</span>
              </span>
              <span className="muted">{u.blocked ? t("Blocked") : u.linked ? t("Linked") : t("Removed")}</span>
              {u.linked && !u.blocked && (
                <button
                  className="link small"
                  onClick={() => {
                    if (window.confirm(t("Remove {name}? Your messages with them are deleted from this PC, and they can't reach you until you connect again.", { name: u.name })))
                      run(() => api.peopleRemove(u.id));
                  }}
                >
                  {t("Remove")}
                </button>
              )}
              {u.blocked ? (
                <button className="link small" onClick={() => run(() => api.peopleBlock(u.id, u.name, false))}>
                  {t("Unblock")}
                </button>
              ) : (
                <button
                  className="link small danger"
                  onClick={() => {
                    if (window.confirm(t("Block {name}? They're removed, and can't link with any of your codes again until you unblock them.", { name: u.name })))
                      run(() => api.peopleBlock(u.id, u.name, true));
                  }}
                >
                  {t("Block")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="row small code-controls">
        <label>
          {t("Good for")}
          <select className="input" value={c.max} onChange={(e) => run(() => api.peopleSetCode(c.id, { max: Number(e.target.value) }))}>
            {maxes.map((n) => (
              <option key={n} value={n}>
                {n === 1 ? t("1 person") : t("{n} people", { n })}
              </option>
            ))}
          </select>
        </label>
        <button className="btn ghost small push-right" onClick={() => run(() => api.peopleSetCode(c.id, { on: off }))} disabled={!c.open && !off}>
          {off ? t("Turn on") : t("Turn off")}
        </button>
        <button
          className="btn ghost small danger"
          onClick={() => {
            if (window.confirm(t("Delete this code? People who linked with it stay linked.")))
              run(() => api.peopleDeleteCode(c.id));
          }}
        >
          {t("Delete")}
        </button>
      </div>
    </div>
  );
}

/** Where others can reach this PC, and whether Windows Firewall lets them. */
function Reachable({ v, toast }: { v: PeopleView; toast: PushToast }) {
  const [fw, setFw] = useState<PeopleFirewall | null>(null);
  const [fixing, setFixing] = useState(false);
  useEffect(() => {
    api.peopleFirewall().then(setFw).catch(() => setFw({ checked: false, networks: [] }));
  }, []);
  const status = (adapter: string) => fw?.networks.find((n) => n.name === adapter);
  const blocked = (fw?.networks ?? []).filter((n) => !n.allowed && v.addresses.some((a) => a.adapter === n.name));
  const fix = async () => {
    setFixing(true);
    try {
      setFw(await api.peopleAllowFirewall());
      toast(t("Done: people can reach SulcusAI on every network now."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setFixing(false);
    }
  };
  if (!v.listening) return <p className="warn-text small">{t("This PC isn't taking connections yet. Wait a moment, or restart SulcusAI if it stays like this.")}</p>;
  return (
    <div className="reachable">
      <strong className="small">{t("They type the code, plus this address if they're not on your network:")}</strong>
      <ul className="people-addresses small">
        {v.addresses.map((a) => {
          const s = status(a.adapter);
          return (
            <li key={a.address} className="row">
              <code>{a.address}</code>
              <span className="muted">{a.network}</span>
              {s && (s.allowed ? <span className="ok-text">✓ {t("reachable")}</span> : <span className="warn-text">✗ {t("blocked by Windows Firewall ({kind} network)", { kind: t(KIND[s.kind] ?? s.kind) })}</span>)}
            </li>
          );
        })}
      </ul>
      {!fw && <p className="muted small live-dot">{t("Checking Windows Firewall…")}</p>}
      {blocked.length > 0 && (
        <div className="fw-warn small">
          <p>
            {t("Windows Firewall stops people reaching SulcusAI on {list}. They can still link if you type their code instead, but your code won't work for them until this is allowed.", {
              list: blocked.map((n) => n.name).join(", "),
            })}
          </p>
          <button className="btn small" onClick={fix} disabled={fixing}>
            {fixing ? t("Waiting for Windows' permission…") : t("Let people reach me on every network")}
          </button>
          <span className="muted"> {t("Windows asks for permission first.")}</span>
        </div>
      )}
    </div>
  );
}

// ---------- their code ----------

const STEPS: { id: string; label: string }[] = [
  { id: "find", label: tx("Finding them") },
  { id: "reach", label: tx("Reaching their PC") },
  { id: "check", label: tx("Checking the code") },
  { id: "save", label: tx("Linking") },
];

function EnterCode({ v, onLinked, onSwap, toast }: { v: PeopleView; onLinked: (id: string | null) => void; onSwap: () => void; toast: PushToast }) {
  const [code, setCode] = useState("");
  const [target, setTarget] = useState("");
  const [step, setStep] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [started, setStarted] = useState(0);
  const [now, setNow] = useState(Date.now());
  const before = useRef(new Set(v.contacts.map((c) => c.id)));
  useEffect(() => {
    const sub = on("people:connecting", (p) => setStep(p.step));
    return () => {
      sub.then((un) => un());
    };
  }, []);
  const ready = code.replace(/[^0-9A-Za-z]/g, "").length === 8;
  const busy = step !== null;
  // One clock for the whole attempt.
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [busy]);
  const connect = async () => {
    setFailed(null);
    setStarted(Date.now());
    setNow(Date.now());
    setStep("find");
    try {
      const pick = target.trim() || (v.nearby.length === 1 ? v.nearby[0].id : "");
      const name = await api.peopleConnect(code, pick || null);
      toast(t("Connected with {name}.", { name }), "success");
      const fresh = await api.peopleView();
      onLinked(fresh.contacts.find((c) => !before.current.has(c.id))?.id ?? null);
    } catch (e) {
      setFailed(errorText(e));
    } finally {
      setStep(null);
    }
  };
  const at = STEPS.findIndex((s) => s.id === step);
  return (
    <div className="form">
      <label>
        {t("The code they see")}
        <input
          className="input code-input"
          value={code}
          autoFocus
          placeholder="K7Q2-9XMB"
          maxLength={12}
          disabled={busy}
          onChange={(e) => setCode(e.target.value.toUpperCase())}
          onKeyDown={(e) => e.key === "Enter" && ready && !busy && connect()}
        />
      </label>
      {v.nearby.length > 1 && (
        <label>
          {t("Who")}
          <select className="input" value={target} disabled={busy} onChange={(e) => setTarget(e.target.value)}>
            <option value="">{t("Choose…")}</option>
            {v.nearby.map((n) => (
              <option key={n.id} value={n.id}>
                {n.name}
              </option>
            ))}
          </select>
        </label>
      )}
      {v.nearby.length === 1 && !target && <p className="muted small">{t("Connecting with {name}, found on this network.", { name: v.nearby[0].name })}</p>}
      <label>
        {v.nearby.length > 0 ? t("Or their address") : t("Their address (shown under their codes)")}
        <input className="input" value={target} placeholder="26.10.4.7" disabled={busy} onChange={(e) => setTarget(e.target.value)} />
      </label>
      {busy && (
        <ol className="connect-steps small" aria-live="polite">
          {STEPS.map((s, i) => (
            <li key={s.id} className={i < at ? "done" : i === at ? "now" : ""}>
              <span aria-hidden>{i < at ? "✓" : i === at ? <span className="spinner" /> : "·"}</span> {t(s.label)}
              {i === at && <span className="muted"> · {clock((now - started) / 1000)}</span>}
            </li>
          ))}
        </ol>
      )}
      {failed && (
        <div className="fw-warn small" role="alert">
          <strong>{t("Couldn't connect.")}</strong> {failed}
          <div className="row wrap">
            <button className="btn small" onClick={connect} disabled={!ready}>
              {t("Try again")}
            </button>
            <button className="btn ghost small" onClick={onSwap}>
              {t("Try the other way round: show your code")}
            </button>
          </div>
        </div>
      )}
      <div className="adv-actions">
        <button className="btn primary" disabled={busy || !ready} onClick={connect}>
          {busy ? t("Connecting…") : t("Connect")}
        </button>
      </div>
    </div>
  );
}

function Help() {
  return (
    <details className="small muted">
      <summary>{t("Not on the same network?")}</summary>
      <p>
        {t("Join the same virtual network first, like Radmin VPN, Tailscale or ZeroTier (each of you installs it and joins the same network). Then use the address listed under that network's name. Your apps talk directly, encrypted; nothing goes through SulcusAI's servers, because there aren't any.")}
      </p>
      <p>
        {t("Windows counts virtual networks as Public, and often keeps SulcusAI out of them. The person whose code is used must be reachable: if the check above shows a block, allow it, or swap who makes the code. Once linked, it's enough that one of you can be reached. SulcusAI needs to be open (and unlocked) on both PCs.")}
      </p>
      <p>
        {t("No virtual network? Once you're linked, either of you can let the other use your own relay (Settings → Your devices, at the Cloud level): messages and the assistants' questions then go through it, sealed so the relay can't read them. Watching shared chats live still needs a direct connection.")}
      </p>
    </details>
  );
}
