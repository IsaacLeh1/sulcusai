// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, errorText, type SecurityStatus } from "../api";
import { APP_NAME } from "../brand";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const MIN_PIN = 4;

function PinField({ label, value, onChange, autoFocus }: { label: string; value: string; onChange: (v: string) => void; autoFocus?: boolean }) {
  return (
    <label>
      {label}
      <input
        className="input pin"
        type="password"
        autoComplete="off"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoFocus={autoFocus}
        maxLength={64}
      />
    </label>
  );
}

/** Two matching PIN fields; calls onSubmit with the PIN. */
export function NewPinForm({ submitLabel, onSubmit, busy }: { submitLabel: string; onSubmit: (pin: string) => Promise<void>; busy?: boolean }) {
  const [pin, setPin] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (pin.length < MIN_PIN) return setError(`Use at least ${MIN_PIN} characters.`);
    if (pin !== again) return setError("The two entries don't match.");
    setError(null);
    setWorking(true);
    try {
      await onSubmit(pin);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setWorking(false);
    }
  };

  return (
    <form className="form" onSubmit={submit}>
      <PinField label="New PIN or passphrase" value={pin} onChange={setPin} autoFocus />
      <PinField label="Enter it again" value={again} onChange={setAgain} />
      <p className="muted small">A longer passphrase is stronger than a 4-digit PIN.</p>
      {error && <p className="error-text">{error}</p>}
      <div>
        <button className="btn primary" type="submit" disabled={working || busy || !pin || !again}>
          {working ? "Securing…" : submitLabel}
        </button>
      </div>
    </form>
  );
}

/** Shows a recovery code once, and only continues after the user confirms they saved it. */
export function RecoveryCode({ code, onDone }: { code: string; onDone: () => void }) {
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);
  return (
    <div className="form">
      <p>
        <strong>Save this recovery code.</strong> If you forget your PIN, it's the only way back into your chats.
        Nobody, including the developers, can recover them without it.
      </p>
      <div className="recovery-code" aria-label="Recovery code">{code}</div>
      <div className="row">
        <button
          className="btn"
          type="button"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(code);
              setCopied(true);
            } catch {
              setCopied(false);
            }
          }}
        >
          {copied ? "Copied" : "Copy"}
        </button>
        <span className="muted small">Write it down or keep it in a password manager, not on this PC alone.</span>
      </div>
      <label className="check">
        <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
        I've saved my recovery code somewhere safe
      </label>
      <div>
        <button className="btn primary" disabled={!saved} onClick={onDone}>
          Done
        </button>
      </div>
    </div>
  );
}

/** Full-window lock screen. */
export function LockScreen({ status, onUnlocked }: { status: SecurityStatus; onUnlocked: () => void }) {
  const [mode, setMode] = useState<"pin" | "recovery" | "reset" | "code">("pin");
  const [secret, setSecret] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [newCode, setNewCode] = useState<string | null>(null);
  const triedHello = useRef(false);

  const hello = async () => {
    setError(null);
    setWorking(true);
    try {
      await api.unlockWithHello();
      onUnlocked();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setWorking(false);
    }
  };

  // Offer Windows Hello straight away, once.
  useEffect(() => {
    if (status.hello_enabled && !triedHello.current) {
      triedHello.current = true;
      hello();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!secret) return;
    setError(null);
    setWorking(true);
    try {
      await api.unlock(mode === "recovery" ? "recovery" : "pin", secret);
      setSecret("");
      if (mode === "recovery") setMode("reset");
      else onUnlocked();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setWorking(false);
    }
  };

  return (
    <div className="lock">
      <div className="lock-card">
        <img src="/logo.svg" alt="" width={56} height={56} />
        <h1>{APP_NAME} is locked</h1>
        {mode === "reset" && (
          <>
            <p className="muted">You're in. Choose a new PIN; your old recovery code is replaced with a new one.</p>
            <NewPinForm
              submitLabel="Set new PIN"
              onSubmit={async (pin) => {
                setNewCode(await api.resetPin(pin));
                setMode("code");
              }}
            />
          </>
        )}
        {mode === "code" && newCode && <RecoveryCode code={newCode} onDone={onUnlocked} />}
        {(mode === "pin" || mode === "recovery") && (
          <>
            <form className="form" onSubmit={submit}>
              <label>
                {mode === "pin" ? "PIN or passphrase" : "Recovery code"}
                <input
                  className={`input ${mode === "pin" ? "pin" : "code"}`}
                  type={mode === "pin" ? "password" : "text"}
                  autoComplete="off"
                  autoFocus
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                  placeholder={mode === "recovery" ? "XXXXX-XXXXX-XXXXX-XXXXX" : undefined}
                />
              </label>
              {error && <p className="error-text">{error}</p>}
              <button className="btn primary block" type="submit" disabled={working || !secret}>
                {working ? "Checking…" : "Unlock"}
              </button>
            </form>
            <div className="lock-links">
              {status.hello_enabled && mode === "pin" && (
                <button className="link" onClick={hello} disabled={working}>
                  Use Windows Hello
                </button>
              )}
              <button
                className="link"
                onClick={() => {
                  setMode(mode === "pin" ? "recovery" : "pin");
                  setSecret("");
                  setError(null);
                }}
              >
                {mode === "pin" ? "Forgot your PIN? Use the recovery code" : "Back to PIN"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

const AUTO_LOCK = [
  { minutes: 5, label: "After 5 minutes" },
  { minutes: 15, label: "After 15 minutes" },
  { minutes: 60, label: "After 1 hour" },
  { minutes: 0, label: "Never (only when I lock it)" },
];

/** Settings → Privacy & security. */
export function SecuritySection({ status, onChanged, toast }: { status: SecurityStatus; onChanged: () => void; toast: PushToast }) {
  const [dialog, setDialog] = useState<null | "enable" | "change" | "disable">(null);
  const [code, setCode] = useState<string | null>(null);

  const toggleHello = async (on: boolean) => {
    try {
      await api.setHello(on);
      toast(on ? "Windows Hello can now unlock SulcusAI." : "Windows Hello unlock is off.", "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
    onChanged();
  };

  return (
    <section className="card">
      <h2>Privacy &amp; security</h2>
      <p className="small">
        🔐 Your chats and profile are <strong>encrypted on this PC</strong>. Only your Windows account can open them
        {status.lock_enabled ? ", and only after you unlock with your PIN." : "."}
      </p>

      <div className="setting-row">
        <div>
          <strong>App lock</strong>
          <span className="small muted block">
            {status.lock_enabled
              ? "On. SulcusAI asks for your PIN when it opens and after it's idle."
              : "Ask for a PIN when SulcusAI opens, so others using this PC can't read your chats."}
          </span>
        </div>
        {status.lock_enabled ? (
          <div className="row">
            <button className="btn" onClick={() => setDialog("change")}>Change PIN</button>
            <button className="btn ghost danger" onClick={() => setDialog("disable")}>Turn off</button>
          </div>
        ) : (
          <button className="btn primary" onClick={() => setDialog("enable")}>Turn on</button>
        )}
      </div>

      {status.lock_enabled && (
        <>
          <div className="setting-row">
            <div>
              <strong>Lock automatically</strong>
              <span className="small muted block">When nobody has used SulcusAI for a while.</span>
            </div>
            <select
              value={status.auto_lock_minutes}
              onChange={async (e) => {
                await api.setAutoLock(Number(e.target.value));
                onChanged();
              }}
              aria-label="Lock automatically"
            >
              {AUTO_LOCK.map((o) => (
                <option key={o.minutes} value={o.minutes}>{o.label}</option>
              ))}
            </select>
          </div>
          <div className="setting-row">
            <div>
              <strong>Windows Hello</strong>
              <span className="small muted block">
                {status.hello_available
                  ? "Unlock with your face, fingerprint or Windows PIN. Your SulcusAI PIN still works too."
                  : "Windows Hello isn't set up on this PC. You can set it up in Windows Settings → Accounts → Sign-in options."}
              </span>
            </div>
            <label className="switch">
              <input
                type="checkbox"
                checked={status.hello_enabled}
                disabled={!status.hello_available && !status.hello_enabled}
                onChange={(e) => toggleHello(e.target.checked)}
                aria-label="Unlock with Windows Hello"
              />
              <span />
            </label>
          </div>
        </>
      )}

      {dialog === "enable" && (
        <Modal title={code ? "Your recovery code" : "Turn on app lock"} onClose={() => { if (!code) setDialog(null); }}>
          {code ? (
            <RecoveryCode
              code={code}
              onDone={() => {
                setCode(null);
                setDialog(null);
                onChanged();
                toast("App lock is on.", "success");
              }}
            />
          ) : (
            <NewPinForm submitLabel="Turn on app lock" onSubmit={async (pin) => setCode(await api.enableLock(pin))} />
          )}
        </Modal>
      )}
      {dialog === "change" && (
        <ChangePinDialog
          onClose={() => setDialog(null)}
          onDone={() => {
            setDialog(null);
            toast("PIN changed.", "success");
          }}
        />
      )}
      {dialog === "disable" && (
        <ConfirmPinDialog
          title="Turn off app lock?"
          body="Your chats stay encrypted on this PC, but anyone using your Windows account will be able to open SulcusAI without a PIN. Your recovery code stops working."
          action="Turn off"
          onClose={() => setDialog(null)}
          onConfirm={async (pin) => {
            await api.disableLock(pin);
            setDialog(null);
            onChanged();
            toast("App lock is off.", "success");
          }}
        />
      )}
    </section>
  );
}

function ChangePinDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const [old, setOld] = useState("");
  const [verified, setVerified] = useState(false);
  return (
    <Modal title="Change PIN" onClose={onClose}>
      {!verified ? (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            if (old) setVerified(true);
          }}
        >
          <PinField label="Current PIN" value={old} onChange={setOld} autoFocus />
          <div className="modal-actions">
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
            <button type="submit" className="btn primary" disabled={!old}>Next</button>
          </div>
        </form>
      ) : (
        <NewPinForm
          submitLabel="Change PIN"
          onSubmit={async (pin) => {
            await api.changePin(old, pin);
            onDone();
          }}
        />
      )}
    </Modal>
  );
}

function ConfirmPinDialog({ title, body, action, onClose, onConfirm }: { title: string; body: string; action: string; onClose: () => void; onConfirm: (pin: string) => Promise<void> }) {
  const [pin, setPin] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  return (
    <Modal title={title} onClose={onClose}>
      <form
        className="form"
        onSubmit={async (e) => {
          e.preventDefault();
          setWorking(true);
          setError(null);
          try {
            await onConfirm(pin);
          } catch (err) {
            setError(errorText(err));
          } finally {
            setWorking(false);
          }
        }}
      >
        <p>{body}</p>
        <PinField label="Your PIN" value={pin} onChange={setPin} autoFocus />
        {error && <p className="error-text">{error}</p>}
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn danger-fill" disabled={!pin || working}>{working ? "Checking…" : action}</button>
        </div>
      </form>
    </Modal>
  );
}
