import { expect, test, type Page } from "@playwright/test";
import type { IssueCommentPage } from "../src/lib/ipc/gen/IssueCommentPage";
import type { IssueDetail } from "../src/lib/ipc/gen/IssueDetail";
import type { IssueListPage } from "../src/lib/ipc/gen/IssueListPage";
import type { IssueActivityBatch } from "../src/lib/ipc/gen/IssueActivityBatch";
import {
  createIssuesFixture,
  ISSUE_FORGE_ERRORS,
  ISSUE_REPO_ID,
  ISSUE_REMOTE,
  issueMockCommands,
} from "./issues-fixtures";
import { installTauriMock } from "./tauri-mock";

async function invoke<T>(
  page: Page,
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  return page.evaluate(
    ({ command: cmd, args: payload }) => {
      const internals = (window as unknown as {
        __TAURI_INTERNALS__: { invoke(name: string, args?: Record<string, unknown>): Promise<unknown> };
      }).__TAURI_INTERNALS__;
      return internals.invoke(cmd, payload) as Promise<T>;
    },
    { command, args },
  );
}

const listArgs = {
  repo_id: ISSUE_REPO_ID,
  remote: ISSUE_REMOTE,
  query: {
    state: "open",
    search: "",
    labels: [],
    assignees: [],
    milestone: "",
    sort: "updated",
    direction: "desc",
    per_page: 30,
    page: 1,
  },
};

test("Issues mock returns generated snake_case page/detail/comment/activity shapes", async ({
  page,
}) => {
  const fixture = createIssuesFixture();
  const mock = await installTauriMock(page, {
    commands: issueMockCommands(),
    issues: fixture,
  });
  await page.goto("/");

  const identity = await invoke<{ user: { node_id: string; login: string }; owner: string }>(
    page,
    "issues_identity",
    { repo_id: ISSUE_REPO_ID, remote: ISSUE_REMOTE },
  );
  expect(identity).toMatchObject({
    owner: "acme",
    user: { node_id: "U_SELF", login: "octocat" },
  });

  const first = await invoke<IssueListPage>(page, "issues_list", listArgs);
  expect(first.items).toHaveLength(30);
  expect(first.items[0]?.number).toBe(1);
  expect(first).toMatchObject({
    page: 1,
    per_page: 30,
    has_next: true,
    has_prev: false,
    total_count: 40,
    incomplete_results: false,
  });
  expect(first.items[0]).toHaveProperty("node_id", "I_0001");
  expect(first.items[0]).toHaveProperty("comment_count", 65);
  expect(first.items[0]).not.toHaveProperty("comments_count");

  const second = await invoke<IssueListPage>(page, "issues_list", {
    ...listArgs,
    query: { ...listArgs.query, page: 2 },
  });
  expect(second.items).toHaveLength(10);
  expect(second.has_prev).toBe(true);
  expect(second.has_next).toBe(false);

  const assigned = await invoke<IssueListPage>(page, "issues_list", {
    ...listArgs,
    query: { ...listArgs.query, assignees: ["alice", "bob"], per_page: 100 },
  });
  expect(assigned.total_count).toBe(40);
  expect(assigned.items.every((item) => item.assignees.some((entry) => entry.login === "alice" || entry.login === "bob"))).toBe(true);

  const detail = await invoke<IssueDetail>(page, "issues_detail", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 1,
  });
  expect(detail.number).toBe(1);
  expect(detail.body).toContain("Reproduction");
  expect(detail).toHaveProperty("issue_type");
  expect(detail).not.toHaveProperty("summary");

  // page=null resolves the final Link page, exactly as the store expects.
  const latest = await invoke<IssueCommentPage>(page, "issues_comments", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 1,
    page: null,
    per_page: 30,
  });
  expect(latest).toMatchObject({
    page: 3,
    per_page: 30,
    has_next: false,
    has_prev: true,
    total_count: 65,
  });
  expect(latest.items).toHaveLength(5);
  expect(latest.items[0]?.node_id).toBe("C_I0001_060");

  const activity = await invoke<IssueActivityBatch>(page, "issues_activity_since", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    issue_since: null,
    comment_since: null,
  });
  expect(activity.issues).toHaveLength(42);
  expect(activity.comments.length).toBeGreaterThan(65);
  expect(activity.comments.every((comment) => comment.issue_number !== 501)).toBe(true);
  expect(activity.comments.every((comment) => "issue_title" in comment && "node_id" in comment)).toBe(true);
  expect(activity).toMatchObject({
    next_issue_cursor: null,
    next_comment_cursor: null,
    complete: true,
  });
  expect((await mock.issueSnapshot()).pullRequestComments).toHaveLength(4);
});

test("Issues mock mutations stay stateful and preserve metadata and state transitions", async ({
  page,
}) => {
  await installTauriMock(page, {
    commands: issueMockCommands(),
    issues: createIssuesFixture(),
  });
  await page.goto("/");

  const created = await invoke<IssueDetail>(page, "issues_create", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    title: "Mock-created issue",
    body: "Created through the snake_case IPC contract.",
    labels: ["bug", "priority: high"],
    assignees: ["alice", "octocat"],
    milestone: 2,
  });
  expect(created).toMatchObject({
    number: 43,
    title: "Mock-created issue",
    state: "open",
    labels: [{ name: "bug" }, { name: "priority: high" }],
    assignees: [{ login: "alice" }, { login: "octocat" }],
    milestone: { number: 2, title: "v1.1" },
  });

  const updated = await invoke<IssueDetail>(page, "issues_update", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 43,
    title: "Mock-updated issue",
    body: "Updated body",
    labels: ["enhancement"],
    assignees: ["bob"],
    milestone: null,
  });
  expect(updated).toMatchObject({
    title: "Mock-updated issue",
    body: "Updated body",
    labels: [{ name: "enhancement" }],
    assignees: [{ login: "bob" }],
    milestone: null,
  });

  const comment = await invoke<{ node_id: string; issue_number: number; body: string }>(
    page,
    "issues_comment",
    {
      repo_id: ISSUE_REPO_ID,
      remote: ISSUE_REMOTE,
      number: 43,
      body: "A deterministic mock comment",
    },
  );
  expect(comment.issue_number).toBe(43);
  expect(comment.body).toBe("A deterministic mock comment");

  const closed = await invoke<IssueDetail>(page, "issues_set_state", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 43,
    state: "closed",
    comment: "Closing this deterministic issue",
  });
  expect(closed).toMatchObject({
    state: "closed",
    state_reason: "completed",
    comment_count: 2,
  });
  expect(closed.closed_at).not.toBeNull();

  const reopened = await invoke<IssueDetail>(page, "issues_set_state", {
    repo_id: ISSUE_REPO_ID,
    remote: ISSUE_REMOTE,
    number: 43,
    state: "open",
    comment: null,
  });
  expect(reopened).toMatchObject({ state: "open", state_reason: null, closed_at: null });

  const persisted = await invoke<IssueListPage>(page, "issues_list", {
    ...listArgs,
    query: { ...listArgs.query, state: "all", search: "mock-updated" },
  });
  expect(persisted.total_count).toBeNull();
  expect(persisted.items.map((issue) => issue.number)).toEqual([43]);
});

test("structured ForgeError objects survive the mock rejection path", async ({ page }) => {
  const mock = await installTauriMock(page, {
    commands: issueMockCommands(),
    issues: createIssuesFixture(),
  });
  await page.goto("/");
  await mock.setIssueFailure("issues_list", ISSUE_FORGE_ERRORS.rateLimited);

  const rejected = await page.evaluate(async () => {
    try {
      const internals = (window as unknown as {
        __TAURI_INTERNALS__: { invoke(command: string, args: Record<string, unknown>): Promise<unknown> };
      }).__TAURI_INTERNALS__;
      await internals.invoke("issues_list", {
        repo_id: "r1",
        remote: "git@github.com:acme/repo.git",
        query: {
          state: "open",
          search: "",
          labels: [],
          assignees: [],
          milestone: "",
          sort: "updated",
          direction: "desc",
          per_page: 30,
          page: 1,
        },
      });
      return null;
    } catch (error) {
      return error;
    }
  });
  expect(rejected).toEqual(ISSUE_FORGE_ERRORS.rateLimited);
  expect(rejected).toHaveProperty("kind", "RateLimited");
  expect(rejected).toHaveProperty("http_status", 403);
  expect(rejected).toHaveProperty("retry_after_seconds", 60);
});
