// SPDX-License-Identifier: AGPL-3.0-only
import { APP_NAME } from "../../brand";
import { t } from "../../i18n";

/** Privacy in plain words: what stays, what can leave, and who decides. */
export function PrivacyPlain({ goTo }: { goTo: (section: string) => void }) {
  const rows: { head: string; body: string; section?: string }[] = [
    { head: t("Your chats stay on this PC"), body: t("Chats, files, voice and pictures are made and kept here, encrypted. Nobody, including the makers of {app}, can read them.", { app: APP_NAME }) },
    { head: t("Offline unless you say otherwise"), body: t("The app starts offline. Going online for web search or cloud models is your choice, and you can switch back any time."), section: "connectivity" },
    { head: t("Cloud models see what you send them"), body: t("If you add an API key and pick a cloud model, that chat goes to that company under their rules. Local models never send anything."), section: "cloud" },
    { head: t("Personal details can be hidden first"), body: t("Privacy mode removes names, addresses and numbers before anything goes out."), section: "privacy-mode" },
    { head: t("You can see everything that left"), body: t("What left this PC lists every message sent out, with the time and where."), section: "sent" },
    { head: t("The relays can't read your messages"), body: t("The phone and sync relays only pass on sealed messages. They don't keep them."), section: "phone" },
    { head: t("No tracking, no ads"), body: t("No usage data is collected. Crash reports are sent only if you turn them on and agree each time."), section: "about" },
    { head: t("Delete it all whenever you like"), body: t("Your data lets you export or erase everything the app keeps."), section: "your-data" },
  ];
  return (
    <section className="card privacy-plain">
      <h2>{t("Privacy in plain words")}</h2>
      <ul className="plain-list">
        {rows.map((r) => (
          <li key={r.head}>
            <strong>{r.head}</strong>
            <span className="muted small block">{r.body}</span>
            {r.section && (
              <button className="btn link small" onClick={() => goTo(r.section!)}>
                {t("Go to the setting")}
              </button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
