// SPDX-License-Identifier: AGPL-3.0-only
import { t } from "../i18n";

/** The shared relay is going away: what to do instead, shown where it's used. */
export function RelayNotice() {
  return (
    <p className="small warn-box relay-notice">
      {t("SulcusAI's shared relay is closing. To keep reaching this PC and your people from away, run your own relay (Settings › Network › Run your own relay), join a friend's, or use the same Wi-Fi.")}
    </p>
  );
}
