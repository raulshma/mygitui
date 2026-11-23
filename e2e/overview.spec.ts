/**
 * Playwright: the Overview — the zero-tabs landing that replaced the recents
 * home.
 *   1. launch → Overview visible with seeded recents (scan tier fills rows),
 *   2. expanding a row warms a session and mounts the inline diffs,
 *   3. filter presets narrow the list (persisted),
 *   4. the Open action switches into the workspace,
 *   5. Esc returns to the Overview; the toggle returns to the workspace.
 *
 * Same SPA mock harness as smoke.spec.ts; `repo_summary` is synthesized per
 * path by tauri-mock.ts (see fakeRepoSummary for the path conventions).
 */

import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  RECENT_REPOS_KEY,
  seedStorage,
  trackErrors,
  type TauriMockHandle,
} from "./tauri-mock";

/** Seeds three recents with different shapes, then boots the app. */
async function boot(
  page: Page,
  paths: string[] = ["/tmp/repo", "/tmp/dirty-one", "/tmp/ahead-one"],
): Promise<TauriMockHandle> {
  await seedStorage(page, {
    [RECENT_REPOS_KEY]: JSON.stringify(
      paths.map((path, i) => ({ path, pinned: false, lastOpened: i + 1 })),
    ),
  });
  return installTauriMock(page);
}

test("launch lands on the Overview with the seeded recents scanned", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page);

  await page.goto("/");

  // The strip is there; the three rows render with their scan data.
  await expect(page.locator(".overview-top")).toContainText("Overview");
  await expect(page.getByRole("article", { name: "Repository repo" })).toBeVisible();
  await expect(page.getByRole("article", { name: "Repository dirty-one" })).toBeVisible();
  await expect(page.getByRole("article", { name: "Repository ahead-one" })).toBeVisible();

  // Scan tier data: branch + last commit + counts from repo_summary.
  await expect(
    page.getByRole("article", { name: "Repository repo" }),
  ).toContainText("main");
  await expect(
    page.getByRole("article", { name: "Repository repo" }),
  ).toContainText("last commit of repo");

  // Dirty row shows its grouped files; ahead row shows the sync count.
  await expect(
    page.getByRole("article", { name: "Repository dirty-one" }),
  ).toContainText("dirty.ts");
  await expect(
    page.getByRole("article", { name: "Repository ahead-one" }),
  ).toContainText("↑2");

  // Totals in the strip: 3 repos, ahead sum 2.
  await expect(page.locator(".overview-top")).toContainText("3 repos");
  await expect(page.locator(".overview-top")).toContainText("↑2");

  // The scans went through the new command (3 rows → 3 calls).
  await expect.poll(async () => (await mock.callsOf("repo_summary")).length).toBe(3);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("expanding a row warms a session and mounts the inline diff", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page, ["/tmp/repo"]);

  await page.goto("/");
  const row = page.getByRole("article", { name: "Repository repo" });
  await expect(row).toBeVisible();

  // Clicking the row header expands the diffs (and warms a real session).
  await row.getByRole("button", { name: "Toggle diffs for repo" }).click();

  await expect
    .poll(async () => (await mock.callsOf("repo_open")).length)
    .toBe(1);
  expect((await mock.callsOf("repo_open"))[0]).toMatchObject({ path: "/tmp/repo" });

  // The diff pane streams the first changed file (index→worktree; the
  // summary and live tiers agree on this repo's entries, so exactly one
  // stream lands).
  await expect
    .poll(async () => (await mock.callsOf("repo_diff_stream")).length)
    .toBe(1);
  expect(await mock.callsOf("repo_diff_stream")).toMatchObject([
    { repo_id: "r1", old: "index", new: "worktree", query: { paths: ["src/app.ts"] } },
  ]);
  await expect(row).toContainText("src/");
  await expect(row).toContainText("unstaged");

  // Collapsing again keeps the warmed session (it lives for the Overview).
  await row.getByRole("button", { name: "Toggle diffs for repo" }).click();
  expect((await mock.callsOf("repo_close")).length).toBe(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("leaving the Overview closes warmed sessions (tabs keep theirs)", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page, ["/tmp/repo", "/tmp/dirty-one"]);

  await page.goto("/");
  await expect(page.getByRole("article", { name: "Repository repo" })).toBeVisible();

  // Open one repo as a genuine tab, then return to the Overview over it.
  await page.getByRole("button", { name: "Open repo" }).click();
  await expect(page.getByRole("tab", { name: "repo — /tmp/repo" })).toBeVisible();
  await page.locator(".overview-toggle").click();
  await expect(page.getByRole("article", { name: "Repository dirty-one" })).toBeVisible();

  // Expanding the non-open dirty row warms a real session (the tab's own
  // open was the first repo_open; the warm is the second).
  await page
    .getByRole("article", { name: "Repository dirty-one" })
    .getByRole("button", { name: "Toggle diffs for dirty-one" })
    .click();
  await expect.poll(async () => (await mock.callsOf("repo_open")).length).toBe(2);
  expect(await mock.callsOf("repo_open")).toMatchObject([
    { path: "/tmp/repo" },
    { path: "/tmp/dirty-one" },
  ]);

  // Leaving the Overview closes exactly the warmed session; the tab's
  // session survives (repo_close fires once, for dirty-one).
  await page.getByRole("button", { name: /Back to/ }).click();
  await expect(page.locator(".overview")).toHaveCount(0);
  await expect.poll(async () => (await mock.callsOf("repo_close")).length).toBe(1);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("filter presets narrow the list and persist", async ({ page }, testInfo) => {
  const tracked = trackErrors(page);
  await boot(page, ["/tmp/repo", "/tmp/dirty-one", "/tmp/ahead-one"]);

  await page.goto("/");
  await expect(page.getByRole("article", { name: "Repository ahead-one" })).toBeVisible();

  await page.getByRole("button", { name: "Needs push" }).click();

  // Only the ahead-one row survives the preset.
  await expect(page.getByRole("article", { name: "Repository ahead-one" })).toBeVisible();
  await expect(page.getByRole("article", { name: "Repository repo" })).toHaveCount(0);
  await expect(page.getByRole("article", { name: "Repository dirty-one" })).toHaveCount(0);

  // The active preset persists across reloads (envelope store).
  await page.reload();
  await expect(page.getByRole("button", { name: "Needs push" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByRole("article", { name: "Repository ahead-one" })).toBeVisible();
  await expect(page.getByRole("article", { name: "Repository repo" })).toHaveCount(0);

  // Clearing the filter restores the list.
  await page.getByRole("button", { name: "Needs push" }).click();
  await expect(page.getByRole("article", { name: "Repository repo" })).toBeVisible();

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("missing repos render dimmed with a Remove action", async ({ page }, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page, ["/tmp/repo", "/tmp/missing-one"]);

  await page.goto("/");
  const row = page.getByRole("article", { name: "Repository missing-one" });
  await expect(row).toBeVisible();
  await expect(row).toContainText("missing");
  await expect(row.getByRole("button", { name: "Open missing-one" })).toHaveCount(0);

  // Remove from library via the row's context menu.
  await row
    .getByRole("button", { name: "Toggle diffs for missing-one" })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Remove from library" }).click();
  await expect(page.getByRole("article", { name: "Repository missing-one" })).toHaveCount(0);

  // Dropped from the recents store, too.
  const stored = await page.evaluate(
    (key) => window.localStorage.getItem(key),
    RECENT_REPOS_KEY,
  );
  expect(stored ?? "").not.toContain("missing-one");
  expect((await mock.callsOf("repo_close")).length).toBe(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("the Open action switches into the workspace; Esc returns", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  await boot(page, ["/tmp/repo"]);

  await page.goto("/");
  await expect(page.getByRole("article", { name: "Repository repo" })).toBeVisible();

  await page.getByRole("button", { name: "Open repo" }).click();

  // The workspace took over: tab strip + the repo view's file tree.
  await expect(page.getByRole("tab", { name: "repo — /tmp/repo" })).toBeVisible();
  await expect(page.getByRole("treeitem", { name: "src/app.ts" })).toBeVisible();
  await expect(page.locator(".overview")).toHaveCount(0);

  // The header toggle shows the Overview again over the open tab.
  await page.locator(".overview-toggle").click();
  await expect(page.getByRole("article", { name: "Repository repo" })).toBeVisible();

  // Esc goes back to the workspace (a tab exists to return to).
  await page.keyboard.press("Escape");
  await expect(page.getByRole("treeitem", { name: "src/app.ts" })).toBeVisible();

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});

test("Add folder… validates via the scan and registers without opening a tab", async ({
  page,
}, testInfo) => {
  const tracked = trackErrors(page);
  const mock = await boot(page, []);

  // plugin:dialog|open resolves a folder path.
  await page.addInitScript(() => {
    const internals = window.__TAURI_INTERNALS__ as unknown as {
      invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
    };
    const invoke = internals.invoke;
    window.__TAURI_INTERNALS__ = {
      ...internals,
      invoke: (cmd, args) => {
        if (cmd === "plugin:dialog|open") {
          return Promise.resolve("/tmp/fresh-repo");
        }
        return invoke(cmd, args);
      },
    } as typeof window.__TAURI_INTERNALS__;
  });

  await page.goto("/");
  await page.locator(".overview-top").getByRole("button", { name: /Add folder/ }).click();

  // Validated through repo_summary, registered in recents, no tab opened.
  await expect
    .poll(async () => (await mock.callsOf("repo_summary")).length)
    .toBeGreaterThanOrEqual(1);
  expect(await mock.callsOf("repo_summary")).toMatchObject([{ path: "/tmp/fresh-repo" }]);
  await expect(page.getByRole("article", { name: "Repository fresh-repo" })).toBeVisible();
  await expect(page.getByRole("tab", { name: /fresh-repo/ })).toHaveCount(0);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
  await tracked.attach(testInfo);
});
