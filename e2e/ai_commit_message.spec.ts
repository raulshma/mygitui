/**
 * AI commit-message button e2e (commit-bar sparkle):
 *   1. first use → the opt-in confirm dialog opens and is FULLY inside the
 *      viewport (regression: it used to open downward from the bottom-docked
 *      commit bar and rendered off-screen — only a sliver at the window edge),
 *   2. opted-in, opencode down → the failure surfaces as a visible toast and
 *      an inline fail box that is fully inside the viewport.
 *
 * Assertions stay on visible DOM + bounding boxes; the Tauri IPC is mocked
 * (e2e/tauri-mock.ts).
 */

import { expect, test, type Page } from "@playwright/test";
import {
  installTauriMock,
  recentRepos,
  seedStorage,
  type TauriMockHandle,
} from "./tauri-mock";

const AI_KEY = "mygitui.ai";

const STAGED_DIFF: unknown[] = [
  {
    path: "src/app.ts",
    old_path: null,
    binary: false,
    is_image: false,
    additions: 2,
    deletions: 1,
    hunks: [
      {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 2,
        header: "@@",
        lines: [
          { origin: " ", text: "const x = 1;" },
          { origin: "+", text: "const y = 2;" },
        ],
      },
    ],
  },
];

async function boot(
  page: Page,
  seedEntries: Record<string, string>,
  commands: Record<string, unknown> = {},
): Promise<TauriMockHandle> {
  await seedStorage(page, seedEntries);
  return installTauriMock(page, { commands });
}

async function openRepo(page: Page): Promise<void> {
  await page.goto("/");
  const repoCard = page.getByRole("button", { name: /repo\s*\/tmp\/repo/ });
  await expect(repoCard).toBeVisible();
  await repoCard.click();
  await expect(page.getByRole("treeitem", { name: "src/app.ts" }).first()).toBeVisible();
}

/** Fails when `box` extends past the viewport (clipped / off-screen). */
async function expectFullyInViewport(
  page: Page,
  locator: Parameters<Page["locator"]>[0],
): Promise<void> {
  const box = await page.locator(locator).boundingBox();
  expect(box, `${locator} has a bounding box`).not.toBeNull();
  const vp = page.viewportSize() ?? { width: 0, height: 0 };
  expect(box!.y, `${locator} top edge on screen`).toBeGreaterThanOrEqual(0);
  expect(box!.x, `${locator} left edge on screen`).toBeGreaterThanOrEqual(0);
  expect(box!.y + box!.height, `${locator} bottom edge on screen`).toBeLessThanOrEqual(vp.height);
  expect(box!.x + box!.width, `${locator} right edge on screen`).toBeLessThanOrEqual(vp.width);
}

test("first use: opt-in dialog opens fully inside the viewport", async ({ page }) => {
  await boot(page, {
    ...recentRepos(["/tmp/repo"]),
  });
  await openRepo(page);

  await page
    .getByRole("button", { name: "Generate commit message with AI" })
    .click();

  const dialog = page.getByRole("dialog", { name: "Allow AI for this repository" });
  await expect(dialog).toBeVisible();
  await expectFullyInViewport(page, '[role="dialog"][aria-label="Allow AI for this repository"]');
});

test("opted-in, opencode down: failure toast + inline fail box visible", async ({ page }) => {
  await boot(
    page,
    {
      ...recentRepos(["/tmp/repo"]),
      [AI_KEY]: JSON.stringify({
        backend: "opencode",
        allowFallback: true,
        repoOptIn: { r1: true },
      }),
    },
    {
      // Managed mode on a machine without the opencode CLI.
      opencode_serve_status: { running: false, url: null, error: "opencode CLI not found" },
      opencode_serve_start: { running: false, url: null, error: "opencode CLI not found" },
      repo_diff: STAGED_DIFF,
    },
  );
  await openRepo(page);

  await page
    .getByRole("button", { name: "Generate commit message with AI" })
    .click();

  const toast = page.locator(".toaster .toast");
  await expect(toast).toContainText("AI commit message failed", { timeout: 30_000 });

  const fail = page.locator(".ai-commit .fail");
  await expect(fail).toBeVisible();
  await expectFullyInViewport(page, ".ai-commit .fail");
});
