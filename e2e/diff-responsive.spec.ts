/**
 * Diff viewer responsive layout (commit detail context): wrapping is ON by
 * default — long lines fold inside the pane, never a horizontal scrollbar in
 * either mode. With wrap toggled OFF, unified grows one horizontal scrollbar
 * when the widest line genuinely overflows, and split grows ONE shared
 * scrollbar serving both halves (halves stay column-aligned).
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
async function bootRepoWithDiff(
  page: Page,
  lineTexts: string[],
  opts: { allContext?: boolean; del?: string } = {},
) {
  await seedStorage(page, recentRepos(["/tmp/repo"]));
  const lines: Array<{
    old_no: number | null;
    new_no: number | null;
    origin: string;
    text: string;
    highlights: never[];
  }> = [];
  // First line context, the rest additions — or all context. `opts.del`
  // inserts a deletion after the first line, pairing with the first
  // addition (a split pair row with DIFFERENT text per side).
  lines.push({ old_no: 1, new_no: 1, origin: " ", text: lineTexts[0]!, highlights: [] });
  if (opts.del) {
    lines.push({ old_no: 2, new_no: null, origin: "-", text: opts.del, highlights: [] });
  }
  lineTexts.slice(1).forEach((text, i) => {
    lines.push({
      old_no: opts.allContext ? lines.length + 1 : null,
      new_no: lines.length + 1,
      origin: opts.allContext ? " " : "+",
      text,
      highlights: [],
    });
  });
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
          deletions: opts.del ? 2 : 1,
          hunks: [{ old_start: 1, new_start: 1, lines }],
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

type HighlightMap = Map<string, Iterable<Range>>;

/** The painted split-view selection: joined range texts + their half sides. */
async function highlightState(page: Page): Promise<{ text: string; sides: string[] }> {
  return page.evaluate(() => {
    const hl = (CSS as { highlights?: HighlightMap }).highlights?.get(
      "mygitui-diff-sel",
    );
    if (!hl) return { text: "", sides: [] as string[] };
    const texts: string[] = [];
    const sides: string[] = [];
    for (const range of hl) {
      texts.push(range.toString());
      const el =
        range.startContainer instanceof Element
          ? range.startContainer
          : range.startContainer.parentElement;
      const half = el?.closest(".half");
      sides.push(half?.classList.contains("right") ? "right" : "left");
    }
    return { text: texts.join("\n"), sides };
  });
}

test("split view confines text selection to the half it starts in", async ({ page }) => {
  const tracked = trackErrors(page);
  // The pair row carries DIFFERENT text per side, so assertions can tell
  // which half a highlight came from.
  await bootRepoWithDiff(page, ["ctx line", "new added line"], {
    del: "old deleted line",
  });
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "ctx line");

  const boxes = await page.evaluate(() => {
    const row = document.querySelector<HTMLElement>(".diff-viewer .row.pair")!;
    const left = row.querySelector<HTMLElement>(".half:not(.right) .txt")!;
    const right = row.querySelector<HTMLElement>(".half.right .txt")!;
    return { from: left.getBoundingClientRect(), to: right.getBoundingClientRect() };
  });
  const midY = (r: DOMRect) => r.top + r.height / 2;

  // Drag from the deleted side across the divider into the added side.
  await page.mouse.move(boxes.from.left - 12, midY(boxes.from));
  await page.mouse.down();
  await page.mouse.move(boxes.to.left + 40, midY(boxes.to), { steps: 8 });
  let hl = await highlightState(page);
  expect(hl.text).toContain("old deleted");
  expect(hl.text).not.toContain("added");
  await page.mouse.up();
  hl = await highlightState(page);
  expect(hl.text).toContain("old deleted");
  expect(hl.text).not.toContain("added");
  expect(hl.sides).toStrictEqual(["left"]);

  // Same from the right half: drag into the left, only added text selected.
  await page.mouse.move(boxes.to.left + 60, midY(boxes.to));
  await page.mouse.down();
  await page.mouse.move(boxes.from.left - 12, midY(boxes.from), { steps: 8 });
  await page.mouse.up();
  hl = await highlightState(page);
  expect(hl.text.startsWith("new")).toBe(true);
  expect(hl.text).not.toContain("deleted");
  expect(hl.sides).toStrictEqual(["right"]);

  // Double-click picks the word under the caret on that side.
  await page.mouse.dblclick(boxes.to.left + 30, midY(boxes.to));
  hl = await highlightState(page);
  expect(hl.text).toBe("added");

  expect(tracked.errors).toEqual([]);
});

test("split view keeps a multi-row selection on the side it starts in", async ({ page }) => {
  const tracked = trackErrors(page);
  // Several context rows: the drag must grow DOWN the starting column and
  // still refuse to cross the divider when it sweeps across it.
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await bootRepoWithDiff(
    page,
    ["alpha context line", "beta context line", "gamma context line", "delta context line"],
    { allContext: true },
  );
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "alpha context line");

  const points = await page.evaluate(() => {
    const rows = [...document.querySelectorAll<HTMLElement>(".diff-viewer .row.context")];
    const leftTxt = (row: HTMLElement) =>
      row.querySelector<HTMLElement>(".half:not(.right) .txt")!.getBoundingClientRect();
    const rightTxt = (row: HTMLElement) =>
      row.querySelector<HTMLElement>(".half.right .txt")!.getBoundingClientRect();
    const mid = (r: DOMRect) => ({ x: r.left + 20, y: r.top + r.height / 2 });
    return {
      start: { x: leftTxt(rows[0]!).left - 12, y: leftTxt(rows[0]!).top + leftTxt(rows[0]!).height / 2 },
      down: { x: leftTxt(rows[2]!).left + 120, y: leftTxt(rows[2]!).top + leftTxt(rows[2]!).height / 2 },
      across: mid(rightTxt(rows[3]!)),
    };
  });

  // Down the left column across three rows, then sweep across the divider
  // while still holding the button.
  await page.mouse.move(points.start.x, points.start.y);
  await page.mouse.down();
  await page.mouse.move(points.down.x, points.down.y, { steps: 10 });
  let hl = await highlightState(page);
  expect(hl.text).toContain("alpha");
  expect(hl.text).toContain("gamma");
  expect(new Set(hl.sides)).toEqual(new Set(["left"]));

  await page.mouse.move(points.across.x, points.across.y, { steps: 10 });
  await page.mouse.up();
  hl = await highlightState(page);
  // The sweep across the divider was ignored: three left rows stay
  // selected, nothing from the right side joined.
  expect(hl.text).toContain("alpha");
  expect(hl.text).toContain("gamma");
  expect(hl.text).not.toContain("delta");
  expect(new Set(hl.sides)).toEqual(new Set(["left"]));

  // Ctrl+C copies exactly the selected side's text.
  await page.keyboard.press("Control+c");
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain("alpha context line");
  expect(copied).toContain("gamma context line");
  expect(copied).not.toContain("delta");

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

  // Wrap is ON by default: the long line folds, no horizontal scrollbar.
  let m = await diffMetrics(page);
  expect(m.vpScroll).toBeLessThanOrEqual(m.vpClient);
  const wrapped = await page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>(".diff-viewer .row.line.single")]
      .some((row) => row.offsetHeight > 20),
  );
  expect(wrapped).toBe(true);

  // Wrap OFF: the widest line really needs more than the pane — the viewer
  // grows a scrollbar, and content/rows must stretch across the full scroll
  // extent (no dead space, no early-cut backgrounds).
  const wrapBtn = page.getByRole("button", { name: "Wrap long lines" });
  await wrapBtn.click();
  await expect(wrapBtn).toHaveAttribute("aria-pressed", "false");
  m = await diffMetrics(page);
  expect(m.contentWidth).toBeGreaterThan(m.vpClient);
  expect(m.vpScroll).toBeGreaterThan(m.vpClient);
  expect(m.contentWidth).toBe(m.vpScroll);
  expect(m.rowWidth).toBe(m.vpScroll);

  expect(tracked.errors).toEqual([]);
});

test("wrap off: split scrolls both halves on one shared scrollbar", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootRepoWithDiff(page, ["context start", "z".repeat(110)]);
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "context start");

  // Default (wrap on): responsive, no horizontal overflow.
  let m = await diffMetrics(page);
  expect(m.vpScroll).toBeLessThanOrEqual(m.vpClient);

  const wrapBtn = page.getByRole("button", { name: "Wrap long lines" });
  await wrapBtn.click();
  await expect(wrapBtn).toHaveAttribute("aria-pressed", "false");

  // One shared horizontal scrollbar; both halves stretch across the full
  // scroll extent and stay equal width (column-aligned).
  m = await diffMetrics(page);
  expect(m.vpScroll).toBeGreaterThan(m.vpClient);
  expect(m.contentWidth).toBe(m.vpScroll);
  expect(m.rowWidth).toBe(m.vpScroll);
  expect(m.halfWidths.length).toBeGreaterThanOrEqual(2);
  expect(Math.abs(m.halfWidths[0]! - m.halfWidths[1]!)).toBeLessThanOrEqual(2);
  expect(m.halfWidths[0]!).toBeGreaterThan(m.vpClient / 2);

  expect(tracked.errors).toEqual([]);
});

test("wrap persists across viewers (preference storage)", async ({ page }) => {
  const tracked = trackErrors(page);
  await bootRepoWithDiff(page, ["context start", "w".repeat(110)]);
  await page.setViewportSize({ width: 1400, height: 1000 });
  await openCommitDiff(page, "context start");

  const wrapBtn = page.getByRole("button", { name: "Wrap long lines" });
  await wrapBtn.click();
  await expect(wrapBtn).toHaveAttribute("aria-pressed", "false");

  const stored = await page.evaluate(() => localStorage.getItem("mygitui.diff.wrap"));
  expect(JSON.parse(stored ?? "null")).toBe(false);

  expect(tracked.errors).toEqual([]);
});
