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
  // Popouts boot behind a dynamic `import()`; on a cold Vite dev server the
  // first parallel workers can wait several seconds for on-demand compiles.
  // 5s (the default) flakes there; the app itself renders as soon as the
  // chunk arrives.
  //
  // Other assertions are timing-sensitive in the same load-dependent way:
  // waiting for the app to boot ("More panels" visible) can outgrow the 30s
  // default timeout on a cold/loaded server, and repo-switch-smoothness's
  // rAF opacity sampler (>10 samples in 700ms) under-samples on a busy
  // machine. Repeat runs showed every test passing somewhere with failures
  // differing per run — load flakiness, not deterministic breaks. Retries
  // absorb those outliers (more in CI) without hiding real regressions:
  // a deterministic break fails on every retry.
  expect: { timeout: 10_000 },
  retries: process.env.CI ? 2 : 1,
  // A stray `test.only` must not silently skip the rest of the suite in CI.
  forbidOnly: !!process.env.CI,
  webServer: {
    command: "npm run dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
