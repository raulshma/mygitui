import { expect, test } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";
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
    commands: {
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
