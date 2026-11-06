// SPDX-License-Identifier: AGPL-3.0-only
// Group letters between people (MLS): the list in the People sidebar and one group's letters.
import { useCallback, useEffect, useState } from "react";
import { api, errorText, on, type PeopleGroups, type PersonContact } from "../api";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";
import { confirmed } from "../confirm";

export function useGroupLetters(toast: PushToast) {
  const [g, setG] = useState<PeopleGroups | null>(null);
  const load = useCallback(() => {
    api.peopleGroups().then(setG).catch(() => setG(null));
  }, []);
  useEffect(() => {
    load();
    const subs = [on("people:groups", load)];
    return () => subs.forEach((u) => u.then((f) => f()));
  }, [load]);
  const act = (p: Promise<unknown>) => p.then(load).catch((e) => toast(errorText(e), "error"));
  return { g, load, act };
}

export function GroupLetterList({ g, openId, onOpen, act }: { g: PeopleGroups; openId: string | null; onOpen: (id: string) => void; act: (p: Promise<unknown>) => void }) {
  const [naming, setNaming] = useState<string | null>(null);
  const create = () => {
    const name = (naming ?? "").trim();
    setNaming(null);
    if (name) act(api.peopleGroupCreate(name).then(onOpen));
  };
  return (
    <div className="people-groups">
      <div className="row people-head">
        <strong className="small">{t("Group letters")}</strong>
        <button className="link small" onClick={() => setNaming("")}>
          {t("+ New group")}
        </button>
      </div>
      {naming !== null && (
        <input
          className="input"
          autoFocus
          maxLength={80}
          value={naming}
          placeholder={t("Group name")}
          aria-label={t("Group name")}
          onChange={(e) => setNaming(e.target.value)}
          onBlur={create}
          onKeyDown={(e) => {
            if (e.key === "Enter") create();
            if (e.key === "Escape") setNaming(null);
          }}
        />
      )}
      {g.invites.map((i) => (
        <div key={i.group} className="small">
          {t("Invited to {name}", { name: i.name })}{" "}
          <button className="link small" onClick={() => act(api.peopleGroupAnswer(i.group, true))}>
            {t("Join")}
          </button>{" "}
          <button className="link small" onClick={() => act(api.peopleGroupAnswer(i.group, false))}>
            {t("No thanks")}
          </button>
        </div>
      ))}
      {g.groups.map((r) => (
        <button key={r.id} className={`person-item ${openId === r.id ? "active" : ""}`} onClick={() => onOpen(r.id)}>
          <span className="person-text">
            <strong className="ellipsis">{r.name}</strong>
            <span className="small muted ellipsis">{r.removed ? t("You were removed") : t("{n} members", { n: r.members.length })}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

export function GroupLetters({ g, id, contacts, act }: { g: PeopleGroups; id: string; contacts: PersonContact[]; act: (p: Promise<unknown>) => void }) {
  const [text, setText] = useState("");
  const [adding, setAdding] = useState("");
  const r = g.groups.find((x) => x.id === id);
  if (!r) return null;
  const name = (m: string) => (m === g.me ? t("You") : contacts.find((c) => c.id === m)?.name ?? m.slice(0, 8));
  const addable = contacts.filter((c) => !r.members.includes(c.id) && !r.invited.includes(c.id));
  const send = () => {
    const s = text.trim();
    if (!s) return;
    setText("");
    act(api.peopleGroupSend(id, s));
  };
  return (
    <div className="people-group">
      <header className="people-chat-head">
        <strong>{r.name}</strong>
        <span className="small muted">{r.members.map(name).join(", ")}</span>
        <button
          className="link small"
          onClick={async () => {
            if (await confirmed(t("Leave {name}? Its letters and keys are removed from this PC.", { name: r.name }))) act(api.peopleGroupLeave(id));
          }}
        >
          {t("Leave")}
        </button>
      </header>
      {r.mine && !r.removed && (
        <div className="row small">
          <select className="input" value={adding} aria-label={t("Add someone")} onChange={(e) => setAdding(e.target.value)}>
            <option value="">{t("Add someone...")}</option>
            {addable.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <button className="btn small" disabled={!adding} onClick={() => (act(api.peopleGroupInvite(id, adding)), setAdding(""))}>
            {t("Invite")}
          </button>
          {r.invited.length > 0 && <span className="muted">{t("Waiting for {names}", { names: r.invited.map(name).join(", ") })}</span>}
          {r.members
            .filter((m) => m !== g.me)
            .map((m) => (
              <button key={m} className="link small" onClick={() => act(api.peopleGroupRemove(id, m))}>
                {t("Remove {name}", { name: name(m) })}
              </button>
            ))}
        </div>
      )}
      <div className="people-messages">
        {r.letters.map((l, i) => (
          <div key={i} className={`people-msg ${l.from === g.me ? "mine" : ""}`}>
            <div className="small muted">{name(l.from)}</div>
            <div>{l.text}</div>
          </div>
        ))}
      </div>
      {!r.removed && (
        <div className="row">
          <input
            className="input"
            value={text}
            placeholder={t("Write to the group")}
            aria-label={t("Write to the group")}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && send()}
          />
          <button className="btn primary" onClick={send}>
            {t("Send")}
          </button>
        </div>
      )}
    </div>
  );
}
