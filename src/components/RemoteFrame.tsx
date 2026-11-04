// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { t } from "../i18n";
import { remoteStop } from "../api/sharing";

/** The windows round the screen while it is controlled: a click-through red border, and a bar with Stop. */
export default function RemoteFrame({ label }: { label: string }) {
  useEffect(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
  }, []);
  if (label === "remote-border") return <div className="remote-border" />;
  return (
    <div className="remote-bar remote-bar-window" role="status">
      <span>{t("Being controlled. Ctrl+Alt+Backspace stops it.")}</span>
      <button onClick={() => remoteStop().catch(() => {})}>{t("Stop")}</button>
    </div>
  );
}
