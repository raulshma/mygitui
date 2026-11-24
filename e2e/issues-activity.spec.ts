import { expect, test, type Page } from "@playwright/test";
import {
  activityPopover,
  activityToast,
  bootIssues,
  enableSubscriptions,
  issueDetail,
  issuesCenter,
  openIssues,
  readIssueSync,
  setPageHidden,
  subscriptionToggle,
} from "./issues-test-utils";
import { createIssuesFixture } from "./issues-fixtures";
import { openPanel } from "./tauri-mock";

async function openActivity(page: Page): Promise<void> {
  await issuesCenter(page).getByRole("button", { name: /^Issue activity(?:\s|$)/ }).click();
  await expect(activityPopover(page)).toBeVisible();
}

test("activity baseline is silent, then another user's issue/comment toast deep-links", async ({
  page,
}) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock);

  const baselineCalls = await mock.callsOf("issues_activity_since");
  expect(baselineCalls.length).toBeGreaterThan(0);
  expect(baselineCalls[0]).toMatchObject({ issue_since: null, comment_since: null });
  expect((await readIssueSync(page))?.outbox).toEqual([]);
  await expect(activityToast(page)).toHaveCount(0);

  await mock.clearCalls();
  // Inject the comment first so the coalesced toast's primary (newest) event
  // is the new issue and its Open action has an unambiguous deep link.
  const comment = await mock.addComment({
    issueNumber: 1,
    body: "A new comment after the activity baseline",
    author: "bob",
  });
  const issue = await mock.addIssue({
    title: "A new issue after the activity baseline",
    body: "Created by another collaborator.",
    author: "alice",
    labels: ["bug"],
  });
  await setPageHidden(page, false);

  await expect.poll(async () => (await mock.callsOf("issues_activity_since")).length).toBeGreaterThan(0);
  await expect(activityToast(page)).toContainText("1 new issue and 1 comment in acme/repo");
  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(2);
  const outbox = (await readIssueSync(page))!.outbox;
  expect(outbox.map((event) => event.kind).sort()).toEqual(["comment", "issue"]);
  expect(outbox.find((event) => event.kind === "issue")?.issueNumber).toBe(issue.number);
  expect(outbox.find((event) => event.kind === "comment")?.commentId).toBe(comment.node_id);

  await activityToast(page).getByRole("button", { name: "Open", exact: true }).click();
  await expect(issueDetail(page)).toContainText("A new issue after the activity baseline");

  await openActivity(page);
  const commentEvent = activityPopover(page)
    .getByRole("button")
    .filter({ hasText: "Keyboard navigation skips the first issue" });
  await expect(commentEvent).toBeVisible();
  await commentEvent.click();
  await expect(issueDetail(page)).toContainText("Keyboard navigation skips the first issue");
  await expect(issueDetail(page)).toContainText("A new comment after the activity baseline");
});

test("new-issue and new-comment toggles operate independently without rebaselining", async ({
  page,
}) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock, { issues: true, comments: false });

  await mock.addComment({ issueNumber: 1, body: "Comment while comments are disabled", author: "bob" });
  await mock.addIssue({ title: "Issue while issues are enabled", author: "alice" });
  await setPageHidden(page, false);
  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(1);
  expect((await readIssueSync(page))?.outbox[0]?.kind).toBe("issue");

  await openActivity(page);
  // Turn comments ON before turning issues OFF, so both are never off and no
  // fresh baseline discards events accumulated while comments were disabled.
  const comments = subscriptionToggle(page, "New comments");
  if (!(await comments.isChecked())) await comments.check();
  const issues = subscriptionToggle(page, "New issues");
  if (await issues.isChecked()) await issues.uncheck();
  await expect(comments).toBeChecked();
  await expect(issues).not.toBeChecked();
  await issuesCenter(page).getByRole("button", { name: /^Issue activity(?:\s|$)/ }).click();

  await mock.addIssue({ title: "Issue while issues are disabled", author: "alice" });
  await mock.addComment({ issueNumber: 2, body: "Comment while comments are enabled", author: "carol" });
  await setPageHidden(page, false);
  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(2);
  expect((await readIssueSync(page))?.outbox.map((event) => event.kind).sort()).toEqual([
    "comment",
    "issue",
  ]);
  expect((await readIssueSync(page))?.outbox.some((event) => event.title.includes("issues are disabled"))).toBe(false);
});

test("self-authored, edited-old, and PR comments are seen but never notified", async ({ page }) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock);
  const baseline = (await readIssueSync(page))!;

  const selfIssue = await mock.addIssue({
    title: "Self-authored issue must be suppressed",
    author: "octocat",
  });
  const selfComment = await mock.addComment({
    issueNumber: 1,
    body: "Self-authored comment must be suppressed",
    author: "octocat",
  });
  const oldComment = await mock.editComment(
    "C_I0001_000",
    "Edited historical comment must not create an event",
  );
  const prComment = await mock.addPullRequestComment({
    issueNumber: 501,
    body: "PR comment must be excluded by the backend",
    author: "alice",
  });
  await mock.clearCalls();
  await setPageHidden(page, false);

  await expect.poll(async () => (await mock.callsOf("issues_activity_since")).length).toBeGreaterThan(0);
  await expect.poll(async () => (await readIssueSync(page))?.lastSuccessAt).not.toBe(baseline.lastSuccessAt);
  const sync = (await readIssueSync(page))!;
  expect(sync.outbox).toEqual([]);
  expect(sync.seenIssueIds).toContain(`issue:${selfIssue.node_id}`);
  expect(sync.seenCommentIds).toContain(`comment:${selfComment.node_id}`);
  expect(sync.seenCommentIds).toContain(`comment:${oldComment.node_id}`);
  expect(sync.seenCommentIds).not.toContain(`comment:${prComment.node_id}`);
  await expect(activityToast(page)).toHaveCount(0);
});

test("hidden visibility performs zero subscription calls and visible focus catches up", async ({
  page,
}) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock, { issues: false, comments: true });
  await mock.clearCalls();

  await setPageHidden(page, true);
  await mock.addComment({
    issueNumber: 1,
    body: "Comment accumulated while the window was hidden",
    author: "dave",
  });
  await page.evaluate(() => {
    document.dispatchEvent(new Event("focus"));
    window.dispatchEvent(new Event("focus"));
  });
  await page.waitForTimeout(150);
  expect((await mock.callsOf("issues_activity_since")).length).toBe(0);
  expect((await readIssueSync(page))?.outbox).toEqual([]);

  await setPageHidden(page, false);
  await expect.poll(async () => (await mock.callsOf("issues_activity_since")).length).toBe(1);
  await expect(activityToast(page)).toContainText("1 comment in acme/repo");
  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(1);
});

test("reload resumes durable cursors, catches up once, and does not duplicate events", async ({
  page,
}) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock);
  const beforeReload = (await readIssueSync(page))!;
  expect(beforeReload.issueCursor).not.toBeNull();
  expect(beforeReload.commentCursor).not.toBeNull();

  await mock.addComment({
    issueNumber: 1,
    body: "Comment accumulated across a restart",
    author: "carol",
  });
  await mock.addIssue({
    title: "Issue accumulated across a restart",
    author: "alice",
  });
  await mock.clearCalls();
  await page.reload();

  await expect(page.getByRole("textbox", { name: "Search issues", exact: true })).toBeVisible();
  await expect(activityToast(page)).toContainText("1 new issue and 1 comment in acme/repo");
  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(2);
  const resumed = (await mock.callsOf("issues_activity_since")).find(
    (args) => typeof args.issue_since === "string" && typeof args.comment_since === "string",
  );
  expect(resumed).toBeDefined();
  expect(resumed?.issue_since).not.toBeNull();
  expect(resumed?.comment_since).not.toBeNull();

  await setPageHidden(page, false);
  await page.waitForTimeout(150);
  expect((await readIssueSync(page))?.outbox).toHaveLength(2);
  expect(new Set((await readIssueSync(page))!.outbox.map((event) => event.id)).size).toBe(2);
});

test("bounded activity pages retain later-page issue comments and exclude PR comments", async ({
  page,
}) => {
  const mock = await bootIssues(page, {
    issues: { ...createIssuesFixture(), activityPageSize: 10 },
  });
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock, { issues: false, comments: true });
  const baseline = (await readIssueSync(page))!;
  expect(baseline.outbox).toEqual([]);
  expect((await mock.callsOf("issues_activity_since")).some((args) =>
    typeof args.issue_since === "string" && args.issue_since.startsWith("/repos/acme/repo/issues?"),
  )).toBe(true);

  const baselineAt = Date.parse(baseline.lastSuccessAt!);
  for (let index = 0; index < 11; index += 1) {
    await mock.addIssue({
      title: `Bounded-page filler issue ${index + 1}`,
      author: "alice",
      createdAt: new Date(baselineAt + 1_000).toISOString(),
      updatedAt: new Date(baselineAt + 1_000).toISOString(),
    });
  }
  const laterIssue = (await mock.issueSnapshot()).issues.find((issue) => issue.number === 42)!;
  const laterComment = await mock.addComment({
    issueNumber: 42,
    body: "Comment on an issue outside the bounded issue page",
    author: "carol",
    createdAt: new Date(baselineAt + 10_000).toISOString(),
    updatedAt: new Date(baselineAt + 10_000).toISOString(),
  });
  const prComment = await mock.addPullRequestComment({
    issueNumber: 501,
    body: "PR comment outside the bounded issue page",
    author: "alice",
    createdAt: new Date(baselineAt + 11_000).toISOString(),
  });
  await mock.clearCalls();
  await setPageHidden(page, false);

  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(1);
  const event = (await readIssueSync(page))!.outbox[0]!;
  expect(event).toMatchObject({
    kind: "comment",
    issueNumber: 42,
    commentId: laterComment.node_id,
    title: laterIssue.title,
    url: laterComment.url,
  });
  expect((await readIssueSync(page))!.seenCommentIds).not.toContain(`comment:${prComment.node_id}`);
  await expect.poll(async () => {
    const calls = await mock.callsOf("issues_activity_since");
    return calls.some((args) =>
      typeof args.issue_since === "string" && args.issue_since.startsWith("/repos/acme/repo/issues?page=2"),
    );
  }).toBe(true);
});

test("account Notifications mark-read does not alter the issue-activity outbox", async ({ page }) => {
  const mock = await bootIssues(page);
  await page.setViewportSize({ width: 1600, height: 900 });
  await openIssues(page);
  await enableSubscriptions(page, mock, { issues: false, comments: true });
  await mock.addComment({
    issueNumber: 1,
    body: "Unread issue activity independent of account notifications",
    author: "bob",
  });
  await setPageHidden(page, false);
  await expect.poll(async () => (await readIssueSync(page))?.outbox.length).toBe(1);
  expect((await readIssueSync(page))?.outbox[0]?.readAt).toBeNull();

  await openPanel(page, "GitHub");
  await page.getByRole("tab", { name: "Notifications", exact: true }).click();
  await expect(
    page
      .getByRole("list", { name: "Notifications", exact: true })
      .getByText("Search quota errors need a retry affordance", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Mark all read", exact: true }).click();
  await expect.poll(async () => (await mock.callsOf("notifications_mark_all_read")).length).toBe(1);
  expect((await readIssueSync(page))?.outbox[0]?.readAt).toBeNull();

  await openActivity(page);
  await activityPopover(page).getByRole("button", { name: "Mark all read", exact: true }).click();
  await expect.poll(async () => (await readIssueSync(page))?.outbox[0]?.readAt).not.toBeNull();
  expect((await mock.callsOf("notifications_mark_all_read")).length).toBe(1);
});
