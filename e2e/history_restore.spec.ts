import { expect, test } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";
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

  const mockCommits = Array.from({ length: 50 }, (_, i) => ({
    sha: i.toString(16).padStart(40, "0"),
    parents: i > 0 ? [(i - 1).toString(16).padStart(40, "0")] : [],
    author: {
      name: `Author ${i}`,
      email: `a${i}@test.com`,
      time: 1700000000 + i,
      offset_minutes: 0,
    },
    committer: {
      name: `Author ${i}`,
      email: `a${i}@test.com`,
      time: 1700000000 + i,
      offset_minutes: 0,
    },
    message: `commit message ${i}\n\nFull description here`,
    summary: `commit message ${i}`,
    refs: i === 0 ? ["HEAD -> main"] : [],
  }));
  const mockRows = mockCommits.map((c, i) => ({
    sha: c.sha,
    lane: 0,
    edges: i < 49 ? [{ from: 0, to: 0 }] : [],
    lane_count: 1,
  }));

  await page.addInitScript((seed) => {
    for (const [key, value] of Object.entries(seed)) {
      window.localStorage.setItem(key, value);
    }
  }, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/repo", pinned: false, lastOpened: 1 },
    ]),
  });

  await installTauriMock(page, {
    commands: {
      // The mock ignores filters — the restored filter re-streams the same
      // log, keeping row indices stable across the reload.
      repo_log_stream: {
        commits: mockCommits,
        rows: mockRows,
        next_cursor: null,
        generation: 1,
      },
      repo_diff: [
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
      describe: "v1.0.0",
      commit_signature: { state: "valid" },
    },
  });

  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();
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
