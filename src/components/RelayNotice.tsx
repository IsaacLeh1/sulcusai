// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";

/** The shared relay is going away: what to do instead, shown where it's used. */
export function RelayNotice() {
  const [closes, setCloses] = useState<number | null>(null);
  useEffect(() => {
    invoke<number | null>("shared_relay_closes").then(setCloses).catch(() => {});
  }, []);
  const when = closes ? new Date(closes * 1000).toLocaleDateString() : null;
  return (
    <p className="small warn-box relay-notice">
      {when
        ? t("SulcusAI's shared relay closes on {date}. To keep reaching this PC and your people from away, run your own relay (Settings › Network › Run your own relay), join a friend's, or use the same Wi-Fi.", { date: when })
        : t("SulcusAI's shared relay is closing. To keep reaching this PC and your people from away, run your own relay (Settings › Network › Run your own relay), join a friend's, or use the same Wi-Fi.")}
    </p>
  );
}
