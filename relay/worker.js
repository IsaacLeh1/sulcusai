// SPDX-License-Identifier: AGPL-3.0-only
// SulcusAI sync relay: a mailbox for paired PCs that aren't on the same
// network. It stores sealed messages it can't read and hands them to the
// other PC, which deletes them once it has applied them.
//
// Cloudflare Worker with one KV namespace bound as BOXES:
//   wrangler kv namespace create BOXES   (put its id in wrangler.toml)
//   wrangler deploy
//
// API (box = 64 hex characters; seq = a whole number):
//   PUT    /v1/box/<box>/<seq>          body: the sealed message (≤ 8 MB)
//   GET    /v1/box/<box>?after=<seq>    → [{ "seq": n, "data": "<base64>" }…] (up to 20)
//   DELETE /v1/box/<box>?through=<seq>  removes messages up to seq
// Every request carries "Authorization: Bearer <token>". The first write to
// a box records a hash of its token; later requests must present the same
// one. Both PCs of a pair derive the token from their pair key; the relay
// only ever sees its hash in storage.

const MAX_BYTES = 8 * 1024 * 1024;
const KEEP_SECONDS = 30 * 24 * 3600;
const PAGE = 20;

const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const pad = (n) => String(n).padStart(16, "0");

async function sha256(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function base64(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** Checks the token; on a box's first write, records it. */
async function authorized(env, box, request, firstWrite) {
  const auth = request.headers.get("authorization") || "";
  const token = auth.startsWith("Bearer ") ? auth.slice(7).trim() : "";
  if (!/^[0-9a-f]{64}$/.test(token)) return false;
  const want = await env.BOXES.get(`auth:${box}`);
  const have = await sha256(token);
  if (want === null) {
    if (!firstWrite) return false;
    await env.BOXES.put(`auth:${box}`, have, { expirationTtl: KEEP_SECONDS });
    return true;
  }
  return want === have;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const m = url.pathname.match(/^\/v1\/box\/([0-9a-f]{64})(?:\/(\d{1,15}))?$/);
    if (!m) return json({ error: "not found" }, 404);
    const [, box, seqText] = m;

    if (request.method === "PUT" && seqText !== undefined) {
      if (!(await authorized(env, box, request, true))) return json({ error: "unauthorized" }, 401);
      const body = new Uint8Array(await request.arrayBuffer());
      if (body.length === 0 || body.length > MAX_BYTES) return json({ error: "size" }, 413);
      await env.BOXES.put(`msg:${box}:${pad(seqText)}`, body, { expirationTtl: KEEP_SECONDS });
      // Keep the token alive as long as messages are flowing.
      await env.BOXES.put(`auth:${box}`, await env.BOXES.get(`auth:${box}`), { expirationTtl: KEEP_SECONDS });
      return json({ ok: true });
    }

    if (seqText !== undefined) return json({ error: "method" }, 405);
    if (!(await authorized(env, box, request, false))) return json({ error: "unauthorized or empty" }, 404);

    if (request.method === "GET") {
      const after = Number(url.searchParams.get("after") || "0");
      const list = await env.BOXES.list({ prefix: `msg:${box}:` });
      const out = [];
      for (const k of list.keys) {
        const seq = Number(k.name.slice(-16));
        if (seq <= after) continue;
        const data = await env.BOXES.get(k.name, "arrayBuffer");
        if (data) out.push({ seq, data: base64(new Uint8Array(data)) });
        if (out.length >= PAGE) break;
      }
      return json(out);
    }

    if (request.method === "DELETE") {
      const through = Number(url.searchParams.get("through") || "0");
      const list = await env.BOXES.list({ prefix: `msg:${box}:` });
      for (const k of list.keys) {
        if (Number(k.name.slice(-16)) <= through) await env.BOXES.delete(k.name);
      }
      return json({ ok: true });
    }

    return json({ error: "method" }, 405);
  },
};
