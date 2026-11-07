// SPDX-License-Identifier: AGPL-3.0-only
// The PC side of the phone remote (phone/controls.rs): the call view mounts
// this with its own mute, deafen, switch and leave actions, so a tap on the
// phone does exactly what the window's buttons do, and it reports the state
// back for the phone to show.
import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type RemoteActions = {
  muted: boolean;
  deafened: boolean;
  setMuted: (on: boolean) => void;
  setDeafened: (on: boolean) => void;
  switchTo: (group: string, channel: string) => void;
  leave: () => void;
};

type Control = { action: "mute" | "deafen"; on: boolean } | { action: "switch"; group: string; channel: string } | { action: "leave" };

export function useRemoteControl(a: RemoteActions) {
  const cur = useRef(a);
  cur.current = a;
  useEffect(() => {
    invoke("remote_controls_report", { muted: a.muted, deafened: a.deafened }).catch(() => {});
  }, [a.muted, a.deafened]);
  useEffect(() => {
    const off = listen<Control>("remote:control", (e) => {
      const c = e.payload;
      if (c.action === "mute") cur.current.setMuted(c.on);
      else if (c.action === "deafen") cur.current.setDeafened(c.on);
      else if (c.action === "switch") cur.current.switchTo(c.group, c.channel);
      else if (c.action === "leave") cur.current.leave();
    });
    return () => {
      off.then((f) => f());
    };
  }, []);
}
