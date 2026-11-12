/**
 * Hint pill dismissal + overflow (e2e): the canonical tooltip must not
 * stick to the screen or bleed past its own surface.
 *
 * Regressions covered (user report: "tooltips sometimes stay open; in
 * status files the tooltip goes out of the section and has transparent
 * background"):
 *   1. exactly one pill open app-wide while hovering list rows (nested
 *      row/path/badge triggers must not stack pills),
 *   2. the pill closes when the wheel-scrolls move rows away (a pill used
 *      to travel with its row / stay glued under a stationary pointer),
 *   3. scrolling does not re-glue a pill via the browser's post-scroll
 *      hover recompute,
 *   4. a long unbreakable path wraps INSIDE the pill (its text used to
 *      spill past the pill's background over neighboring panels),
 *   5. the pill is never the hit-test target (`pointer-events: none`) —
 *      rows beneath it stay hoverable.
 *
 * Asserted on live DOM geometry/state against the mocked Tauri backend
 * (see tauri-mock.ts and smoke.spec.ts for the boot pattern).
 */

import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";
/** Deep directory of the seeded changes — long unbreakable path tokens. */
const DEEP_DIR = "app/src/main/java/com/raulshma/lenscast/streaming";
const rowPath = (n: number) => `${DEEP_DIR}/EncodedStreamHub${n}.kt`;

const entries = Array.from({ length: 200 }, (_, i) => ({
  path: rowPath(i),
  old_path: null,
  index: "unmodified",
  worktree: "modified",
}));

/** Live state of every open tooltip pill in the page. */
interface PillState {
  state: string | null;
  scrollW: number;
  clientW: number;
  pointerEvents: string;
}

async function pillState(page: Page): Promise<PillState[]> {
  return page.evaluate(() =>
    Array.from(
      document.querySelectorAll('[data-slot="tooltip-content"]'),
    ).map((el) => {
      const style = getComputedStyle(el);
      return {
        state: el.getAttribute("data-state"),
        scrollW: el.scrollWidth,
        clientW: el.clientWidth,
        pointerEvents: style.pointerEvents,
      };
    }),
  );
}

async function boot(page: Page): Promise<void> {
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
      repo_status: {
        branch: "main",
        head: "0123456789abcdef0123456789abcdef01234567",
        detached: false,
        ahead: 0,
        behind: 0,
        merging: false,
        rebasing: false,
        sequencer: false,
        entries,
      },
    },
  });
  await page.goto("/");
}

test("hint pill: single open pill, closes on scroll, wraps long paths", async ({
  page,
}, testInfo) => {
  test.setTimeout(90_000); // cold dev-server compiles the 200-row panel
  const tracked = trackErrors(page);
  await boot(page);

  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();
  const row10 = page.getByRole("treeitem", { name: rowPath(10) });
  await expect(row10).toBeVisible({ timeout: 20_000 });

  // 1. Dwell on a row: exactly one pill opens.
  await row10.hover();
  await expect
    .poll(() => pillState(page), { timeout: 5_000 })
    .toHaveLength(1);
  const open = await pillState(page);
  // bits-ui labels controlled-open pills "delayed-open" or "instant-open"
  // depending on its provider grace path; both mean the pill is showing.
  expect(["delayed-open", "instant-open"]).toContain(open[0]!.state);

  // 4. The long path wraps inside the pill — no horizontal spill past the
  //    pill's own background (it used to overflow onto the diff panel).
  expect(open[0]!.scrollW).toBeLessThanOrEqual(open[0]!.clientW);

  // 5. The pill is never the hit-test target: rows beneath stay hoverable.
  expect(open[0]!.pointerEvents).toBe("none");

  // 2. Move to the neighboring row: still exactly one pill — the previous
  //    one closed instead of stacking.
  const row11 = page.getByRole("treeitem", { name: rowPath(11) });
  await row11.hover();
  await expect
    .poll(() => pillState(page), { timeout: 5_000 })
    .toHaveLength(1);

  // 3. Wheel-scroll: the pill closes and does NOT re-glue via the
  //    post-scroll hover recompute (pointer stays stationary).
  await page.mouse.wheel(0, 160);
  await expect
    .poll(() => pillState(page), { timeout: 5_000 })
    .toHaveLength(0);
  await page.waitForTimeout(600);
  expect(await pillState(page)).toHaveLength(0);

  // Moving the pointer again onto a row shows its pill once more.
  const rowAgain = page.getByRole("treeitem", { name: rowPath(5) });
  await expect(rowAgain).toBeVisible({ timeout: 20_000 });
  await rowAgain.hover();
  await expect
    .poll(() => pillState(page), { timeout: 5_000 })
    .toHaveLength(1);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});
