import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [solid()],

  // Tauri ships modern webviews (WebView2 / WKWebView / WebKitGTK), so we can
  // target ES2021 and skip legacy transpilation and modulepreload polyfills.
  build: {
    target: "es2021",
    // Tauri webviews understand modern CSS; do not downlevel it for size.
    cssTarget: false,
    minify: "oxc",
    cssMinify: "lightningcss",
    sourcemap: false,
    modulePreload: { polyfill: false },
  },

  // Pre-bundle the hot dependencies once so dev startup does not discover them
  // lazily, and warm the entry files for faster first paint in dev.
  optimizeDeps: {
    include: [
      "solid-js",
      "solid-js/web",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
    ],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    warmup: {
      clientFiles: ["./src/index.tsx", "./src/App.tsx"],
    },
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
