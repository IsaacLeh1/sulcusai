// SPDX-License-Identifier: AGPL-3.0-only
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

// Chunks that import each other can run one before the other has set up what
// it needs: 0.9.48's window stayed blank that way. Such a build fails here.
function noChunkLoops(): Plugin {
  return {
    name: "no-chunk-loops",
    generateBundle(_, bundle) {
      const imports = new Map<string, string[]>();
      for (const out of Object.values(bundle)) if (out.type === "chunk") imports.set(out.fileName, out.imports);
      const done = new Set<string>();
      const path: string[] = [];
      const visit = (name: string): string[] | null => {
        if (path.includes(name)) return [...path.slice(path.indexOf(name)), name];
        if (done.has(name)) return null;
        path.push(name);
        for (const next of imports.get(name) ?? []) {
          const loop = visit(next);
          if (loop) return loop;
        }
        path.pop();
        done.add(name);
        return null;
      };
      for (const name of imports.keys()) {
        const loop = visit(name);
        if (loop) this.error(`These chunks import each other in a loop: ${loop.join(" -> ")}`);
      }
    },
  };
}

// Tauri expects a fixed dev port and must not clear its console output.
export default defineConfig({
  plugins: [react(), noChunkLoops()],
  clearScreen: false,
  // Files are found relative to the page, so the phone's copy works under
  // the relay's /l/<link>/ as well as at the PC's own root.
  base: "./",
  // The Rust side rebuilds itself; watching its build folder only causes noise.
  server: { port: 1430, strictPort: true, host: "127.0.0.1", watch: { ignored: ["**/src-tauri/**"] } },
  build: {
    target: "es2022",
    outDir: "dist",
    // The PC's window, and the same app for a paired phone.
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        phone: fileURLToPath(new URL("./phone.html", import.meta.url)),
      },
    },
  },
});
