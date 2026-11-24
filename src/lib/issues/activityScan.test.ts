import { describe, expect, it, vi } from "vitest";
import {
  defaultIssueSyncState,
  type IssueSyncState,
} from "./activity.svelte";
import {
  ACTIVITY_OVERLAP_MS,
  activityOverlapCursor,
  commitActivityScan,
  reduceActivity,
} from "./activityScan";
import { batch, comment, identity, issue } from "./testFixtures";

const CURSOR = "2026-01-01T12:00:00.000Z";

function ready(overrides: Partial<IssueSyncState> = {}): IssueSyncState {
  return {
    ...defaultIssueSyncState(),
    newIssues: true,
    newComments: true,
    phase: "ready",
    identity: identity(),
    issueCursor: CURSOR,
    commentCursor: CURSOR,
    ...overrides,
  };
}

describe("Issues activity reducer", () => {
  it("establishes a silent baseline and marks history seen", () => {
    const prior = {
      ...defaultIssueSyncState(),
      newIssues: true,
      newComments: true,
      phase: "baseline-pending" as const,
      identity: identity(),
      baselineIssues: true,
      baselineComments: true,
    };
    const result = reduceActivity(prior, batch({
      issues: [issue()],
      comments: [comment()],
    }), { now: Date.parse("2026-01-02") });
    expect(result.events).toEqual([]);
    expect(result.state.phase).toBe("ready");
    expect(result.state.seenIssueIds).toEqual(["issue:I_1"]);
    expect(result.state.seenCommentIds).toEqual(["comment:C_1"]);
    expect(result.state.outbox).toEqual([]);
  });

  it("catches up after restart using created_at, not updated_at", () => {
    const result = reduceActivity(ready(), batch({
      issues: [
        issue({ id: "I_new", number: 2, created_at: "2026-01-01T13:00:00.000Z", updated_at: "2026-01-01T13:00:00.000Z" }),
        issue({ id: "I_old", number: 1, created_at: "2025-12-01T00:00:00.000Z", updated_at: "2026-01-01T13:00:00.000Z" }),
      ],
    }));
    expect(result.events.map((event) => event.id)).toEqual(["issue:I_new"]);
    expect(result.state.issueCursor).toBe("2026-01-02T00:00:00.000Z");
  });

  it("ignores edited old comments but still marks their stable IDs seen", () => {
    const result = reduceActivity(ready(), batch({
      issues: [issue()],
      comments: [comment({ created_at: "2025-12-01T00:00:00.000Z", updated_at: "2026-01-01T13:00:00.000Z" })],
    }));
    expect(result.events).toEqual([]);
    expect(result.state.seenCommentIds).toContain("comment:C_1");
  });

  it("suppresses self-authored events while marking them seen", () => {
    const result = reduceActivity(ready(), batch({
      issues: [issue({ id: "I_self", created_at: "2026-01-01T13:00:00.000Z", author: { id: "user-self", login: "self", avatar_url: null } })],
      comments: [comment({ id: "C_self", created_at: "2026-01-01T13:00:00.000Z", author: { id: "other", login: "SELF", avatar_url: null } })],
    }));
    expect(result.events).toEqual([]);
    expect(result.state.seenIssueIds).toContain("issue:I_self");
    expect(result.state.seenCommentIds).toContain("comment:C_self");
  });

  it("filters PR comments through the resolved actual-issue set", () => {
    const result = reduceActivity(ready(), batch({
      issues: [issue()],
      actual_issue_numbers: [1],
      comments: [
        comment({ id: "C_issue", issue_number: 1, created_at: "2026-01-01T13:00:00.000Z" }),
        comment({ id: "C_pr", issue_number: 99, created_at: "2026-01-01T13:00:00.000Z" }),
      ],
    }));
    expect(result.events.map((event) => event.id)).toEqual(["comment:C_issue"]);
    expect(result.state.seenCommentIds).not.toContain("comment:C_pr");
  });

  it("emits a later-page comment with its transported issue title", () => {
    const result = reduceActivity(ready(), batch({
      comments: [comment({
        issue_number: 42,
        issue_title: "Later-page issue",
        created_at: "2026-01-01T13:00:00.000Z",
      })],
      actual_issue_numbers: [42],
    }));
    expect(result.events).toEqual([expect.objectContaining({
      kind: "comment",
      issueNumber: 42,
      title: "Later-page issue",
    })]);
  });

  it("is idempotent for duplicate batches and stable node IDs", () => {
    const scan = batch({ issues: [issue({ id: "I_new", created_at: "2026-01-01T13:00:00.000Z" })] });
    const first = reduceActivity(ready(), scan);
    const second = reduceActivity(first.state, scan);
    expect(first.events[0]?.id).toBe("issue:I_new");
    expect(second.events).toEqual([]);
    expect(second.state.outbox).toHaveLength(1);
  });

  it("advances partial progress only to explicit completed boundaries", () => {
    const first = reduceActivity(ready(), batch({
      issues_complete: false,
      comments_complete: false,
      next_issue_cursor: "2026-01-01T12:30:00.000Z",
      next_comment_cursor: "2026-01-01T12:45:00.000Z",
    }));
    expect(first.continuation).toBe(true);
    expect(first.state.issueCursor).toBe("2026-01-01T12:30:00.000Z");
    expect(first.state.commentCursor).toBe("2026-01-01T12:45:00.000Z");
    expect(first.state.issueCursor).not.toBe("2026-01-02T00:00:00.000Z");
  });

  it("advances nothing and emits nothing when atomic persistence fails", () => {
    const prior = ready();
    const persist = vi.fn(() => { throw new Error("quota"); });
    const result = commitActivityScan(prior, batch({
      issues: [issue({ id: "I_new", created_at: "2026-01-01T13:00:00.000Z" })],
    }), { persist });
    expect(persist).toHaveBeenCalledOnce();
    expect(result.persisted).toBe(false);
    expect(result.state).toBe(prior);
    expect(result.events).toEqual([]);
  });

  it("uses a two-second overlap without moving invalid cursors", () => {
    expect(ACTIVITY_OVERLAP_MS).toBe(2_000);
    expect(activityOverlapCursor(CURSOR)).toBe("2026-01-01T11:59:58.000Z");
    expect(activityOverlapCursor("/repos/octo/demo/issues?page=2")).toBe("/repos/octo/demo/issues?page=2");
    expect(activityOverlapCursor("bad")).toBeNull();
  });

  it("persists opaque Link progress exactly and timestamps only completed streams", () => {
    const link = "/repos/octo/demo/issues?page=2";
    const partial = reduceActivity(ready(), batch({
      issues_complete: false,
      comments_complete: true,
      next_issue_cursor: link,
      next_comment_cursor: null,
    }), { now: Date.parse("2026-01-03") });
    expect(partial.state.issueCursor).toBe(link);
    expect(partial.state.commentCursor).toBe("2026-01-02T00:00:00.000Z");
    const completed = reduceActivity(partial.state, batch({
      scanned_at: "2026-01-04T00:00:00.000Z",
      next_issue_cursor: null,
      next_comment_cursor: null,
    }), { now: Date.parse("2026-01-04") });
    expect(completed.state.issueCursor).toBe("2026-01-04T00:00:00.000Z");
  });
});
