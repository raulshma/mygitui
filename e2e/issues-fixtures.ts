/**
 * Deterministic GitHub Issues fixtures for the stateful Tauri mock.
 *
 * Everything here is an IPC wire shape (snake_case), not the frontend model.
 * Keep issue #1 rich: it is the browse/detail fixture and has 65 comments so
 * the comments pane must walk more than two 30-item pages.
 */

import type { ForgeError } from "../src/lib/ipc/gen/ForgeError";
import type { IssueComment } from "../src/lib/ipc/gen/IssueComment";
import type { IssueDetail } from "../src/lib/ipc/gen/IssueDetail";
import type { IssueIdentity } from "../src/lib/ipc/gen/IssueIdentity";
import type { IssueLabel } from "../src/lib/ipc/gen/IssueLabel";
import type { IssueMetadata } from "../src/lib/ipc/gen/IssueMetadata";
import type { IssueMilestone } from "../src/lib/ipc/gen/IssueMilestone";
import type { IssueSummary } from "../src/lib/ipc/gen/IssueSummary";
import type { IssueType } from "../src/lib/ipc/gen/IssueType";
import type { IssueUser } from "../src/lib/ipc/gen/IssueUser";

export const ISSUE_REPO_ID = "r1";
export const ISSUE_REMOTE = "git@github.com:acme/repo.git";
export const ISSUE_REPO_ROOT = "/tmp/repo";
export const ISSUE_REPO_LABEL = "acme/repo";

export type IssuesCommand =
  | "issues_identity"
  | "issues_list"
  | "issues_detail"
  | "issues_comments"
  | "issues_metadata"
  | "issues_create"
  | "issues_update"
  | "issues_comment"
  | "issues_set_state"
  | "issues_activity_since";

export interface IssuesMockOptions {
  identity?: IssueIdentity;
  issues?: IssueSummary[];
  details?: Record<number, IssueDetail>;
  comments?: IssueComment[];
  /** Kept in the fake backend but never returned by issue commands. */
  pullRequestComments?: IssueComment[];
  metadata?: IssueMetadata;
  /** Structured ForgeError replies, keyed by IPC command. */
  errors?: Partial<Record<IssuesCommand, ForgeError>>;
  /** Query-specific structured failures (key is lower-cased search text). */
  searchErrors?: Record<string, ForgeError>;
  /** Searches for which GitHub returns incomplete_results=true. */
  incompleteSearches?: string[];
  /** Artificial command latency in milliseconds. */
  latencyMs?: number;
  /** Bounds activity lanes and returns opaque Link-style continuations. */
  activityPageSize?: number;
}

export interface NewMockIssue {
  title: string;
  body?: string;
  author?: string;
  labels?: string[];
  assignees?: string[];
  milestone?: number | null;
  state?: "open" | "closed";
  createdAt?: string;
  updatedAt?: string;
  nodeId?: string;
}

export interface NewMockComment {
  issueNumber: number;
  body: string;
  author?: string;
  createdAt?: string;
  updatedAt?: string;
  nodeId?: string;
}

export interface IssueBackendSnapshot {
  issues: IssueSummary[];
  comments: IssueComment[];
  pullRequestComments: IssueComment[];
}

export const ISSUE_USERS: Record<string, IssueUser> = {
  self: { node_id: "U_SELF", login: "octocat" },
  alice: { node_id: "U_ALICE", login: "alice" },
  bob: { node_id: "U_BOB", login: "bob" },
  carol: { node_id: "U_CAROL", login: "carol" },
  dave: { node_id: "U_DAVE", login: "dave" },
};

export const ISSUE_LABELS: IssueLabel[] = [
  {
    node_id: "L_BUG",
    name: "bug",
    color: "d73a4a",
    description: "Something is not working",
  },
  {
    node_id: "L_ENHANCEMENT",
    name: "enhancement",
    color: "a2eeef",
    description: "Improve existing behavior",
  },
  {
    node_id: "L_DOCS",
    name: "documentation",
    color: "0075ca",
    description: "Documentation only",
  },
  {
    node_id: "L_GOOD_FIRST",
    name: "good first issue",
    color: "7057ff",
    description: "A friendly first contribution",
  },
  {
    node_id: "L_PRIORITY",
    name: "priority: high",
    color: "b60205",
    description: "Needs attention before the next release",
  },
  {
    node_id: "L_QUESTION",
    name: "question",
    color: "d876e3",
    description: "More information is requested",
  },
];

export const ISSUE_MILESTONES: IssueMilestone[] = [
  {
    number: 1,
    title: "v1.0",
    state: "open",
    due_on: "2026-02-15T00:00:00Z",
    html_url: "https://github.com/acme/repo/milestone/1",
  },
  {
    number: 2,
    title: "v1.1",
    state: "open",
    due_on: "2026-03-31T00:00:00Z",
    html_url: "https://github.com/acme/repo/milestone/2",
  },
  {
    number: 3,
    title: "Backlog",
    state: "open",
    due_on: null,
    html_url: "https://github.com/acme/repo/milestone/3",
  },
];

export const ISSUE_METADATA: IssueMetadata = {
  labels: ISSUE_LABELS,
  assignees: Object.values(ISSUE_USERS),
  milestones: ISSUE_MILESTONES,
};

export const ISSUE_IDENTITY: IssueIdentity = {
  executor: "local",
  device_id: null,
  github_host: "github.com",
  owner: "acme",
  repo: "repo",
  remote: ISSUE_REMOTE,
  user: ISSUE_USERS.self,
};

/** Structured failures are rejected as objects so ForgeIpcError survives. */
export const ISSUE_FORGE_ERRORS = {
  noGh: {
    kind: "NoGh",
    message: "GitHub CLI was not found on PATH",
    url: null,
    http_status: null,
    request_id: null,
    retry_after_seconds: null,
    rate_limit_reset_at: null,
  } satisfies ForgeError,
  notAuthed: {
    kind: "NotAuthed",
    message: "GitHub CLI is not authenticated",
    url: "https://github.com/login",
    http_status: 401,
    request_id: "req-not-authed",
    retry_after_seconds: null,
    rate_limit_reset_at: null,
  } satisfies ForgeError,
  offline: {
    kind: "Offline",
    message: "The network is unavailable",
    url: null,
    http_status: null,
    request_id: null,
    retry_after_seconds: null,
    rate_limit_reset_at: null,
  } satisfies ForgeError,
  permission: {
    kind: "Other",
    message: "Resource not accessible by integration",
    url: "https://api.github.com/repos/acme/repo/issues",
    http_status: 403,
    request_id: "req-issues-permission",
    retry_after_seconds: null,
    rate_limit_reset_at: null,
  } satisfies ForgeError,
  validation: {
    kind: "Other",
    message: "Validation Failed: title cannot be blank",
    url: "https://api.github.com/repos/acme/repo/issues",
    http_status: 422,
    request_id: "req-issues-validation",
    retry_after_seconds: null,
    rate_limit_reset_at: null,
  } satisfies ForgeError,
  rateLimited: {
    kind: "RateLimited",
    message: "API rate limit exceeded",
    url: "https://api.github.com/rate_limit",
    http_status: 403,
    request_id: "req-issues-rate-limit",
    retry_after_seconds: 60,
    rate_limit_reset_at: 1_900_000_000,
  } satisfies ForgeError,
} as const;

function user(login: string): IssueUser {
  return ISSUE_USERS[login] ?? { node_id: `U_${login.toUpperCase()}`, login };
}

function isoAt(base: string, minutes: number): string {
  return new Date(Date.parse(base) + minutes * 60_000).toISOString();
}

const SPECIAL_TITLES: Record<number, string> = {
  1: "Keyboard navigation skips the first issue",
  2: "Search quota errors need a retry affordance",
  3: "Narrow layout keeps the composer reachable",
  4: "Restore the selected issue after a silent refresh",
  5: "Show milestone and assignee metadata in issue details",
  6: "Copied issue links should use the canonical HTTPS URL",
};

const TITLE_TOPICS = [
  "Reduce duplicate repository scans",
  "Clarify the empty issues state",
  "Keep filter state across a restart",
  "Expose a readable activity baseline",
  "Avoid stale comment cursors after focus",
  "Support keyboard-only comment navigation",
  "Render label contrast accessibly",
  "Preserve list scroll while posting a comment",
  "Explain pull request comments are excluded",
  "Make the split divider keyboard resizable",
  "Use stable node ids for activity deduplication",
  "Handle closed issues without losing their discussion",
];

function summaryFor(number: number): IssueSummary {
  const closed = number > 40;
  const created = isoAt("2026-01-02T09:00:00.000Z", number * 37);
  // Issue #1 leads the default updated/desc list, followed by #2..#5.
  const updated =
    number <= 5
      ? isoAt("2026-01-20T12:00:00.000Z", (5 - number) * 90)
      : isoAt("2026-01-15T12:00:00.000Z", (42 - number) * 23);
  const labelNames = [
    ISSUE_LABELS[number % ISSUE_LABELS.length]!.name,
    ...(number % 3 === 0 ? [ISSUE_LABELS[4]!.name] : []),
    ...(number % 5 === 0 ? [ISSUE_LABELS[3]!.name] : []),
  ];
  const assigneeLogins = [
    number % 2 === 0 ? "alice" : "bob",
    ...(number % 4 === 0 ? ["octocat"] : []),
    ...(number % 7 === 0 ? ["carol"] : []),
  ];
  const milestone =
    number % 3 === 0 ? ISSUE_MILESTONES[0]! : number % 3 === 1 ? ISSUE_MILESTONES[1]! : null;
  const commentCount = number === 1 ? 65 : number <= 12 ? (number % 6) + 1 : number % 4;
  return {
    node_id: `I_${String(number).padStart(4, "0")}`,
    number,
    title:
      SPECIAL_TITLES[number] ??
      `${TITLE_TOPICS[(number - 7) % TITLE_TOPICS.length]} (${number})`,
    state: closed ? "closed" : "open",
    author: user(number % 6 === 0 ? "octocat" : number % 2 === 0 ? "alice" : "bob"),
    created_at: created,
    updated_at: updated,
    closed_at: closed ? isoAt(updated, 120) : null,
    url: `https://github.com/acme/repo/issues/${number}`,
    labels: ISSUE_LABELS.filter((label) => labelNames.includes(label.name)),
    assignees: assigneeLogins.map(user),
    milestone,
    comment_count: commentCount,
    locked: number === 7,
    state_reason: closed ? "completed" : null,
  };
}

function detailFor(summary: IssueSummary): IssueDetail {
  return {
    body: `## Context\n\n${summary.title}.\n\nThis deterministic fixture covers issue browsing, metadata, discussion pagination, and activity catch-up.\n\n### Reproduction\n\n1. Open **${ISSUE_REPO_LABEL}**.\n2. Select issue #${summary.number}.\n3. Verify the state remains stable after refresh.`,
    issue_type: {
      name: summary.labels.some((label) => label.name === "bug") ? "Bug" : "Task",
    } satisfies IssueType,
    ...summary,
  };
}

function commentFor(
  issueNumber: number,
  index: number,
  authorLogin: string,
  base = "2026-01-03T08:00:00.000Z",
): IssueComment {
  const created = isoAt(base, index * 17 + issueNumber);
  return {
    node_id: `C_I${String(issueNumber).padStart(4, "0")}_${String(index).padStart(3, "0")}`,
    issue_number: issueNumber,
    author: user(authorLogin),
    body:
      index % 5 === 0
        ? `Update ${index}: I reproduced this on issue #${issueNumber} and attached a minimal example.`
        : `Discussion update ${index} for issue #${issueNumber}. The behavior is deterministic and covered by E2E tests.`,
    created_at: created,
    updated_at: created,
    url: `https://github.com/acme/repo/issues/${issueNumber}#issuecomment-C_I${String(issueNumber).padStart(4, "0")}_${String(index).padStart(3, "0")}`,
  };
}

export function createIssuesFixture(): Required<
  Pick<IssuesMockOptions, "identity" | "issues" | "details" | "comments" | "pullRequestComments" | "metadata">
> {
  const issues = Array.from({ length: 42 }, (_, index) => summaryFor(index + 1));
  const details: Record<number, IssueDetail> = {};
  for (const issue of issues) details[issue.number] = detailFor(issue);

  const comments: IssueComment[] = [];
  for (const issue of issues) {
    const count = issue.comment_count;
    for (let index = 0; index < count; index += 1) {
      const authors = ["alice", "bob", "carol", "dave", "octocat"];
      comments.push(commentFor(issue.number, index, authors[index % authors.length]!));
    }
  }

  // These rows emulate PR comments already filtered out by the Rust backend.
  const pullRequestComments = Array.from({ length: 4 }, (_, index) =>
    commentFor(501, index, index % 2 === 0 ? "alice" : "octocat", "2026-01-04T08:00:00.000Z"),
  );

  return {
    identity: ISSUE_IDENTITY,
    issues,
    details,
    comments,
    pullRequestComments,
    metadata: ISSUE_METADATA,
  };
}

/** Commands/notifications shared by the focused Issues specs. */
export function issueMockCommands(): Record<string, unknown> {
  return {
    forge_status: { available: true, version: "2.63.3", authed: true },
    forge_context: {
      owner: "acme",
      repo: "repo",
      branch: "main",
      remote_url: ISSUE_REMOTE,
    },
    notifications_list: [
      {
        id: 101,
        kind: "issue",
        subject_title: "Search quota errors need a retry affordance",
        subject_url: "https://api.github.com/repos/acme/repo/issues/2",
        repository: "acme/repo",
        updated_at: "2026-01-20T11:00:00Z",
        reason: "mention",
      },
    ],
    notifications_mark_all_read: null,
    notification_mark_read: null,
  };
}
