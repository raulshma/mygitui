/**
 * Actions panel vertical scroll e2e (regression):
 * the panel packs detected projects, custom actions, the add form and
 * recent runs into one column, so with enough defs it outgrows its tab
 * body. The section itself is the scroll container (overflow-y auto,
 * constrained height) — before the fix the overflow was clipped and
 * unreachable. Assertions stay on bounding boxes; the Tauri IPC is
 * mocked (e2e/tauri-mock.ts).
 */

import { expect, test } from "@playwright/test";
import {
  installTauriMock,
  openPanel,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

const DEF_COUNT = 14;

function seededDefs(): string {
  const defs = Array.from({ length: DEF_COUNT }, (_, i) => ({
    id: `def-${i + 1}`,
    name: `action-${i + 1}`,
    command: `echo ${i + 1}`,
    scope: "repo",
    repoRoot: "/tmp/repo",
  }));
  return JSON.stringify({ defs });
}

test("actions panel scrolls vertically when content overflows", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  await seedStorage(page, {
    ...recentRepos(["/tmp/repo"]),
    "mygitui.actions": seededDefs(),
  });
  await installTauriMock(page);
  await openRecentRepo(page);
  await openPanel(page, "Actions");

  const panel = page.locator(".actions-panel");
  await expect(panel).toBeVisible();
  await expect(page.locator(".def-list .def-row")).toHaveCount(DEF_COUNT);

  const scroll = await panel.evaluate((el) => ({
    overflowY: getComputedStyle(el).overflowY,
    clientHeight: el.clientHeight,
    scrollHeight: el.scrollHeight,
    scrollTop: el.scrollTop,
  }));
  // The tab body constrains the panel's height and the content is taller,
  // so the section itself must be the scroll container.
  expect(scroll.overflowY).toBe("auto");
  expect(scroll.clientHeight).toBeGreaterThan(0);
  expect(scroll.scrollHeight).toBeGreaterThan(scroll.clientHeight);
  expect(scroll.scrollTop).toBe(0);

  // The last def starts below the fold, and scrolling brings it into view.
  const belowFoldBefore = await panel.evaluate((el) => {
    const panelBox = el.getBoundingClientRect();
    const lastRow = el.querySelector<HTMLElement>(
      ".def-list .def-row:last-child",
    );
    return lastRow!.getBoundingClientRect().bottom > panelBox.bottom;
  });
  expect(belowFoldBefore).toBe(true);

  await panel.evaluate((el) => el.scrollTo({ top: el.scrollHeight }));
  await expect.poll(() => panel.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);

  const inViewAfter = await panel.evaluate((el) => {
    const panelBox = el.getBoundingClientRect();
    const lastRow = el.querySelector<HTMLElement>(
      ".def-list .def-row:last-child",
    );
    const rowBox = lastRow!.getBoundingClientRect();
    return rowBox.bottom <= panelBox.bottom + 1 && rowBox.top >= panelBox.top - 1;
  });
  expect(inViewAfter).toBe(true);

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
