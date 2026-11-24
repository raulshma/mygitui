/** Pure activity baseline/cursor/seen-ID/outbox reducer. */

import {
  boundIssueSyncState,
  type IssueActivityEvent,
  type IssueSyncState,
} from "./activity.svelte";
import {
  commentAuthorLogin,
  commentNodeKey,
  issueAuthorLogin,
  issueNodeKey,
  type Issue,
  type IssueActivityBatch,
  type IssueComment,
  type IssueIdentity,
} from "./model";

export const ACTIVITY_OVERLAP_MS = 2_000;

export interface ActivityScanOptions {
  now?: number;
  overlapMs?: number;
  /** Force a fresh baseline after an identity change. */
  baseline?: boolean;
  baselineIssues?: boolean;
  baselineComments?: boolean;
  /** Transaction hook used by `commitActivityScan`, not by the pure reducer. */
  persist?: (state: IssueSyncState) => void;
}

export interface ActivityScanResult {
  state: IssueSyncState;
  events: IssueActivityEvent[];
  continuation: boolean;
  persisted: boolean;
}

function time(value: string | null | undefined): number | null {
  if (typeof value !== "string") return null;
  // Date.parse accepts surprising path-like strings; Link cursors are opaque.
  if (value.startsWith("/") || value.includes("://")) return null;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function isOpaqueCursor(cursor: string | null): cursor is string {
  return cursor !== null && (cursor.startsWith("/") || cursor.includes("://"));
}

/** GitHub timestamps have second precision; opaque Link cursors are exact. */
export function activityOverlapCursor(
  cursor: string | null,
  overlapMs: number = ACTIVITY_OVERLAP_MS,
): string | null {
  if (isOpaqueCursor(cursor)) return cursor;
  const parsed = time(cursor);
  if (parsed === null) return null;
  return new Date(Math.max(0, parsed - Math.max(0, overlapMs))).toISOString();
}

function advanceCursor(
  prior: string | null,
  next: string | null,
  complete: boolean,
  scannedAt: string,
): string | null {
  if (next !== null) {
    const previousTime = time(prior);
    const nextTime = time(next);
    if (nextTime !== null && previousTime !== null && previousTime > nextTime) return prior;
    // Rust pagination continuations are opaque, same-endpoint Link paths.
    return next;
  }
  if (!complete) return prior;
  const previousTime = time(prior);
  const scannedTime = time(scannedAt);
  if (previousTime !== null && (scannedTime === null || previousTime > scannedTime)) return prior;
  return scannedAt;
}

function isPullRequest(issue: Issue): boolean {
  return issue.is_pr === true || Object.prototype.hasOwnProperty.call(issue, "pull_request");
}

function isSelfAuthored(authorId: string, login: string, identity: IssueIdentity | null): boolean {
  if (identity === null) return false;
  return (
    (authorId !== "" && authorId === identity.user_id) ||
    (login !== "" && login.toLocaleLowerCase() === identity.user_login.toLocaleLowerCase())
  );
}

function addSeen(ids: readonly string[], additions: readonly string[]): string[] {
  const additionsSet = new Set(additions);
  return [...additionsSet, ...ids.filter((id) => !additionsSet.has(id))].slice(0, 2_000);
}

function crosses(cursor: string | null, createdAt: string): boolean {
  const created = time(createdAt);
  const boundary = time(cursor);
  return created !== null && (boundary === null || created > boundary);
}

function eventFromIssue(issue: Issue, detectedAt: string): IssueActivityEvent | null {
  if (time(issue.created_at) === null) return null;
  return {
    id: issueNodeKey(issue.id),
    kind: "issue",
    issueId: issue.id,
    issueNumber: issue.number,
    commentId: null,
    title: issue.title.slice(0, 1_000),
    actor: issueAuthorLogin(issue).slice(0, 512),
    createdAt: issue.created_at,
    detectedAt,
    url: issue.url.slice(0, 4_000),
    readAt: null,
  };
}

function eventFromComment(
  comment: IssueComment,
  issue: Issue | undefined,
  detectedAt: string,
): IssueActivityEvent | null {
  if (time(comment.created_at) === null) return null;
  const issueId = issue?.id ?? `issue-number:${comment.issue_number}`;
  const title = issue?.title ?? comment.issue_title ?? "";
  return {
    id: commentNodeKey(comment.id),
    kind: "comment",
    issueId,
    issueNumber: comment.issue_number,
    commentId: comment.id,
    title: title.slice(0, 1_000),
    actor: commentAuthorLogin(comment).slice(0, 512),
    createdAt: comment.created_at,
    detectedAt,
    url: comment.url.slice(0, 4_000),
    readAt: null,
  };
}

/**
 * Computes the next state without mutating its inputs or performing I/O.
 * `persisted: false` denotes an uncommitted proposal; callers expose nothing
 * until `commitActivityScan` has durably written it.
 */
export function reduceActivity(
  prior: IssueSyncState,
  batch: IssueActivityBatch,
  options: ActivityScanOptions = {},
): ActivityScanResult {
  const now = options.now ?? Date.now();
  const detectedAt = new Date(now).toISOString();
  const baselineIssues =
    options.baselineIssues ??
    (options.baseline === true ||
      (prior.phase === "baseline-pending" && (prior.baselineIssues || prior.issueCursor === null)));
  const baselineComments =
    options.baselineComments ??
    (options.baseline === true ||
      (prior.phase === "baseline-pending" && (prior.baselineComments || prior.commentCursor === null)));

  const actualIssues = batch.issues.filter((issue) => !isPullRequest(issue));
  const byNumber = new Map(actualIssues.map((issue) => [issue.number, issue]));
  const allowedNumbers =
    batch.actual_issue_numbers === undefined
      ? null
      : new Set(batch.actual_issue_numbers.filter((number) => Number.isInteger(number) && number > 0));
  const comments = batch.comments.filter(
    (comment) => allowedNumbers === null || allowedNumbers.has(comment.issue_number),
  );

  // Newest candidates survive a bounded seen ring. Objects are marked seen
  // even when old, self-authored, or disabled; only genuinely new others emit.
  const seenIssueIds = addSeen(
    prior.seenIssueIds,
    actualIssues
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      .map((issue) => issueNodeKey(issue.id)),
  );
  const seenCommentIds = addSeen(
    prior.seenCommentIds,
    comments
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      .map((comment) => commentNodeKey(comment.id)),
  );

  const events: IssueActivityEvent[] = [];
  if (prior.newIssues && !baselineIssues) {
    for (const issue of actualIssues) {
      if (!crosses(prior.issueCursor, issue.created_at)) continue;
      if (prior.seenIssueIds.includes(issueNodeKey(issue.id))) continue;
      if (isSelfAuthored(issue.author.id, issueAuthorLogin(issue), prior.identity)) continue;
      const event = eventFromIssue(issue, detectedAt);
      if (event !== null) events.push(event);
    }
  }
  if (prior.newComments && !baselineComments) {
    for (const comment of comments) {
      if (!crosses(prior.commentCursor, comment.created_at)) continue;
      if (prior.seenCommentIds.includes(commentNodeKey(comment.id))) continue;
      if (isSelfAuthored(comment.author.id, commentAuthorLogin(comment), prior.identity)) continue;
      const event = eventFromComment(comment, byNumber.get(comment.issue_number), detectedAt);
      if (event !== null) events.push(event);
    }
  }

  events.sort((a, b) => b.createdAt.localeCompare(a.createdAt) || a.id.localeCompare(b.id));
  const existing = new Set(prior.outbox.map((event) => event.id));
  const unique = events.filter((event) => {
    if (existing.has(event.id)) return false;
    existing.add(event.id);
    return true;
  });
  const outbox = [...unique, ...prior.outbox]
    .sort((a, b) => b.detectedAt.localeCompare(a.detectedAt) || a.id.localeCompare(b.id))
    .slice(0, 200);

  const nextIssueCursor = advanceCursor(
    prior.issueCursor,
    batch.next_issue_cursor,
    batch.issues_complete,
    batch.scanned_at,
  );
  const nextCommentCursor = advanceCursor(
    prior.commentCursor,
    batch.next_comment_cursor,
    batch.comments_complete,
    batch.scanned_at,
  );
  const nextBaselineIssues = baselineIssues && !batch.issues_complete;
  const nextBaselineComments = baselineComments && !batch.comments_complete;
  const continuation = !batch.issues_complete || !batch.comments_complete;
  const phase = nextBaselineIssues || nextBaselineComments ? "baseline-pending" : "ready";
  const next = boundIssueSyncState(
    {
      ...prior,
      phase,
      issueCursor: nextIssueCursor,
      commentCursor: nextCommentCursor,
      seenIssueIds,
      seenCommentIds,
      baselineIssues: nextBaselineIssues,
      baselineComments: nextBaselineComments,
      lastSuccessAt: batch.scanned_at,
      lastError: null,
      outbox,
    },
    now,
  );
  return { state: next, events: unique, continuation, persisted: false };
}

/** Persists the complete proposal before returning any events to the caller. */
export function commitActivityScan(
  prior: IssueSyncState,
  batch: IssueActivityBatch,
  options: ActivityScanOptions = {},
): ActivityScanResult {
  const result = reduceActivity(prior, batch, options);
  if (options.persist === undefined) return result;
  try {
    options.persist(result.state);
  } catch {
    return {
      state: prior,
      events: [],
      continuation: false,
      persisted: false,
    };
  }
  return { ...result, persisted: true };
}
