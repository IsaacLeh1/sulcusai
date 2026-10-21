// SPDX-License-Identifier: AGPL-3.0-only
// SulcusAI sync relay: a mailbox for paired PCs that aren't on the same
// network. It stores sealed messages it can't read and hands them to the
// other PC, which deletes them once it has applied them.
//
// Cloudflare Worker + one SQLite-backed Durable Object per mailbox (both on
// the Workers free plan). Deploy: `npx wrangler deploy` in this folder.
//
// API (box = 64 hex characters; seq = a whole number):
//   PUT    /v1/box/<box>/<seq>          body: the sealed message (≤ 2 MB)
//   GET    /v1/box/<box>?after=<seq>    → [{ "seq": n, "data": "<base64>" }…] (up to 20)
//   DELETE /v1/box/<box>?through=<seq>  removes messages up to seq
// Every request carries "Authorization: Bearer <token>". The first write to
// a box records a hash of its token; later requests must present the same
// one. Both PCs of a pair derive the token from their pair key; the relay
// keeps only its hash. A mailbox untouched for 30 days is erased.
//
// Feedback inbox (bug reports and ideas from the app's Bug / Feedback
// button, and crash reports people choose to send), one Durable Object:
//   POST   /feedback                    {kind: "bug"|"idea"|"crash"|"dev", text, version?, contact?}
//                                       Open to anyone: text only, size-limited, a few an
//                                       hour from one sender. Nothing about the sender is kept
//                                       (the hourly count uses a hash of their address that
//                                       changes every hour and is deleted after it).
//   GET    /feedback/inbox?key=<key>    the developer's page: everything sent, newest first
//   POST   /feedback/delete?key=<key>   form field id (or "all")
// The key is the FEEDBACK_KEY secret (wrangler secret put FEEDBACK_KEY).
//
// Phone link (a person's phone reaching their own PC from anywhere), one
// Durable Object per PC. The PC hosts: it keeps one WebSocket open here and
// answers everything; the relay only passes messages on. What passes is the
// app's own files (the same for everyone) or requests and answers sealed
// with the phone's key, which the relay can't read. With the PC away, the
// phone is told so.
//   GET    /v1/link/<id>/host           the PC's WebSocket, "Authorization: Bearer <token>"
//                                       (64 hex); the first connection records a hash of it
//   GET    /l/<id>/<path>               the app's files, from the PC
//   POST   /l/<id>/api                  a sealed request ("X-Phone: <phone id>"); the PC's answer
// id = 32 hex characters. A link whose PC hasn't connected for 90 days is erased.
// Frames on the WebSocket are JSON. To the PC: {t: "get", rid, path} or
// {t: "api", rid, phone, data, more?}. From the PC: {rid, status?, headers?,
// data?, more?}. data is base64, in parts of under 1 MB (more: true until the
// last); an answer streams to the phone as its parts arrive.

const MAX_BYTES = 2_000_000;
const KEEP_MS = 30 * 24 * 3600 * 1000;
const PAGE = 20;

const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

async function sha256(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function base64(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/feedback" || url.pathname.startsWith("/feedback/")) {
      if (!env.INBOX) return json({ error: "not found" }, 404);
      return env.INBOX.get(env.INBOX.idFromName("inbox")).fetch(request);
    }
    // The phone link: open to anyone who has the link's address, like the
    // feedback inbox (what passes is sealed, and only the PC can answer).
    const link = url.pathname.match(LINK_PATH);
    if (link) {
      if (!env.LINK) return json({ error: "not found" }, 404);
      return env.LINK.get(env.LINK.idFromName(link[2])).fetch(request);
    }
    // With RELAY_SECRET set (wrangler secret put RELAY_SECRET), only
    // addresses that start with /k/<secret> are served, so strangers can't
    // store things here. The app's relay address then ends in /k/<secret>.
    let path = url.pathname;
    if (env.RELAY_SECRET) {
      const prefix = `/k/${env.RELAY_SECRET}`;
      if (!path.startsWith(prefix + "/")) return json({ error: "not found" }, 404);
      path = path.slice(prefix.length);
    }
    const m = path.match(/^\/v1\/box\/([0-9a-f]{64})(?:\/(\d{1,15}))?$/);
    if (!m) return json({ error: "not found" }, 404);
    // One object per mailbox: its messages, token hash and expiry live together.
    const inner = new URL(request.url);
    inner.pathname = path;
    const stub = env.BOX.get(env.BOX.idFromName(m[1]));
    return stub.fetch(new Request(inner, request));
  },
};

export class Box {
  constructor(ctx) {
    this.ctx = ctx;
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS msgs (seq INTEGER PRIMARY KEY, data BLOB NOT NULL)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
  }

  /** Checks the token; on the box's first write, records it. */
  async authorized(request, firstWrite) {
    const auth = request.headers.get("authorization") || "";
    const token = auth.startsWith("Bearer ") ? auth.slice(7).trim() : "";
    if (!/^[0-9a-f]{64}$/.test(token)) return false;
    const have = await sha256(token);
    const row = this.sql.exec("SELECT v FROM meta WHERE k = 'auth'").toArray()[0];
    if (!row) {
      if (!firstWrite) return false;
      this.sql.exec("INSERT INTO meta (k, v) VALUES ('auth', ?)", have);
      return true;
    }
    return row.v === have;
  }

  async fetch(request) {
    const url = new URL(request.url);
    const m = url.pathname.match(/^\/v1\/box\/([0-9a-f]{64})(?:\/(\d{1,15}))?$/);
    const seqText = m && m[2];
    // Erase the mailbox after 30 quiet days.
    await this.ctx.storage.setAlarm(Date.now() + KEEP_MS);

    if (request.method === "PUT" && seqText !== undefined) {
      if (!(await this.authorized(request, true))) return json({ error: "unauthorized" }, 401);
      const body = new Uint8Array(await request.arrayBuffer());
      if (body.length === 0 || body.length > MAX_BYTES) return json({ error: "size" }, 413);
      this.sql.exec("INSERT OR REPLACE INTO msgs (seq, data) VALUES (?, ?)", Number(seqText), body);
      return json({ ok: true });
    }

    if (seqText !== undefined) return json({ error: "method" }, 405);
    if (!(await this.authorized(request, false))) return json({ error: "unauthorized or empty" }, 404);

    if (request.method === "GET") {
      const after = Number(url.searchParams.get("after") || "0");
      const rows = this.sql.exec("SELECT seq, data FROM msgs WHERE seq > ? ORDER BY seq LIMIT ?", after, PAGE).toArray();
      return json(rows.map((r) => ({ seq: Number(r.seq), data: base64(new Uint8Array(r.data)) })));
    }

    if (request.method === "DELETE") {
      const through = Number(url.searchParams.get("through") || "0");
      this.sql.exec("DELETE FROM msgs WHERE seq <= ?", through);
      return json({ ok: true });
    }

    return json({ error: "method" }, 405);
  }

  async alarm() {
    await this.ctx.storage.deleteAll();
  }
}

// "dev": reports from PCs with developer mode on (the person agreed to send them).
const KINDS = ["bug", "idea", "crash", "dev"];
const MAX_TEXT = 8000;
const PER_HOUR = 8;
const PER_DAY = 400;

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

export class Inbox {
  constructor(ctx, env) {
    this.ctx = ctx;
    this.env = env || {};
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS feedback (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, kind TEXT NOT NULL, text TEXT NOT NULL, version TEXT, contact TEXT)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS rate (k TEXT PRIMARY KEY, n INTEGER NOT NULL, hour INTEGER NOT NULL)");
  }

  async fetch(request) {
    const url = new URL(request.url);
    const key = this.env.FEEDBACK_KEY || "";
    const hour = Math.floor(Date.now() / 3600_000);
    if (request.method === "POST" && url.pathname === "/feedback") {
      let body;
      try {
        body = await request.json();
      } catch {
        return json({ error: "bad request" }, 400);
      }
      const kind = KINDS.includes(body.kind) ? body.kind : "idea";
      const text = String(body.text || "").trim().slice(0, MAX_TEXT);
      if (!text) return json({ error: "empty" }, 400);
      // A few an hour from one sender, and a ceiling for the day; the old counts go.
      this.sql.exec("DELETE FROM rate WHERE hour < ?", hour);
      const who = await sha256(`${request.headers.get("cf-connecting-ip") || "local"}|${hour}|${key}`);
      const mine = this.sql.exec("SELECT n FROM rate WHERE k = ?", who).toArray()[0];
      const today = this.sql.exec("SELECT COUNT(*) AS n FROM feedback WHERE at > ?", Date.now() - 86_400_000).toArray()[0];
      if ((mine && mine.n >= PER_HOUR) || (today && today.n >= PER_DAY)) return json({ error: "too many" }, 429);
      this.sql.exec("INSERT INTO rate (k, n, hour) VALUES (?, 1, ?) ON CONFLICT(k) DO UPDATE SET n = n + 1", who, hour);
      const version = String(body.version || "").slice(0, 20);
      const contact = String(body.contact || "").trim().slice(0, 200);
      this.sql.exec("INSERT INTO feedback (at, kind, text, version, contact) VALUES (?, ?, ?, ?, ?)", Date.now(), kind, text, version || null, contact || null);
      return json({ ok: true });
    }
    // The developer's side: the key, or nothing at all.
    if (!key || url.searchParams.get("key") !== key) return json({ error: "not found" }, 404);
    if (request.method === "POST" && url.pathname === "/feedback/delete") {
      const form = await request.formData().catch(() => null);
      const id = form && form.get("id");
      if (id === "all") this.sql.exec("DELETE FROM feedback");
      else if (id) this.sql.exec("DELETE FROM feedback WHERE id = ?", Number(id));
      return Response.redirect(`${url.origin}/feedback/inbox?key=${encodeURIComponent(key)}`, 303);
    }
    if (request.method === "GET" && url.pathname === "/feedback/inbox") {
      const rows = this.sql.exec("SELECT id, at, kind, text, version, contact FROM feedback ORDER BY id DESC LIMIT 500").toArray();
      const k = esc(encodeURIComponent(key));
      const items = rows
        .map(
          (r) =>
            `<article class="${esc(r.kind)}"><header><b>${esc(r.kind)}</b> · ${esc(new Date(Number(r.at)).toISOString().replace("T", " ").slice(0, 16))} UTC${r.version ? " · " + esc(r.version) : ""}${r.contact ? " · reply to: " + esc(r.contact) : ""}` +
            `<form method="post" action="/feedback/delete?key=${k}"><input type="hidden" name="id" value="${Number(r.id)}"><button>Delete</button></form></header><pre>${esc(r.text)}</pre></article>`,
        )
        .join(String.fromCharCode(10));
      const page =
        `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>SulcusAI feedback</title>` +
        `<style>body{font:15px system-ui,sans-serif;max-width:860px;margin:24px auto;padding:0 16px;background:#111018;color:#e8e6f0}article{border:1px solid #2b2938;border-radius:10px;padding:10px 14px;margin:12px 0}article.bug{border-color:#7a3b52}article.crash{border-color:#8a5a1c}article.dev{border-color:#2f5d7a}header{display:flex;gap:8px;align-items:center;color:#a09db0}header form{margin-left:auto}pre{white-space:pre-wrap;font:14px ui-monospace,Consolas,monospace;margin:8px 0 0}button{background:#1b1a24;color:#e8e6f0;border:1px solid #3a3850;border-radius:6px;padding:3px 10px;cursor:pointer}</style>` +
        `<h1>SulcusAI feedback <small>(${rows.length})</small></h1>` +
        (rows.length
          ? `<form method="post" action="/feedback/delete?key=${k}" onsubmit="return confirm('Delete everything?')"><input type="hidden" name="id" value="all"><button>Delete all</button></form>`
          : "<p>Nothing yet.</p>") +
        items;
      return new Response(page, { headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store", "x-robots-tag": "noindex" } });
    }
    return json({ error: "not found" }, 404);
  }
}

const LINK_PATH = /^\/(l|v1\/link)\/([0-9a-f]{32})(\/.*)?$/;
// How long the phone waits for the PC to start answering.
const LINK_WAIT_MS = 35_000;
// What a phone may send in one request, and requests a minute.
const LINK_MAX_BODY = 30_000_000;
const LINK_PER_MINUTE = 1200;
// Parts sent to the PC (WebSocket messages stay under 1 MB).
const LINK_PART = 600_000;
const LINK_KEEP_MS = 90 * 24 * 3600 * 1000;
// Headers the PC may set on what it sends the phone.
const LINK_HEADERS = ["content-type", "cache-control", "content-security-policy", "x-frame-options", "referrer-policy", "x-content-type-options"];

const unbase64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

function away(path) {
  const page =
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>SulcusAI</title>` +
    `<style>body{font:16px/1.5 system-ui,sans-serif;max-width:420px;margin:18vh auto 0;padding:0 20px;text-align:center;background:#f6f7f8;color:#1b1f23}` +
    `@media (prefers-color-scheme:dark){body{background:#111417;color:#e6e8eb}}button{font:inherit;padding:10px 18px;border-radius:10px;border:1px solid #888;background:none;color:inherit}</style>` +
    `<h1>Your PC isn't reachable</h1><p>Make sure your PC is on, SulcusAI is open on it, and "Reach this phone from anywhere" is on in Settings, Phone.</p>` +
    `<p><button onclick="location.reload()">Try again</button></p>`;
  const html = path === "/" || path === "" || path.endsWith(".html");
  return new Response(html ? page : "Your PC isn't reachable right now.", {
    status: 503,
    headers: { "content-type": html ? "text/html; charset=utf-8" : "text/plain", "cache-control": "no-store", "retry-after": "10" },
  });
}

export class Link {
  constructor(ctx) {
    this.ctx = ctx;
    // Requests waiting for the PC: rid -> { resolve, timer, writer }.
    this.waiting = new Map();
    this.next = 1;
    this.minute = 0;
    this.count = 0;
    // The PC's keep-alive, answered without waking this object.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  host() {
    return this.ctx.getWebSockets("host")[0];
  }

  async fetch(request) {
    const url = new URL(request.url);
    const m = url.pathname.match(LINK_PATH);
    if (!m) return json({ error: "not found" }, 404);
    const rest = m[3] || "";

    if (m[1] === "v1/link") {
      if (rest !== "/host" || request.headers.get("upgrade") !== "websocket") return json({ error: "not found" }, 404);
      const auth = request.headers.get("authorization") || "";
      const token = auth.startsWith("Bearer ") ? auth.slice(7).trim() : "";
      if (!/^[0-9a-f]{64}$/.test(token)) return json({ error: "unauthorized" }, 401);
      const have = await sha256(token);
      const stored = await this.ctx.storage.get("auth");
      if (stored && stored !== have) return json({ error: "unauthorized" }, 401);
      if (!stored) await this.ctx.storage.put("auth", have);
      // One PC per link: a new connection replaces the old one.
      for (const old of this.ctx.getWebSockets("host")) {
        try {
          old.close(4000, "replaced");
        } catch {}
      }
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1], ["host"]);
      await this.ctx.storage.setAlarm(Date.now() + LINK_KEEP_MS);
      return new Response(null, { status: 101, webSocket: pair[0] });
    }

    if (rest === "") return Response.redirect(`${url.origin}${url.pathname}/`, 302);
    const minute = Math.floor(Date.now() / 60_000);
    if (minute !== this.minute) {
      this.minute = minute;
      this.count = 0;
    }
    if (++this.count > LINK_PER_MINUTE) return new Response("Too many requests. Wait a minute.", { status: 429 });
    const ws = this.host();
    if (!ws) return away(rest);
    if (request.method === "POST" && rest === "/api") {
      const body = await request.text();
      if (!body || body.length > LINK_MAX_BODY) return new Response("Too big.", { status: 413 });
      return this.ask(ws, { t: "api", phone: (request.headers.get("x-phone") || "").slice(0, 64) }, body);
    }
    if (request.method === "GET") return this.ask(ws, { t: "get", path: rest.slice(0, 300) }, "");
    return json({ error: "method" }, 405);
  }

  /** Sends a request to the PC (data in parts) and waits for its answer to start. */
  ask(ws, msg, data) {
    const rid = this.next++;
    return new Promise((resolve) => {
      const timer = setTimeout(() => this.finish(rid, new Response("Your PC didn't answer in time.", { status: 504, headers: { "cache-control": "no-store" } })), LINK_WAIT_MS);
      this.waiting.set(rid, { resolve, timer, writer: null, ws });
      try {
        if (!data) {
          ws.send(JSON.stringify({ ...msg, rid }));
        } else {
          for (let i = 0; i < data.length; i += LINK_PART) {
            const part = data.slice(i, i + LINK_PART);
            ws.send(JSON.stringify({ ...msg, rid, data: part, more: i + LINK_PART < data.length }));
          }
        }
      } catch {
        this.finish(rid, away(msg.path || "/api"));
      }
    });
  }

  /** Answers a waiting request that hasn't started streaming. */
  finish(rid, response) {
    const w = this.waiting.get(rid);
    if (!w) return;
    clearTimeout(w.timer);
    this.waiting.delete(rid);
    if (w.writer) {
      w.writer.abort("the PC went away").catch(() => {});
    } else {
      w.resolve(response);
    }
  }

  async webSocketMessage(ws, message) {
    if (typeof message !== "string") return;
    let m;
    try {
      m = JSON.parse(message);
    } catch {
      return;
    }
    const w = this.waiting.get(m.rid);
    if (!w) return;
    const bytes = m.data ? unbase64(m.data) : new Uint8Array(0);
    if (!w.writer) {
      clearTimeout(w.timer);
      const headers = { "cache-control": "no-store", "x-content-type-options": "nosniff" };
      for (const [k, v] of Object.entries(m.headers || {})) {
        if (LINK_HEADERS.includes(k.toLowerCase())) headers[k.toLowerCase()] = String(v);
      }
      if (!m.more) {
        this.waiting.delete(m.rid);
        w.resolve(new Response(bytes, { status: m.status || 200, headers }));
        return;
      }
      // A long answer: the phone gets it as the parts arrive.
      const { readable, writable } = new TransformStream();
      w.writer = writable.getWriter();
      w.resolve(new Response(readable, { status: m.status || 200, headers }));
    }
    await w.writer.write(bytes);
    if (!m.more) {
      this.waiting.delete(m.rid);
      await w.writer.close();
    }
  }

  webSocketClose(ws) {
    this.drop(ws);
  }

  webSocketError(ws) {
    this.drop(ws);
  }

  /** The PC's connection closed: what it was answering won't come. */
  drop(ws) {
    for (const [rid, w] of [...this.waiting]) {
      if (w.ws === ws) this.finish(rid, away("/api"));
    }
  }

  async alarm() {
    if (this.host()) await this.ctx.storage.setAlarm(Date.now() + LINK_KEEP_MS);
    else await this.ctx.storage.deleteAll();
  }
}
