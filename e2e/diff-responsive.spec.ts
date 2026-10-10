/**
 * Diff viewer responsive layout (commit detail context): split halves share
 * the pane 50/50 and never grow a horizontal scrollbar; unified fills the
 * pane and scrolls only when the widest line genuinely overflows it.
 *
 * Asserted on DOM geometry (scrollWidth/clientWidth/rects) against the
 * mocked Tauri backend — see tauri-mock.ts and history_click.spec.ts for
 * the boot pattern.
 */

import { expect, test, type Page } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

const PARENT = "0000000000000000000000000000000000000001";
const SHA = "0000000000000000000000000000000000000002";

/** One-commit history whose detail diff carries `lineTexts` as added lines. */
async function bootRepoWithDiff(page: Page, lineTexts: string[]) {
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, {
    commands: historyCommands({
      commits: [
        {
          sha: SHA,
          parents: [PARENT],
          author: { name: "A", email: "a@t.co", time: 1700000000, offset_minutes: 0 },
          committer: { name: "A", email: "a@t.co", time: 1700000000, offset_minutes: 0 },
          message: "diff fixture\n",
          summary: "diff fixture",
          refs: ["HEAD -> main"],
        },
      ],
      rows: [{ sha: SHA, lane: 0, edges: [], lane_count: 1 }],
      diff: [
        {
          path: "src/file.ts",
          old_path: null,
          status: "modified",
          binary: false,
          additions: lineTexts.length,
          deletions: 1,
          hunks: [
            {
              old_start: 1,
              new_start: 1,
              lines: lineTexts.map((text, i) => ({
                old_no: i === 0 ? 1 : null,
                new_no: i === 0 ? 1 : i + 1,
                origin: i === 0 ? " " : "+",
                text,
                highlights: [],
              })),
            },
          ],
        },
      ],
    }),
  });
}

async function openCommitDiff(page: Page, firstLine: string) {
  await page.goto("/");
  await page.getByRole("button", { name: /repo\s*\/tmp\/repo/ }).click();
  await page.locator("#commit-row-0").click();
  await expect(page.locator(".diff-viewer .viewport")).toBeVisible();
  // Context rows render the line in both halves, so the first context line
  // matches twice — any one instance proves the diff landed.
  await expect(page.getByText(firstLine).first()).toBeVisible();
}

interface DiffMetrics {
  vpClient: number;
  vpScroll: number;
  contentWidth: number;
  rowWidth: number;
  halfWidths: number[];
}

async function diffMetrics(page: Page): Promise<DiffMetrics> {
  return page.evaluate(() => {
    const vp = document.querySelector(".diff-viewer .viewport") as HTMLElement;
    const content = document.querySelector(".diff-viewer .content") as HTMLElement;
    const row = document.querySelector(".diff-viewer .row.line") as HTMLElement | null;
    const halves = [...document.querySelectorAll(".diff-viewer .half")].map((h) =>
      Math.round(h.getBoundingClientRect().width),
    );
    return {
      vpClient: vp.clientWidth,
      vpScroll: vp.scrollWidth,
      contentWidth: Math.round(content.getBoundingClientRect().width),
      rowWidth: row ? Math.round(row.getBoundingClientRect().width) : 0,
      halfWidths: halves,
    };
  });
}

test("split view shares the pane and never scrolls horizontally", async ({ page }) => {
  const tracked = trackErrors(page);
  const long = "x".repeat(110);
  await bootRepoWithDiff(page, ["context start", long, long + "+tail", "\tindented line"]);
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "context start");

  // Wide window: halves fill the row 50/50, no horizontal overflow.
  let m = await diffMetrics(page);
  expect(m.vpScroll).toBeLessThanOrEqual(m.vpClient);
  expect(m.rowWidth).toBe(m.vpClient);
  expect(m.halfWidths.length).toBeGreaterThanOrEqual(2);
  expect(Math.abs(m.halfWidths[0]! - m.halfWidths[1]!)).toBeLessThanOrEqual(2);

  // Narrow window: still fits — the layout is responsive, not fixed-width.
  await page.setViewportSize({ width: 900, height: 800 });
  m = await diffMetrics(page);
  expect(m.vpScroll).toBeLessThanOrEqual(m.vpClient);
  expect(m.rowWidth).toBe(m.vpClient);

  expect(tracked.errors).toEqual([]);
});

test("split view keeps context text inside its own half", async ({ page }) => {
  const tracked = trackErrors(page);
  // A context line far wider than the pane: it must be clipped at the
  // divider in each half (rendered per half), never spill from the left
  // section into the right one.
  const long = "unchanged ".repeat(30).trim();
  await bootRepoWithDiff(page, [long, "second line"]);
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, long);

  const laidOut = await page.evaluate(() => {
    const rows = [...document.querySelectorAll<HTMLElement>(".diff-viewer .row.context")];
    return rows.every((row) => {
      const halves = [...row.querySelectorAll<HTMLElement>(".half")];
      if (halves.length !== 2) return false;
      const [left, right] = halves.map((h) => h.getBoundingClientRect());
      // Divider edge shared, and each half's text box stays inside it.
      const divider = Math.round(left.right);
      return (
        Math.abs(divider - Math.round(right.left)) <= 1 &&
        [...halves].every((half, i) => {
          const box = half.getBoundingClientRect();
          const txt = half.querySelector<HTMLElement>(".txt")!.getBoundingClientRect();
          return i === 0
            ? txt.right <= box.right + 1
            : txt.left >= box.left - 1;
        })
      );
    });
  });
  expect(laidOut).toBe(true);

  expect(tracked.errors).toEqual([]);
});

test("unified view fills the pane when lines are short", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootRepoWithDiff(page, ["short context line", "a change", "another"]);
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "short context line");

  await page.getByRole("button", { name: "Switch to unified view" }).click();
  await expect(page.getByRole("button", { name: "Switch to split view" })).toBeVisible();

  const m = await diffMetrics(page);
  expect(m.vpScroll).toBeLessThanOrEqual(m.vpClient);
  expect(m.contentWidth).toBe(m.vpClient);
  expect(m.rowWidth).toBe(m.vpClient);

  expect(tracked.errors).toEqual([]);
});

test("unified view scrolls only when the widest line genuinely overflows", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootRepoWithDiff(page, ["context start", "y".repeat(110)]);
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "context start");

  await page.getByRole("button", { name: "Switch to unified view" }).click();
  await expect(page.getByRole("button", { name: "Switch to split view" })).toBeVisible();

  // The 110-char line really needs more than the pane: the viewer may grow
  // a scrollbar, and content/rows must stretch across the full scroll
  // extent (no dead space, no early-cut backgrounds).
  const m = await diffMetrics(page);
  expect(m.contentWidth).toBeGreaterThan(m.vpClient);
  expect(m.vpScroll).toBeGreaterThan(m.vpClient);
  expect(m.contentWidth).toBe(m.vpScroll);
  expect(m.rowWidth).toBe(m.vpScroll);

  expect(tracked.errors).toEqual([]);
});
