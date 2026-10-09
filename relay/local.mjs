// SPDX-License-Identifier: AGPL-3.0-only
// Runs worker.js locally with an in-memory store, for tests and for trying a
// relay without Cloudflare: node relay/local.mjs [port]
import http from "node:http";
import worker from "./worker.js";

class MemoryKV {
  constructor() {
    this.map = new Map();
  }
  async get(key, type) {
    if (!this.map.has(key)) return null;
    const v = this.map.get(key);
    if (type === "arrayBuffer") return v instanceof Uint8Array ? v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength) : new TextEncoder().encode(v).buffer;
    return v instanceof Uint8Array ? new TextDecoder().decode(v) : v;
  }
  async put(key, value) {
    this.map.set(key, value instanceof Uint8Array ? new Uint8Array(value) : value);
  }
  async delete(key) {
    this.map.delete(key);
  }
  async list({ prefix }) {
    return { keys: [...this.map.keys()].filter((k) => k.startsWith(prefix)).sort().map((name) => ({ name })) };
  }
}

const env = { BOXES: new MemoryKV() };
const port = Number(process.argv[2] || 8787);
http
  .createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const body = Buffer.concat(chunks);
    const request = new Request(`http://localhost:${port}${req.url}`, {
      method: req.method,
      headers: req.headers,
      body: ["GET", "HEAD"].includes(req.method) ? undefined : body,
    });
    const response = await worker.fetch(request, env);
    res.writeHead(response.status, Object.fromEntries(response.headers));
    res.end(Buffer.from(await response.arrayBuffer()));
  })
  .listen(port, "127.0.0.1", () => console.log(`relay on http://127.0.0.1:${port}`));

// Exposed for tests that want to look inside: GET /__dump lists what's stored.
export { env };
