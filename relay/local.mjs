// SPDX-License-Identifier: AGPL-3.0-only
// Runs worker.js locally, with each mailbox's Durable Object backed by an
// in-memory SQLite database (Node's node:sqlite), for tests and for trying
// a relay without Cloudflare: node relay/local.mjs [port]
import http from "node:http";
import { DatabaseSync } from "node:sqlite";
import worker, { Box } from "./worker.js";

/** Just enough of a Durable Object's storage for the relay. */
function storage() {
  let db = new DatabaseSync(":memory:");
  return {
    sql: {
      exec(query, ...binds) {
        const stmt = db.prepare(query);
        const rows = /^\s*SELECT/i.test(query) ? stmt.all(...binds) : (stmt.run(...binds), []);
        return { toArray: () => rows };
      },
    },
    async setAlarm() {},
    async deleteAll() {
      db = new DatabaseSync(":memory:");
    },
  };
}

const objects = new Map();
const env = {
  BOX: {
    idFromName: (name) => name,
    get(id) {
      if (!objects.has(id)) objects.set(id, new Box({ storage: storage() }));
      const box = objects.get(id);
      return { fetch: (request) => box.fetch(request) };
    },
  },
};

const port = Number(process.argv[2] || 8787);
http
  .createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const request = new Request(`http://localhost:${port}${req.url}`, {
      method: req.method,
      headers: req.headers,
      body: ["GET", "HEAD"].includes(req.method) ? undefined : Buffer.concat(chunks),
    });
    const response = await worker.fetch(request, env);
    res.writeHead(response.status, Object.fromEntries(response.headers));
    res.end(Buffer.from(await response.arrayBuffer()));
  })
  .listen(port, "127.0.0.1", () => console.log(`relay on http://127.0.0.1:${port}`));
