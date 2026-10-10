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

/** historyPrefs key: prefix + repo root (the persistence identity). */
const PREFS_KEY = "mygitui.history./tmp/repo";

/**
 * Per-repo history UI state survives an app restart (modeled as a page
 * reload): the selected commit, its open detail pane and the filter text
 * all come back (HistoryView hydrates them from `historyPrefs` before the
 * first log stream starts).
 */
test("history selection, detail pane and filter survive a reload", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(50);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, {
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

  // Type a filter (persisted on change, debounced 150 ms)…
  const filter = page.locator(
    '.filterbar input[aria-label="Search commit text"]',
  );
  await filter.fill("commit message 1");
  await expect
    .poll(() => page.evaluate((k) => localStorage.getItem(k), PREFS_KEY))
    .not.toBeNull();

  // …select commit 10 (detail opens)…
  await page.locator("#commit-row-10").click();
  await expect(page.locator(".detail")).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("commit message 10");

  // …wait for the debounced prefs write to carry the selection…
  await expect
    .poll(() =>
      page.evaluate((k) => {
        const raw = localStorage.getItem(k);
        if (raw === null) return null;
        try {
          return (JSON.parse(raw) as { selectedSha: string | null }).selectedSha;
        } catch {
          return null;
        }
      }, PREFS_KEY),
    )
    .toBe("000000000000000000000000000000000000000a");

  // …and restart the app (session restore reopens the repo tab).
  await page.reload();
  await expect(page.locator(".history .row").first()).toBeVisible({
    timeout: 15_000,
  });

  // Filter came back.
  await expect(filter).toHaveValue("commit message 1");
  // Selection came back: the row highlights and the detail reopened on it.
  await expect(page.locator("#commit-row-10")).toHaveClass(/selected/);
  await expect(page.locator(".detail")).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("commit message 10");
  await expect(page.locator(".row.selected")).toHaveCount(1);

  expect(tracked.errors).toEqual([]);
});
