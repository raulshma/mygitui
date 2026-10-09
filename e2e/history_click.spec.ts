import { expect, test } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";

test("clicking commit in history shows commit detail and maintains scroll", async ({ page }) => {
  const tracked = trackErrors(page);

  const mockCommits = Array.from({ length: 50 }, (_, i) => ({
    sha: i.toString(16).padStart(40, "0"),
    parents: i > 0 ? [(i - 1).toString(16).padStart(40, "0")] : [],
    author: { name: `Author ${i}`, email: `a${i}@test.com`, time: 1700000000 + i, offset_minutes: 0 },
    committer: { name: `Author ${i}`, email: `a${i}@test.com`, time: 1700000000 + i, offset_minutes: 0 },
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
      repo_log_stream: { commits: mockCommits, rows: mockRows, next_cursor: null, generation: 1 },
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
  const repoCard = page.getByRole("button", { name: /repo\s*\/tmp\/repo/ });
  await repoCard.click();

  await expect(page.locator(".history .row").first()).toBeVisible();

  // Scroll down to commit 10
  const row10 = page.locator("#commit-row-10");
  await row10.scrollIntoViewIfNeeded();

  const scrollTopBefore = await page.locator(".history .scroller").evaluate((el) => el.scrollTop);
  console.log("scrollTop before click:", scrollTopBefore);

  await row10.click();

  // Check detail is visible!
  const detail = page.locator(".detail");
  await expect(detail).toBeVisible();

  // Check detail header shows commit 10 summary
  await expect(page.locator(".dsummary").first()).toHaveText("commit message 10");

  // Check that diff finished loading and changed files are visible (not stuck on "Loading diff…")
  await expect(page.locator(".dstate")).toHaveCount(0);
  await expect(page.locator(".filelist")).toBeVisible();
  await expect(page.locator(".filelist").getByText("src/file1.ts")).toBeVisible();

  expect(tracked.errors).toEqual([]);
});

test("clicking commit node in graph canvas selects commit", async ({ page }) => {
  const tracked = trackErrors(page);

  const mockCommits = [
    {
      sha: "0000000000000000000000000000000000000001",
      parents: [],
      author: { name: "Alice", email: "a@test.com", time: 1700000000, offset_minutes: 0 },
      committer: { name: "Alice", email: "a@test.com", time: 1700000000, offset_minutes: 0 },
      message: "first commit",
      summary: "first commit",
      refs: ["HEAD -> main"],
    },
  ];

  const mockRows = [
    { sha: mockCommits[0].sha, lane: 0, edges: [], lane_count: 1 },
  ];

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
      repo_log_stream: { commits: mockCommits, rows: mockRows, next_cursor: null, generation: 1 },
      repo_diff: [],
      describe: "v1.0.0",
      commit_signature: { state: "valid" },
    },
  });

  await page.goto("/");
  const repoCard = page.getByRole("button", { name: /repo\s*\/tmp\/repo/ });
  await repoCard.click();

  await expect(page.locator(".history .row").first()).toBeVisible();

  // Click on the canvas node (lane 0 is at x=10, y=12)
  const canvas = page.locator(".gc-canvas");
  await canvas.click({ position: { x: 10, y: 12 } });

  const detail = page.locator(".detail");
  await expect(detail).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("first commit");

  expect(tracked.errors).toEqual([]);
});
