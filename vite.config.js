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

  build: {
    // Tauri ships its own evergreen webview (WebView2 / WKWebView /
    // WebKitGTK): esnext output skips esbuild's downleveling — smaller
    // chunks, less parse work at boot, faster builds.
    target: "esnext",
    sourcemap: false,
  },

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
    // jsdom costs ~13s per environment on Windows. The default `forks` pool
    // builds one per test file (98 of them); `vmThreads` builds one per worker
    // and resets globals between files, keeping per-file isolation while
    // cutting setup from two thirds of the run to a rounding error.
    pool: "vmThreads",
    // Component tests that take ~1s in isolation can exceed the 5s default
    // while dozens of suites start in parallel. The timeout only caps
    // failures, so passing tests are unaffected.
    testTimeout: 20_000,
    include: ["src/**/*.{test,spec}.{js,ts}"],
    setupFiles: ["./src/test-setup.ts"],
    server: {
      deps: {},
    },
    // Coverage (roadmap item 68): provider v8 over the app code. The include
    // is extension-scoped AND `include` must stay set (the honest all-files
    // scope): a bare `src/lib/**` would pull `src/lib/theme/styles/*.css`
    // into the remapper, which parses every included file as JS and dies on
    // CSS (PARSE_ERROR → truncated report). Note the measured baseline
    // differs by scope: with vitest's default (no `include`) only files
    // LOADED by tests count (74.5% lines); with `include` set, never-imported
    // modules count as 0% too, giving the honest all-files numbers the
    // thresholds below are derived from (lines 43.85%, statements 39.76% —
    // dominated by the untested UI components). Thresholds sit slightly
    // BELOW the measurement so CI stays green with headroom; tighten as
    // coverage grows. Deliberately NOT wired into CI in this pass — when
    // that happens, add `--coverage` to the workflow's test step (or a
    // dedicated job).
    coverage: {
      provider: "v8",
      include: ["src/lib/**/*.{js,ts,svelte}"],
      // Machine-generated ts-rs IPC bindings (type-only, zero runtime
      // statements) are report noise — hundreds of meaningless 0% rows.
      exclude: ["src/lib/ipc/gen/**"],
      thresholds: {
        lines: 42,
        statements: 38,
      },
    },
  },
});
