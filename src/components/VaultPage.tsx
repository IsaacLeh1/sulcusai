// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useState, type FormEvent } from "react";
import { open as openFile, save } from "@tauri-apps/plugin-dialog";
import { errorText } from "../api";
import {
  vaultApi,
  type GenOptions,
  type ImportFormat,
  type MergeReport,
  type Revealed,
  type Strength,
  type VaultChecks,
  type VaultEntry,
  type VaultEntryInput,
  type VaultStatus,
} from "../api/vault";
import { confirmed } from "../confirm";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";
import "./vaultPage.css";

export const HIDE_AFTER_MS = 30_000;
export const CLIPBOARD_CLEAR_MS = 30_000;
export const MIN_MASTER = 10;

const NOTICE = [
  tx("The password vault keeps your sign-ins in an encrypted file on this PC, under a master password only you know."),
  tx("You are responsible for that password and for the recovery key shown once at setup. If you lose both, nobody can open the vault, SulcusAI included."),
  tx("Keep your own copy of anything you can't afford to lose, and use this feature at your own risk."),
  tx("While this page is open, SulcusAI hides its window from screenshots and screen sharing. The assistant, its tools and memory never see the vault."),
];

const STRENGTH_LABEL: Record<Strength, string> = {
  very_weak: tx("Very weak"),
  weak: tx("Weak"),
  fair: tx("Fair"),
  strong: tx("Strong"),
};

const EMPTY_INPUT: VaultEntryInput = { origin: "", name: "", username: "", password: "", notes: "" };

const DEFAULT_GEN: GenOptions = { length: 20, lower: true, upper: true, digits: true, symbols: true, avoid_ambiguous: false };

// ---------- plain helpers ----------

function hostOf(origin: string): string {
  try {
    return new URL(origin).host || origin;
  } catch {
    return origin;
  }
}

export function entryLabel(e: Pick<VaultEntry, "name" | "origin">): string {
  return e.name || hostOf(e.origin);
}

/** Matches the name, address and username, in name order. */
export function filterEntries(entries: VaultEntry[], query: string): VaultEntry[] {
  const q = query.trim().toLowerCase();
  const hit = q ? entries.filter((e) => [e.name, e.origin, e.username].some((s) => s.toLowerCase().includes(q))) : entries;
  return [...hit].sort((a, b) => entryLabel(a).localeCompare(entryLabel(b)));
}

export function importSummary(r: MergeReport): string {
  const parts = [t("{n} added", { n: r.added })];
  if (r.updated) parts.push(t("{n} updated", { n: r.updated }));
  if (r.unchanged) parts.push(t("{n} already saved", { n: r.unchanged }));
  if (r.skipped) parts.push(t("{n} skipped", { n: r.skipped }));
  return parts.join(", ");
}

export interface ClipboardLike {
  writeText(text: string): Promise<void>;
  readText?(): Promise<string>;
}

let pendingClear: { timer: ReturnType<typeof setTimeout>; text: string; clip: ClipboardLike } | null = null;

/** Puts `text` on the clipboard and empties it again after `ms`, unless
 * something else has been copied since. A copy that is still waiting is
 * emptied at once when the next one starts, so only one is ever pending. */
export async function copyAndClear(text: string, clip: ClipboardLike = navigator.clipboard, ms = CLIPBOARD_CLEAR_MS): Promise<void> {
  await clearClipboardNow();
  await clip.writeText(text);
  pendingClear = { timer: setTimeout(() => void clearClipboardNow(), ms), text, clip };
}

/** Called on lock too, so a copied password doesn't outlive the unlocked vault. */
export async function clearClipboardNow(): Promise<void> {
  const p = pendingClear;
  if (!p) return;
  pendingClear = null;
  clearTimeout(p.timer);
  try {
    const now = p.clip.readText ? await p.clip.readText() : p.text;
    if (now === p.text) await p.clip.writeText("");
  } catch {
    // Reading can be refused; an empty clipboard is the safer way to fail.
    await p.clip.writeText("").catch(() => {});
  }
}

/** Runs `expire` after `ms` while `value` is set; a new value restarts the wait. */
function useExpiry<T>(value: T | null, ms: number, expire: () => void) {
  useEffect(() => {
    if (value === null) return;
    const id = window.setTimeout(expire, ms);
    return () => window.clearTimeout(id);
  }, [value, ms, expire]);
}

// ---------- small pieces ----------

function PasswordField({ label, value, onChange, autoFocus, fresh }: { label: string; value: string; onChange: (v: string) => void; autoFocus?: boolean; fresh?: boolean }) {
  return (
    <label>
      {label}
      <input
        className="input"
        type="password"
        autoComplete={fresh ? "new-password" : "off"}
        spellCheck={false}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoFocus={autoFocus}
        maxLength={256}
      />
    </label>
  );
}

function Notice() {
  return (
    <div className="vault-notice">
      {NOTICE.map((line) => (
        <p key={line} className="small">{t(line)}</p>
      ))}
    </div>
  );
}

/** The switch and, to turn it on, the notice that has to be accepted first. */
export function VaultSwitch({ enabled, busy, onTurnOn, onTurnOff }: { enabled: boolean; busy: boolean; onTurnOn: () => void; onTurnOff: () => void }) {
  return (
    <div className="setting-row">
      <div>
        <strong>{t("Password vault")}</strong>
        <span className="muted small block">{t("Save and fill sign-ins, kept in an encrypted file on this PC. Off until you turn it on.")}</span>
      </div>
      <label className="switch" title={enabled ? t("On") : t("Off")}>
        <input type="checkbox" checked={enabled} disabled={busy} onChange={(e) => (e.target.checked ? onTurnOn() : onTurnOff())} aria-label={t("Password vault on or off")} />
        <span />
      </label>
    </div>
  );
}

export function NoticeStep({ busy, onAccept, onCancel }: { busy: boolean; onAccept: () => void; onCancel: () => void }) {
  const [agreed, setAgreed] = useState(false);
  return (
    <div>
      <Notice />
      <label className="row">
        <input type="checkbox" checked={agreed} onChange={(e) => setAgreed(e.target.checked)} />
        <span>{t("I have read this and accept it.")}</span>
      </label>
      <div className="row">
        <button className="btn primary" disabled={!agreed || busy} onClick={onAccept}>{t("Turn on")}</button>
        <button className="btn ghost" onClick={onCancel}>{t("Cancel")}</button>
      </div>
    </div>
  );
}

/** A master password asked for twice. */
export function SetupForm({ busy, onSetup }: { busy: boolean; onSetup: (master: string) => Promise<void> }) {
  const [master, setMaster] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (master.length < MIN_MASTER) return setError(t("Use at least {n} characters.", { n: MIN_MASTER }));
    if (master !== again) return setError(t("The two entries don't match."));
    setError(null);
    try {
      await onSetup(master);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setMaster("");
      setAgain("");
    }
  };
  return (
    <form className="vault-form" onSubmit={submit}>
      <p className="muted small">{t("Choose a master password. A few unrelated words make a strong one. It can't be recovered without the recovery key you get next.")}</p>
      <PasswordField label={t("Master password")} value={master} onChange={setMaster} autoFocus fresh />
      <PasswordField label={t("Enter it again")} value={again} onChange={setAgain} fresh />
      {error && <p className="error-text">{error}</p>}
      <div className="row">
        <button className="btn primary" type="submit" disabled={busy || !master || !again}>{busy ? t("Working…") : t("Set up the vault")}</button>
      </div>
    </form>
  );
}

/** The recovery key, shown once. It leaves the screen at "I saved it". */
export function RecoveryKeyStep({ recoveryKey, onSaved }: { recoveryKey: string; onSaved: () => void }) {
  return (
    <div className="vault-printable">
      <h3>{t("Your recovery key")}</h3>
      <p className="small">{t("This is the only way back in if you forget the master password. It is shown once and not stored in readable form. Print it or write it down and keep it somewhere safe, away from this PC.")}</p>
      <code className="vault-key">{recoveryKey}</code>
      <div className="row vault-noprint">
        <button className="btn" onClick={() => window.print()}>{t("Print")}</button>
        <button className="btn primary" onClick={onSaved}>{t("I saved it")}</button>
      </div>
    </div>
  );
}

export function UnlockForm({ busy, onUnlock, onRecover }: { busy: boolean; onUnlock: (master: string) => Promise<void>; onRecover: (recoveryKey: string, master: string) => Promise<void> }) {
  const [master, setMaster] = useState("");
  const [recovery, setRecovery] = useState("");
  const [recovering, setRecovering] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      if (recovering) await onRecover(recovery, master);
      else await onUnlock(master);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setMaster("");
      setRecovery("");
    }
  };
  return (
    <form className="vault-form" onSubmit={submit}>
      <p className="muted small">{recovering ? t("Type the recovery key you printed, then choose a new master password.") : t("The vault is locked.")}</p>
      {recovering && <PasswordField label={t("Recovery key")} value={recovery} onChange={setRecovery} autoFocus />}
      <PasswordField label={recovering ? t("New master password") : t("Master password")} value={master} onChange={setMaster} autoFocus={!recovering} fresh={recovering} />
      {error && <p className="error-text">{error}</p>}
      <div className="row">
        <button className="btn primary" type="submit" disabled={busy || !master || (recovering && !recovery)}>{recovering ? t("Recover") : t("Unlock")}</button>
        <button className="btn ghost" type="button" onClick={() => setRecovering(!recovering)}>{recovering ? t("Back") : t("Forgot the master password?")}</button>
      </div>
    </form>
  );
}

// ---------- the list ----------

export function EntryRow({
  entry,
  revealed,
  onShow,
  onHide,
  onCopyUser,
  onCopyPassword,
  onEdit,
  onDelete,
}: {
  entry: VaultEntry;
  revealed: Revealed | null;
  onShow: () => void;
  onHide: () => void;
  onCopyUser: () => void;
  onCopyPassword: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <li className="vault-entry">
      <div className="vault-entry-head">
        <span>
          <strong>{entryLabel(entry)}</strong>
          {!entry.fillable && <span className="tag">{t("Not filled")}</span>}
          <span className="muted small block">{entry.username || t("No username")} · {hostOf(entry.origin)}</span>
        </span>
        <span className="row">
          <button className="btn ghost small" onClick={revealed ? onHide : onShow}>{revealed ? t("Hide") : t("Show")}</button>
          <button className="btn ghost small" onClick={onCopyPassword}>{t("Copy password")}</button>
          {entry.username && <button className="btn ghost small" onClick={onCopyUser}>{t("Copy username")}</button>}
          <button className="btn ghost small" onClick={onEdit}>{t("Edit")}</button>
          <button className="btn ghost danger small" onClick={onDelete}>{t("Delete")}</button>
        </span>
      </div>
      {revealed && (
        <div className="vault-revealed">
          <span className="vault-secret">{revealed.password}</span>
          {revealed.notes && <span className="small block">{revealed.notes}</span>}
          <span className="muted small block">{t("Hidden again after 30 seconds.")}</span>
        </div>
      )}
    </li>
  );
}

export function EntryForm({
  title,
  initial,
  editing,
  busy,
  error,
  onSubmit,
  onCancel,
  onGenerate,
}: {
  title: string;
  initial: VaultEntryInput;
  editing: boolean;
  busy: boolean;
  error: string | null;
  onSubmit: (input: VaultEntryInput) => void;
  onCancel: () => void;
  onGenerate: () => Promise<string>;
}) {
  const [f, setF] = useState(initial);
  const [shown, setShown] = useState(false);
  const set = (k: keyof VaultEntryInput) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });
  return (
    <form
      className="vault-form"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit(f);
      }}
    >
      <h3>{title}</h3>
      <label>
        {t("Website address")}
        <input className="input" value={f.origin} onChange={set("origin")} placeholder="https://example.com" autoFocus />
      </label>
      <label>
        {t("Name")}
        <input className="input" value={f.name} onChange={set("name")} />
      </label>
      <label>
        {t("Username")}
        <input className="input" value={f.username} onChange={set("username")} autoComplete="off" />
      </label>
      <label>
        {editing ? t("New password (leave empty to keep the saved one)") : t("Password")}
        <input className="input" type={shown ? "text" : "password"} value={f.password} onChange={set("password")} autoComplete="new-password" spellCheck={false} />
      </label>
      <div className="row">
        <button className="btn ghost small" type="button" onClick={() => setShown(!shown)}>{shown ? t("Hide") : t("Show")}</button>
        <button
          className="btn ghost small"
          type="button"
          onClick={async () => {
            try {
              setF({ ...f, password: await onGenerate() });
              setShown(true);
            } catch {
              // The page shows why on its own (locked, off).
            }
          }}
        >
          {t("Generate")}
        </button>
      </div>
      <label>
        {t("Notes")}
        <textarea className="input" rows={3} value={f.notes} onChange={set("notes")} />
      </label>
      {error && <p className="error-text">{error}</p>}
      <div className="row">
        <button className="btn primary" type="submit" disabled={busy || (!editing && !f.password)}>{busy ? t("Saving…") : t("Save")}</button>
        <button className="btn ghost" type="button" onClick={onCancel}>{t("Cancel")}</button>
      </div>
    </form>
  );
}

// ---------- checks ----------

export function ChecksView({ report, entries }: { report: VaultChecks; entries: VaultEntry[] }) {
  const byId = new Map(entries.map((e) => [e.id, e]));
  const label = (id: string) => {
    const e = byId.get(id);
    return e ? `${entryLabel(e)}${e.username ? ` (${e.username})` : ""}` : t("A removed entry");
  };
  if (report.weak.length + report.reused.length + report.old.length === 0) return <p className="muted small">{t("No weak, reused or old passwords found.")}</p>;
  return (
    <>
      {report.weak.length > 0 && (
        <div className="vault-check-group">
          <strong>{t("Weak passwords")}</strong>
          <ul className="teach-list small">
            {report.weak.map(([id, s]) => (
              <li key={id}><span>{label(id)}</span><span className="muted">{t(STRENGTH_LABEL[s])}</span></li>
            ))}
          </ul>
        </div>
      )}
      {report.reused.length > 0 && (
        <div className="vault-check-group">
          <strong>{t("Passwords used more than once")}</strong>
          <ul className="teach-list small">
            {report.reused.map((ids) => (
              <li key={ids.join("|")}><span>{ids.map(label).join(", ")}</span></li>
            ))}
          </ul>
        </div>
      )}
      {report.old.length > 0 && (
        <div className="vault-check-group">
          <strong>{t("Not changed in over a year")}</strong>
          <ul className="teach-list small">
            {report.old.map((id) => (
              <li key={id}><span>{label(id)}</span></li>
            ))}
          </ul>
        </div>
      )}
    </>
  );
}

// ---------- generator ----------

function Generator({ onUse, toast }: { onUse: (password: string) => void; toast: PushToast }) {
  const [opts, setOpts] = useState(DEFAULT_GEN);
  const [made, setMade] = useState<string | null>(null);
  const clear = useCallback(() => setMade(null), []);
  useExpiry(made, HIDE_AFTER_MS, clear);
  const flag = (k: "lower" | "upper" | "digits" | "symbols" | "avoid_ambiguous", label: string) => (
    <label className="row">
      <input type="checkbox" checked={opts[k]} onChange={(e) => setOpts({ ...opts, [k]: e.target.checked })} />
      <span>{label}</span>
    </label>
  );
  return (
    <div className="vault-form">
      <label>
        {t("Length")}
        <input className="input" type="number" min={8} max={128} value={opts.length} onChange={(e) => setOpts({ ...opts, length: Math.min(128, Math.max(8, Number(e.target.value) || 20)) })} />
      </label>
      {flag("lower", t("Lowercase letters"))}
      {flag("upper", t("Uppercase letters"))}
      {flag("digits", t("Digits"))}
      {flag("symbols", t("Symbols"))}
      {flag("avoid_ambiguous", t("Leave out look-alike characters (l, 1, O, 0)"))}
      <div className="row">
        <button
          className="btn primary"
          onClick={async () => {
            try {
              setMade(await vaultApi.generate(opts));
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        >
          {t("Generate")}
        </button>
      </div>
      {made && (
        <div className="vault-revealed">
          <span className="vault-secret">{made}</span>
          <span className="row">
            <button className="btn ghost small" onClick={() => copyAndClear(made).then(() => toast(t("Copied. The clipboard is emptied in 30 seconds."), "success"), () => {})}>{t("Copy")}</button>
            <button className="btn ghost small" onClick={() => onUse(made)}>{t("Use in a new entry")}</button>
          </span>
          <span className="muted small block">{t("Hidden again after 30 seconds.")}</span>
        </div>
      )}
    </div>
  );
}

// ---------- import and export ----------

function Transfer({ toast, onChanged }: { toast: PushToast; onChanged: () => void }) {
  const [format, setFormat] = useState<ImportFormat>("other");
  const [importPw, setImportPw] = useState("");
  const [exportPw, setExportPw] = useState("");
  const [exportAgain, setExportAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  const doImport = async () => {
    const path = await openFile({ multiple: false, filters: format === "sulcus" ? [{ name: t("Vault export"), extensions: ["sulcusvault"] }] : undefined });
    if (typeof path !== "string") return;
    setBusy(true);
    try {
      const r = await vaultApi.import(path, format, format === "sulcus" ? importPw : undefined);
      setResult(importSummary(r.report));
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setImportPw("");
      setBusy(false);
    }
  };

  const doExport = async () => {
    if (exportPw !== exportAgain) return toast(t("The two entries don't match."), "error");
    const path = await save({ defaultPath: "SulcusAI passwords.sulcusvault", filters: [{ name: t("Vault export"), extensions: ["sulcusvault"] }] });
    if (!path) return;
    setBusy(true);
    try {
      await vaultApi.export(path, exportPw);
      toast(t("Exported. Keep the export password: the file can't be opened without it."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setExportPw("");
      setExportAgain("");
      setBusy(false);
    }
  };

  return (
    <>
      <h3>{t("Import")}</h3>
      <div className="vault-form">
        <label>
          {t("File from")}
          <select className="input" value={format} onChange={(e) => setFormat(e.target.value as ImportFormat)}>
            <option value="other">{t("Another program (Chrome, Edge, Firefox, KeePass, Bitwarden, 1Password)")}</option>
            <option value="sulcus">{t("A SulcusAI vault export")}</option>
          </select>
        </label>
        {format === "sulcus" && <PasswordField label={t("Export password")} value={importPw} onChange={setImportPw} />}
        <p className="muted small">{t("Another program's export holds every password as readable text. Delete the file once it is imported.")}</p>
        <div className="row">
          <button className="btn" disabled={busy || (format === "sulcus" && !importPw)} onClick={doImport}>{t("Choose a file…")}</button>
        </div>
        {result && <p className="small">{result}</p>}
      </div>

      <h3>{t("Export")}</h3>
      <div className="vault-form">
        <p className="muted small">{t("A copy of the vault sealed with a password of its own. Use it to move the vault to another PC or keep a backup.")}</p>
        <PasswordField label={t("Export password")} value={exportPw} onChange={setExportPw} fresh />
        <PasswordField label={t("Enter it again")} value={exportAgain} onChange={setExportAgain} fresh />
        <div className="row">
          <button className="btn" disabled={busy || !exportPw} onClick={doExport}>{t("Export…")}</button>
        </div>
      </div>
    </>
  );
}

// ---------- master password ----------

function MasterPanel({ toast, onRecoveryKey }: { toast: PushToast; onRecoveryKey: (key: string) => void }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);

  const change = async (e: FormEvent) => {
    e.preventDefault();
    if (next.length < MIN_MASTER) return toast(t("Use at least {n} characters.", { n: MIN_MASTER }), "error");
    if (next !== again) return toast(t("The two entries don't match."), "error");
    setBusy(true);
    try {
      await vaultApi.changeMaster(current, next);
      toast(t("Master password changed."), "success");
    } catch (err) {
      toast(errorText(err), "error");
    } finally {
      setCurrent("");
      setNext("");
      setAgain("");
      setBusy(false);
    }
  };

  const newKey = async () => {
    if (!current) return toast(t("Type the current master password first."), "error");
    if (!(await confirmed(t("Make a new recovery key? The old one stops working.")))) return;
    setBusy(true);
    try {
      onRecoveryKey(await vaultApi.newRecoveryKey(current));
    } catch (err) {
      toast(errorText(err), "error");
    } finally {
      setCurrent("");
      setBusy(false);
    }
  };

  return (
    <form className="vault-form" onSubmit={change}>
      <PasswordField label={t("Current master password")} value={current} onChange={setCurrent} />
      <PasswordField label={t("New master password")} value={next} onChange={setNext} fresh />
      <PasswordField label={t("Enter it again")} value={again} onChange={setAgain} fresh />
      <div className="row">
        <button className="btn primary" type="submit" disabled={busy || !current || !next}>{t("Change master password")}</button>
        <button className="btn ghost" type="button" disabled={busy || !current} onClick={newKey}>{t("Make a new recovery key")}</button>
      </div>
    </form>
  );
}

// ---------- the page ----------

type Tab = "passwords" | "generator" | "transfer" | "checks" | "master";

const TABS: [Tab, string][] = [
  ["passwords", tx("Passwords")],
  ["generator", tx("Generator")],
  ["transfer", tx("Import and export")],
  ["checks", tx("Checks")],
  ["master", tx("Master password")],
];

/** Settings: the password vault. Off until switched on; every field and
 * revealed password is dropped when the vault locks or the page closes. */
export function VaultPage({ toast }: { toast: PushToast }) {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [asking, setAsking] = useState(false);
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const [entries, setEntries] = useState<VaultEntry[]>([]);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("passwords");
  const [revealed, setRevealed] = useState<{ id: string; value: Revealed } | null>(null);
  const [form, setForm] = useState<{ id: string | null; input: VaultEntryInput } | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [checks, setChecks] = useState<VaultChecks | null>(null);

  const dropSecrets = useCallback(() => {
    setRevealed(null);
    setForm(null);
    setFormError(null);
    setChecks(null);
    setEntries([]);
    setQuery("");
    void clearClipboardNow();
  }, []);

  const refresh = useCallback(async () => {
    try {
      const s = await vaultApi.status();
      setStatus(s);
      if (s.enabled && s.unlocked) setEntries(await vaultApi.list());
      else dropSecrets();
    } catch (e) {
      toast(errorText(e), "error");
    }
  }, [toast, dropSecrets]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // The window stays out of screen capture while the vault is on and this
  // page is open.
  const hidden = status?.enabled === true;
  useEffect(() => {
    if (!hidden) return;
    vaultApi.hideFromCapture(true).catch(() => {});
    return () => {
      vaultApi.hideFromCapture(false).catch(() => {});
      setRevealed(null);
      void clearClipboardNow();
    };
  }, [hidden]);

  const hide = useCallback(() => setRevealed(null), []);
  useExpiry(revealed, HIDE_AFTER_MS, hide);

  /** Runs a call that changes the vault, then reloads; a lock that happened
   * meanwhile (idle, another window) shows as the lock screen. */
  const act = async <T,>(fn: () => Promise<T>): Promise<T | undefined> => {
    setBusy(true);
    try {
      return await fn();
    } catch (e) {
      toast(errorText(e), "error");
      void refresh();
      return undefined;
    } finally {
      setBusy(false);
    }
  };

  const turnOn = async () => {
    await act(async () => {
      setStatus(await vaultApi.setEnabled(true, true));
      setAsking(false);
    });
  };

  const turnOff = async () => {
    if (!(await confirmed(t("Turn the password vault off? It locks now. Your saved passwords stay in their encrypted file.")))) return;
    await act(async () => {
      setStatus(await vaultApi.setEnabled(false, false));
      dropSecrets();
      setRecoveryKey(null);
    });
  };

  const lock = async () => {
    await act(async () => {
      await vaultApi.lock();
      await refresh();
    });
  };

  const show = async (e: VaultEntry) => {
    await act(async () => setRevealed({ id: e.id, value: await vaultApi.reveal(e.id) }));
  };

  const copyPassword = async (e: VaultEntry) => {
    await act(async () => {
      const r = await vaultApi.reveal(e.id);
      await copyAndClear(r.password);
      toast(t("Copied. The clipboard is emptied in 30 seconds."), "success");
    });
  };

  const copyUser = async (e: VaultEntry) => {
    await navigator.clipboard.writeText(e.username).catch(() => {});
  };

  const edit = async (e: VaultEntry) => {
    // Notes are kept only if the form starts with them; the password stays
    // empty, which keeps the saved one.
    const r = await act(() => vaultApi.reveal(e.id));
    if (!r) return;
    setFormError(null);
    setForm({ id: e.id, input: { origin: e.origin, name: e.name, username: e.username, password: "", notes: r.notes } });
  };

  const remove = async (e: VaultEntry) => {
    if (!(await confirmed(t("Delete the saved password for {name}? This can't be undone.", { name: entryLabel(e) })))) return;
    await act(async () => {
      await vaultApi.remove(e.id);
      if (revealed?.id === e.id) setRevealed(null);
      await refresh();
    });
  };

  const submit = async (input: VaultEntryInput) => {
    if (!form) return;
    setBusy(true);
    setFormError(null);
    try {
      if (form.id) await vaultApi.edit(form.id, input);
      else await vaultApi.add(input);
      setForm(null);
      setRevealed(null);
      await refresh();
    } catch (e) {
      setFormError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const runChecks = async () => {
    const r = await act(() => vaultApi.check());
    if (r) setChecks(r);
  };

  if (!status) return null;

  return (
    <section className="card vault-page" id="settings-password-vault">
      <h2>{t("Password vault")}</h2>
      <VaultSwitch enabled={status.enabled} busy={busy} onTurnOn={() => setAsking(true)} onTurnOff={turnOff} />

      {!status.enabled && asking && <NoticeStep busy={busy} onAccept={turnOn} onCancel={() => setAsking(false)} />}

      {status.enabled && recoveryKey && <RecoveryKeyStep recoveryKey={recoveryKey} onSaved={() => setRecoveryKey(null)} />}

      {status.enabled && !recoveryKey && !status.set_up && (
        <SetupForm
          busy={busy}
          onSetup={async (master) => {
            setBusy(true);
            try {
              setRecoveryKey(await vaultApi.setup(master));
              await refresh();
            } finally {
              setBusy(false);
            }
          }}
        />
      )}

      {status.enabled && !recoveryKey && status.set_up && !status.unlocked && (
        <UnlockForm
          busy={busy}
          onUnlock={async (master) => {
            setBusy(true);
            try {
              await vaultApi.unlock(master);
              await refresh();
            } finally {
              setBusy(false);
            }
          }}
          onRecover={async (key, master) => {
            setBusy(true);
            try {
              await vaultApi.recover(key, master);
              await refresh();
            } finally {
              setBusy(false);
            }
          }}
        />
      )}

      {status.enabled && !recoveryKey && status.unlocked && (
        <>
          <div className="vault-tabs">
            {TABS.map(([id, label]) => (
              <button key={id} className={`btn ghost small${tab === id ? " active" : ""}`} onClick={() => setTab(id)}>{t(label)}</button>
            ))}
            <button className="btn small" onClick={lock} disabled={busy}>{t("Lock now")}</button>
          </div>

          {tab === "passwords" &&
            (form ? (
              <EntryForm
                key={form.id ?? "new"}
                title={form.id ? t("Edit saved password") : t("Add a saved password")}
                initial={form.input}
                editing={form.id !== null}
                busy={busy}
                error={formError}
                onSubmit={submit}
                onCancel={() => setForm(null)}
                onGenerate={() => vaultApi.generate(DEFAULT_GEN)}
              />
            ) : (
              <>
                <div className="row">
                  <input className="input" type="search" placeholder={t("Search")} aria-label={t("Search")} value={query} onChange={(e) => setQuery(e.target.value)} />
                  <button className="btn" onClick={() => { setFormError(null); setForm({ id: null, input: EMPTY_INPUT }); }}>{t("Add")}</button>
                </div>
                {entries.length === 0 ? (
                  <p className="muted small">{t("Nothing saved yet. Add a password or import them from another program.")}</p>
                ) : (
                  <ul className="teach-list vault-list">
                    {filterEntries(entries, query).map((e) => (
                      <EntryRow
                        key={e.id}
                        entry={e}
                        revealed={revealed?.id === e.id ? revealed.value : null}
                        onShow={() => show(e)}
                        onHide={hide}
                        onCopyUser={() => copyUser(e)}
                        onCopyPassword={() => copyPassword(e)}
                        onEdit={() => edit(e)}
                        onDelete={() => remove(e)}
                      />
                    ))}
                  </ul>
                )}
              </>
            ))}

          {tab === "generator" && (
            <Generator
              toast={toast}
              onUse={(password) => {
                setFormError(null);
                setForm({ id: null, input: { ...EMPTY_INPUT, password } });
                setTab("passwords");
              }}
            />
          )}

          {tab === "transfer" && <Transfer toast={toast} onChanged={refresh} />}

          {tab === "checks" && (
            <>
              <div className="row">
                <button className="btn" disabled={busy} onClick={runChecks}>{checks ? t("Check again") : t("Check passwords")}</button>
              </div>
              <p className="muted small">{t("Worked out on this PC. Nothing is sent anywhere.")}</p>
              {checks && <ChecksView report={checks} entries={entries} />}
            </>
          )}

          {tab === "master" && <MasterPanel toast={toast} onRecoveryKey={setRecoveryKey} />}
        </>
      )}
    </section>
  );
}
