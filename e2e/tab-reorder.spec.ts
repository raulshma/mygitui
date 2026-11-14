/**
 * Tab drag-to-reorder (e2e): the repo tabs in the header strip must reorder
 * by press-and-drag, including the "long press then drag" gesture (the user
 * presses, holds past the Hint dwell window, then drags).
 *
 * Regression guard for the user report "rearranging the repos from the
 * header by long press and drag is not working": Tauri's native drag-drop
 * handler used to swallow every in-page HTML5 drag event (fixed by
 * `dragDropEnabled: false`), and a drop on the near half of the target tab
 * is a deliberate no-op (insertion before the tab it is already before).
 *
 * Three gesture timings, so a regression localizes itself:
 *  1. fast drag       — press and move at once (no dwell pause)
 *  2. long-press drag — press, hold ~500ms (the Hint dwell swap window),
 *                       then drag
 *  3. post-dwell drag — hover until the pill is fully armed + swapped,
 *                       THEN long-press drag
 */

import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, trackErrors } from "./tauri-mock";

const RECENT_REPOS_KEY = "mygitui.recent-repos";
const SESSION_KEY = "mygitui.session";

const REPOS: Record<string, Record<string, unknown>> = {
  "/tmp/alpha": {
    repo_id: "ra",
    root: "/tmp/alpha",
    name: "alpha",
    bare: false,
    git_dir: "/tmp/alpha/.git",
  },
  "/tmp/beta": {
    repo_id: "rb",
    root: "/tmp/beta",
    name: "beta",
    bare: false,
    git_dir: "/tmp/beta/.git",
  },
};

/** Boot with two restored tabs, [alpha, beta]. */
async function bootTwoTabs(page: Page): Promise<void> {
  await page.addInitScript((seed) => {
    for (const [key, value] of Object.entries(seed)) {
      window.localStorage.setItem(key, value);
    }
  }, {
    [RECENT_REPOS_KEY]: JSON.stringify([
      { path: "/tmp/alpha", pinned: false, lastOpened: 1 },
      { path: "/tmp/beta", pinned: false, lastOpened: 2 },
    ]),
    [SESSION_KEY]: JSON.stringify({
      roots: ["/tmp/alpha", "/tmp/beta"],
      activeRoot: "/tmp/alpha",
    }),
  });
  // The shared mock answers repo_open with one fixed repo; intercept it
  // below (invoke wrapper) so both restored roots open as distinct tabs.
  await installTauriMock(page);
  await page.addInitScript((repos) => {
    const internals = window.__TAURI_INTERNALS__;
    const real = internals.invoke.bind(internals);
    internals.invoke = (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "repo_open") {
        const repo = repos[(args?.path as string) ?? ""] ?? repos["/tmp/alpha"];
        return Promise.resolve(repo);
      }
      return real(cmd, args);
    };
  }, REPOS);

  await page.goto("/");
  await expect(page.getByRole("tab", { name: "alpha — /tmp/alpha" })).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.getByRole("tab", { name: "beta — /tmp/beta" })).toBeVisible();
  // The gesture computes drop coordinates from label geometry, and the
  // side test in TabStrip compares them against the label's LIVE midpoint.
  // Tab labels first lay out in the fallback font; Inter Variable (served
  // by the dev server on demand) swaps in shortly after the first paint
  // and re-metrics every label. A drag started before the swap aims at
  // stale geometry: under CI load the swap can land mid-gesture, putting
  // the recorded drop point on the "before" side of the moved midpoint —
  // the deliberate no-op. Fonts settled ⇒ label metrics final.
  await page.evaluate(() => document.fonts.ready);
}

/** The visible tab order, as rendered in the strip. */
async function tabOrder(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll("button[role='tab'] .name")).map(
      (el) => el.textContent ?? "",
    ),
  );
}

/**
 * The user gesture: press on the "from" tab label, optionally hold ("long
 * press"), drag to the "to" tab, release.
 */
async function dragTab(
  page: Page,
  fromLabel: RegExp,
  toLabel: RegExp,
  opts: { holdBeforeMove?: number; dropFraction?: number } = {},
): Promise<void> {
  const holdBeforeMove = opts.holdBeforeMove ?? 0;
  // Drop on the far half of the target: the insertion side is the hovered
  // tab's midpoint, so a drop left of it ("before") can be a genuine no-op.
  const dropFraction = opts.dropFraction ?? 0.9;
  const from = page.getByRole("tab", { name: fromLabel });
  const to = page.getByRole("tab", { name: toLabel });
  const fromBox = (await from.boundingBox())!;
  const toBox = (await to.boundingBox())!;
  const startX = fromBox.x + fromBox.width / 2;
  const startY = fromBox.y + fromBox.height / 2;
  const aim = (box: { x: number; y: number; width: number; height: number }) => ({
    x: box.x + box.width * dropFraction,
    y: box.y + box.height / 2,
  });
  const end = aim(toBox);

  await page.mouse.move(startX, startY);
  await page.mouse.down();
  if (holdBeforeMove > 0) await page.waitForTimeout(holdBeforeMove);
  // Several moves: Chromium's native drag needs a few px of travel, and
  // dragover must land on the target for the insertion bar + drop.
  const steps = 12;
  const approach = 9;
  for (let i = 1; i <= approach; i++) {
    await page.mouse.move(
      startX + ((end.x - startX) * i) / steps,
      startY + ((end.y - startY) * i) / steps,
    );
  }
  // Re-aim the final stretch at the target's LIVE box: the drop side is
  // decided against the label's geometry at dragover time, so coordinates
  // captured before the press go stale if the strip re-lays out mid-gesture
  // (a loaded CI runner stretches the gesture, and a hold adds ~500ms).
  const live = aim((await to.boundingBox()) ?? toBox);
  const lastX = startX + ((end.x - startX) * approach) / steps;
  const lastY = startY + ((end.y - startY) * approach) / steps;
  const rest = steps - approach;
  for (let i = 1; i <= rest; i++) {
    await page.mouse.move(
      lastX + ((live.x - lastX) * i) / rest,
      lastY + ((live.y - lastY) * i) / rest,
    );
  }
  await page.waitForTimeout(50);
  await page.mouse.up();
}

test("fast press-drag reorders tabs", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootTwoTabs(page);
  expect(await tabOrder(page)).toEqual(["alpha", "beta"]);

  await dragTab(page, /alpha — /, /beta — /);
  await expect.poll(() => tabOrder(page)).toEqual(["beta", "alpha"]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("dropping on the target label's center reorders tabs", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootTwoTabs(page);

  // The natural gesture: release right on the next tab's label. The tab
  // CELL extends over the close button, so the side test must use the
  // label's midpoint — a cell-midpoint test made this exact drop a silent
  // no-op (insert before the tab the dragged tab is already before).
  //
  // Just past the label's center rather than dead on it: the side test is
  // `clientX >= midpoint` and drag-event coordinates are floored, so the
  // exact center of an odd-width label lands on the "before" side — the
  // no-op. 0.6 is still left of the CELL's midpoint, so a cell-midpoint
  // regression keeps failing here.
  await dragTab(page, /alpha — /, /beta — /, { dropFraction: 0.6 });
  await expect.poll(() => tabOrder(page)).toEqual(["beta", "alpha"]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("long-press drag (hold past the dwell) reorders tabs", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootTwoTabs(page);

  // Hover first (dwell timer runs out during the hold), press, HOLD ~500ms
  // — past the ~300ms Hint dwell — then drag. The user's "long press and
  // drag".
  await dragTab(page, /alpha — /, /beta — /, { holdBeforeMove: 500 });
  await expect.poll(() => tabOrder(page)).toEqual(["beta", "alpha"]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});

test("long-press drag after the pill fully armed reorders tabs", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await bootTwoTabs(page);

  // Let the dwell complete and the idle→armed swap settle BEFORE pressing.
  const alpha = page.getByRole("tab", { name: "alpha — /tmp/alpha" });
  await alpha.hover();
  await expect
    .poll(() =>
      page.evaluate(() =>
        Array.from(
          document.querySelectorAll('[data-slot="tooltip-content"]'),
        ).some((el) => el.getAttribute("data-state") !== "closed"),
      ),
    )
    .toBe(true);
  await page.waitForTimeout(300);

  await dragTab(page, /alpha — /, /beta — /, { holdBeforeMove: 500 });
  await expect.poll(() => tabOrder(page)).toEqual(["beta", "alpha"]);

  expect(tracked.errors, tracked.errors.join("\n")).toEqual([]);
});
