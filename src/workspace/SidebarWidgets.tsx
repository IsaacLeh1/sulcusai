// SPDX-License-Identifier: AGPL-3.0-only
// Pinned widgets under the chats, shown only while the Widgets switch is on.
import type { View } from "../App";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";
import { WidgetFrame } from "./WidgetFrame";
import { useWidgets } from "./widgets";

export function SidebarWidgets({ setView, toast }: { setView: (v: View) => void; toast: PushToast }) {
  const s = useWidgets();
  if (!s.on) return null;
  const pinned = s.widgets.filter((w) => w.pinned);
  if (!pinned.length) return null;
  return (
    <section className="side-widgets" aria-label={t("Widgets")}>
      {pinned.map((w) => (
        <WidgetFrame key={w.id} widget={w} height={120} notify={(text) => toast(text, "info")} openPage={setView} />
      ))}
    </section>
  );
}
