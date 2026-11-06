// SPDX-License-Identifier: AGPL-3.0-only
// The call this window is in, kept above every page so leaving the group or
// opening another page doesn't hang up. The call's view is drawn into one
// box that the voice channel's page borrows while it's open.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { t } from "../../i18n";
import { GroupCall, type CallPerson } from "./GroupCall";
import type { ChannelView } from "./socialApi";

export type CallPlace = { group: string; groupName: string; channel: string; channelName: string; me: string; myName: string; voice?: ChannelView; hd: boolean };

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
  const leave = useCallback(() => {
    setPlace(null);
    setPeople([]);
  }, []);
  const toast = useCallback((m: string) => window.dispatchEvent(new CustomEvent("sulcus:error", { detail: m })), []);
  const value: CallState = {
    place,
    people,
    muted,
    deafened,
    setMuted,
    setDeafened,
    join: (p) => setPlace((old) => (old && old.group === p.group && old.channel === p.channel ? old : p)),
    leave,
    show,
    openChannel: () => place && opener.current?.(place),
    onOpenChannel: (f) => (opener.current = f),
  };
  return (
    <Ctx.Provider value={value}>
      {children}
      <div ref={parking} hidden />
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
            onPeople={setPeople}
            big={big}
            onMuted={setMuted}
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

export function HangUpIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden>
      <path fill="currentColor" d="M12 9c-1.6 0-3.2.3-4.6.8v3.1c0 .4-.2.7-.6.9-1 .5-1.9 1.1-2.6 1.8-.2.2-.4.3-.7.3s-.5-.1-.7-.3L.3 13.1c-.2-.2-.3-.4-.3-.7s.1-.5.3-.7C3.3 8.9 7.4 7 12 7s8.7 1.9 11.7 4.7c.2.2.3.4.3.7s-.1.5-.3.7l-2.5 2.5c-.2.2-.4.3-.7.3s-.5-.1-.7-.3c-.8-.7-1.7-1.3-2.6-1.8-.3-.2-.6-.5-.6-.9V9.8C15.2 9.3 13.6 9 12 9z" />
    </svg>
  );
}
