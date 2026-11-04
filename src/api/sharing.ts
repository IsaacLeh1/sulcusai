// SPDX-License-Identifier: AGPL-3.0-only
// Sharing and private networks (src-tauri/src/p2p/sharing.rs).
import { invoke } from "@tauri-apps/api/core";

export interface SharingSettings {
  fileSharing: boolean;
  modelCards: boolean;
  virtualLan: boolean;
  remoteControl: boolean;
}

export interface Hosted {
  id: string;
  name: string;
  size: number;
  link: string;
}

export interface Made {
  link: string;
  fileText: string;
  pieces: number;
}

export interface Network {
  id: string;
  name: string;
  admins: string[];
  members: Record<string, string>;
  waiting: string[];
}

export interface RemoteStatus {
  enabled: boolean;
  state: { state: "idle" | "asking" | "live"; controller?: string; code?: string };
  show: { prompt: boolean; border: boolean; bar: boolean };
}

export const sharingSettings = () => invoke<SharingSettings>("sharing_settings");
export const setSharing = (s: SharingSettings) => invoke<SharingSettings>("sharing_set", { s });
export const shareMake = (path: string) => invoke<Made>("share_make", { path });
export const shareList = () => invoke<Hosted[]>("share_list");
export const shareStop = (id: string) => invoke<void>("share_stop", { id });
export const shareRead = (text: string) => invoke<{ name: string; size: number; host: string }>("share_read", { text });
export const vlanCreate = (name: string) => invoke<Network>("vlan_create", { name });
export const vlanList = () => invoke<Network[]>("vlan_list");
export const remoteStatus = () => invoke<RemoteStatus>("remote_status");
export const remoteAccept = (code: string) => invoke<RemoteStatus>("remote_accept", { code });
export const remoteStop = () => invoke<RemoteStatus>("remote_stop");

/** Whether pasted text looks like a share link or a .sulcus file's text. */
export function looksShared(text: string): boolean {
  const s = text.trim();
  return s.startsWith("sulcus://share/") || (s.startsWith("{") && s.includes('"manifest"'));
}

export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export type RemoteInput =
  | { do: "move"; x: number; y: number }
  | { do: "click"; button: number; down: boolean }
  | { do: "key"; code: string; down: boolean }
  | { do: "scroll"; dy: number };

export interface Vpn {
  adapter: string;
  kind: string;
  up: boolean;
}

export const shareFetch = (text: string, saveDir: string) => invoke<string>("share_fetch", { text, saveDir });
export const modelCardFetch = (peer: string) => invoke<unknown>("model_card_fetch", { peer });
export const vlanConnect = (id: string) => invoke<string>("vlan_connect", { id });
export const vlanDisconnect = () => invoke<void>("vlan_disconnect");
export const vlanStatus = () => invoke<string | null>("vlan_status");
export const vpnList = () => invoke<Vpn[]>("vpn_list");
export const remoteConnect = (peer: string) => invoke<string>("remote_connect", { peer });
export const remoteSend = (input: RemoteInput) => invoke<void>("remote_send", { input });
export const shareOpened = () => invoke<string | null>("share_opened");
export const remoteDisconnect = () => invoke<void>("remote_disconnect");

/** Seconds left to accept a remote request, from when it was asked. */
export function secondsLeft(askedAt: number, now: number): number {
  return Math.max(0, Math.ceil((askedAt + 10_000 - now) / 1000));
}

/** A pointer position as fractions of the shown picture. */
export function fraction(clientX: number, clientY: number, box: { left: number; top: number; width: number; height: number }): { x: number; y: number } {
  const clamp = (v: number) => Math.min(1, Math.max(0, v));
  return { x: clamp((clientX - box.left) / Math.max(1, box.width)), y: clamp((clientY - box.top) / Math.max(1, box.height)) };
}
