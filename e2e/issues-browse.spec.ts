import { expect, test } from "@playwright/test";
import {
  ISSUE_FORGE_ERRORS,
  ISSUE_REPO_ID,
  ISSUE_REMOTE,
  createIssuesFixture,
} from "./issues-fixtures";
import {
  bootIssues,
  chooseOption,
  issueDetail,
  issueList,
  issueOption,
  issuesCenter,
  openIssues,
  selectIssue,
} from "./issues-test-utils";

const ISSUE_ONE_URL = "https://github.com/acme/repo/issues/1";
const COMMENT_60_URL =
  "https://github.com/acme/repo/issues/1#issuecomment-C_I0001_060";

test("Issues opens from the GitHub sidebar and command palette", async ({ page }) => {
  const mock = await bootIssues(page);
  await openIssues(page, "sidebar");
  await expect(issueList(page)).toBeVisible();
  await expect(issueOption(page, 1)).toContainText("Keyboard navigation skips the first issue");
  expect((await mock.callsOf("issues_identity"))[0]).toEqual({
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
  });

  await issuesCenter(page).getByRole("button", { name: "Close issues", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Search issues", exact: true })).toHaveCount(0);
  await openIssues(page, "palette");
  await expect(issueList(page)).toBeVisible();
  await expect(issueOption(page, 1)).toBeVisible();
});

test("server-side search/filter/sort/pagination and refresh preserve selection", async ({ page }) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);

  await expect(issueList(page).getByRole("option")).toHaveCount(30);
  await expect(issueOption(page, 1)).toBeVisible();
  expect((await mock.callsOf("issues_list"))[0]?.query).toEqual({
    state: "open",
    search: "",
    labels: [],
    assignees: [],
    milestone: "",
    sort: "updated",
    direction: "desc",
    per_page: 30,
    page: 1,
  });

  await selectIssue(page, 1);
  await issuesCenter(page).getByRole("button", { name: "Refresh issues", exact: true }).first().click();
  await expect(issueOption(page, 1)).toHaveAttribute("aria-selected", "true");
  await expect(issueDetail(page)).toContainText("Keyboard navigation skips the first issue");

  await issuesCenter(page).getByRole("button", { name: /^next$/i }).click();
  await expect(issueList(page).getByRole("option")).toHaveCount(10);
  await expect
    .poll(async () => {
      const calls = await mock.callsOf("issues_list");
      return (calls.at(-1)?.query as { page?: number } | undefined)?.page;
    })
    .toBe(2);
  await issuesCenter(page).getByRole("button", { name: /^previous$/i }).click();
  await expect(issueList(page).getByRole("option")).toHaveCount(30);

  const search = page.getByRole("textbox", { name: "Search issues", exact: true });
  const beforeTyping = (await mock.callsOf("issues_list")).length;
  await search.fill("Keyboard navigation skips the first issue");
  await page.waitForTimeout(150);
  expect((await mock.callsOf("issues_list")).length).toBe(beforeTyping);
  await search.press("Enter");
  await expect(issueList(page).getByRole("option")).toHaveCount(1);
  await expect(issueOption(page, 1)).toBeVisible();
  expect((await mock.callsOf("issues_list")).at(-1)?.query).toMatchObject({
    search: "Keyboard navigation skips the first issue",
    page: 1,
  });

  await page.getByRole("button", { name: "Clear", exact: true }).click();
  await expect(issueList(page).getByRole("option")).toHaveCount(30);
  await chooseOption(issuesCenter(page), "Issue state", "Closed");
  await expect(issueList(page).getByRole("option")).toHaveCount(2);
  await expect(issueOption(page, 41)).toBeVisible();
  expect((await mock.callsOf("issues_list")).at(-1)?.query).toMatchObject({
    state: "closed",
    page: 1,
  });

  await chooseOption(issuesCenter(page), "Issue state", "Open");
  await expect(issueList(page).getByRole("option")).toHaveCount(30);
  const filters = issuesCenter(page).locator("summary").filter({ hasText: /^Filters/ });
  await filters.click();
  await issuesCenter(page).getByRole("checkbox", { name: "bug", exact: true }).check();
  await expect
    .poll(async () => {
      const calls = await mock.callsOf("issues_list");
      return (calls.at(-1)?.query as { labels?: string[] } | undefined)?.labels;
    })
    .toEqual(["bug"]);
  expect((await mock.callsOf("issues_metadata")).length).toBeGreaterThan(0);
  await filters.click();

  await chooseOption(issuesCenter(page), "Sort issues", "Comments");
  await chooseOption(issuesCenter(page), "Sort direction", "Ascending");
  await expect
    .poll(async () => {
      const calls = await mock.callsOf("issues_list");
      return (calls.at(-1)?.query as { sort?: string; direction?: string; page?: number } | undefined)?.sort;
    })
    .toBe("comments");
  expect((await mock.callsOf("issues_list")).at(-1)?.query).toMatchObject({
    direction: "asc",
    page: 1,
  });
});

test("issue detail shows metadata and walks all comment pages without losing the composer", async ({
  page,
}) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await selectIssue(page, 1);

  const detail = issueDetail(page);
  await expect(detail).toContainText("enhancement");
  await expect(detail).toContainText("v1.1");
  await expect(detail).toContainText("bob");
  await expect(detail).toContainText("Reproduction");

  const comments = detail.locator('section[aria-label="Issue comments"] ol > li');
  await expect(comments).toHaveCount(5);
  await expect(comments.first()).toContainText("Update 60");
  expect((await mock.callsOf("issues_comments"))[0]).toEqual({
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 1,
    page: null,
    per_page: 30,
  });

  const composer = detail.getByRole("textbox", { name: /comment/i });
  await composer.fill("Keep this draft while older comments load");
  await detail.getByRole("button", { name: "Load earlier comments", exact: true }).click();
  await expect(comments).toHaveCount(35);
  await expect(composer).toHaveValue("Keep this draft while older comments load");
  await detail.getByRole("button", { name: "Load earlier comments", exact: true }).click();
  await expect(comments).toHaveCount(65);
  await expect(comments.first()).toContainText("Update 0");
  await expect(composer).toHaveValue("Keep this draft while older comments load");
  expect((await mock.callsOf("issues_comments")).map((args) => args.page)).toEqual([null, 2, 1]);
});

test("create/edit metadata, comment, close and reopen return and render updated data", async ({
  page,
}) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);

  await issuesCenter(page).getByRole("button", { name: "New issue", exact: true }).click();
  let dialog = page.locator(".sdlg-dialog").last();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("textbox", { name: "Title", exact: true }).fill("E2E issue with metadata");
  await dialog.getByRole("textbox", { name: "Description", exact: true }).fill("Original deterministic body");
  await dialog.getByRole("checkbox", { name: "bug", exact: true }).check();
  await dialog.getByRole("checkbox", { name: "alice", exact: true }).check();
  await chooseOption(dialog, "Milestone", "v1.0");
  await dialog.getByRole("button", { name: "Create issue", exact: true }).click();

  await expect(issueDetail(page)).toContainText("E2E issue with metadata");
  await expect(issueDetail(page)).toContainText("bug");
  await expect(issueDetail(page)).toContainText("alice");
  await expect(issueDetail(page)).toContainText("v1.0");
  expect((await mock.callsOf("issues_create"))[0]).toEqual({
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    title: "E2E issue with metadata",
    body: "Original deterministic body",
    labels: ["bug"],
    assignees: ["alice"],
    milestone: 1,
  });

  await issueDetail(page).getByRole("button", { name: "Edit issue", exact: true }).click();
  dialog = page.locator(".sdlg-dialog").last();
  await expect(dialog.getByRole("textbox", { name: "Title", exact: true })).toHaveValue(
    "E2E issue with metadata",
  );
  await dialog.getByRole("textbox", { name: "Title", exact: true }).fill("E2E issue updated");
  await dialog.getByRole("textbox", { name: "Description", exact: true }).fill("Updated deterministic body");
  await dialog.getByRole("button", { name: /save|update/i }).last().click();
  await expect(issueDetail(page)).toContainText("E2E issue updated");
  await expect(issueDetail(page)).toContainText("Updated deterministic body");
  expect((await mock.callsOf("issues_update"))[0]).toMatchObject({
    number: 43,
    title: "E2E issue updated",
    body: "Updated deterministic body",
    labels: ["bug"],
    assignees: ["alice"],
    milestone: 1,
  });

  const composer = issueDetail(page).getByRole("textbox", { name: /comment/i });
  await composer.fill("A comment posted through the mock backend");
  await issueDetail(page).getByRole("button", { name: "Post comment", exact: true }).click();
  await expect(issueDetail(page)).toContainText("A comment posted through the mock backend");
  expect((await mock.callsOf("issues_comment"))[0]).toEqual({
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 43,
    body: "A comment posted through the mock backend",
  });

  await issueDetail(page).getByRole("button", { name: "Close issue", exact: true }).click();
  const closeDialog = page.locator(".sdlg-dialog").last();
  if (await closeDialog.isVisible()) {
    await closeDialog.getByRole("textbox", { name: /comment/i }).fill("Closing after verification");
    await closeDialog.getByRole("button", { name: "Close issue", exact: true }).click();
  }
  await expect(issueDetail(page).getByText("Closed", { exact: true })).toBeVisible();
  expect((await mock.callsOf("issues_set_state")).at(-1)).toMatchObject({
    number: 43,
    state: "closed",
  });

  await issueDetail(page).getByRole("button", { name: "Reopen issue", exact: true }).click();
  const reopenDialog = page.locator(".sdlg-dialog").last();
  if (await reopenDialog.isVisible()) {
    await reopenDialog.getByRole("button", { name: "Reopen issue", exact: true }).click();
  }
  await expect(issueDetail(page).getByText("Open", { exact: true })).toBeVisible();
  expect((await mock.callsOf("issues_set_state")).at(-1)).toMatchObject({
    number: 43,
    state: "open",
  });
});

test("issue/comment open and copy actions use canonical URLs", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await selectIssue(page, 1);

  const detail = issueDetail(page);
  await detail.getByRole("button", { name: "Copy issue link", exact: true }).click();
  await expect.poll(async () => page.evaluate(() => navigator.clipboard.readText())).toBe(ISSUE_ONE_URL);
  await detail.getByRole("button", { name: "Open issue on GitHub", exact: true }).click();
  await expect
    .poll(async () => (await mock.callsOf("plugin:opener|open_url")).at(-1)?.url)
    .toBe(ISSUE_ONE_URL);

  const firstComment = detail.locator('section[aria-label="Issue comments"] ol > li').first();
  await firstComment.getByRole("button", { name: /^Copy comment link by .*$/ }).click();
  await expect.poll(async () => page.evaluate(() => navigator.clipboard.readText())).toBe(COMMENT_60_URL);
  await firstComment.getByRole("button", { name: /^Open comment by .* on GitHub$/ }).click();
  await expect
    .poll(async () => (await mock.callsOf("plugin:opener|open_url")).at(-1)?.url)
    .toBe(COMMENT_60_URL);
});

test("structured permission/rate-limit failures surface inline and can be retried", async ({ page }) => {
  const mock = await bootIssues(page, {
    issues: {
      ...createIssuesFixture(),
      errors: { issues_list: ISSUE_FORGE_ERRORS.permission },
      searchErrors: { quota: ISSUE_FORGE_ERRORS.rateLimited },
    },
  });
  await openIssues(page);
  const center = issuesCenter(page);
  await expect(center.getByRole("alert")).toContainText("Resource not accessible by integration");
  await mock.setIssueFailure("issues_list", null);
  await center.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(issueList(page).getByRole("option")).toHaveCount(30);

  const search = page.getByRole("textbox", { name: "Search issues", exact: true });
  await search.fill("quota");
  await search.press("Enter");
  await expect(center.getByRole("alert")).toContainText("API rate limit exceeded");
  await expect(center).toContainText(/retry|rate limit/i);
});
