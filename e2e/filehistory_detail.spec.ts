import { expect, test } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  trackErrors,
} from "./tauri-mock";

/**
 * FileHistoryView popout (`?panel=filehistory`): clicking a commit shows
 * that commit's diff (CommitDetail); shift-clicking a consecutive range
 * shows the aggregate diff (oldest^..newest) as the compare tab.
 */

const sha = (prefix: string) => (prefix + prefix.repeat(20)).slice(0, 40);

// Newest-first walk, like the file history list.
const COMMITS = [
  { prefix: "aaaa111", parent: "bbbb222" },
  { prefix: "bbbb222", parent: "cccc333" },
  { prefix: "cccc333", parent: "dddd444" },
  { prefix: "dddd444", parent: null },
].map(({ prefix, parent }, i) => ({
  sha: sha(prefix),
  parents: parent ? [sha(parent)] : [],
  author: { name: `Author ${i}`, email: `a${i}@test.com`, time: 1700000000 + i, offset_minutes: 0 },
  committer: { name: `Author ${i}`, email: `a${i}@test.com`, time: 1700000000 + i, offset_minutes: 0 },
  message: `touch file ${prefix}`,
  summary: `touch file ${prefix}`,
  refs: [],
}));

const DIFF_FILES = [
  {
    path: "src/app.ts",
    old_path: null,
    status: "modified",
    binary: false,
    additions: 10,
    deletions: 2,
    hunks: [],
  },
  {
    path: "src/lib.ts",
    old_path: null,
    status: "modified",
    binary: false,
    additions: 1,
    deletions: 1,
    hunks: [],
  },
];

async function openPopout(page: import("@playwright/test").Page): Promise<void> {
  await installTauriMock(page, {
    commands: historyCommands({
      commits: COMMITS,
      rows: COMMITS.map((c) => ({ sha: c.sha, lane: 0, edges: [], lane_count: 1 })),
      diff: DIFF_FILES,
      signature: { state: "none" },
    }),
  });
  await page.goto("/?panel=filehistory&repo=mock-repo&path=src%2Fapp.ts");
  await expect(page.locator(".file-history .row").first()).toBeVisible();
}

test("clicking a commit shows its diff in the detail pane", async ({ page }) => {
  const tracked = trackErrors(page);
  await openPopout(page);

  const first = page.locator(".file-history .row").first();
  await first.click();

  // Detail opens below the list, showing the clicked commit's own diff.
  const detail = page.locator(".detail");
  await expect(detail).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("touch file aaaa111");
  await expect(page.locator(".filelist")).toBeVisible();
  await expect(page.locator(".filelist").getByText("src/app.ts")).toBeVisible();

  // No compare tab in single-commit mode.
  await expect(page.locator(".dtab")).toHaveCount(0);

  expect(tracked.errors).toEqual([]);
});

test("shift-click selects a consecutive range and shows the aggregate diff", async ({ page }) => {
  const tracked = trackErrors(page);
  await openPopout(page);

  await page.locator(".file-history .row").first().click();
  await page.locator(".file-history .row").nth(2).click({ modifiers: ["Shift"] });

  // Range rows 0..2 are highlighted; the clicked row is the primary selection.
  await expect(page.locator(".file-history .row.selected")).toHaveCount(1);
  await expect(page.locator(".file-history .row.multisel")).toHaveCount(2);

  // The detail header shows the range as the compare tab: oldest^ → newest.
  const tab = page.locator(".dtab span").first();
  await expect(tab).toContainText(
    `Compare: ${"cccc333".slice(0, 7)}^ → ${"aaaa111".slice(0, 7)} (2 files)`,
  );
  // Compare mode renders the aggregate straight into the diff body.
  await expect(page.locator(".dtabbody")).toBeVisible();

  // CommitDetail reports the selection size.
  await expect(page.locator(".cinfo")).toContainText("3 commits selected");

  // Closing the compare tab falls back to the clicked commit's own diff.
  await page.locator("button[aria-label='Close compare']").click();
  await expect(page.locator(".dtab")).toHaveCount(0);
  await expect(page.locator(".dsummary").first()).toHaveText("touch file cccc333");
  await expect(page.locator(".filelist")).toBeVisible();

  expect(tracked.errors).toEqual([]);
});

test("plain click after a range collapses back to a single commit", async ({ page }) => {
  const tracked = trackErrors(page);
  await openPopout(page);

  await page.locator(".file-history .row").first().click();
  await page.locator(".file-history .row").nth(2).click({ modifiers: ["Shift"] });
  await expect(page.locator(".file-history .row.multisel")).toHaveCount(2);

  await page.locator(".file-history .row").nth(3).click();
  await expect(page.locator(".file-history .row.multisel")).toHaveCount(0);
  await expect(page.locator(".dsummary").first()).toHaveText("touch file dddd444");

  expect(tracked.errors).toEqual([]);
});
