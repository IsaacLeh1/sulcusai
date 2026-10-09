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
