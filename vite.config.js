import { svelte } from "@sveltejs/vite-plugin-svelte";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],

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
    include: ["src/**/*.{test,spec}.{js,ts}"],
    server: {
      deps: {
        // @material/material-color-utilities@0.4.0 ships extensionless ESM
        // relative imports ("../dynamiccolor/dynamic_scheme") that Node's
        // loader rejects when the package is externalized; inlining it lets
        // Vite resolve them. Applies to all suites (added by entry UX lane).
        inline: ["@material/material-color-utilities"],
      },
    },
  },
});
