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

/**
 * A background stream restart must not throw a scrolled user back to the
 * top. Every restart (watcher event, abort→restart, filter edit) wipes the
 * store's pages — the list spacer collapses and scrollTop clamps to 0 — so
 * the view re-anchors the viewport on the pre-restart top row's commit once
 * the re-walked log delivers it. Modeled here with a `repo-changed` watcher
 * event (what autofetch / a HEAD move / a remote-poll resync produce); the
 * mock serves the same log, so the position must come back exactly.
 */
test("history scroll survives a background stream restart", async ({ page }) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(200);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: historyCommands({ commits, rows }),
  });

  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();

  // Scroll deep into the virtualized list: rows beyond the render window
  // are not in the DOM, so scroll the container itself. ROW_HEIGHT is 28.
  const scroller = page.locator(".history .scroller");
  await scroller.evaluate((el) => {
    el.scrollTop = 120 * 28;
  });
  await expect(page.locator("#commit-row-120")).toBeVisible();
  const scrollTopBefore = await scroller.evaluate((el) => el.scrollTop);
  expect(scrollTopBefore).toBeGreaterThan(0);

  // A significant background change (HEAD moved) restarts the stream.
  const streamsBefore = (await mock.callsOf("repo_log_stream")).length;
  await mock.emit("repo-changed", {
    repo_id: "r1",
    paths: [],
    head_moved: true,
    full: false,
  });
  await expect
    .poll(async () => (await mock.callsOf("repo_log_stream")).length)
    .toBe(streamsBefore + 1);

  // …and the viewport is back on the same rows instead of the top.
  await expect(page.locator("#commit-row-120")).toBeVisible();
  const scrollTopAfter = await scroller.evaluate((el) => el.scrollTop);
  expect(Math.abs(scrollTopAfter - scrollTopBefore)).toBeLessThanOrEqual(2);

  expect(tracked.errors).toEqual([]);
});

/**
 * The restore must not fight a genuinely different log: an anchor commit
 * the re-walked stream never delivers (here: a text filter excluding it)
 * drops the restore, and the filtered list simply opens at its top.
 */
test("history restart with a filter change does not restore a dropped anchor", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  const full = linearLog(200);
  const filtered = linearLog(5);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: historyCommands({ commits: full.commits, rows: full.rows }),
  });

  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();

  const scroller = page.locator(".history .scroller");
  await scroller.evaluate((el) => {
    el.scrollTop = 120 * 28;
  });
  await expect(page.locator("#commit-row-120")).toBeVisible();

  // Narrow the log to commits that exclude the anchor row's commit (the
  // mock never filters — the restarted stream just serves this page).
  await mock.setCommand("repo_log_stream", {
    commits: filtered.commits,
    rows: filtered.rows,
    next_cursor: null,
    generation: 1,
  });
  const filter = page.locator('.filterbar input[aria-label="Search commit text"]');
  await filter.fill("commit message 3");

  // The filtered stream lands with the anchor gone: the list stays at its
  // top (no phantom jump to a row the new log does not have).
  await expect(page.locator(".history .row")).toHaveCount(5);
  await expect(page.locator("#commit-row-0")).toBeVisible();
  const scrollTop = await scroller.evaluate((el) => el.scrollTop);
  expect(scrollTop).toBeLessThanOrEqual(2);

  expect(tracked.errors).toEqual([]);
});
