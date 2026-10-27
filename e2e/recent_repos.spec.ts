/**
 * Playwright: recent-repositories home screen —
 *   1. removing a repo (row ✕) drops it from the list and localStorage,
 *   2. "Add existing repo…" opens the folder-picker dialog.
 *
 * Same SPA mock harness as smoke.spec.ts.
 */

import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  RECENT_REPOS_KEY,
  SESSION_KEY,
  seedStorage,
  trackErrors,
  type TauriMockHandle,
} from "./tauri-mock";

/** Boot the app with the Tauri mock (and optional pre-boot page setup). */
async function boot(
  page: Page,
  seedEntries: Record<string, string> = {},
): Promise<TauriMockHandle> {
  await seedStorage(page, seedEntries);
  return installTauriMock(page);
}

test("remove drops the repo from the recent list and storage", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/alpha", pinned: false, lastOpened: 1 },
      { path: "/tmp/beta", pinned: false, lastOpened: 2 },
    ]),
  });

  await page.goto("/");

  // Both seeded cards are listed; the card's accessible name starts with
  // the repo name ("alpha /tmp/alpha") — the remove button's label starts
  // with "Remove", so this regex only matches the card.
  const alphaCard = page.getByRole("button", { name: /^alpha\s/ });
  await expect(alphaCard).toBeVisible();
  await expect(page.getByRole("button", { name: /^beta\s/ })).toBeVisible();

  await page
    .getByRole("button", { name: "Remove alpha from the recent list" })
    .click();

  // The row disappears, a toast confirms, and only beta persists.
  await expect(page.getByText("Removed alpha from recent repositories"))
    .toBeVisible();
  await expect(alphaCard).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^beta\s/ })).toBeVisible();

  const stored = await page.evaluate((key) =>
    window.localStorage.getItem(key), RECENT_REPOS_KEY);
  expect(JSON.parse(stored ?? "[]")).toEqual([
    { path: "/tmp/beta", pinned: false, lastOpened: 2 },
  ]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("launch restore opens the last repo without flashing recents", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const session = { roots: ["/tmp/repo"], activeRoot: "/tmp/repo" };
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
    [SESSION_KEY]: JSON.stringify(session),
  });

  await page.goto("/");

  // Straight into the restored workspace: the recents home never renders.
  await expect(page.getByRole("tab", { name: "repo — /tmp/repo" }))
    .toBeVisible();
  await expect(
    page.getByRole("treeitem", { name: "src/app.ts" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Recent repositories" }),
  ).toHaveCount(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("top-bar home opens recents above open tabs; back returns", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
    [SESSION_KEY]: JSON.stringify({
      roots: ["/tmp/repo"],
      activeRoot: "/tmp/repo",
    }),
  });

  await page.goto("/");
  const tab = page.getByRole("tab", { name: "repo — /tmp/repo" });
  await expect(tab).toBeVisible();

  await page.getByRole("button", { name: "Recent repositories" }).click();

  // The overlay lists recents while the tab strip stays put.
  await expect(
    page.getByRole("heading", { name: "Recent repositories" }),
  ).toBeVisible();
  await expect(tab).toBeVisible();
  await expect(
    page.getByRole("treeitem", { name: "src/app.ts" }),
  ).toHaveCount(0);

  // Clicking the open tab returns to its workspace without reopening it.
  await tab.click();
  await expect(
    page.getByRole("heading", { name: "Recent repositories" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("treeitem", { name: "src/app.ts" }),
  ).toBeVisible();
  expect((await page.evaluate(() =>
    window.localStorage.getItem("mygitui.session"))) ?? "")
    .toContain("/tmp/repo");

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("add-existing repo opens the folder picker", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page);

  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Recent repositories" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Add existing repo…" }).click();

  // plugin:dialog|open is the @tauri-apps/plugin-dialog folder picker.
  await expect
    .poll(async () => (await mock.callsOf("plugin:dialog|open")).length)
    .toBe(1);
  expect(await mock.callsOf("plugin:dialog|open")).toMatchObject([
    { options: { directory: true, multiple: false } },
  ]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});
