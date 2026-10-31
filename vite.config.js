import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [tailwindcss(), svelte()],

  resolve: {
    alias: {
      $lib: fileURLToPath(new URL("./src/lib", import.meta.url)),
    },
    // Component tests mount real components (@testing-library/svelte);
    // under vitest, Svelte must resolve to its client build or mount()
    // hits the server runtime ("mount(...) is not available on the
    // server"). Builds and the dev server are unaffected.
    ...(process.env.VITEST ? { conditions: ["browser"] } : {}),
  },

  // The Material Symbols icon set is consumed through deep default imports
  // (`@ktibow/iconset-material-symbols/<name>`), each a tiny dependency the
  // dev optimizer would otherwise discover lazily — re-optimizing mid
  // session and 504-ing already-loaded pages ("Outdated Optimize Dep").
  // Serving them raw keeps the dev server graph stable; they have no deps.
  optimizeDeps: {
    exclude: ["@ktibow/iconset-material-symbols"],
    // Lazily imported deps are discovered only when their panel/AI provider
    // first mounts. Without pre-bundling, Vite re-optimizes mid-session and
    // forces a full page reload — the boot splash flashes over the app when
    // opening Terminal, GitHub, Actions, or AI for the first time. Listing
    // them here keeps the dev graph stable and those opens instant.
    include: [
      "@codemirror/autocomplete",
      "@codemirror/commands",
      "@codemirror/language",
      "@codemirror/language-data",
      "@codemirror/search",
      "@codemirror/state",
      "@codemirror/view",
      "@lezer/highlight",
      "@opencode-ai/sdk/client",
      "@opencode/client",
      "@openrouter/ai-sdk-provider",
      "@tauri-apps/plugin-deep-link",
      "@tauri-apps/plugin-dialog",
      "@tauri-apps/plugin-opener",
      "@xterm/addon-fit",
      "@xterm/addon-web-links",
      "@xterm/xterm",
      "ai",
      "codemirror",
      "dompurify",
      "markdown-it",
      "shiki",
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
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  test: {
    environment: "jsdom",
    // Vitest creates one jsdom environment per suite; on Windows that setup
    // dominates the run, and component tests that take ~1s in isolation can
    // exceed the 5s default while dozens of suites start in parallel. The
    // timeout only caps failures, so passing tests are unaffected.
    testTimeout: 20_000,
    include: ["src/**/*.{test,spec}.{js,ts}"],
    setupFiles: ["./src/test-setup.ts"],
    server: {
      deps: {},
    },
  },
});
