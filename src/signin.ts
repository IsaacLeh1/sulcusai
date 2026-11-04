// SPDX-License-Identifier: AGPL-3.0-only
import { t } from "./i18n";

const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;

/** The system sign-in that can unlock SulcusAI here: Windows Hello, Touch ID
 * on a Mac, the desktop's own password or fingerprint prompt on Linux. */
export function systemSignIn(): string {
  if (/Mac OS X|Macintosh/.test(ua)) return t("Touch ID");
  if (/Linux/.test(ua) && !/Android/.test(ua)) return t("your system sign-in");
  return "Windows Hello";
}

/** Where to set it up when it isn't. */
export function systemSignInSetup(): string {
  if (/Mac OS X|Macintosh/.test(ua)) return t("Touch ID isn't set up on this Mac. You can add a fingerprint in System Settings → Touch ID & Password.");
  if (/Linux/.test(ua) && !/Android/.test(ua)) return t("Your desktop has no sign-in prompt for apps (polkit). Install your desktop's polkit agent to use it.");
  return t("Windows Hello isn't set up on this PC. You can set it up in Windows Settings → Accounts → Sign-in options.");
}
