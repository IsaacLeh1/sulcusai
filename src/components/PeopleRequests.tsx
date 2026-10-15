// SPDX-License-Identifier: AGPL-3.0-only
// Someone's assistant asks this user's assistant something, or wants to work
// something out with it: a popup wherever the person is in the app.
import { useEffect, useState } from "react";
import { api, errorText, on, type PersonRequest } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

export function PeopleRequests({ enabled, onOpenPeople, toast }: { enabled: boolean; onOpenPeople: () => void; toast: PushToast }) {
  const [list, setList] = useState<PersonRequest[]>([]);
  const [writing, setWriting] = useState<string | null>(null);
  const [always, setAlways] = useState(false);
  // Put off for now: it stays in People to answer there.
  const [later, setLater] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (!enabled) return setList([]);
    const load = () => api.peopleRequests().then((x) => setList(x ?? [])).catch(() => {});
    load();
    const subs = [on("people:ask", load)];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [enabled]);
  const r = list.find((x) => !later.has(x.id));
  if (!r) return null;
  const putOff = () => setLater((s) => new Set(s).add(r.id));
  const others = list.filter((x) => !later.has(x.id)).length - 1;

  const reply = async (choice: "assistant" | "mine" | "decline") => {
    try {
      await api.peopleReply(r.id, choice, choice === "mine" ? writing : null, always && choice !== "decline");
      setList((all) => all.filter((x) => x.id !== r.id));
      setWriting(null);
      setAlways(false);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const task = r.kind === "task";
  return (
    <Modal title={task ? t("{name}'s assistant wants to work something out", { name: r.name }) : t("{name}'s assistant has a question", { name: r.name })} onClose={putOff}>
      <p className="muted small">{task ? t("It wants to work this out with your assistant:") : t("It asks your assistant:")}</p>
      <p className="web-ask-reason">{r.question}</p>
      <p className="muted small">
        {t("If you allow it, your assistant answers on this PC with your model. It can only look, never change anything, and it may use: {list}.", { list: r.uses.join(", ") })}{" "}
        {task && t("The two assistants take turns until they agree (up to 8 turns); you can watch and stop it in People.")}
      </p>
      {writing !== null && !task && (
        <textarea
          className="input"
          rows={3}
          autoFocus
          value={writing}
          placeholder={t("Your answer")}
          aria-label={t("Your answer")}
          onChange={(e) => setWriting(e.target.value)}
        />
      )}
      <label className="check small">
        <input type="checkbox" checked={always} onChange={(e) => setAlways(e.target.checked)} />
        {t("From now on, let {name}'s assistant ask without asking me", { name: r.name })}
      </label>
      <div className="modal-actions">
        <button
          className="btn ghost"
          onClick={() => {
            putOff();
            onOpenPeople();
          }}
        >
          {t("Later")}
        </button>
        <span className="spacer" />
        <button className="btn" onClick={() => reply("decline")}>
          {t("Decline")}
        </button>
        {!task &&
          (writing === null ? (
            <button className="btn" onClick={() => setWriting("")}>
              {t("Answer myself…")}
            </button>
          ) : (
            <button className="btn" disabled={!writing.trim()} onClick={() => reply("mine")}>
              {t("Send my answer")}
            </button>
          ))}
        <button className="btn primary" onClick={() => reply("assistant")} autoFocus={writing === null}>
          {task ? t("Let my assistant work on it") : t("Let my assistant answer")}
        </button>
      </div>
      {others > 0 && <p className="muted small">{t("{n} more waiting.", { n: others })}</p>}
    </Modal>
  );
}
