import { expect, test } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";

/**
 * Layout-stability regression: selecting a tab must not change any tab's
 * geometry. A selected-state `font-weight` bump widens the selected label,
 * resizing that tab and shifting every sibling tab — a visible twitch.
 */
test("selecting a panel tab keeps tab geometry stable", async ({ page }) => {
  const tracked = trackErrors(page);

  const sha = "0000000000000000000000000000000000000001";

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
      repo_log_stream: {
        commits: [
          {
            sha,
            parents: [],
            author: { name: "Alice", email: "a@test.com", time: 1700000000, offset_minutes: 0 },
            committer: { name: "Alice", email: "a@test.com", time: 1700000000, offset_minutes: 0 },
            message: "first commit",
            summary: "first commit",
            refs: ["HEAD -> main"],
          },
        ],
        rows: [{ sha, lane: 0, edges: [], lane_count: 1 }],
        next_cursor: null,
        generation: 1,
      },
    },
  });

  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();

  const tabs = page.locator(".panel-tab");
  await expect(tabs.first()).toBeVisible();
  expect(await tabs.count()).toBeGreaterThanOrEqual(2);

  const boxes = () =>
    tabs.evaluateAll((els) =>
      els.map((el) => {
        const { x, y, width, height } = el.getBoundingClientRect();
        // Round to skip subpixel noise that carries no layout meaning.
        return {
          x: Math.round(x * 100) / 100,
          y: Math.round(y * 100) / 100,
          width: Math.round(width * 100) / 100,
          height: Math.round(height * 100) / 100,
        };
      }),
    );

  const before = await boxes();
  await tabs.nth(1).click();
  await expect(tabs.nth(1)).toHaveAttribute("aria-selected", "true");
  // Let the panel swap settle a frame before re-measuring the strip.
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => resolve(null))),
  );
  const after = await boxes();

  // Horizontal geometry must be bit-identical: a label that widens on
  // selection shifts every following tab.
  expect(after.map(({ x, width }) => ({ x, width }))).toEqual(
    before.map(({ x, width }) => ({ x, width })),
  );
  // Vertical: the selected tab intentionally extends 1px into the strip's
  // bottom border ("merge with body"); nothing may move by more than that.
  after.forEach((box, i) => {
    expect(box.y).toBe(before[i].y);
    expect(Math.abs(box.height - before[i].height)).toBeLessThanOrEqual(1);
  });

  expect(tracked.errors).toEqual([]);
});
