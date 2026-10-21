// SPDX-License-Identifier: AGPL-3.0-only
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and must not clear its console output.
export default defineConfig({
  plugins: [react()],
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
