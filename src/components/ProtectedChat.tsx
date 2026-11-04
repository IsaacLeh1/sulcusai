// SPDX-License-Identifier: AGPL-3.0-only
import { systemSignIn } from "../signin";
import { useState, type FormEvent } from "react";
import { api, errorText, type SecurityStatus } from "../api";
import { t } from "../i18n";
import { ShieldIcon } from "./Icons";
import { Modal } from "./Modal";

/** The PIN (or Windows Hello) that opens a protected chat, or with
 * `unprotect` takes its protection off. */
function PinForm({ chatId, security, unprotect, onDone }: { chatId: string; security: SecurityStatus; unprotect: boolean; onDone: () => void }) {
  const [pin, setPin] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  const run = async (how: () => Promise<void>) => {
    setError(null);
    setWorking(true);
    try {
      await how();
      setPin("");
      onDone();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setWorking(false);
    }
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (pin) run(() => api.openProtectedChat(chatId, pin, unprotect));
  };

  return (
    <>
      <form className="form" onSubmit={submit}>
        <label>
          {t("PIN or passphrase")}
          <input className="input pin" type="password" autoComplete="off" autoFocus value={pin} onChange={(e) => setPin(e.target.value)} />
        </label>
        {error && <p className="error-text">{error}</p>}
        <button className="btn primary block" type="submit" disabled={working || !pin}>
          {working ? t("Checking…") : unprotect ? t("Remove protection") : t("Open")}
        </button>
      </form>
      {security.hello_enabled && (
        <div className="lock-links">
          <button className="link" onClick={() => run(() => api.openProtectedChatHello(chatId, unprotect))} disabled={working}>
            {t("Use {name}", { name: systemSignIn() })}
          </button>
        </div>
      )}
    </>
  );
}

/** Shown in place of a protected chat until it's opened. */
export function ProtectedGate({ chatId, security, onOpened }: { chatId: string; security: SecurityStatus; onOpened: () => void }) {
  return (
    <div className="lock protected-gate">
      <div className="lock-card">
        <ShieldIcon size={40} />
        <h1>{t("Protected chat")}</h1>
        <p className="muted">{t("This chat is protected with your PIN. It shuts again when you leave it or the app locks.")}</p>
        <PinForm chatId={chatId} security={security} unprotect={false} onDone={onOpened} />
      </div>
    </div>
  );
}

/** Taking the protection off a chat asks for the PIN first. */
export function UnprotectDialog({ chatId, security, onClose, onDone }: { chatId: string; security: SecurityStatus; onClose: () => void; onDone: () => void }) {
  return (
    <Modal title={t("Remove PIN protection")} onClose={onClose}>
      <p className="muted">{t("The chat will open without your PIN, and its title shows in the list again.")}</p>
      <PinForm chatId={chatId} security={security} unprotect onDone={onDone} />
    </Modal>
  );
}
