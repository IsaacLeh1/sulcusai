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
