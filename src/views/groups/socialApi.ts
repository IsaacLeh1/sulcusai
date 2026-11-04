// SPDX-License-Identifier: AGPL-3.0-only
// Commands for groups (src-tauri/src/social/store.rs). Kept beside the one
// view that uses them, so no shared bundle pulls them in.
import { invoke } from "@tauri-apps/api/core";

export type Role = "member" | "moderator" | "owner";
export type ChannelKind = "text" | "voice" | "announcements";
export type Switches = { groups: boolean; calls: boolean; model_moderation: boolean; assistants_in_groups: boolean };
export type Rules = { blocked_words: string[]; block_links: boolean; burst: number; max_caps_ratio: number };
export type TeenSettings = { on: boolean; dms_from_contacts_only: boolean; blur_unknown_pictures: boolean; strict_filter: boolean; no_adult_channels: boolean };
export type MemberView = { id: string; name: string; role: Role; muted: boolean; assistant: boolean; blocked: boolean };
export type ChannelView = { id: string; name: string; kind: ChannelKind; slow_secs: number; adults_only: boolean; unread: number };
export type GroupView = {
  id: string;
  name: string;
  mine: boolean;
  role: Role | null;
  members: MemberView[];
  channels: ChannelView[];
  held: number;
  reports: number;
  rules: Rules;
  teens_allowed: boolean;
  assistant: string | null;
  docs: string[];
};
export type SocialView = { switches: Switches; me: string; groups: GroupView[]; teen: TeenSettings };
export type GroupMessage = {
  id: string;
  channel: string;
  author: string;
  author_name: string;
  text: string;
  at: number;
  reply_to: string | null;
  thread: string | null;
  edited: boolean;
  deleted: boolean;
  reactions: Record<string, string[]>;
  mentions: string[];
  pinned: boolean;
  replies: number;
  hidden: boolean;
};
export type Event =
  | { ev: "post"; id: string; channel: string; text: string; reply_to: string | null; thread: string | null; mentions: string[] }
  | { ev: "edit"; id: string; text: string }
  | { ev: "delete"; id: string }
  | { ev: "react"; id: string; reaction: string; on: boolean }
  | { ev: "pin"; id: string; on: boolean };
export type Op =
  | { op: "rename"; name: string }
  | { op: "add_channel"; id: string; name: string; kind: ChannelKind; adults_only: boolean }
  | { op: "remove_channel"; id: string }
  | { op: "slow_mode"; channel: string; secs: number }
  | { op: "set_role"; member: string; role: Role }
  | { op: "kick"; member: string }
  | { op: "ban"; member: string }
  | { op: "unban"; member: string }
  | { op: "mute"; member: string; until: number }
  | { op: "teens_allowed"; allowed: boolean };
export type Report = { id: string; group: string; by: string; about: string; message: string | null; quote: string; reason: string; at: number; resolved: boolean };
export type ModQueue = { reports: Report[]; held: { index: number; from: string; text: string; why: string }[] };

export const social = {
  view: () => invoke<SocialView>("social_view"),
  setSwitches: (switches: Switches) => invoke<void>("social_set_switches", { switches }),
  setTeen: (teen: TeenSettings, pin?: string) => invoke<void>("social_set_teen", { teen, pin: pin ?? null }),
  create: (name: string, myName: string) => invoke<string>("social_create", { name, myName }),
  invite: (group: string, uses: number, hours: number) => invoke<string>("social_invite", { group, uses, hours }),
  join: (hub: string, invite: string, myName: string) => invoke<void>("social_join", { hub, invite, myName }),
  act: (group: string, op: Op) => invoke<void>("social_act", { group, op }),
  leave: (group: string) => invoke<void>("social_leave", { group }),
  send: (group: string, channel: string, event: Event) => invoke<void>("social_send", { group, channel, event }),
  messages: (group: string, channel: string, thread: string | null) => invoke<GroupMessage[]>("social_messages", { group, channel, thread }),
  report: (group: string, about: string, message: string | null, quote: string, reason: string) => invoke<void>("social_report", { group, about, message, quote, reason }),
  modQueue: (group: string) => invoke<ModQueue>("social_mod_queue", { group }),
  release: (group: string, index: number, allow: boolean) => invoke<void>("social_release", { group, index, allow }),
  resolveReport: (id: string) => invoke<void>("social_resolve_report", { id }),
  setRules: (group: string, rules: Rules) => invoke<void>("social_set_rules", { group, rules }),
  block: (member: string, block: boolean, muteOnly: boolean) => invoke<void>("social_block", { member, block, muteOnly }),
  catchUp: (group: string, channel: string) => invoke<string>("social_catch_up", { group, channel }),
  translate: (text: string, language: string) => invoke<string>("social_translate", { text, language }),
  addAssistant: (group: string, name: string, channel: string) => invoke<void>("social_add_assistant", { group, name, channel }),
  doc: (group: string, doc: string) => invoke<string>("social_doc", { group, doc }),
  docEdit: (group: string, doc: string, text: string) => invoke<void>("social_doc_edit", { group, doc, text }),
  dmSend: (group: string, to: string, text: string) => invoke<void>("social_dm_send", { group, to, text }),
  dms: () => invoke<Dm[]>("social_dms"),
  dmAnswer: (from: string, accept: boolean) => invoke<void>("social_dm_answer", { from, accept }),
  callJoin: (group: string, channel: string, name: string) => invoke<string>("social_call_join", { group, channel, name }),
  callLeave: () => invoke<void>("social_call_leave"),
  callPiece: (kind: StreamKind, first: boolean, data: string) => invoke<void>("social_call_piece", { kind, first, data }),
  callView: () => invoke<CallView | null>("social_call_view"),
};

export type Dm = { group: string; with: string; with_name: string; from_me: boolean; text: string; at: number; request: boolean };
export type StreamKind = "voice" | "camera" | "screen";
export type CallView = { call: string; group: string; channel: string; people: [string, string][] };
export type CallPiece = { call: string; from: string; kind: StreamKind; first: boolean; data: string };

/** A fresh id for a message or channel. */
export function newId(): string {
  return crypto.randomUUID().replace(/-/g, "");
}

/** The members named with @ in a message. */
export function mentionsIn(text: string, members: { id: string; name: string }[]): string[] {
  const lower = text.toLowerCase();
  return members.filter((m) => lower.includes("@" + m.name.toLowerCase())).map((m) => m.id);
}
