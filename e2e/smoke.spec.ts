/**
 * Playwright smoke (M12, contracts.md quality gates): SPA-level tests with
 * a mocked `window.__TAURI_INTERNALS__` (e2e/tauri-mock.ts) —
 *   1. app boots to the home / recent-repositories screen without console
 *      errors,
 *   2. open-repo flow: a pre-seeded recent repo opens as a tab, the status
 *      panel renders the 2 mocked entries, staging one records a `stage`
 *      invoke, and committing records a `commit` invoke.
 *
 * Assertions stay on visible DOM plus the mock's recorded invoke log.
 */

import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  trackErrors,
  type TauriMockHandle,
} from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";

/** Boot the app with the Tauri mock (and optional pre-boot page setup). */
async function boot(
  page: Page,
  seedStorage: Record<string, string> = {},
): Promise<TauriMockHandle> {
  await page.addInitScript((seed) => {
    for (const [key, value] of Object.entries(seed)) {
      window.localStorage.setItem(key, value);
    }
  }, seedStorage);
  return installTauriMock(page);
}

test("app boots to the home/recent screen without console errors", async ({
  page,
}, testInfo) => {
  const mock = await boot(page);
  const tracked = trackErrors(page);

  await page.goto("/");

  // Stable landmark: the home screen's recent-repositories heading.
  await expect(
    page.getByRole("heading", { name: "Recent repositories" }),
  ).toBeVisible();

  // The Tauri mock was consulted for first-launch argv (empty here).
  await expect
    .poll(() => mock.callsOf("cli_args_initial"))
    .toHaveLength(1);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("open-repo flow: recent card → status entries → stage → commit", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await page.goto("/");

  // 1. The seeded recent-repo card is listed on the home screen; click it.
  const repoCard = page.getByRole("button", { name: /repo\s*\/tmp\/repo/ });
  await expect(repoCard).toBeVisible();
  await repoCard.click();

  // 2. The tab opened: repo name + status entries render (rows are
  //    treeitems labelled with the full path; dir/base are split nodes).
  await expect(page.getByRole("treeitem", { name: "src/app.ts" })).toBeVisible();
  await expect(page.getByRole("treeitem", { name: "notes.md" })).toBeVisible();

  // The status came through the mocked command chain.
  await expect
    .poll(async () => (await mock.callsOf("repo_status")).length)
    .toBeGreaterThan(0);

  // 3. Stage the modified file via its row checkbox.
  await page.getByRole("checkbox", { name: "Stage src/app.ts" }).click();
  const stageCalls = await mock.callsOf("stage");
  expect(stageCalls).toHaveLength(1);
  expect(stageCalls[0]).toMatchObject({
    repo_id: "r1",
    request: { unstage: false, targets: [{ file: "src/app.ts" }] },
  });

  // 4. Commit: type a message and submit the commit bar form.
  await page.locator("#commit-message").fill("test: smoke commit");
  await page.getByRole("button", { name: "Commit", exact: true }).click();

  const commitCalls = await mock.callsOf("commit");
  expect(commitCalls).toHaveLength(1);
  expect(commitCalls[0]).toMatchObject({
    repo_id: "r1",
    options: { message: "test: smoke commit" },
  });

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});
