import { expect, test, type Page } from "@playwright/test";
import {
  historyCommands,
  installTauriMock,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";

const PARENT = "0000000000000000000000000000000000000001";
const SHA = "0000000000000000000000000000000000000002";

interface MockLine {
  old_no: number | null;
  new_no: number | null;
  origin: string;
  text: string;
  highlights: never[];
}

/**
 * One-commit history whose detail diff has `lineCount` added lines across
 * several files — tall enough that the diff viewport must scroll. `pages`
 * > 1 streams the files as several `{ pages: [...] }` batches (with
 * `pageGapMs` pacing) like the real backend's multi-page diff streams.
 */
async function bootBigDiff(
  page: Page,
  lineCount = 400,
  fileCount = 6,
  pages = 1,
) {
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const mkFile = (f: number) => {
    const lines: MockLine[] = Array.from({ length: lineCount }, (_, i) => ({
      old_no: null,
      new_no: i + 1,
      origin: "+",
      text: `file ${f} line ${i} — const value = ${f * 1000 + i};`,
      highlights: [],
    }));
    return {
      path: `src/file${f}.ts`,
      old_path: null,
      status: "added",
      binary: false,
      additions: lineCount,
      deletions: 0,
      hunks: [{ old_start: 0, new_start: 1, lines }],
    };
  };
  const allFiles = Array.from({ length: fileCount }, (_, f) => mkFile(f));
  const batches: number = Math.min(pages, fileCount);
  const per = Math.ceil(fileCount / batches);
  const pageArrays: unknown[] = [];
  for (let i = 0; i < batches; i++) {
    pageArrays.push(allFiles.slice(i * per, (i + 1) * per));
  }
  const diff = pages > 1 ? { pages: pageArrays, pageGapMs: 400 } : pageArrays[0];
  await installTauriMock(page, {
    commands: historyCommands({
      commits: [
        {
          sha: SHA,
          parents: [PARENT],
          author: { name: "A", email: "a@t.co", time: 1700000000, offset_minutes: 0 },
          committer: { name: "A", email: "a@t.co", time: 1700000000, offset_minutes: 0 },
          message: "big diff fixture\n",
          summary: "big diff fixture",
          refs: ["HEAD -> main"],
        },
      ],
      rows: [{ sha: SHA, lane: 0, edges: [], lane_count: 1 }],
      diff,
    }),
  });
}

async function openCommitDetail(page: Page) {
  await openRecentRepo(page);
  await expect(page.locator(".history .row").first()).toBeVisible();
  await page.locator("#commit-row-0").click();
  await expect(page.locator(".detail")).toBeVisible();
  await expect(page.locator(".diff-viewer .viewport")).toBeVisible();
  await expect(page.locator(".dstate")).toHaveCount(0);
}

test("commit detail diff section scrolls vertically", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootBigDiff(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openCommitDetail(page);

  const viewport = page.locator(".diff-viewer .viewport");
  const geometry = await viewport.evaluate((el) => ({
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
    scrollTop: el.scrollTop,
    overflowY: getComputedStyle(el).overflowY,
  }));

  // The content is far taller than the pane: the viewport must be the
  // scroller (scrollHeight > clientHeight) and actually move on wheel.
  expect(geometry.scrollHeight).toBeGreaterThan(geometry.clientHeight + 100);

  // Virtualization guard: the mounted row count tracks the pane's visible
  // window, not the whole diff (a grown viewport mounts every row — the
  // "opening the detail lags the whole app" regression).
  const mountedRows = await page.locator(".diff-viewer .slice > *").count();
  expect(mountedRows).toBeLessThan(200);

  await viewport.hover();
  await page.mouse.wheel(0, 600);
  await expect
    .poll(() => viewport.evaluate((el) => el.scrollTop), { timeout: 3000 })
    .toBeGreaterThan(0);

  expect(tracked.errors).toEqual([]);
});

test("commit detail diff scrolls in the narrow chip-strip layout too", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await bootBigDiff(page);
  // Default 1280px viewport: the history/detail split lands the detail
  // pane under the 700px narrow threshold (chip strip + diff below).
  await openCommitDetail(page);

  const viewport = page.locator(".diff-viewer .viewport");
  const geometry = await viewport.evaluate((el) => ({
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
  }));
  expect(geometry.scrollHeight).toBeGreaterThan(geometry.clientHeight + 100);

  await viewport.hover();
  await page.mouse.wheel(0, 600);
  await expect
    .poll(() => viewport.evaluate((el) => el.scrollTop), { timeout: 3000 })
    .toBeGreaterThan(0);

  expect(tracked.errors).toEqual([]);
});

test("streamed diff pages do not yank the reader back to the top", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  // Two file pages 400ms apart (paced by the mock): scroll after the first
  // lands, then the second page must append without resetting the offset.
  await bootBigDiff(page, 300, 8, 2);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openCommitDetail(page);

  const viewport = page.locator(".diff-viewer .viewport");
  // Wait for the second page's last file to land in the file list.
  await expect(
    page.locator(".detail .filelist").getByText("src/file7.ts"),
  ).toBeVisible({ timeout: 5000 });

  // After both pages landed, scroll — and keep scrolling across any
  // late-page boundary assertions by pinning the position check right
  // after the second page's arrival window.
  await viewport.hover();
  await page.mouse.wheel(0, 900);
  const afterScroll = await viewport.evaluate((el) => el.scrollTop);
  expect(afterScroll).toBeGreaterThan(0);

  // Give any (buggy) reset effect a chance to fire, then re-check.
  await page.waitForTimeout(300);
  const settled = await viewport.evaluate((el) => el.scrollTop);
  expect(settled).toBe(afterScroll);

  expect(tracked.errors).toEqual([]);
});

test("commit detail file list scrolls and the whole detail pane is confined", async ({
  page,
}) => {
  const tracked = trackErrors(page);
  await bootBigDiff(page, 50, 60);
  // Wide viewport: the changed-files column branch (not the <700px chip
  // strip, which is a horizontal scroller by design).
  await page.setViewportSize({ width: 1600, height: 900 });
  await openCommitDetail(page);

  // The detail pane must not push beyond its SplitPane slot: its bounding
  // box stays inside the window (no clipped, unscrollable overflow).
  const detail = await page.locator(".detail").evaluate((el) => {
    const rect = el.getBoundingClientRect();
    return {
      top: rect.top,
      bottom: rect.bottom,
      winH: window.innerHeight,
      height: rect.height,
      width: rect.width,
    };
  });
  expect(detail.top).toBeGreaterThanOrEqual(0);
  expect(detail.bottom).toBeLessThanOrEqual(detail.winH);

  const filelist = page.locator(".detail .filelist");
  const listGeom = await filelist.evaluate((el) => {
    const pane = el.parentElement; // SplitPane pane a
    const paneRect = pane?.getBoundingClientRect();
    return {
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      paneH: paneRect?.height ?? -1,
      panePos: getComputedStyle(pane!).position,
    };
  });
  expect(listGeom.scrollHeight).toBeGreaterThan(listGeom.clientHeight);

  expect(tracked.errors).toEqual([]);
});
