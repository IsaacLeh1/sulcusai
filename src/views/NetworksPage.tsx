// SPDX-License-Identifier: AGPL-3.0-only
import type { PushToast } from "../components/Toasts";
import { Networks } from "./groups/Networks";
import "./groups/social.css";

/** Networks as a top tab of its own, the same panel the pop-out window shows. */
export function NetworksPage({ toast }: { toast: PushToast }) {
  return (
    <div className="page nets-page">
      <Networks toast={toast} />
    </div>
  );
}
