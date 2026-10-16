// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type Connectivity, type OfflineKeep, type OfflineView } from "../api";
import { Modal } from "./Modal";
import { t, tx } from "../i18n";

export const LEVELS: { id: Connectivity; icon: string; label: string; short: string; detail: string }[] = [
  {
    id: "offline",
    icon: "🔒",
    label: tx("Offline"),
    short: tx("Offline"),
    detail: tx("Nothing leaves this PC, except model downloads you start and the check for app updates (a switch in Settings)."),
  },
  {
    id: "web",
    icon: "🌐",
    label: tx("Local AI + Web"),
    short: tx("Web"),
    detail: tx("Adds web search, the browser, and email/calendar sync. All AI still runs on this PC."),
  },
  {
    id: "cloud",
    icon: "☁",
    label: tx("Cloud"),
    short: tx("Cloud"),
    detail: tx("Adds cloud AI models and cloud sync. Prompts sent to cloud models leave this PC."),
  },
];

export function levelInfo(level: Connectivity) {
  return LEVELS.find((l) => l.id === level)!;
}

/** Status-bar switch: one click to see and change the level (or go fully
 * offline, keeping what the person picks). */
/** Opens Settings at a section (by its anchor id), from anywhere. */
export function openSettings(section: string) {
  window.dispatchEvent(new CustomEvent("open-settings", { detail: section }));
}

export function ConnectivityMenu({ level, onChange, onRefresh }: { level: Connectivity; onChange: (l: Connectivity) => void; onRefresh: () => void }) {
  const [open, setOpen] = useState(false);
  const [confirmCloud, setConfirmCloud] = useState(false);
  const [offline, setOffline] = useState<OfflineView | null>(null);
  const [choosing, setChoosing] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const fully = !!offline?.on;
  const info = fully ? FULLY : levelInfo(level);

  useEffect(() => {
    api.offlineView().then(setOffline).catch(() => {});
    const sub = on("offline:changed", () => api.offlineView().then(setOffline).catch(() => {}));
    return () => {
      sub.then((un) => un());
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  const pick = (l: Connectivity) => {
    setOpen(false);
    if (l === "cloud" && level !== "cloud") setConfirmCloud(true);
    else onChange(l);
    if (fully) setOffline((o) => (o ? { ...o, on: false } : o));
  };

  return (
    <div className="conn" ref={ref}>
      <button className={`conn-pill ${fully ? "fully" : level}`} onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open}>
        <span aria-hidden>{info.icon}</span> {t(info.short)}
      </button>
      {open && (
        <div className="conn-menu" role="menu">
          <button
            role="menuitemradio"
            aria-checked={fully}
            className={fully ? "active" : ""}
            onClick={() => {
              setOpen(false);
              setChoosing(true);
            }}
          >
            <span className="conn-icon" aria-hidden>{FULLY.icon}</span>
            <span>
              <strong>{t(FULLY.label)}</strong>
              <span className="small muted block">{fully ? t("On. Click to change what may still go out.") : t(FULLY.detail)}</span>
            </span>
          </button>
          {LEVELS.map((l) => (
            <button key={l.id} role="menuitemradio" aria-checked={!fully && l.id === level} className={!fully && l.id === level ? "active" : ""} onClick={() => pick(l.id)}>
              <span className="conn-icon" aria-hidden>{l.icon}</span>
              <span>
                <strong>{t(l.label)}</strong>
                <span className="small muted block">{t(l.detail)}</span>
              </span>
            </button>
          ))}
          {level === "cloud" && (
            <button
              className="link conn-more"
              onClick={() => {
                setOpen(false);
                openSettings("cloud");
              }}
            >
              {t("☁ Cloud models and API keys…")}
            </button>
          )}
        </div>
      )}
      {choosing && offline && (
        <FullyOfflineDialog
          view={offline}
          onClose={() => setChoosing(false)}
          onDone={(v) => {
            setOffline(v);
            setChoosing(false);
            onRefresh();
          }}
        />
      )}
      {confirmCloud && <CloudConfirm onCancel={() => setConfirmCloud(false)} onConfirm={() => { setConfirmCloud(false); onChange("cloud"); }} />}
    </div>
  );
}

const FULLY = {
  icon: "✈",
  label: tx("Fully offline"),
  short: tx("Fully offline"),
  detail: tx("Nothing goes out at all: no update checks, downloads, web or local network, except what you choose to keep."),
};

const KEEP_TEXT: Record<OfflineKeep, { name: string; detail: string }> = {
  updates: { name: tx("Update checks"), detail: tx("Asks GitHub for new versions of SulcusAI, and installs them.") },
  downloads: { name: tx("Downloads"), detail: tx("Models, the AI engine, voices and other parts you add (only when you ask).") },
  web: { name: tx("Web"), detail: tx("Web search and reading pages in chats, weather, finding your location, and the browser.") },
  local_network: { name: tx("Local network"), detail: tx("Syncing your PCs, People, and the phone page, over your network or a VPN.") },
  connectors: { name: tx("Connectors"), detail: tx("Their programs run on this PC and may go online themselves.") },
};

/** Turning Fully offline on: what's on now that needs the network, and
 * which of it to keep. */
function FullyOfflineDialog({ view, onClose, onDone }: { view: OfflineView; onClose: () => void; onDone: (v: OfflineView) => void }) {
  const [keep, setKeep] = useState<Set<OfflineKeep>>(() => new Set(view.on ? view.items.filter((i) => i.kept).map((i) => i.id) : []));
  const [error, setError] = useState<string | null>(null);
  const inUse = view.items.filter((i) => i.in_use);
  const rest = view.items.filter((i) => !i.in_use);
  const row = (i: OfflineView["items"][number]) => (
    <label key={i.id} className="check">
      <input
        type="checkbox"
        checked={keep.has(i.id)}
        onChange={(e) =>
          setKeep((k) => {
            const n = new Set(k);
            if (e.target.checked) n.add(i.id);
            else n.delete(i.id);
            return n;
          })
        }
      />
      <span>
        {t("Keep {name}", { name: t(KEEP_TEXT[i.id]?.name ?? i.name) })}
        <span className="muted small block">{t(KEEP_TEXT[i.id]?.detail ?? i.detail)}</span>
      </span>
    </label>
  );
  const save = async (turnOn: boolean) => {
    try {
      onDone(await api.setFullyOffline(turnOn, [...keep]));
    } catch (e) {
      setError(errorText(e));
    }
  };
  return (
    <Modal title={view.on ? t("Fully offline") : t("Go fully offline?")} onClose={onClose}>
      <p>{t("Nothing leaves this PC: SulcusAI stops checking for updates, downloading, using the web and the local network, and running connectors. Your AI keeps working here.")}</p>
      {inUse.length > 0 ? (
        <>
          <p className="small">{t("These are on now and need the network. Tick any you want to keep:")}</p>
          <div className="offline-keep">{inUse.map(row)}</div>
        </>
      ) : (
        <p className="muted small">{t("Nothing you have on right now needs the network.")}</p>
      )}
      {rest.length > 0 && (
        <details className="offline-more">
          <summary className="small">{t("Other things you could keep")}</summary>
          <div className="offline-keep">{rest.map(row)}</div>
        </details>
      )}
      <p className="muted small">{t("Commands the assistant runs for you are their own programs and can still use the network.")}</p>
      {error && <p className="error-text">{error}</p>}
      <div className="modal-actions">
        {view.on && (
          <button className="btn ghost" onClick={() => save(false)}>
            {t("Turn off Fully offline")}
          </button>
        )}
        <span className="spacer" />
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={() => save(true)} autoFocus>
          {view.on ? t("Save") : t("Go fully offline")}
        </button>
      </div>
    </Modal>
  );
}

export function CloudConfirm({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  return (
    <Modal title={t("Turn on cloud features?")} onClose={onCancel}>
      <p>{t("With Cloud on, you can choose cloud AI models and cloud sync. When you use them:")}</p>
      <ul>
        <li>{t("Your messages and any files you attach are sent to the provider you picked.")}</li>
        <li>{t("Each cloud feature shows a ☁ badge, and you turn providers on one at a time.")}</li>
        <li>{t("Local models stay the default unless you pick a cloud one.")}</li>
      </ul>
      <p className="muted small">{t("Cloud providers arrive in a later update; this setting is ready for them.")}</p>
      <div className="modal-actions">
        <button className="btn" onClick={onCancel}>{t("Keep it local")}</button>
        <button className="btn primary" onClick={onConfirm}>{t("Turn on Cloud")}</button>
      </div>
    </Modal>
  );
}
