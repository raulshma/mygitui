import { defineConfig } from "@playwright/test";

/**
 * Playwright smoke config (M12) — SPA-level tests against the Vite dev
 * server with a mocked `window.__TAURI_INTERNALS__` (see e2e/tauri-mock.ts).
 * Chromium only: the smoke tests exercise app wiring, not cross-browser
 * rendering.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:1420",
  },
  webServer: {
    command: "npm run dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
