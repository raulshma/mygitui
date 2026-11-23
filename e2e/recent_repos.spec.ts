/**
 * Playwright: the Overview library —
 *   1. "Remove from library" (row context menu) drops the repo from the
 *      list and localStorage (an open tab stays),
 *   2. the launch restore still opens the last repo without flashing the
 *      Overview,
 *   3. the tab-strip home button shows the Overview over open tabs and a
 *      tab click returns,
 *   4. a delayed tab close preserves a newer repository selection,
 *   5. "Add folder…" opens the folder-picker dialog.
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

test("remove from library drops the repo from the list and storage", async ({
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

  // Both seeded rows are listed; the row header carries the repo name.
  const alphaRow = page.getByRole("article", { name: "Repository alpha" });
  await expect(alphaRow).toBeVisible();
  await expect(page.getByRole("article", { name: "Repository beta" })).toBeVisible();

  await alphaRow
    .getByRole("button", { name: "Toggle diffs for alpha" })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Remove from library" }).click();

  // The row disappears, a toast confirms, and only beta persists.
  await expect(page.getByText("Removed alpha from the library")).toBeVisible();
  await expect(alphaRow).toHaveCount(0);
  await expect(page.getByRole("article", { name: "Repository beta" })).toBeVisible();

  const stored = await page.evaluate((key) =>
    window.localStorage.getItem(key), RECENT_REPOS_KEY);
  expect(JSON.parse(stored ?? "[]")).toEqual([
    { path: "/tmp/beta", pinned: false, lastOpened: 2 },
  ]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("launch restore opens the last repo without flashing the Overview", async ({
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

  // Straight into the restored workspace: the Overview never renders.
  await expect(page.getByRole("tab", { name: "repo — /tmp/repo" }))
    .toBeVisible();
  await expect(
    page.getByRole("treeitem", { name: "src/app.ts" }),
  ).toBeVisible();
  await expect(page.locator(".overview")).toHaveCount(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("the tab-strip home shows the Overview above open tabs; a tab click returns", async ({
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

  await page.locator(".tab-home").click();

  // The Overview lists the library while the tab strip stays put.
  await expect(page.getByRole("article", { name: "Repository repo" })).toBeVisible();
  await expect(tab).toBeVisible();
  await expect(
    page.getByRole("treeitem", { name: "src/app.ts" }),
  ).toHaveCount(0);

  // Clicking the open tab returns to its workspace without reopening it.
  await tab.click();
  await expect(page.getByRole("article", { name: "Repository repo" })).toHaveCount(0);
  await expect(
    page.getByRole("treeitem", { name: "src/app.ts" }),
  ).toBeVisible();
  expect((await page.evaluate(() =>
    window.localStorage.getItem("mygitui.session"))) ?? "")
    .toContain("/tmp/repo");

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("a delayed tab close preserves a newer repository selection", async ({ page }) => {
  const tracked = trackErrors(page);
  await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify(["alpha", "beta", "gamma"].map((name) => ({
      path: `/tmp/${name}`, pinned: false, lastOpened: 1,
    }))),
  });
  await page.goto("/");
  await expect(page.getByRole("article", { name: "Repository alpha" })).toBeVisible();
  await page.evaluate(() => {
    const runtime = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
      finishTabClose?: () => void;
    });
    const invoke = runtime.__TAURI_INTERNALS__.invoke;
    runtime.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === "repo_open") {
        const info = await invoke(cmd, args) as Record<string, unknown>;
        const root = String(args?.path);
        return { ...info, root, repo_id: root, name: root.split("/").pop() };
      }
      if (cmd === "repo_close") {
        await new Promise<void>((resolve) => { runtime.finishTabClose = resolve; });
      }
      return invoke(cmd, args);
    };
  });

  for (const name of ["alpha", "beta", "gamma"]) {
    if (name !== "alpha") {
      await page.locator(".tab-home").click();
    }
    await page.getByRole("button", { name: `Open ${name}` }).click();
    // Not exact: the tab's accessible name carries the change-count badge
    // ("alpha — /tmp/alpha, 2 changed") once the status lands.
    await expect(page.getByRole("tab", { name: `${name} — /tmp/${name}` })).toBeVisible();
  }
  await page.getByRole("button", { name: "Close gamma", exact: true }).click();
  const alpha = page.getByRole("tab", { name: "alpha — /tmp/alpha" });
  await alpha.click();
  await page.evaluate(() => (window as unknown as { finishTabClose: () => void }).finishTabClose());
  await expect(page.getByRole("tab", { name: "gamma — /tmp/gamma" })).toHaveCount(0);
  await expect(alpha).toHaveAttribute("aria-selected", "true");
  expect(tracked.errors).toEqual([]);
});

test("Add folder… opens the folder picker", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page);

  await page.goto("/");
  await expect(page.locator(".overview-top")).toBeVisible();

  await page.locator(".overview-top").getByRole("button", { name: /Add folder/ }).click();

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
