// SPDX-License-Identifier: AGPL-3.0-only
// Safe mode: the window starts without widgets and without layout changes, so
// a bad widget or a broken look can be fixed from Settings. It lasts until it
// is turned off; nothing stored is lost.
const KEY = "sulcusai.safeMode";

export function safeModeOn(): boolean {
  try {
    if (new URLSearchParams(window.location.search).get("safe") === "1") return true;
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function setSafeMode(on: boolean) {
  try {
    if (on) localStorage.setItem(KEY, "1");
    else localStorage.removeItem(KEY);
  } catch {
    // Can't be kept; the reload below still applies the URL form.
  }
  const url = new URL(window.location.href);
  url.searchParams.delete("safe");
  window.location.replace(url.toString());
}
