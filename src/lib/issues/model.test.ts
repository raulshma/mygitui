import { describe, expect, it, vi } from "vitest";
import {
  createIssuesTransport,
  normalizeIssueActivityBatch,
  normalizeIssueCommentPage,
  normalizeIssueIdentity,
  normalizeIssueListPage,
  normalizeIssueSummary,
  type IssuesIpcClient,
  type WireIssueDetail,
  type WireIssueIdentity,
  type WireIssueSummary,
} from "./model";
import { DEFAULT_ISSUE_QUERY } from "./query";

function wireIssue(overrides: Partial<WireIssueSummary> = {}): WireIssueSummary {
  return {
    node_id: "I_1",
    number: 1,
    title: "Issue",
    state: "open",
    author: { node_id: "U_1", login: "alice" },
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    closed_at: null,
    url: "https://github.com/octo/demo/issues/1",
    labels: [{ node_id: "L_1", name: "bug", color: "ff0000", description: null }],
    assignees: [{ node_id: "U_2", login: "bob" }],
    milestone: { number: 2, title: "v2", state: "open", due_on: null, html_url: "https://github.com/octo/demo/milestone/2" },
    comment_count: 3,
    locked: false,
    state_reason: null,
    ...overrides,
  };
}

function wireDetail(overrides: Partial<WireIssueDetail> = {}): WireIssueDetail {
  return { ...wireIssue(), body: "detail body", issue_type: null, ...overrides };
}

describe("Issues backend boundary", () => {
  it("normalizes Rust node IDs, flattened details, pagination, and identity", () => {
    const summary = normalizeIssueSummary(wireIssue());
    expect(summary).toMatchObject({ id: "I_1", body: "", comments_count: 3, author: { id: "U_1", avatar_url: null } });
    expect(normalizeIssueSummary(wireIssue({ state: "closed" })).state).toBe("closed");
    const list = normalizeIssueListPage({
      items: [wireIssue()], page: 2, per_page: 30, has_next: true, has_prev: true,
      total_count: 40, incomplete_results: false,
    });
    expect(list).toMatchObject({ has_previous: true, items: [{ id: "I_1" }] });
    const comments = normalizeIssueCommentPage({
      items: [{ node_id: "C_1", issue_number: 1, author: { node_id: "U_1", login: "alice" }, body: "comment", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", url: "https://github.com/octo/demo/issues/1#issuecomment-C_1" }],
      page: 3, per_page: 30, has_next: false, has_prev: true, total_count: null,
    });
    expect(comments).toMatchObject({ page: 3, has_earlier: true, previous_page: 2, items: [{ id: "C_1" }] });
    const identity: WireIssueIdentity = {
      executor: "device", device_id: "device-1", github_host: "github.example.com",
      owner: "octo", repo: "demo", remote: "upstream", user: { node_id: "U_self", login: "self" },
    };
    expect(normalizeIssueIdentity("repo-1", identity)).toEqual({
      repo_id: "repo-1", executor: "device", device_id: "device-1", host: "github.example.com",
      owner: "octo", repo: "demo", remote: "upstream", user_id: "U_self", user_login: "self",
    });
  });

  it("adapts command arguments and fills required fields for partial updates", async () => {
    const client: IssuesIpcClient = {
      issuesIdentity: vi.fn(async () => ({ executor: "local" as const, device_id: null, github_host: "github.com", owner: "octo", repo: "demo", remote: "origin", user: { node_id: "U_self", login: "self" } })),
      issuesList: vi.fn(async () => ({ items: [], page: 1, per_page: 30, has_next: false, has_prev: false, total_count: null, incomplete_results: false })),
      issuesDetail: vi.fn(async () => wireDetail()),
      issuesComments: vi.fn(async () => ({ items: [], page: 1, per_page: 30, has_next: false, has_prev: false, total_count: null })),
      issuesMetadata: vi.fn(async () => ({ labels: [], assignees: [], milestones: [] })),
      issuesCreate: vi.fn(async () => wireDetail()),
      issuesUpdate: vi.fn(async () => wireDetail({ title: "Updated" })),
      issuesComment: vi.fn(async () => ({ node_id: "C_1", issue_number: 1, author: { node_id: "U_self", login: "self" }, body: "new", created_at: "2026-01-02T00:00:00Z", updated_at: "2026-01-02T00:00:00Z", url: "https://github.com/octo/demo/issues/1#issuecomment-C_1" })),
      issuesSetState: vi.fn(async () => wireDetail({ state: "closed" })),
      issuesActivitySince: vi.fn(async () => ({ issues: [], comments: [], next_issue_cursor: null, next_comment_cursor: null, complete: true })),
    };
    const transport = createIssuesTransport(client, () => "upstream");
    await transport.issuesList("repo-1", { ...DEFAULT_ISSUE_QUERY, search: "hello world", assignees: ["bob", "carol"] }, 4);
    expect(client.issuesList).toHaveBeenCalledWith("repo-1", "upstream", expect.objectContaining({ page: 4, search: "hello world", assignees: ["bob", "carol"] }));
    await transport.issuesUpdate("repo-1", 1, { title: "Updated" });
    expect(client.issuesUpdate).toHaveBeenCalledWith(
      "repo-1", "upstream", 1, "Updated", "detail body", ["bug"], ["bob"], 2,
    );
    await transport.issuesComment("repo-1", 1, "new");
    expect(client.issuesComment).toHaveBeenCalledWith("repo-1", "upstream", 1, "new");
  });

  it("preserves activity comment titles when their issue is outside the current page", () => {
    const now = Date.parse("2026-02-01T00:00:00.000Z");
    const batch = normalizeIssueActivityBatch({
      issues: [],
      comments: [{
        ...wireDetail(),
        issue_number: 42,
        issue_title: "Later-page issue",
      }],
      next_issue_cursor: null,
      next_comment_cursor: null,
      complete: true,
    }, now);
    expect(batch.comments[0]).toMatchObject({ issue_number: 42, issue_title: "Later-page issue" });
    expect(batch.actual_issue_numbers).toEqual([42]);
  });

  it("normalizes bounded activity and preserves opaque continuation cursors", () => {
    const now = Date.parse("2026-02-01T00:00:00.000Z");
    const batch = normalizeIssueActivityBatch({
      issues: [wireIssue()],
      comments: [],
      next_issue_cursor: "/repos/octo/demo/issues?page=2",
      next_comment_cursor: null,
      complete: false,
    }, now);
    expect(batch).toMatchObject({
      scanned_at: "2026-02-01T00:00:00.000Z",
      actual_issue_numbers: [1],
      issues_complete: false,
      comments_complete: true,
      next_issue_cursor: "/repos/octo/demo/issues?page=2",
    });
  });
});
