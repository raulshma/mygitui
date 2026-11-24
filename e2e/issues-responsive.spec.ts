import { expect, test } from "@playwright/test";
import {
  bootIssues,
  issueDetail,
  issueList,
  issuesCenter,
  openIssues,
  readIssueViewPrefs,
  selectIssue,
} from "./issues-test-utils";

test("wide split, narrow Back, and wide restoration preserve query, selection, and ratio", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  const mock = await bootIssues(page);
  await openIssues(page);
  await selectIssue(page, 1);

  const center = issuesCenter(page);
  const list = issueList(page);
  const detail = issueDetail(page);
  const back = center.getByRole("button", { name: "Back to issues", exact: true }).first();
  await expect(list).toBeVisible();
  await expect(detail).toBeVisible();
  await expect(back).toBeHidden();

  const divider = center.getByRole("separator", { name: /issues list width/i });
  await expect(divider).toBeVisible();
  const initialRatio = Number(await divider.getAttribute("aria-valuenow"));
  await divider.focus();
  await divider.press("ArrowRight");
  await expect.poll(async () => Number(await divider.getAttribute("aria-valuenow"))).toBeGreaterThan(
    initialRatio,
  );
  const resizedRatio = Number(await divider.getAttribute("aria-valuenow"));
  await expect.poll(async () => (await readIssueViewPrefs(page))?.splitRatio).toBeCloseTo(
    resizedRatio / 100,
    2,
  );

  const search = page.getByRole("textbox", { name: "Search issues", exact: true });
  await search.fill("Keyboard navigation skips the first issue");
  await search.press("Enter");
  await expect(list.getByRole("option")).toHaveCount(1);

  // Stacked mode gives the center the full window; 650px is below the
  // center's 700px split threshold.
  await page.setViewportSize({ width: 650, height: 900 });
  await expect(back).toBeVisible();
  await expect(detail).toBeVisible();
  await expect(list).not.toBeVisible();
  await back.click();
  await expect(list).toBeVisible();
  await expect(detail).not.toBeVisible();
  await expect(search).toHaveValue("Keyboard navigation skips the first issue");

  await selectIssue(page, 1);
  await expect(detail).toBeVisible();
  await page.setViewportSize({ width: 1600, height: 900 });
  await expect(list).toBeVisible();
  await expect(detail).toBeVisible();
  await expect(back).toBeHidden();

  await page.reload();
  await expect(page.getByRole("textbox", { name: "Search issues", exact: true })).toHaveValue(
    "Keyboard navigation skips the first issue",
  );
  await expect(list.getByRole("option")).toHaveCount(1);
  await expect(detail).toContainText("Keyboard navigation skips the first issue");
  await expect.poll(async () => (await readIssueViewPrefs(page))?.splitRatio).toBeCloseTo(
    resizedRatio / 100,
    2,
  );
  expect((await mock.callsOf("repo_open")).at(-1)?.path).toBe("/tmp/repo");

  await center.getByRole("button", { name: "Close issues", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Search issues", exact: true })).toHaveCount(0);
});
