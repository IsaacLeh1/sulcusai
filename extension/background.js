// SPDX-License-Identifier: AGPL-3.0-only
// SulcusAI browser control. Talks only to the SulcusAI app on this PC,
// through the browser's native messaging (no network, no debugging port).
// Each SulcusAI chat works in a tab of its own, kept in a "SulcusAI" tab
// group so you can see them; close a tab to take it back.

const HOST = "app.sulcusai.bridge";
let port = null;
// Each chat's tab (chat key → tab id); kept for the session, so a restart of
// this background worker still finds them.
let tabs = {};
let retry = 2000;
chrome.storage?.session?.get("tabs").then((v) => {
  if (v && v.tabs) tabs = { ...v.tabs, ...tabs };
});
const keep = () => chrome.storage?.session?.set({ tabs }).catch(() => {});

function badge(text, color) {
  chrome.action.setBadgeText({ text });
  if (color) chrome.action.setBadgeBackgroundColor({ color });
}

// Stays the same across browser restarts, so pairing is asked once.
async function installId() {
  try {
    const v = await chrome.storage.local.get("install");
    if (v && v.install) return v.install;
    const id = crypto.randomUUID();
    await chrome.storage.local.set({ install: id });
    return id;
  } catch (e) {
    return "";
  }
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
    // The app shows the same code: the person checks they match once.
    if (msg && msg.type === "pair") {
      badge(String(msg.code), "#b45309");
      chrome.action.setTitle({ title: "SulcusAI: pairing code " + msg.code + ". Check SulcusAI shows the same code." });
      return;
    }
    if (msg && msg.type === "paired") {
      badge("on", "#0f766e");
      chrome.action.setTitle({ title: "SulcusAI" });
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
  installId().then((install) => send({ type: "hello", version: chrome.runtime.getManifest().version, install }));
}

function send(m) {
  try {
    port && port.postMessage(m);
  } catch {
    // The connection closed; it reconnects by itself.
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function liveTab(chat) {
  const id = tabs[chat];
  if (id === undefined) return null;
  try {
    return await chrome.tabs.get(id);
  } catch {
    delete tabs[chat];
    keep();
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

async function call(chat, name, args) {
  const t = await liveTab(chat);
  if (!t) throw new Error("This chat's SulcusAI tab isn't open. Open a page first.");
  const target = { tabId: t.id };
  await chrome.scripting.executeScript({ target, files: ["page.js"] });
  const [r] = await chrome.scripting.executeScript({ target, func: (n, a) => globalThis.__sulcus[n](...a), args: [name, args] });
  return r ? r.result : null;
}

/** The SulcusAI tab group in a window, made when it isn't there. */
async function group(windowId, tabId) {
  try {
    const mine = Object.values(tabs).filter((id) => id !== tabId);
    for (const id of mine) {
      const other = await chrome.tabs.get(id).catch(() => null);
      if (other && other.windowId === windowId && other.groupId >= 0) {
        await chrome.tabs.group({ groupId: other.groupId, tabIds: [tabId] });
        return;
      }
    }
    const g = await chrome.tabs.group({ tabIds: [tabId] });
    await chrome.tabGroups.update(g, { title: "SulcusAI", color: "cyan" });
  } catch {
    // Tab groups are optional.
  }
}

async function handle(m) {
  // Requests from older apps name no chat: one shared tab, as before.
  const chat = m.chat || "main";
  switch (m.op) {
    case "ping":
      return { version: chrome.runtime.getManifest().version, tab: !!(await liveTab(chat)) };
    case "open": {
      let t = await liveTab(chat);
      if (t) {
        t = await chrome.tabs.update(t.id, { url: m.url, active: true });
      } else {
        t = await chrome.tabs.create({ url: m.url, active: true });
        tabs[chat] = t.id;
        keep();
        await group(t.windowId, t.id);
      }
      await chrome.windows.update(t.windowId, { focused: true }).catch(() => {});
      await settle(t.id);
      return true;
    }
    case "read":
      return call(chat, "read", []);
    case "target":
      return call(chat, "target", [m.ref]);
    case "click": {
      const ok = await call(chat, "click", [m.ref]);
      if (!ok) throw new Error(`There's no element [${m.ref}] on the page now. Read the page again.`);
      await settle(tabs[chat]);
      return true;
    }
    case "type": {
      const ok = await call(chat, "type", [m.ref, m.text, !!m.submit]);
      if (ok === "no-choice") throw new Error(`“${m.text}” isn't one of the choices.`);
      if (!ok) throw new Error(`There's no element [${m.ref}] on the page now. Read the page again.`);
      if (m.submit) await settle(tabs[chat]);
      return true;
    }
    case "scroll":
      await call(chat, "scroll", [!!m.down, Number(m.screens) || 1]);
      await sleep(400);
      return true;
    case "key": {
      const ok = await call(chat, "key", [String(m.key || "")]);
      if (!ok) throw new Error(`${m.key} isn't a key I can press here.`);
      await sleep(300);
      return true;
    }
    case "back": {
      const t = await liveTab(chat);
      if (!t) throw new Error("This chat's SulcusAI tab isn't open.");
      // tabs.goBack misses entries made by form submissions; the page's own history doesn't.
      await chrome.scripting.executeScript({ target: { tabId: t.id }, func: () => history.back() });
      await settle(t.id);
      return true;
    }
    case "screenshot": {
      // What the tab shows now, as a JPEG (the model reads pictures, not pixels).
      const t = await liveTab(chat);
      if (!t) throw new Error("This chat's SulcusAI tab isn't open.");
      await chrome.tabs.update(t.id, { active: true });
      await sleep(150);
      return chrome.tabs.captureVisibleTab(t.windowId, { format: "jpeg", quality: 70 });
    }
    case "text": {
      // The whole page as text, for what the numbered reading leaves out.
      const t = await liveTab(chat);
      if (!t) throw new Error("This chat's SulcusAI tab isn't open.");
      const [r] = await chrome.scripting.executeScript({ target: { tabId: t.id }, func: (max) => (document.body ? document.body.innerText : "").slice(0, max), args: [Number(m.max) || 60000] });
      return r ? r.result : "";
    }
    case "tabs": {
      // Only the tabs in SulcusAI's group: the person's other tabs stay theirs.
      const own = new Set(Object.values(tabs));
      const mine = await liveTab(chat);
      const groups = new Set();
      for (const id of own) {
        const x = await chrome.tabs.get(id).catch(() => null);
        if (x && x.groupId >= 0) groups.add(x.groupId);
      }
      const all = await chrome.tabs.query({});
      return all.filter((x) => own.has(x.id) || groups.has(x.groupId)).map((x) => ({ id: x.id, title: x.title || "", url: x.url || "", current: !!mine && x.id === mine.id }));
    }
    case "switch": {
      const x = await chrome.tabs.get(Number(m.tab)).catch(() => null);
      const owned = x && (Object.values(tabs).includes(x.id) || (x.groupId >= 0 && (await chrome.tabGroups.get(x.groupId).catch(() => null))?.title === "SulcusAI"));
      if (!owned) throw new Error("That tab isn't one of SulcusAI's. List them with browser_tabs.");
      tabs[chat] = x.id;
      keep();
      await chrome.tabs.update(x.id, { active: true });
      return true;
    }
    case "new_tab": {
      const t = await chrome.tabs.create({ url: m.url, active: true });
      tabs[chat] = t.id;
      keep();
      await group(t.windowId, t.id);
      await settle(t.id);
      return true;
    }
    case "close": {
      const t = await liveTab(chat);
      if (!t) return true;
      await chrome.tabs.remove(t.id);
      delete tabs[chat];
      keep();
      return true;
    }
    case "wait":
      await sleep(Math.min(Math.max(Number(m.seconds) || 1, 0), 10) * 1000);
      return true;
    default:
      throw new Error(`Unknown request ${m.op}.`);
  }
}

chrome.tabs.onRemoved.addListener((id) => {
  for (const [chat, tab] of Object.entries(tabs)) if (tab === id) delete tabs[chat];
  keep();
});
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.action.onClicked.addListener(connect);
connect();
