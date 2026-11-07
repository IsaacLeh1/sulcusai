// SPDX-License-Identifier: AGPL-3.0-only
// Friends on the Groups home: the list with who's online, requests to answer,
// and requests waiting on the other side.
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import { confirmed } from "../../confirm";
import type { PushToast } from "../../components/Toasts";
import { Avatar } from "./Profiles";
import { social, type Friends, type GroupView, type Profile } from "./socialApi";

const EMPTY: Friends = { friends: {}, incoming: {}, outgoing: {}, notes: {} };

export function useFriends() {
  const [f, setF] = useState<Friends>(EMPTY);
  const load = useCallback(() => {
    social.friends().then(setF).catch(() => setF(EMPTY));
  }, []);
  useEffect(() => {
    load();
    const un = listen("social:friends", load);
    return () => {
      un.then((x) => x());
    };
  }, [load]);
  return f;
}

export function FriendsPanel({ groups, profiles, myName, toast, onOpen }: { groups: GroupView[]; profiles: Record<string, Profile>; myName: string; toast: PushToast; onOpen: (id: string) => void }) {
  const f = useFriends();
  const [tab, setTab] = useState<"online" | "all" | "pending">("online");
  const run = (p: Promise<unknown>) => p.catch((e) => toast(errorText(e)));
  const online = (id: string) => groups.some((g) => g.members.some((m) => m.id === id && m.online)) && profiles[id]?.presence !== "invisible";
  const ids = Object.keys(f.friends).filter((id) => tab !== "online" || online(id));
  const pending = Object.keys(f.incoming).length + Object.keys(f.outgoing).length;
  return (
    <section className="social-friends" aria-label={t("Friends")}>
      <div className="row">
        <h4>{t("Friends")}</h4>
        {(["online", "all", "pending"] as const).map((k) => (
          <button key={k} className={tab === k ? "chip active" : "chip"} aria-pressed={tab === k} onClick={() => setTab(k)}>
            {k === "online" ? t("Online") : k === "all" ? t("All") : t("Pending ({n})", { n: pending })}
          </button>
        ))}
      </div>
      {tab !== "pending" &&
        (ids.length === 0 ? (
          <p className="small muted">{tab === "online" ? t("No friends online right now.") : t("No friends yet. Add someone from their profile card.")}</p>
        ) : (
          ids.map((id) => (
            <div key={id} className="social-friend row">
              <button className="link row" onClick={() => onOpen(id)}>
                <Avatar name={f.friends[id]} p={profiles[id]} size={32} id={id} />
                <span>
                  <strong>{profiles[id]?.name || f.friends[id]}</strong>
                  <span className="small muted" style={{ display: "block" }}>
                    {online(id) ? profiles[id]?.status || t("Online") : t("Offline")}
                  </span>
                </span>
              </button>
              <button className="link small danger" onClick={async () => (await confirmed(t("Remove {name} as a friend?", { name: f.friends[id] }))) && run(social.friendRemove(id))}>
                {t("Remove")}
              </button>
            </div>
          ))
        ))}
      {tab === "pending" && (
        <>
          {Object.entries(f.incoming).map(([id, name]) => (
            <div key={id} className="social-friend row">
              <span>
                <strong>{name}</strong> <span className="small muted">{t("wants to be friends")}</span>
              </span>
              <button className="btn-discord small" onClick={() => run(social.friendAnswer(id, true, myName))}>
                {t("Accept")}
              </button>
              <button className="btn-discord secondary small" onClick={() => run(social.friendAnswer(id, false, myName))}>
                {t("Decline")}
              </button>
            </div>
          ))}
          {Object.entries(f.outgoing).map(([id, name]) => (
            <div key={id} className="social-friend row">
              <span>
                <strong>{name}</strong> <span className="small muted">{t("request sent")}</span>
              </span>
              <button className="link small" onClick={() => run(social.friendRemove(id))}>
                {t("Withdraw")}
              </button>
            </div>
          ))}
          {pending === 0 && <p className="small muted">{t("No requests waiting.")}</p>}
        </>
      )}
    </section>
  );
}
