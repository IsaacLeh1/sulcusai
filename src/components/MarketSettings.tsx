// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { t } from "../i18n";
import { useConsentGate } from "./ConsentNotice";
import type { PushToast } from "./Toasts";

/** Settings: market prices are always plain; analysis is switched on here, after its notice. */
export function MarketSettings({ toast }: { toast: PushToast }) {
  const [on, setOn] = useState<boolean | null>(null);
  const consent = useConsentGate();
  useEffect(() => {
    invoke<boolean>("market_analytics").then(setOn).catch(() => setOn(false));
  }, []);
  if (on === null) return null;

  const change = async (want: boolean) => {
    if (want && !(await consent.ensure("market"))) return;
    try {
      setOn(await invoke<boolean>("set_market_analytics", { on: want }));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="card">
      {consent.prompt}
      <h2>{t("Market data")}</h2>
      <div className="toggle-row">
        <div>
          <strong>{t("Analysis of prices")}</strong>
          <span className="small muted block">
            {t("Prices are always given plainly, as information. With this on, answers also work out averages and momentum, show charts and give the case for and against. None of it is advice.")}
          </span>
        </div>
        <label className="switch">
          <input type="checkbox" checked={on} onChange={(e) => change(e.target.checked)} aria-label={t("Analysis of prices")} />
          <span />
        </label>
      </div>
    </section>
  );
}
