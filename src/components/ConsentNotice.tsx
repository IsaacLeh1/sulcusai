// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useRef, useState } from "react";
import { errorText } from "../api";
import { t } from "../i18n";
import { consentApi, documentLink, DOCUMENT_TITLES, dueFor, isUpdate, type ConsentStatus } from "./ConsentApi";
import { Modal } from "./Modal";

interface NoticeText {
  title: string;
  points: string[];
  /** The longer document behind the notice, when there is one. */
  file?: string;
}

/** What each feature asks a person to take on before it turns on. The
 * version that matters lives in consent.rs; change both together. */
export const NOTICES: Record<string, NoticeText> = {
  social: {
    title: "Before you join groups",
    points: [
      "Groups run on their members' PCs. Nobody else can see, moderate or remove what's in them.",
      "You're responsible for what you post, and for the groups you own or moderate and their rules.",
      "Report crimes to the authorities. Reports inside a group go to its moderators.",
    ],
    file: "community-guidelines.md",
  },
  vpn: {
    title: "Before you use the virtual LAN",
    points: [
      "You're responsible for the networks you create or join, whom you let in, and what passes over them.",
      "Other members' traffic can pass through your PC and, to the outside, look as if it came from your address.",
      "Use it only on devices and networks you're allowed to use.",
    ],
  },
  file_sharing: {
    title: "Before you share files",
    points: [
      "Sharing uploads the pieces you share to other people and uses your connection. Only the files you choose are shared.",
      "Share your own files and models, and others' only when their license or owner allows it.",
      "Files from other people may be harmful. You open them at your own risk.",
    ],
    file: "copyright-policy.md",
  },
  relay: {
    title: "Before you run a relay",
    points: [
      "You're the operator of your relay, with any duties the law gives operators where you live.",
      "Other people's sealed traffic passes through your connection and leaves from your address.",
      "Keep the limits on, publish a contact, and keep it up to date.",
    ],
    file: "running-relays-and-servers.md",
  },
  remote_control: {
    title: "Before you allow remote control",
    points: [
      "Whoever you let in can see your screen and use your PC as you can.",
      "Allow only people you trust, and only on PCs you're allowed to share.",
      "The stop key ends a session at once.",
    ],
  },
  uncensored: {
    title: "Before you use uncensored models",
    points: [
      "These models have had their refusals removed. They will write things other models won't.",
      "Everything they produce, and what you do with it, is your responsibility.",
      "They are for adults only, and the law still applies to what you make.",
    ],
  },
  likeness: {
    title: "Faces, voices and LoRAs",
    points: [
      "Use a real person's face or voice only with their clear consent for that use. Yours is fine.",
      "Never make sexual content of anyone without their consent, or of anyone under 18.",
      "Never use a copied voice or face to deceive, defraud or get past a security check.",
    ],
    file: "likeness-consent.md",
  },
  market: {
    title: "Market data is information only",
    points: [
      "Prices can be late or wrong, and AI can misread them.",
      "Nothing here is advice or a recommendation to buy, sell or hold anything.",
      "Your financial decisions, and their risk, are yours alone.",
    ],
    file: "market-data-disclaimer.md",
  },
};

/** The short responsibility notice shown before a feature turns on. It
 * accepts the notice (and documents that come with it) at the versions it
 * was shown. */
export function ConsentNotice({ feature, due, onAccepted, onCancel }: { feature: string; due: ConsentStatus[]; onAccepted: (all: ConsentStatus[]) => void; onCancel: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const notice = NOTICES[feature];
  if (!notice) return null;
  const updated = due.some(isUpdate);
  const extra = due.filter((d) => d.kind === "document");
  const accept = async () => {
    setBusy(true);
    setError(null);
    try {
      onAccepted(await consentApi.accept(due.map((d) => ({ id: d.id, version: d.version }))));
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal title={t(notice.title)} onClose={() => !busy && onCancel()}>
      {updated && <p className="small muted">{t("This notice changed since you last accepted it.")}</p>}
      <ul className="small">
        {notice.points.map((p) => (
          <li key={p}>{t(p)}</li>
        ))}
      </ul>
      <p className="small">
        {t("SulcusAI is a tool. How you use it is up to you, and so is the responsibility for that use.")}{" "}
        {notice.file && (
          <a href={documentLink(notice.file)} target="_blank" rel="noreferrer">
            {t("Read the full text")}
          </a>
        )}
      </p>
      {extra.length > 0 && (
        <p className="small">
          {t("Accepting also accepts:")}{" "}
          {extra.map((d, i) => (
            <span key={d.id}>
              {i > 0 && ", "}
              {d.file ? (
                <a href={documentLink(d.file)} target="_blank" rel="noreferrer">
                  {t(DOCUMENT_TITLES[d.id] ?? d.id)}
                </a>
              ) : (
                t(DOCUMENT_TITLES[d.id] ?? d.id)
              )}
            </span>
          ))}
        </p>
      )}
      {error && <p className="warn-text small">{error}</p>}
      <div className="modal-actions">
        <button className="btn" onClick={onCancel} disabled={busy}>
          {t("Not now")}
        </button>
        <button className="btn primary" onClick={accept} disabled={busy}>
          {busy ? t("Saving…") : t("I understand and agree")}
        </button>
      </div>
    </Modal>
  );
}

/** For a feature's switch: `ensure("vpn")` resolves true once the notice is
 * accepted (at once if it already was), false if the person says not now.
 * Render `prompt` somewhere in the tree. */
export function useConsentGate() {
  const [asking, setAsking] = useState<{ feature: string; due: ConsentStatus[] } | null>(null);
  const settle = useRef<((ok: boolean) => void) | null>(null);
  const ensure = useCallback(async (feature: string): Promise<boolean> => {
    const due = dueFor(await consentApi.status(), feature);
    if (due.length === 0) return true;
    settle.current?.(false);
    return new Promise<boolean>((resolve) => {
      settle.current = resolve;
      setAsking({ feature, due });
    });
  }, []);
  const finish = (ok: boolean) => {
    settle.current?.(ok);
    settle.current = null;
    setAsking(null);
  };
  const prompt = asking ? <ConsentNotice feature={asking.feature} due={asking.due} onAccepted={() => finish(true)} onCancel={() => finish(false)} /> : null;
  return { ensure, prompt };
}
