import { expect, test } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  linearLog,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

/** Per-repo layout overlay key: prefix + repo root. */
const OVERLAY_KEY = "mygitui.layouts.overlay./tmp/repo";

/**
 * TabGroup keep-alive mounting: switching panel tabs must not unmount the
 * hidden panel. The history panel keeps its selection, open commit detail,
 * scroll position and loaded stream across a switch to Status and back —
 * no re-stream, no scroll reset (what cross-restart prefs can't restore).
 *
 * The default layout puts history in a right-hand leaf, so the test seeds
 * a per-repo overlay that places history inside a tab group next to
 * Status — the arrangement "New group to the right" produces.
 */
test("switching panel tabs keeps history state alive", async ({ page }) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(50);

  await seedStorage(page, {
    ...recentRepos(["/tmp/repo"]),
    [OVERLAY_KEY]: JSON.stringify({
      treeOverride: {
        kind: "split",
        id: "split-main",
        ratio: 0.35,
        a: {
          kind: "tabs",
          id: "tabs-keepalive",
          tabs: ["history", "status"],
          active: 0,
        },
        b: { kind: "leaf", id: "leaf-reflog", panel: "reflog" },
      },
    }),
  });

  const mock = await installTauriMock(page, {
    commands: historyCommands({
      commits,
      rows,
      diff: [
        {
          path: "src/file1.ts",
          old_path: null,
          status: "modified",
          binary: false,
          additions: 10,
          deletions: 2,
          hunks: [],
        },
      ],
    }),
  });

  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();

  // Select commit 10 — the detail pane opens and the list moves into the
  // detail SplitPane.
  await page.locator("#commit-row-10").click();
  await expect(page.locator(".detail")).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("commit message 10");

  // Scroll well past the selection: the scroll position is state that
  // cross-restart prefs cannot restore — only keep-alive can.
  await page.locator("#commit-row-30").scrollIntoViewIfNeeded();
  const scroller = page.locator(".history .scroller");
  const scrollTopBefore = await scroller.evaluate((el) => el.scrollTop);
  expect(scrollTopBefore).toBeGreaterThan(0);

  const streamsBefore = (await mock.callsOf("repo_log_stream")).length;

  // Switch to Status: history must hide, not unmount.
  await page.getByRole("tab", { name: "Status" }).click();
  const historyWhileHidden = page.locator(".history");
  await expect(historyWhileHidden).toHaveCount(1);
  await expect(historyWhileHidden).not.toBeVisible();
  expect((await mock.callsOf("repo_log_stream")).length).toBe(streamsBefore);

  // Switch back: everything is exactly where we left it.
  await page.getByRole("tab", { name: "History" }).click();
  await expect(page.locator(".history")).toBeVisible();
  await expect(page.locator(".detail")).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("commit message 10");
  await expect(page.locator("#commit-row-10")).toHaveClass(/selected/);
  const scrollTopAfter = await scroller.evaluate((el) => el.scrollTop);
  expect(Math.abs(scrollTopAfter - scrollTopBefore)).toBeLessThanOrEqual(2);

  expect((await mock.callsOf("repo_log_stream")).length).toBe(streamsBefore);
  expect(tracked.errors).toEqual([]);
});
