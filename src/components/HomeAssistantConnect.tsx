// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { api, errorText, type Connector } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

/** Home Assistant's own MCP server address, from what the person typed. */
export function homeAssistantUrl(typed: string): string {
  let base = typed.trim().replace(/\/+$/, "");
  if (!/^[a-z]+:\/\//i.test(base)) base = `https://${base}`;
  return base.endsWith("/api/mcp") ? base : `${base}/api/mcp`;
}

/** Connects Home Assistant as a connector, so the assistant can see and control the home through it. */
export function HomeAssistantConnect({ onClose, onSaved, toast }: { onClose: () => void; onSaved: (c: Connector) => void; toast: PushToast }) {
  const [address, setAddress] = useState("");
  const [token, setToken] = useState("");
  const [saving, setSaving] = useState(false);

  const save = async () => {
    setSaving(true);
    try {
      onSaved(await api.saveConnector({ id: "", name: "Home Assistant", command: "", args: [], env: {}, url: homeAssistantUrl(address), headers: { Authorization: `Bearer ${token.trim()}` } }));
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal title={t("Connect Home Assistant")} onClose={onClose}>
      <ol className="small">
        <li>{t("In Home Assistant, add the Model Context Protocol Server integration (Settings › Devices & services) and choose which devices it exposes.")}</li>
        <li>{t("On your Home Assistant profile, under Security, make a long-lived access token and paste it here.")}</li>
        <li>{t("Use Home Assistant's https address (for example your Home Assistant Cloud address). Keys aren't sent over plain http to other devices.")}</li>
      </ol>
      <input className="input" placeholder={t("Home Assistant address")} aria-label={t("Home Assistant address")} value={address} onChange={(e) => setAddress(e.target.value)} />
      <input className="input" type="password" autoComplete="off" placeholder={t("Long-lived access token")} aria-label={t("Long-lived access token")} value={token} onChange={(e) => setToken(e.target.value)} />
      <div className="row">
        <button className="btn" onClick={onClose} disabled={saving}>{t("Cancel")}</button>
        <button className="btn primary" onClick={save} disabled={saving || !address.trim() || !token.trim()}>
          {saving ? t("Checking…") : t("Connect")}
        </button>
      </div>
    </Modal>
  );
}
