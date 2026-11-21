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
 * A multi-page log must settle and STAY settled — including while scrolling.
 * Tauri delivers channel payloads over 8 KB through an async fetch
 * round-trip, so `repo_log_stream` resolves while the last page is still in
 * flight; the store used to read that resolve as a truncated walk and
 * restart forever (the eternal "Streaming…" spinner + reloading list).
 * Modeled with `resolveBeforeLastPage` and a 2500-commit / 5-page walk.
 */
test("paginated log settles despite late final page and survives scrolling", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(2500);
  const pages = [];
  for (let i = 0; i < commits.length; i += 500) {
    const slice = commits.slice(i, i + 500);
    pages.push({
      commits: slice,
      rows: rows.slice(i, i + 500),
      next_cursor:
        i + 500 < commits.length ? slice[slice.length - 1].sha : null,
      generation: 1,
    });
  }

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      ...historyCommands({ commits, rows }),
      repo_log_stream: { pages, resolveBeforeLastPage: true },
    },
  });

  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();
  const status = page.locator(".history .status[role='status']");
  await expect(status).not.toContainText("Streaming", { timeout: 10_000 });
  expect((await mock.callsOf("repo_log_stream")).length).toBe(1);

  // Scroll to the bottom in jumps: each release boundary must hand over the
  // next buffered chunk without re-streaming the log.
  const scroller = page.locator(".history .scroller");
  for (let step = 0; step < 10; step++) {
    await scroller.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await page.waitForTimeout(150);
    await expect(status).not.toContainText("Streaming");
  }
  // Workstream D §8.1 formats the settled count with a thousands separator
  // ("2,500 commits"); the buffered-pages detail moved into a tooltip.
  await expect(status).toContainText("2,500", { timeout: 10_000 });
  await expect(page.locator("#commit-row-2499")).toBeVisible();

  // Quiescence: no abort→restart loop after the walk exhausted.
  await page.waitForTimeout(1_500);
  expect((await mock.callsOf("repo_log_stream")).length).toBe(1);
  await expect(status).not.toContainText("Streaming");
  expect(tracked.errors).toEqual([]);
});
