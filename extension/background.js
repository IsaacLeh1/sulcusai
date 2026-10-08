// SPDX-License-Identifier: AGPL-3.0-only
// SulcusAI browser control. Talks only to the SulcusAI app on this PC,
// through the browser's native messaging (no network, no debugging port).
// The assistant works in one tab, kept in a "SulcusAI" tab group so you can
// see it; close the tab to take it back.

const HOST = "app.sulcusai.bridge";
let port = null;
let tabId = null;
let retry = 2000;

function badge(text, color) {
  chrome.action.setBadgeText({ text });
  if (color) chrome.action.setBadgeBackgroundColor({ color });
}

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (e) {
    port = null;
    return setTimeout(connect, retry);
  }
  port.onMessage.addListener((msg) => {
    if (msg && msg.type === "status") {
      badge(msg.app ? "on" : "", "#0f766e");
      return;
    }
    handle(msg).then(
      (result) => send({ id: msg.id, ok: true, result }),
      (e) => send({ id: msg.id, ok: false, error: String((e && e.message) || e) }),
    );
  });
  port.onDisconnect.addListener(() => {
    port = null;
    badge("", null);
    retry = Math.min(retry * 2, 60000);
    setTimeout(connect, retry);
  });
  send({ type: "hello", version: chrome.runtime.getManifest().version });
}

function send(m) {
  try {
    port && port.postMessage(m);
  } catch {
    // The connection closed; it reconnects by itself.
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function liveTab() {
  if (tabId === null) return null;
  try {
    return await chrome.tabs.get(tabId);
  } catch {
    tabId = null;
    return null;
  }
}

async function settle(id) {
  await sleep(500);
  for (let i = 0; i < 30; i++) {
    const t = await chrome.tabs.get(id).catch(() => null);
    if (!t) throw new Error("The tab was closed.");
    if (t.status === "complete") return t;
    await sleep(500);
  }
  return chrome.tabs.get(id);
}

async function call(name, args) {
  const t = await liveTab();
  if (!t) throw new Error("The SulcusAI tab isn't open. Open a page first.");
  const target = { tabId: t.id };
  await chrome.scripting.executeScript({ target, files: ["page.js"] });
  const [r] = await chrome.scripting.executeScript({ target, func: (n, a) => globalThis.__sulcus[n](...a), args: [name, args] });
  return r ? r.result : null;
}

async function handle(m) {
  switch (m.op) {
    case "ping":
      return { version: chrome.runtime.getManifest().version, tab: !!(await liveTab()) };
    case "open": {
      let t = await liveTab();
      if (t) {
        t = await chrome.tabs.update(t.id, { url: m.url, active: true });
      } else {
        t = await chrome.tabs.create({ url: m.url, active: true });
        tabId = t.id;
        try {
          const group = await chrome.tabs.group({ tabIds: [t.id] });
          await chrome.tabGroups.update(group, { title: "SulcusAI", color: "cyan" });
        } catch {
          // Tab groups are optional.
        }
      }
      await chrome.windows.update(t.windowId, { focused: true }).catch(() => {});
      await settle(t.id);
      return true;
    }
    case "read":
      return call("read", []);
    case "target":
      return call("target", [m.ref]);
    case "click": {
      const ok = await call("click", [m.ref]);
      if (!ok) throw new Error(`There's no element [${m.ref}] on the page now. Read the page again.`);
      await settle(tabId);
      return true;
    }
    case "type": {
      const ok = await call("type", [m.ref, m.text, !!m.submit]);
      if (ok === "no-choice") throw new Error(`“${m.text}” isn't one of the choices.`);
      if (!ok) throw new Error(`There's no element [${m.ref}] on the page now. Read the page again.`);
      if (m.submit) await settle(tabId);
      return true;
    }
    case "back": {
      const t = await liveTab();
      if (!t) throw new Error("The SulcusAI tab isn't open.");
      // tabs.goBack misses entries made by form submissions; the page's own history doesn't.
      await chrome.scripting.executeScript({ target: { tabId: t.id }, func: () => history.back() });
      await settle(t.id);
      return true;
    }
    default:
      throw new Error(`Unknown request ${m.op}.`);
  }
}

chrome.tabs.onRemoved.addListener((id) => {
  if (id === tabId) tabId = null;
});
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.action.onClicked.addListener(connect);
connect();
