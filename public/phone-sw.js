// SPDX-License-Identifier: AGPL-3.0-only
// The phone page's service worker. It exists only to receive Web Push from
// the PC (webpush.rs) while the page is closed; it caches nothing, so the
// page always comes from the PC.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let note = {};
  try {
    note = event.data ? event.data.json() : {};
  } catch {
    note = { body: event.data ? event.data.text() : "" };
  }
  // Browsers (iOS in particular) cancel a subscription whose pushes show nothing.
  event.waitUntil(
    self.registration.showNotification(note.title || "SulcusAI", {
      body: note.body || "",
      tag: note.tag || undefined,
      data: { url: note.url || "" },
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const wanted = new URL(event.notification.data?.url || "./", self.registration.scope);
  // A link to another site is never followed from a notification.
  const target = wanted.origin === self.location.origin ? wanted.href : self.registration.scope;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(async (open) => {
      const page = open.find((c) => c.url.startsWith(self.registration.scope));
      if (!page) return self.clients.openWindow(target);
      await page.focus();
      if (page.url !== target && event.notification.data?.url) await page.navigate(target).catch(() => {});
    }),
  );
});

// The browser replaced the subscription (expired or rotated). The PC only
// learns the new one from the page, which reports it when it next opens.
self.addEventListener("pushsubscriptionchange", (event) => {
  const key = event.oldSubscription?.options?.applicationServerKey;
  if (!key) return;
  event.waitUntil(self.registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key }).catch(() => {}));
});
