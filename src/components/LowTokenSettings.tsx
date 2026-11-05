// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { errorText } from "../api";
import { t } from "../i18n";
import { lowTokensGet, lowTokensSet } from "../lowTokens";
import type { PushToast } from "./Toasts";

/** Settings → Low token mode: the default every chat follows unless it picks its own. */
export function LowTokenSettings({ toast }: { toast: PushToast }) {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    lowTokensGet(null).then((v) => setOn(v.global)).catch(() => setOn(false));
  }, []);
  if (on === null) return null;
  const flip = async (next: boolean) => {
    try {
      await lowTokensSet(null, next);
      setOn(next);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <section className="card">
      <h2>{t("Low token mode")}</h2>
      <label className="row">
        <input type="checkbox" checked={on} onChange={(e) => flip(e.target.checked)} />
        <span>{t("Use as few tokens as works well")}</span>
      </label>
      <p className="muted small">
        {t("Shorter answers, less thinking, fewer tools offered, long tool results cut, fewer earlier messages sent, and compacting sooner. Cloud models cost less and models on this PC answer sooner. A chat can turn it on or off for itself.")}
      </p>
    </section>
  );
}
