// SPDX-License-Identifier: AGPL-3.0-only
// "Google" in a chat's model list: Gemini through Google's own API, set up
// with a key from Google AI Studio the first time it's picked.
import { useState } from "react";
import { api, errorText, openInViewer, type CloudModel } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const PROVIDER = "gemini";
const KEY_PAGE = "https://aistudio.google.com/apikey";

/** The Gemini model to start with: a Flash one (fast, and within the free
    tier), not a lite, image, speech or preview one. */
export function firstGemini(models: CloudModel[]): CloudModel | undefined {
  const plain = models.filter((m) => !/lite|image|tts|audio|live|embed|preview|exp/i.test(m.id));
  return plain.find((m) => /flash/i.test(m.id)) ?? plain[0] ?? models[0];
}

/** An "Ask Google" address: Google's AI Mode answering `text`. */
export const aiModeUrl = (text: string) => `https://www.google.com/search?udm=50&q=${encodeURIComponent(text.trim())}`;

export function GoogleSetup({ onDone, onClose, toast }: { onDone: (modelRef: string) => void; onClose: () => void; toast: PushToast }) {
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setBusy(true);
    try {
      const view = await api.cloudView();
      if (!view.providers.some((p) => p.id === PROVIDER)) await api.addCloudProvider(PROVIDER, null, null);
      const saved = await api.setCloudProvider(PROVIDER, true, key.trim());
      const mine = saved.providers.find((p) => p.id === PROVIDER)?.models ?? [];
      const first = firstGemini(await api.cloudModelsAvailable(PROVIDER));
      if (!first) throw new Error(t("Google didn't list any Gemini models for this key."));
      if (!mine.some((m) => m.id === first.id)) await api.setCloudModels(PROVIDER, [...mine, first]);
      onDone(`cloud:${PROVIDER}:${first.id}`);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={t("Google (Gemini)")} onClose={() => !busy && onClose()}>
      <p>{t("Google's Gemini answers in this chat, on Google's servers. It needs an API key from Google AI Studio, which is free for light use.")}</p>
      <p className="small muted">{t("Get a key opens Google AI Studio in the built-in browser: sign in, make a key, and paste it here. It's kept encrypted on this PC.")}</p>
      <div className="row wrap">
        <input className="input grow" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={t("API key")} aria-label={t("Gemini API key")} autoFocus />
        <button className="btn ghost" onClick={() => openInViewer(KEY_PAGE)}>{t("Get a key…")}</button>
      </div>
      <div className="row">
        <span className="spacer" />
        <button className="btn" onClick={onClose} disabled={busy}>{t("Cancel")}</button>
        <button className="btn primary" onClick={save} disabled={busy || !key.trim()}>{busy ? t("Checking…") : t("Use Google")}</button>
      </div>
    </Modal>
  );
}
