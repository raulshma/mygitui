import { expect, test } from "@playwright/test";
import {
  installTauriMock,
  openPanel,
  openRecentRepo,
  recentRepos,
  seedStorage,
  trackErrors,
} from "./tauri-mock";
import { expectFullyVisible } from "./visible";

/** 400 consecutive days of activity (more than the 53-week heatmap window). */
function activity(): Array<{ day: string; count: number }> {
  const out: Array<{ day: string; count: number }> = [];
  const start = Date.UTC(2025, 9, 1);
  for (let i = 0; i < 400; i += 1) {
    const date = new Date(start + i * 86_400_000);
    const day = `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, "0")}-${String(date.getUTCDate()).padStart(2, "0")}`;
    out.push({ day, count: i % 5 });
  }
  return out;
}

/** Stats panel: the 53-week heatmap is a real table and stays horizontal. */
test("contribution heatmap lays out horizontally without a vertical scrollbar", async ({
  page,
}) => {
  const tracked = trackErrors(page);

  await seedStorage(page, recentRepos(["/tmp/repo"]));
  await installTauriMock(page, {
    commands: {
      commit_activity: activity(),
      contributor_stats: [
        {
          name: "Alice",
          email: "alice@example.com",
          count: 12,
          first_day: "2025-10-01",
          last_day: "2026-10-01",
        },
      ],
    },
  });

  await openRecentRepo(page);
  await openPanel(page, "Stats");

  await expect(page.locator(".panel-title")).toHaveText("Stats");
  const table = page.locator(".heatmap");
  await expect(table).toBeVisible();
  await expect(table.locator("td")).toHaveCount(53 * 7);
  await expect(page.locator(".legend-cell")).toHaveCount(5);

  const layout = await page.evaluate(() => {
    const heatmap = document.querySelector(".heatmap") as HTMLElement;
    const wrap = document.querySelector(".heatmap-wrap") as HTMLElement;
    const cell = heatmap.querySelector("td") as HTMLElement;
    const legend = document.querySelector(".legend-cell") as HTMLElement;
    const rect = heatmap.getBoundingClientRect();
    return {
      width: rect.width,
      height: rect.height,
      cellDisplay: getComputedStyle(cell).display,
      legendWidth: legend.getBoundingClientRect().width,
      legendHeight: legend.getBoundingClientRect().height,
      wrapScrollWidth: wrap.scrollWidth,
      wrapClientWidth: wrap.clientWidth,
      wrapScrollHeight: wrap.scrollHeight,
      wrapClientHeight: wrap.clientHeight,
    };
  });

  // Regression guard: `td` must keep table-cell layout (an inline-block
  // override collapsed the grid into a tall, vertically scrolling column).
  expect(layout.cellDisplay).toBe("table-cell");
  expect(layout.legendWidth).toBeCloseTo(11, 0);
  expect(layout.legendHeight).toBeCloseTo(11, 0);
  expect(layout.width).toBeGreaterThan(layout.height * 3);
  expect(layout.wrapScrollHeight).toBeLessThan(200);
  // 53 weeks overflow a side panel horizontally, not vertically.
  expect(layout.wrapScrollWidth).toBeGreaterThan(layout.wrapClientWidth);
  expect(layout.wrapScrollHeight).toBeLessThanOrEqual(layout.wrapClientHeight + 1);

  // The Colors dialog must escape the panel's scroll clipping.
  await page.getByRole("button", { name: "Colors…" }).click();
  const colors = page.getByRole("dialog", { name: "Branch color rules" });
  await expect(colors).toBeVisible();
  await expectFullyVisible(page, colors);

  expect(tracked.errors).toEqual([]);
  await tracked.attach(test.info());
});
