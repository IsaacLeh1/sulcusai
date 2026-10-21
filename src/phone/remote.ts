// SPDX-License-Identifier: AGPL-3.0-only
// The phone's way into the app (phone.html, served by the PC on the local
// network or through SulcusAI's relay): the same window as on the PC, with
// every command sent to the PC sealed with this phone's key, and the events
// it listens to fetched from there. The phone signs in first with the
// password chosen when it was paired.
//
// Tauri's own pieces are stood in for here (window.__TAURI_INTERNALS__):
// commands go to the PC, events come back from it, and the few things that
// only make sense on a PC answer like a phone would (files are picked on the
// phone and sent over; yes/no questions use the phone's own dialogs).
import naclModule from "../../src-tauri/src/phone/nacl-fast.min.js";
import type { PhoneInfo } from "./env";

interface Nacl {
  randomBytes(n: number): Uint8Array;
  secretbox: ((msg: Uint8Array, nonce: Uint8Array, key: Uint8Array) => Uint8Array) & {
    open(box: Uint8Array, nonce: Uint8Array, key: Uint8Array): Uint8Array | null;
  };
}
// Bundled as a CommonJS module; without a bundler it sets self.nacl.
const nacl = ((naclModule as Nacl | undefined)?.secretbox ? naclModule : (self as unknown as { nacl: Nacl }).nacl) as Nacl;

/** Through the relay the page lives under /l/<link id>/; on the local network at the root. */
const base = /^(\/l\/[0-9a-f]{32})\//.exec(location.pathname)?.[1] ?? "";
const API = `${base}/api`;
const AWAY = "Your PC isn't reachable. Make sure it's on and SulcusAI is open on it.";
const ONLY_PC = "That works on the PC only.";
/** The biggest file sent from the phone (the PC takes 15 MB). */
const MAX_UPLOAD = 15 * 1024 * 1024;

const enc = new TextEncoder();
const dec = new TextDecoder();

function toB64(u: Uint8Array): string {
  let s = "";
  for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s);
}

function fromB64(s: string): Uint8Array {
  const bin = atob(s.trim());
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function stored(k: string): string {
  try {
    return localStorage.getItem(k) ?? "";
  } catch {
    return "";
  }
}

// The key arrives after "#" in the QR code's link (never sent over the
// network) and stays on this phone.
let phone = "";
let key: Uint8Array | null = null;
(function pair() {
  const m = /p=([0-9A-Za-z]+)\.([A-Za-z0-9_-]+)/.exec(location.hash);
  if (m) {
    try {
      localStorage.setItem("sulcus.phone", m[1]);
      localStorage.setItem("sulcus.key", m[2]);
    } catch {
      /* kept for this visit only */
    }
    phone = m[1];
    history.replaceState(null, "", location.pathname);
  }
  phone = phone || stored("sulcus.phone");
  const k = m ? m[2] : stored("sulcus.key");
  if (k) key = fromB64(k.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((k.length + 3) % 4));
})();

let session = "";
try {
  session = sessionStorage.getItem("sulcus.session") ?? "";
} catch {
  /* signs in each time */
}

type Kind = "login" | "away" | "unpaired" | "other";
class PhoneError extends Error {
  constructor(
    message: string,
    readonly kind: Kind,
  ) {
    super(message);
  }
}

/** One sealed request to the PC. */
async function call<T = unknown>(op: string, args: unknown = {}): Promise<T> {
  const k = key as Uint8Array;
  const nonce = nacl.randomBytes(24);
  const box = nacl.secretbox(enc.encode(JSON.stringify({ op, args, ts: Date.now(), session })), nonce, k);
  const body = new Uint8Array(24 + box.length);
  body.set(nonce);
  body.set(box, 24);
  let r: Response;
  try {
    r = await fetch(API, { method: "POST", headers: { "X-Phone": phone, "Content-Type": "text/plain" }, body: toB64(body), cache: "no-store" });
  } catch {
    throw new PhoneError("Can't reach your PC. Check this phone's connection.", "away");
  }
  const text = await r.text();
  if (!r.ok) {
    if (r.status >= 502 && r.status <= 504) throw new PhoneError(text && !text.trimStart().startsWith("<") ? text : AWAY, "away");
    throw new PhoneError(text || "The PC didn't answer.", r.status === 401 ? "unpaired" : "other");
  }
  const raw = fromB64(text);
  const plain = nacl.secretbox.open(raw.subarray(24), raw.subarray(0, 24), k);
  if (!plain) throw new PhoneError("The answer didn't check out.", "other");
  const res = JSON.parse(dec.decode(plain));
  if (!res.ok) {
    if (res.login && op !== "login") {
      session = "";
      try {
        sessionStorage.removeItem("sulcus.session");
      } catch {
        /* nothing kept */
      }
      signIn(res.error);
      throw new PhoneError(res.error, "login");
    }
    throw new PhoneError(res.error, "other");
  }
  return res.data as T;
}

// ---------- signing in ----------

let info: PhoneInfo | null = null;
let signedIn: Promise<void> | null = null;

const css = `
#phone-login{position:fixed;inset:0;z-index:2147483000;display:flex;align-items:center;justify-content:center;padding:24px;background:var(--bg,#f6f7f8);color:var(--text,#1b1f23);font:16px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
@media (prefers-color-scheme:dark){#phone-login{background:var(--bg,#111417);color:var(--text,#e6e8eb)}}
#phone-login form,#phone-login .box{display:flex;flex-direction:column;gap:12px;width:100%;max-width:340px;text-align:center}
#phone-login input{font:inherit;font-size:16px;color:inherit;background:transparent;border:1px solid #8888;border-radius:10px;padding:12px}
#phone-login button{font:inherit;border-radius:10px;padding:12px;border:1px solid #1f6f5c;background:#1f6f5c;color:#fff}
#phone-login .why{min-height:1.5em;color:#b3261e}
#phone-status{position:fixed;left:0;right:0;top:0;z-index:2147482000;padding:6px 12px;padding-top:max(6px,env(safe-area-inset-top));background:#b3261e;color:#fff;font:13px system-ui,sans-serif;text-align:center}
`;

function layer(): HTMLElement {
  if (!document.getElementById("phone-css")) {
    const s = document.createElement("style");
    s.id = "phone-css";
    s.textContent = css;
    document.head.append(s);
  }
  let el = document.getElementById("phone-login");
  if (!el) {
    el = document.createElement("div");
    el.id = "phone-login";
    document.body.append(el);
  }
  return el;
}

function message(text: string, again = true) {
  const el = layer();
  el.innerHTML = `<div class="box"><p></p>${again ? '<button type="button">Try again</button>' : ""}</div>`;
  (el.querySelector("p") as HTMLElement).textContent = text;
  el.querySelector("button")?.addEventListener("click", () => location.reload());
}

/** Asks for the phone's password (once at a time); resolves when signed in. */
function signIn(why?: string): Promise<void> {
  if (signedIn) return signedIn;
  signedIn = new Promise<void>((resolve) => {
    const el = layer();
    el.innerHTML = `<form><p>Enter the password you chose for this phone on your PC.</p>
      <input type="password" autocomplete="current-password" placeholder="Phone password" required minlength="6">
      <button type="submit">Unlock</button><p class="why"></p></form>`;
    const form = el.querySelector("form") as HTMLFormElement;
    const input = el.querySelector("input") as HTMLInputElement;
    const said = el.querySelector(".why") as HTMLElement;
    said.textContent = why && !/Enter this phone's password/.test(why) ? why : "";
    input.focus();
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const button = form.querySelector("button") as HTMLButtonElement;
      button.disabled = true;
      try {
        const r = await call<{ session: string }>("login", { password: input.value });
        session = r.session;
        try {
          sessionStorage.setItem("sulcus.session", session);
        } catch {
          /* signs in again next time */
        }
        el.remove();
        signedIn = null;
        resubscribe();
        resolve();
      } catch (e) {
        button.disabled = false;
        input.value = "";
        said.textContent = (e as Error).message;
      }
    });
  });
  return signedIn;
}

function setAway(away: boolean) {
  let bar = document.getElementById("phone-status");
  if (!away) {
    bar?.remove();
    return;
  }
  if (!bar) {
    bar = document.createElement("div");
    bar.id = "phone-status";
    bar.setAttribute("role", "status");
    document.body.append(bar);
  }
  bar.textContent = "Can't reach your PC right now. Trying again…";
}

// ---------- events ----------

const callbacks = new Map<number, (data: unknown) => void>();
const listeners = new Map<string, Set<number>>();

function transformCallback(cb?: (data: unknown) => void, once = false): number {
  const id = crypto.getRandomValues(new Uint32Array(1))[0];
  callbacks.set(id, (data) => {
    if (once) callbacks.delete(id);
    cb?.(data);
  });
  return id;
}

function deliver(event: string, payload: unknown) {
  for (const id of listeners.get(event) ?? []) {
    try {
      callbacks.get(id)?.({ event, id, payload });
    } catch (e) {
      console.error(e);
    }
  }
}

function subscribe(event: string) {
  if (session) call("listen", { event }).catch(() => {});
}

function resubscribe() {
  for (const event of listeners.keys()) subscribe(event);
}

function eventPlugin(fn: string, args: Record<string, unknown>): unknown {
  const event = String(args.event ?? "");
  if (fn === "listen") {
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
      subscribe(event);
    }
    set.add(Number(args.handler));
    return Number(args.handler);
  }
  if (fn === "unlisten") {
    unregisterListener(event, Number(args.eventId));
    return null;
  }
  // Events the window sends itself stay here.
  if (fn === "emit" || fn === "emit_to") {
    deliver(event, args.payload);
    return null;
  }
  return null;
}

function unregisterListener(event: string, id: number) {
  callbacks.delete(id);
  const set = listeners.get(event);
  set?.delete(id);
  if (set && set.size === 0) {
    listeners.delete(event);
    if (session) call("unlisten", { event }).catch(() => {});
  }
}

/** Fetches events from the PC for as long as the page is open. */
async function pump() {
  let cursor = 0;
  for (;;) {
    try {
      const r = await call<{ events: { event: string; payload: unknown }[]; cursor: number; listening: number }>("events", { after: cursor });
      setAway(false);
      cursor = r.cursor;
      // The PC restarted (it knows of no events this phone listens to).
      if (r.listening === 0 && listeners.size > 0) resubscribe();
      for (const e of r.events) deliver(e.event, e.payload);
      if (r.events.length === 0 && listeners.size === 0) await new Promise((w) => setTimeout(w, 1500));
    } catch (e) {
      const err = e as PhoneError;
      if (err.kind === "login") await signIn();
      else {
        if (err.kind === "away") setAway(true);
        await new Promise((w) => setTimeout(w, 3000));
      }
    }
  }
}

// ---------- what a PC would do ----------

function pickFiles(multiple: boolean, filters?: { extensions?: string[] }[]): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = multiple;
    const exts = (filters ?? []).flatMap((f) => f.extensions ?? []).filter((x) => x && x !== "*");
    if (exts.length) input.accept = exts.map((x) => `.${x}`).join(",");
    let done = false;
    const finish = (files: File[]) => {
      if (done) return;
      done = true;
      input.remove();
      resolve(files);
    };
    input.addEventListener("change", () => finish([...(input.files ?? [])]));
    input.addEventListener("cancel", () => finish([]));
    input.style.display = "none";
    document.body.append(input);
    input.click();
  });
}

/** Sends a file chosen on the phone to the PC; answers with its path there. */
async function upload(f: File): Promise<string> {
  if (f.size > MAX_UPLOAD) throw new Error(`${f.name} is too big to send from the phone (15 MB at most).`);
  const data = toB64(new Uint8Array(await f.arrayBuffer()));
  return call<string>("upload", { name: f.name, data });
}

async function dialog(fn: string, args: Record<string, unknown>): Promise<unknown> {
  const o = ((args.options as Record<string, unknown>) ?? args) as Record<string, unknown>;
  const text = String(args.message ?? o.message ?? "");
  if (fn === "message") {
    alert(text);
    return "Ok";
  }
  if (fn === "ask" || fn === "confirm") return confirm(text);
  if (fn === "open") {
    if (o.directory) throw new Error("Choose folders on the PC.");
    const files = await pickFiles(!!o.multiple, o.filters as { extensions?: string[] }[] | undefined);
    if (!files.length) return null;
    const paths: string[] = [];
    for (const f of files) paths.push(await upload(f));
    return o.multiple ? paths : paths[0];
  }
  if (fn === "save") throw new Error("Saving into a folder works on the PC.");
  throw new Error(ONLY_PC);
}

async function plugin(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  const [name, fn] = cmd.slice("plugin:".length).split("|");
  switch (name) {
    case "event":
      return eventPlugin(fn, args);
    case "app":
      return fn === "version" ? (info?.version ?? "") : fn === "name" ? "SulcusAI" : null;
    case "window":
    case "webview":
      // Moving, sizing and theming the PC's window: nothing to do on a phone.
      return fn === "is_focused" ? document.hasFocus() : fn === "is_visible" ? true : fn === "scale_factor" ? devicePixelRatio : null;
    case "dialog":
      return dialog(fn, args);
    case "opener":
      if (fn === "open_url") {
        window.open(String(args.url ?? ""), "_blank", "noopener");
        return null;
      }
      break;
    case "clipboard-manager":
      if (fn === "write_text") {
        await navigator.clipboard?.writeText(String(args.text ?? ""));
        return null;
      }
      break;
  }
  throw new Error(ONLY_PC);
}

/** What came back from the PC: files arrive as base64 and become bytes again. */
function fromWire(v: unknown): unknown {
  if (v && typeof v === "object" && !Array.isArray(v)) {
    const raw = (v as { __raw?: unknown }).__raw;
    if (typeof raw === "string" && Object.keys(v).length === 1) return fromB64(raw).buffer;
  }
  return v;
}

async function invoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
  if (cmd.startsWith("plugin:")) return plugin(cmd, args ?? {});
  if (signedIn) await signedIn;
  let r = await call<{ done: boolean; ticket?: string; data?: unknown }>("invoke", { cmd, args: args ?? {} });
  // A long one (a chat reply): asked for again until it's done.
  while (!r.done) r = await call<{ done: boolean; ticket?: string; data?: unknown }>("result", { ticket: r.ticket });
  return fromWire(r.data);
}

// ---------- start ----------

function install() {
  const w = window as unknown as Record<string, unknown>;
  w.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback,
    unregisterCallback: (id: number) => callbacks.delete(id),
    runCallback: (id: number, data: unknown) => callbacks.get(id)?.(data),
    callbacks,
    convertFileSrc: (p: string) => p,
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    plugins: {},
  };
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener };
}

async function start() {
  install();
  if (!phone || !key || key.length !== 32) {
    message("To use SulcusAI here, open it on your PC: Settings, Phone, Add a phone, and scan the code with this phone's camera.", false);
    return;
  }
  if (!session) await signIn();
  for (;;) {
    try {
      info = { ...(await call<Omit<PhoneInfo, "relay">>("hello")), relay: !!base };
      break;
    } catch (e) {
      const err = e as PhoneError;
      if (err.kind === "login") await signIn();
      else if (err.kind === "unpaired") return message(err.message, false);
      else {
        message(err.message);
        await new Promise((w) => setTimeout(w, 4000));
      }
    }
  }
  layer().remove();
  window.__SULCUS_PHONE__ = info;
  document.title = info.pc ? `SulcusAI · ${info.pc}` : "SulcusAI";
  pump();
  await import("../main");
}

start();
