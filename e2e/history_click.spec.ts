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

test("clicking commit in history shows commit detail and maintains scroll", async ({ page }) => {
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

  // Scroll down to commit 10
  const row10 = page.locator("#commit-row-10");
  await row10.scrollIntoViewIfNeeded();

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
  const { commits, rows } = linearLog(1);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, {
    commands: historyCommands({ commits, rows }),
  });

  await openRecentRepo(page);

  await expect(page.locator(".history .row").first()).toBeVisible();

  // Click on the canvas node (lane 0 is at x=10, y=12)
  const canvas = page.locator(".gc-canvas");
  await canvas.click({ position: { x: 10, y: 12 } });

  const detail = page.locator(".detail");
  await expect(detail).toBeVisible();
  await expect(page.locator(".dsummary").first()).toHaveText("commit message 0");

  expect(tracked.errors).toEqual([]);
});
