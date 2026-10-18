// SPDX-License-Identifier: AGPL-3.0-only
// The approvals waiting for the person, for the popups that ask wherever
// they are in the app: the live requests, plus the ones made before a popup
// mounted (while the app was locked, or as it started), which would
// otherwise wait unseen and leave their chat hanging.
import { useEffect, useRef, useState } from "react";
import { api, on, type Chat, type PendingApproval } from "./api";

// One look per chat list, shared by the popups that mount together (they're
// handed the same list).
const loads = new WeakMap<Chat[], Promise<PendingApproval[]>>();

/** What's already waiting, in every chat (one call to the core). */
function alreadyWaiting(chats: Chat[]): Promise<PendingApproval[]> {
  let p = loads.get(chats);
  if (!p) {
    p = api.allPendingApprovals().catch(() => [] as PendingApproval[]);
    loads.set(chats, p);
  }
  return p;
}

/** The requests this popup handles (`wanted`), oldest first, kept current
 * as they're asked and answered. `onAdded` runs once for each new one. */
export function useWaitingApprovals(chats: Chat[], wanted: (p: PendingApproval) => boolean, onAdded?: (p: PendingApproval) => void) {
  const [asks, setAsks] = useState<PendingApproval[]>([]);
  const wantedRef = useRef(wanted);
  wantedRef.current = wanted;
  const addedRef = useRef(onAdded);
  addedRef.current = onAdded;
  // Answered since this mounted (here, in the chat, or from the phone).
  const answered = useRef(new Set<string>());
  const add = (list: PendingApproval[]) => {
    const fresh = list.filter((p) => wantedRef.current(p) && !answered.current.has(p.call_id));
    if (fresh.length === 0) return;
    setAsks((all) => [...all.filter((a) => !fresh.some((p) => p.call_id === a.call_id)), ...fresh]);
    fresh.forEach((p) => addedRef.current?.(p));
  };
  const addRef = useRef(add);
  addRef.current = add;

  useEffect(() => {
    const subs = [
      on("agent:approval", (p) => addRef.current([p])),
      on("agent:approval_done", (p) => {
        answered.current.add(p.call_id);
        setAsks((all) => all.filter((a) => a.call_id !== p.call_id));
      }),
    ];
    return () => {
      subs.forEach((s) => s.then((un) => un()));
    };
  }, []);

  // Once the chats are known: the requests made before this mounted.
  const looked = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    if (looked.current || chats.length === 0) return;
    looked.current = true;
    alreadyWaiting(chats).then((list) => mounted.current && addRef.current(list));
  }, [chats]);

  return [asks, setAsks] as const;
}
