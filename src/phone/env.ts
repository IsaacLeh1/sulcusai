// SPDX-License-Identifier: AGPL-3.0-only
// Whether the window runs on a paired phone (phone.html), and what it knows
// about the PC it reaches.

export interface PhoneInfo {
  /** The PC's name. */
  pc: string;
  /** The person's name on the PC. */
  name: string;
  /** This phone's name. */
  phone: string;
  version: string;
  /** Reached through the relay (else the local network). */
  relay: boolean;
}

declare global {
  interface Window {
    __SULCUS_PHONE__?: PhoneInfo;
  }
}

export function onPhone(): boolean {
  return typeof window !== "undefined" && !!window.__SULCUS_PHONE__;
}

export function phoneInfo(): PhoneInfo | null {
  return (typeof window !== "undefined" && window.__SULCUS_PHONE__) || null;
}
