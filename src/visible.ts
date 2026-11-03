// SPDX-License-Identifier: AGPL-3.0-only

type Doc = Pick<Document, "hidden" | "addEventListener" | "removeEventListener">;

/** Calls `fn` every `ms` while the window shows, and once each time it shows
 * again (to catch up). A hidden window keeps no timer, so it wakes nothing.
 * Returns the function that stops it. */
export function everyWhileVisible(fn: () => void, ms: number, doc: Doc = document): () => void {
  let timer: ReturnType<typeof setInterval> | undefined;
  const follow = () => {
    clearInterval(timer);
    timer = doc.hidden ? undefined : setInterval(fn, ms);
  };
  const changed = () => {
    if (!doc.hidden) fn();
    follow();
  };
  follow();
  doc.addEventListener("visibilitychange", changed);
  return () => {
    clearInterval(timer);
    doc.removeEventListener("visibilitychange", changed);
  };
}
