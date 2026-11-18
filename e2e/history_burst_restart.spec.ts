import { expect, test } from "@playwright/test";
import {
  installTauriMock,
  linearLog,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
  type CommandMock,
} from "./tauri-mock";

/**
 * The history store under a MID-STREAM `repo-changed` burst (a HEAD move
 * landing while a paginated log walk is still in flight — what a concurrent
 * commit/push produces). The contract: the significant event restarts the
 * stream immediately, the aborted walk's leftover pages are ignored (token +
 * generation gates), the previous log stays rendered until the fresh walk's
 * first page swaps in atomically (no blank flash), and the list settles on
 * exactly the fresh log. The mock's `pageGapMs` keeps each walk slow enough
 * to land the burst deterministically between pages; the restarted walk is
 * served via `setCommand` (init-script options are captured per invoke), so
 * only the new walk reads it.
 */

/** Chunks a log into pages of `size` commits (next_cursor chains, gen `generation`). */
function logPages(
  log: { commits: Array<Record<string, unknown>>; rows: Array<Record<string, unknown>> },
  size: number,
  generation: number,
): Array<Record<string, unknown>> {
  const pages = [];
  for (let i = 0; i < log.commits.length; i += size) {
    const slice = log.commits.slice(i, i + size);
    pages.push({
      commits: slice,
      rows: log.rows.slice(i, i + size),
      next_cursor: i + size < log.commits.length ? slice[slice.length - 1]!.sha : null,
      generation,
    });
  }
  return pages;
}

/** Prefixes a log's summaries/messages so old vs fresh rows are tellable. */
function reworded(
  log: { commits: Array<Record<string, unknown>>; rows: Array<Record<string, unknown>> },
  prefix: string,
): { commits: Array<Record<string, unknown>>; rows: Array<Record<string, unknown>> } {
  return {
    commits: log.commits.map((commit) => ({
      ...commit,
      message: `${prefix} ${commit.message}`,
      summary: `${prefix} ${commit.summary}`,
    })),
    rows: log.rows,
  };
}

/** A head_moved watcher payload (registry: docs/contracts.md). */
function headMoved(): CommandMock {
  return { repo_id: "r1", paths: [], head_moved: true, full: false };
}

test("a mid-stream head_moved burst restarts the walk once and lands the fresh rows", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  const first = linearLog(300);
  const fresh = reworded(linearLog(300), "fresh");

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      repo_log_stream: { pages: logPages(first, 100, 1), pageGapMs: 400 },
    },
  });

  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();

  // The burst must hit MID-walk: the stream is still running when it lands.
  const status = page.locator(".history .status[role='status']");
  const streamsBefore = (await mock.callsOf("repo_log_stream")).length;
  expect(streamsBefore).toBe(1);
  await expect(status).toContainText("Streaming");

  // Swap in the fresh walk (the in-flight one captured its own pages), then
  // move HEAD behind the backend's back. The restarted walk carries the
  // bumped generation, so the aborted walk's leftover pages are stale twice
  // over (token AND generation).
  await mock.setCommand("repo_log_stream", {
    pages: logPages(fresh, 100, 2),
    pageGapMs: 300,
  });
  await mock.emit("repo-changed", headMoved());
  await expect
    .poll(async () => (await mock.callsOf("repo_log_stream")).length)
    .toBe(streamsBefore + 1);

  // The fresh log replaces the old one — the aborted walk contributes
  // nothing (no doubled rows, no 600-commit mix).
  await expect(page.locator(".history")).toContainText("fresh commit message 0");
  await expect(status).toContainText("300", { timeout: 10_000 });
  await expect(status).not.toContainText("Streaming");
  // The last fresh row exists beyond the render window: scroll to it.
  const scroller = page.locator(".history .scroller");
  await scroller.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await expect(page.locator("#commit-row-299")).toBeVisible();
  await expect(page.locator(".history")).toContainText("fresh commit message 299");

  // Quiescence: exactly one restart, no abort→restart loop afterwards.
  await page.waitForTimeout(1_500);
  expect((await mock.callsOf("repo_log_stream")).length).toBe(streamsBefore + 1);
  expect(tracked.errors).toEqual([]);
});

test("rows from the aborted walk stay rendered until the fresh walk's first page", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  const first = linearLog(300);
  const fresh = reworded(linearLog(300), "fresh");

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      repo_log_stream: { pages: logPages(first, 100, 1), pageGapMs: 400 },
    },
  });

  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();

  // The restarted walk's first page is held back for 1.2s: the whole stale-
  // render window is observable from the test side.
  await mock.setCommand("repo_log_stream", {
    pages: logPages(fresh, 100, 2),
    pageGapMs: 1_200,
  });
  const streamsBefore = (await mock.callsOf("repo_log_stream")).length;
  await mock.emit("repo-changed", headMoved());
  await expect
    .poll(async () => (await mock.callsOf("repo_log_stream")).length)
    .toBe(streamsBefore + 1);

  // Mid-restart: the list still shows the aborted walk's rows (never blanked
  // into a spinner) and none of the fresh content has landed yet.
  expect((await page.locator(".history .row").count()) > 0).toBe(true);
  const midText = (await page.locator(".history").textContent()) ?? "";
  expect(midText).toContain("commit message 0");
  expect(midText).not.toContain("fresh commit message");
  await expect(page.locator(".history .status[role='status']")).toContainText(
    "Streaming",
  );

  // The fresh walk then swaps in atomically and settles as complete.
  await expect(page.locator(".history")).toContainText("fresh commit message 0");
  await expect(page.locator(".history .status[role='status']")).toContainText("300");
  await expect(page.locator(".history .status[role='status']")).not.toContainText(
    "Streaming",
  );

  await page.waitForTimeout(1_000);
  expect((await mock.callsOf("repo_log_stream")).length).toBe(streamsBefore + 1);
  expect(tracked.errors).toEqual([]);
});
