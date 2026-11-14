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

test("row hover actions copy the commit hash", async ({ page }) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(10);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, {
    commands: historyCommands({ commits, rows }),
  });
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);

  await openRecentRepo(page);
  const row1 = page.locator("#commit-row-1");
  await expect(row1).toBeVisible();

  // Hover reveals the action cluster (VSCode-style); the chip shows the
  // short hash and copies the full one.
  await row1.hover();
  const copy = row1.locator(".sha-copy");
  await expect(copy).toBeVisible();
  await expect(copy).toHaveText(commits[1].sha.slice(0, 7));
  await copy.click();

  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toBe(commits[1].sha);
  expect(tracked.errors).toEqual([]);
});

test("checkout commit action confirms and invokes the backend", async ({ page }) => {
  const tracked = trackErrors(page);
  const { commits, rows } = linearLog(10);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mock = await installTauriMock(page, {
    commands: {
      ...historyCommands({ commits, rows }),
      // Pin HEAD at commit 0 so row 0 hides the checkout action and row 1
      // offers it (default mock head matches no fixture sha).
      repo_status: {
        branch: "main",
        head: commits[0].sha,
        detached: false,
        ahead: 0,
        behind: 0,
        merging: false,
        rebasing: false,
        sequencer: false,
        bisecting: false,
        entries: [],
      },
    },
  });

  await openRecentRepo(page);
  const row0 = page.locator("#commit-row-0");
  const row1 = page.locator("#commit-row-1");
  await expect(row0).toBeVisible();

  // HEAD commit carries no checkout affordance.
  await row0.hover();
  await expect(row0.locator(".checkout-action")).toHaveCount(0);

  await row1.hover();
  await row1.locator(".checkout-action").click();

  // Confirm dialog (clean worktree override → plain "Checkout" label).
  const dialog = page.getByRole("dialog", { name: `Checkout ${commits[1].sha.slice(0, 7)}` });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText(commits[1].sha.slice(0, 7));
  await dialog.getByRole("button", { name: "Checkout", exact: true }).click();

  // Guard checkpoint first (the worktree is forced), then the checkout.
  const guards = await mock.callsOf("guard_checkpoint");
  expect(guards.length).toBe(1);
  expect(guards[0].reason).toBe("pre-checkout");

  const checkouts = await mock.callsOf("commit_checkout");
  expect(checkouts.length).toBe(1);
  expect(checkouts[0].sha).toBe(commits[1].sha);

  expect(tracked.errors).toEqual([]);
});
