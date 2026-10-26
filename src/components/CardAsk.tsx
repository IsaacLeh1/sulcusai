// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, on, type CardAsk } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";

/** Work meant for the graphics card that can't go there right now: why, and
 * whether to run it on the processor and system memory instead (slower and
 * hotter), free the card and try again, start SulcusAI again, or stop. The
 * work waits for the answer; closing this counts as Stop. */
export function CardAskPopup() {
  const [asks, setAsks] = useState<CardAsk[]>([]);
  const [always, setAlways] = useState(false);
  useEffect(() => {
    api.cardAsks().then((list) => setAsks(list ?? [])).catch(() => {});
    const subs = [
      on("card:ask", (a) => setAsks((cur) => (cur.some((x) => x.id === a.id) ? cur : [...cur, a]))),
      on("card:ask_done", (p) => setAsks((cur) => cur.filter((x) => x.id !== p.id))),
    ];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []);
  const a = asks[0];
  if (!a) return null;
  const answer = (choice: "processor" | "always" | "free" | "restart" | "stop") => {
    setAsks((cur) => cur.filter((x) => x.id !== a.id));
    setAlways(false);
    api.cardAskAnswer(a.id, choice).catch(() => {});
  };
  return (
    <Modal title={t("The graphics card can't do this right now")} onClose={() => answer("stop")}>
      <p>
        <strong>{t(a.what)}</strong>: {t(a.why)}.
      </p>
      <p className="muted small">
        {t("It can run on the processor and system memory instead. That works, but it's slower (often several times) and the PC gets warmer. Why is kept in the app log.")}
      </p>
      {a.restart && <p className="small">{t("The graphics card's libraries are ready: starting SulcusAI again lets it use the card.")}</p>}
      {a.free && <p className="small">{t("A chat model is holding the card's memory. Unloading it may make room.")}</p>}
      <label className="check small">
        <input type="checkbox" checked={always} onChange={(e) => setAlways(e.target.checked)} />
        {t("Don't ask again: use the processor whenever the card can't")}
      </label>
      <div className="modal-actions wrap">
        <button className="btn ghost" onClick={() => answer("stop")}>
          {t("Stop it")}
        </button>
        <span className="spacer" />
        {a.restart && (
          <button className="btn" onClick={() => answer("restart")}>
            {t("Start SulcusAI again")}
          </button>
        )}
        {a.free && (
          <button className="btn" onClick={() => answer("free")}>
            {t("Free the card and try again")}
          </button>
        )}
        <button className="btn primary" onClick={() => answer(always ? "always" : "processor")} autoFocus>
          {t("Use the processor")}
        </button>
      </div>
    </Modal>
  );
}
