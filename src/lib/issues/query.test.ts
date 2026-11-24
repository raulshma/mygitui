import { describe, expect, it } from "vitest";
import { issue } from "./testFixtures";
import {
  DEFAULT_ISSUE_QUERY,
  issueDetailCacheKey,
  issueListCacheKey,
  issueMatchesQuery,
  issueQueryKey,
  normalizeIssuePage,
  normalizeIssueQuery,
  resolveIssuePage,
  sortIssues,
} from "./query";

describe("Issues query normalization", () => {
  it("defaults malformed fields and clamps page size", () => {
    expect(normalizeIssueQuery({ state: "bad", sort: "bad", direction: "bad", per_page: 500 } as unknown as Parameters<typeof normalizeIssueQuery>[0])).toEqual({
      ...DEFAULT_ISSUE_QUERY,
      per_page: 100,
    });
    expect(normalizeIssueQuery({ per_page: 0 })).toEqual({ ...DEFAULT_ISSUE_QUERY, per_page: 1 });
    expect(normalizeIssuePage(Number.NaN)).toBe(1);
    expect(normalizeIssuePage(3.9)).toBe(3);
  });

  it("trims, deduplicates, and stably orders filter arrays/search text", () => {
    const query = normalizeIssueQuery({
      labels: [" bug ", "bug", "help", ""],
      assignees: ["bob", "alice", "bob"],
      milestone: "  v1  ",
      search: "  hello   world  ",
    });
    expect(query.labels).toEqual(["bug", "help"]);
    expect(query.assignees).toEqual(["alice", "bob"]);
    expect(query.milestone).toBe("v1");
    expect(query.search).toBe("hello world");
  });

  it("serializes equivalent queries to the same cache key", () => {
    const a = normalizeIssueQuery({ labels: ["b", "a"], assignees: ["z", "y"] });
    const b = normalizeIssueQuery({ assignees: ["y", "z"], labels: ["a", "b"] });
    expect(issueQueryKey(a)).toBe(issueQueryKey(b));
    expect(issueListCacheKey("repo:1", "scope", a)).toBe(issueListCacheKey("repo:1", "scope", b));
    expect(issueListCacheKey("repo:1", "scope-a", a)).not.toBe(issueListCacheKey("repo:1", "scope-b", a));
    expect(issueListCacheKey("repo:1", "scope", a, 1)).not.toBe(issueListCacheKey("repo:1", "scope", a, 2));
    expect(issueDetailCacheKey("repo:1", "scope", 0)).toContain(":1");
  });

  it("resets to page one whenever the normalized query changes", () => {
    const first = resolveIssuePage(null, DEFAULT_ISSUE_QUERY, 4);
    expect(first.page).toBe(4);
    expect(resolveIssuePage(first, DEFAULT_ISSUE_QUERY, 5).page).toBe(5);
    expect(resolveIssuePage(first, { ...DEFAULT_ISSUE_QUERY, state: "closed" }, 5).page).toBe(1);
  });
});

describe("Issues query derivations", () => {
  it("matches state, metadata, and submitted search text", () => {
    const row = issue({
      labels: [{ id: "L1", name: "bug", color: null, description: null }],
      assignees: [{ id: "U2", login: "bob", avatar_url: null }],
      milestone: { number: 1, title: "v1", state: "open", due_on: null, html_url: "https://github.com/octo/demo/milestone/1" },
    });
    expect(issueMatchesQuery(row, { state: "all", labels: ["bug"], assignees: ["bob"], milestone: "v1", search: "first" })).toBe(true);
    expect(issueMatchesQuery(row, { assignees: ["alice", "bob"] })).toBe(true);
    expect(issueMatchesQuery(row, { assignees: ["alice", "carol"] })).toBe(false);
    expect(issueMatchesQuery(row, { labels: ["feature"] })).toBe(false);
    expect(issueMatchesQuery(row, { state: "closed" })).toBe(false);
  });

  it("sorts a copy by the selected field and direction", () => {
    const rows = [
      issue({ id: "A", number: 1, comments_count: 1, created_at: "2026-01-01", updated_at: "2026-01-03" }),
      issue({ id: "B", number: 2, comments_count: 3, created_at: "2026-01-02", updated_at: "2026-01-01" }),
    ];
    expect(sortIssues(rows, { sort: "comments" }).map((row) => row.number)).toEqual([2, 1]);
    expect(sortIssues(rows, { sort: "created", direction: "asc" }).map((row) => row.number)).toEqual([1, 2]);
    expect(rows.map((row) => row.number)).toEqual([1, 2]);
  });
});
