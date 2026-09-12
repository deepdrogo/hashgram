import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// Tauri sets TAURI_DEV_HOST when the dev server should be reachable from a
// device; on the desktop it is unset and the server stays on localhost.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  resolve: { alias: { "~": path.resolve(__dirname, "src") } },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // WebView2 on Windows 11 is evergreen Chromium.
    target: "chrome120",
    minify: "esbuild",
    sourcemap: false,
    cssMinify: true,
    // One vendor chunk, one app chunk: two requests at start, both cached
    // by the webview.
    rollupOptions: {
      output: {
        manualChunks: {
          vendor: ["solid-js", "solid-js/web", "@solidjs/router", "@tauri-apps/api/core"],
        },
      },
    },
  },
});
