// SPDX-License-Identifier: AGPL-3.0-only
// Notifications on the phone: subscribes this browser to Web Push and tells
// the PC where to send them (webpush.rs). `invoke` is the phone page's own
// sealed command call, so the subscription reaches the PC under this phone's
// key like every other request.
//
// Browsers only offer push on a secure page (the relay's https address, or
// localhost), and an iPhone only from a page added to the Home Screen.

export type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

export type PushState = "unsupported" | "denied" | "off" | "on";

const SW_URL = "./phone-sw.js";

export function pushSupported(): boolean {
  return window.isSecureContext && "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
}

function keyBytes(b64url: string): Uint8Array {
  const bin = atob(b64url.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(b64url.length / 4) * 4, "="));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

async function registration(): Promise<ServiceWorkerRegistration> {
  await navigator.serviceWorker.register(SW_URL);
  return navigator.serviceWorker.ready;
}

export async function pushState(): Promise<PushState> {
  if (!pushSupported()) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  const reg = await navigator.serviceWorker.getRegistration(SW_URL);
  const sub = await reg?.pushManager.getSubscription();
  return sub && Notification.permission === "granted" ? "on" : "off";
}

async function subscribe(invoke: Invoke): Promise<void> {
  const reg = await registration();
  const publicKey = (await invoke("push_public_key")) as string;
  const sub =
    (await reg.pushManager.getSubscription()) ??
    (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(publicKey) as BufferSource }));
  await invoke("push_subscribe", { subscription: sub.toJSON() });
}

/** Call from a tap: browsers only ask for notification permission after one. */
export async function enablePush(invoke: Invoke): Promise<PushState> {
  if (!pushSupported()) return "unsupported";
  if ((await Notification.requestPermission()) !== "granted") return pushState();
  await subscribe(invoke);
  return "on";
}

export async function disablePush(invoke: Invoke): Promise<PushState> {
  const reg = await navigator.serviceWorker.getRegistration(SW_URL);
  const sub = await reg?.pushManager.getSubscription();
  if (sub) {
    await invoke("push_unsubscribe", { endpoint: sub.endpoint });
    await sub.unsubscribe();
  }
  return pushState();
}

/** On every open, once push is on: the browser may have replaced the subscription since. */
export async function syncPush(invoke: Invoke): Promise<void> {
  if ((await pushState()) !== "on") return;
  await subscribe(invoke).catch(() => {});
}
