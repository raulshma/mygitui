/** Shared deterministic fixtures for Issues domain tests. */

import type { StorageLike } from "$lib/state/persist";
import type {
  Issue,
  IssueActivityBatch,
  IssueAuthor,
  IssueComment,
  IssueIdentity,
  IssueTransport,
} from "./model";

export class MemoryStorage implements StorageLike {
  readonly values = new Map<string, string>();
  failWrites = false;

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    if (this.failWrites) throw new Error("storage full");
    this.values.set(key, value);
  }

  seed(key: string, value: unknown): void {
    this.values.set(key, JSON.stringify(value));
  }
}

export function author(id: string, login: string): IssueAuthor {
  return { id, login, avatar_url: null };
}

export function identity(overrides: Partial<IssueIdentity> = {}): IssueIdentity {
  return {
    repo_id: "repo-1",
    executor: "local",
    device_id: null,
    host: "github.com",
    owner: "octo",
    repo: "demo",
    remote: "origin",
    user_id: "user-self",
    user_login: "self",
    ...overrides,
  };
}

export function issue(overrides: Partial<Issue> = {}): Issue {
  return {
    id: "I_1",
    number: 1,
    title: "First issue",
    body: "Issue body must never enter subscription persistence",
    state: "open",
    author: author("U_1", "alice"),
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
    closed_at: null,
    url: "https://github.com/octo/demo/issues/1",
    labels: [],
    assignees: [],
    milestone: null,
    comments_count: 0,
    locked: false,
    ...overrides,
  };
}

export function comment(overrides: Partial<IssueComment> = {}): IssueComment {
  return {
    id: "C_1",
    issue_number: 1,
    author: author("U_1", "alice"),
    body: "Comment body must never enter subscription persistence",
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
    url: "https://github.com/octo/demo/issues/1#issuecomment-C_1",
    ...overrides,
  };
}

export function batch(overrides: Partial<IssueActivityBatch> = {}): IssueActivityBatch {
  return {
    scanned_at: "2026-01-02T00:00:00.000Z",
    issues: [],
    comments: [],
    next_issue_cursor: "2026-01-02T00:00:00.000Z",
    next_comment_cursor: "2026-01-02T00:00:00.000Z",
    issues_complete: true,
    comments_complete: true,
    ...overrides,
  };
}

export function transport(overrides: Partial<IssueTransport> = {}): IssueTransport {
  const fail = (name: string): never => {
    throw new Error(`Unexpected transport call: ${name}`);
  };
  return {
    issuesIdentity: async () => identity(),
    issuesList: async () => fail("issuesList"),
    issuesDetail: async () => fail("issuesDetail"),
    issuesComments: async () => fail("issuesComments"),
    issuesMetadata: async () => fail("issuesMetadata"),
    issuesCreate: async () => fail("issuesCreate"),
    issuesUpdate: async () => fail("issuesUpdate"),
    issuesComment: async () => fail("issuesComment"),
    issuesSetState: async () => fail("issuesSetState"),
    issuesActivitySince: async () => fail("issuesActivitySince"),
    ...overrides,
  };
}
