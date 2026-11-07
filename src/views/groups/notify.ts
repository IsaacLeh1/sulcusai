// SPDX-License-Identifier: AGPL-3.0-only
// Notification level per group and per channel, kept on this PC. A channel's
// own choice wins over its group's; nothing is notified unless chosen.
import type { SocialView } from "./socialApi";
import { playGroupMessage } from "./sounds";

export type Level = "all" | "mentions" | "none";
export const LEVELS: [Level | "", string][] = [
  ["", "Same as the group"],
  ["all", "Every message"],
  ["mentions", "Only when I'm mentioned"],
  ["none", "Nothing"],
];

const key = (group: string, channel?: string) => `social.notify.${group}${channel ? "." + channel : ""}`;

export function getLevel(group: string, channel?: string): Level | "" {
  try {
    return (localStorage.getItem(key(group, channel)) as Level | null) ?? "";
  } catch {
    return "";
  }
}

export function setLevel(group: string, channel: string | undefined, level: Level | "") {
  try {
    if (level) localStorage.setItem(key(group, channel), level);
    else localStorage.removeItem(key(group, channel));
  } catch {
    // Without storage the choice lasts until the app closes.
  }
}

export function levelFor(group: string, channel: string): Level {
  return getLevel(group, channel) || getLevel(group) || "none";
}

/** Plays the message sound when counts grew in a channel set to notify, and not for the one being read. */
export function notifyChanges(before: SocialView | null, after: SocialView, open?: { group: string; channel: string }) {
  if (!before) return;
  for (const g of after.groups) {
    const old = before.groups.find((x) => x.id === g.id);
    if (!old) continue;
    for (const c of g.channels) {
      if (open && open.group === g.id && open.channel === c.id) continue;
      const was = old.channels.find((x) => x.id === c.id);
      if (!was) continue;
      const level = levelFor(g.id, c.id);
      if ((level === "all" && c.unread > was.unread) || (level !== "none" && c.mentions > was.mentions)) {
        playGroupMessage();
        return;
      }
    }
  }
}
