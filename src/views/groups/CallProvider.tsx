// SPDX-License-Identifier: AGPL-3.0-only
// The call this window is in, kept above every page so leaving the group or
// opening another page doesn't hang up. The call's view is drawn into one
// box that the voice channel's page borrows while it's open.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { t } from "../../i18n";
import { GroupCall, type CallPerson } from "./GroupCall";
import { HangUpIcon } from "./SocialIcons";
import { NewDevicePrompt } from "./Devices";
import type { ChannelView } from "./socialApi";
import { playDeafen, playJoinCall, playLeaveCall, playMute, playPeerJoin } from "./sounds";

export type CallPlace = { group: string; groupName: string; channel: string; channelName: string; me: string; myName: string; voice?: ChannelView; hd: boolean; moderator?: boolean };

type CallState = {
  place: CallPlace | null;
  people: CallPerson[];
  muted: boolean;
  deafened: boolean;
  setMuted: (m: boolean) => void;
  setDeafened: (d: boolean) => void;
  join: (p: CallPlace) => void;
  leave: () => void;
  /** The voice channel's page shows the call's tiles in `el`; null gives it back. */
  show: (el: HTMLElement | null) => void;
  /** Asked from the voice bar: open the channel. */
  openChannel: () => void;
  onOpenChannel: (f: ((p: CallPlace) => void) | null) => void;
};

const Ctx = createContext<CallState | null>(null);

export function useCall(): CallState {
  const c = useContext(Ctx);
  if (!c) throw new Error("useCall outside CallProvider");
  return c;
}

export function CallProvider({ children }: { children: ReactNode }) {
  const [place, setPlace] = useState<CallPlace | null>(null);
  const [people, setPeople] = useState<CallPerson[]>([]);
  const [muted, setMuted] = useState(false);
  const [deafened, setDeafened] = useState(false);
  const [big, setBig] = useState(false);
  // One box for the call's view; moving the box between pages never restarts the call.
  const box = useMemo(() => document.createElement("div"), []);
  const parking = useRef<HTMLDivElement>(null);
  const opener = useRef<((p: CallPlace) => void) | null>(null);
  const show = useCallback(
    (el: HTMLElement | null) => {
      (el ?? parking.current)?.appendChild(box);
      setBig(!!el);
    },
    [box],
  );
  useEffect(() => {
    if (!box.parentElement) parking.current?.appendChild(box);
  }, [box]);
  const prevPeopleIds = useRef<Set<string>>(new Set());
  const leave = useCallback(() => {
    if (place) playLeaveCall();
    setPlace(null);
    setPeople([]);
    prevPeopleIds.current.clear();
  }, [place]);
  const handleSetMuted = useCallback((m: boolean) => {
    setMuted(m);
    playMute(m);
  }, []);
  const handleSetDeafened = useCallback((d: boolean) => {
    setDeafened(d);
    playDeafen(d);
  }, []);
  const join = useCallback((p: CallPlace) => {
    setPlace((old) => {
      if (old && old.group === p.group && old.channel === p.channel) return old;
      playJoinCall();
      return p;
    });
  }, []);
  const handlePeople = useCallback((newPeople: CallPerson[]) => {
    setPeople(newPeople);
    if (place && prevPeopleIds.current.size > 0) {
      for (const p of newPeople) {
        if (p.id !== place.me && !prevPeopleIds.current.has(p.id)) {
          playPeerJoin();
          break;
        }
      }
    }
    prevPeopleIds.current = new Set(newPeople.map((p) => p.id));
  }, [place]);
  const toast = useCallback((m: string) => window.dispatchEvent(new CustomEvent("sulcus:error", { detail: m })), []);
  const value: CallState = {
    place,
    people,
    muted,
    deafened,
    setMuted: handleSetMuted,
    setDeafened: handleSetDeafened,
    join,
    leave,
    show,
    openChannel: () => place && opener.current?.(place),
    onOpenChannel: (f) => (opener.current = f),
  };
  return (
    <Ctx.Provider value={value}>
      {children}
      <div ref={parking} hidden />
      <NewDevicePrompt />
      {place &&
        createPortal(
          <GroupCall
            key={`${place.group}:${place.channel}`}
            group={place.group}
            channel={place.channel}
            myName={place.myName}
            me={place.me}
            voice={place.voice}
            hd={place.hd}
            onClose={leave}
            toast={toast}
            muted={muted}
            deafened={deafened}
            onPeople={handlePeople}
            big={big}
            onMuted={handleSetMuted}
            moderator={place.moderator}
          />,
          box,
        )}
    </Ctx.Provider>
  );
}

/** Above the user bar while in a call: where, and hang up. Click it to go back to the channel. */
export function VoiceBar() {
  const c = useContext(Ctx);
  if (!c?.place) return null;
  return (
    <div className="voice-bar">
      <button className="voice-bar-where" onClick={c.openChannel} title={t("Go to the call")}>
        <span className="call-state on">{t("Voice Connected")}</span>
        <span className="small muted ellipsis">
          {c.place.channelName} / {c.place.groupName}
        </span>
      </button>
      <button className="icon-ghost danger" onClick={c.leave} title={t("Disconnect")} aria-label={t("Disconnect")}>
        <HangUpIcon />
      </button>
    </div>
  );
}
