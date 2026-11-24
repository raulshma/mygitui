/**
 * Issues domain contract and backend boundary.
 *
 * The `Wire*` aliases are generated IPC bindings from the Rust Issues
 * commands; the transport adapter normalizes them into the frontend shapes
 * used by the store. No credential is part of any shape.
 */

import * as ipc from "$lib/ipc/client";
import { loadForgeRemote } from "$lib/forge/remotePrefs";
import { repoRootFor } from "$lib/stores/tabs.svelte";
import type { IssueUser as WireIssueUser } from "$lib/ipc/gen/IssueUser";
import type { IssueLabel as WireIssueLabel } from "$lib/ipc/gen/IssueLabel";
import type { IssueMilestone as WireIssueMilestone } from "$lib/ipc/gen/IssueMilestone";
import type { IssueType as WireIssueType } from "$lib/ipc/gen/IssueType";
import type { IssueSummary as WireIssueSummary } from "$lib/ipc/gen/IssueSummary";
import type { IssueDetail as WireIssueDetail } from "$lib/ipc/gen/IssueDetail";
import type { IssueComment as WireIssueComment } from "$lib/ipc/gen/IssueComment";
import type { IssueActivityComment as WireIssueActivityComment } from "$lib/ipc/gen/IssueActivityComment";
import type { IssueListQuery as WireIssueListQuery } from "$lib/ipc/gen/IssueListQuery";
import type { IssueListPage as WireIssueListPage } from "$lib/ipc/gen/IssueListPage";
import type { IssueCommentPage as WireIssueCommentPage } from "$lib/ipc/gen/IssueCommentPage";
import type { IssueMetadata as WireIssueMetadata } from "$lib/ipc/gen/IssueMetadata";
import type { IssueIdentity as WireIssueIdentity } from "$lib/ipc/gen/IssueIdentity";
import type { IssueActivityBatch as WireIssueActivityBatch } from "$lib/ipc/gen/IssueActivityBatch";

export type {
  WireIssueUser,
  WireIssueLabel,
  WireIssueMilestone,
  WireIssueType,
  WireIssueSummary,
  WireIssueDetail,
  WireIssueComment,
  WireIssueActivityComment,
  WireIssueListQuery,
  WireIssueListPage,
  WireIssueCommentPage,
  WireIssueMetadata,
  WireIssueIdentity,
  WireIssueActivityBatch,
};

export type IssueState = "open" | "closed";
export type IssueSort = "created" | "updated" | "comments";
export type IssueDirection = "asc" | "desc";
export type IssueActivityKind = "issue" | "comment";

export interface IssueAuthor {
  id: string;
  login: string;
  avatar_url: string | null;
}

export interface IssueLabel {
  id: string;
  name: string;
  color: string | null;
  description: string | null;
}

export interface IssueAssignee {
  id: string;
  login: string;
  avatar_url: string | null;
}

export interface IssueMilestone {
  number: number;
  title: string;
  state: IssueState;
  due_on: string | null;
  html_url: string;
}

export interface IssueType {
  name: string;
}

/** Normalized issue. List summaries use an empty body until detail is loaded. */
export interface Issue {
  id: string;
  number: number;
  title: string;
  body: string;
  state: IssueState;
  author: IssueAuthor;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
  url: string;
  labels: IssueLabel[];
  assignees: IssueAssignee[];
  milestone: IssueMilestone | null;
  comments_count: number;
  locked: boolean;
  state_reason?: string | null;
  issue_type?: IssueType | null;
  /** Tolerant markers for defensive tests/older payloads. */
  is_pr?: boolean;
  pull_request?: unknown;
}

export interface IssueComment {
  id: string;
  issue_number: number;
  author: IssueAuthor;
  body: string;
  created_at: string;
  updated_at: string;
  url: string;
  /** Present only on repository-wide activity comments. */
  issue_title?: string;
}

export interface IssueQuery {
  state: IssueState | "all";
  labels: string[];
  assignees: string[];
  milestone: string | null;
  sort: IssueSort;
  direction: IssueDirection;
  per_page: number;
  /** Explicitly submitted GitHub search text; null means the list endpoint. */
  search: string | null;
}

export interface IssueListPage {
  items: Issue[];
  page: number;
  per_page: number;
  has_next: boolean;
  has_previous: boolean;
  total_count: number | null;
  incomplete_results: boolean;
}

export interface IssueCommentPage {
  items: IssueComment[];
  /** Null on a request for the final page; the response is always concrete. */
  page: number | null;
  per_page: number;
  has_earlier: boolean;
  has_later: boolean;
  previous_page: number | null;
  next_page: number | null;
}

export interface IssueMetadata {
  labels: IssueLabel[];
  assignees: IssueAssignee[];
  milestones: IssueMilestone[];
}

export interface IssueCreateInput {
  title: string;
  body?: string;
  labels?: string[];
  assignees?: string[];
  /** GitHub milestone number, not its mutable title. */
  milestone?: number | null;
}

export type IssueUpdateInput = Partial<IssueCreateInput>;

export interface IssueIdentity {
  repo_id: string;
  executor: "local" | "device";
  device_id: string | null;
  host: string;
  owner: string;
  repo: string;
  remote: string;
  user_id: string;
  user_login: string;
}

/** Normalized bounded activity page consumed by the pure reducer. */
export interface IssueActivityBatch {
  scanned_at: string;
  issues: Issue[];
  comments: IssueComment[];
  actual_issue_numbers?: number[];
  next_issue_cursor: string | null;
  next_comment_cursor: string | null;
  issues_complete: boolean;
  comments_complete: boolean;
}

/** The injectable seam used by stores/tests and by the real IPC adapter. */
export interface IssueTransport {
  issuesIdentity(repoId: string): Promise<IssueIdentity>;
  issuesList(repoId: string, query: IssueQuery, page: number): Promise<IssueListPage>;
  issuesDetail(repoId: string, number: number): Promise<Issue>;
  issuesComments(
    repoId: string,
    number: number,
    page: number | null,
    perPage: number,
  ): Promise<IssueCommentPage>;
  issuesMetadata(repoId: string): Promise<IssueMetadata>;
  issuesCreate(repoId: string, input: IssueCreateInput): Promise<Issue>;
  issuesUpdate(repoId: string, number: number, input: IssueUpdateInput): Promise<Issue>;
  issuesComment(repoId: string, number: number, body: string): Promise<IssueComment>;
  issuesSetState(
    repoId: string,
    number: number,
    state: IssueState,
    comment?: string | null,
  ): Promise<Issue>;
  issuesActivitySince(
    repoId: string,
    issueSince: string | null,
    commentSince: string | null,
  ): Promise<IssueActivityBatch>;
}

// -- Rust wire boundary ----------------------------------------------------

/** Current thin-command signatures; generated wrappers should match these. */
export type IssuesIpcClient = Pick<
  typeof ipc,
  | "issuesIdentity"
  | "issuesList"
  | "issuesDetail"
  | "issuesComments"
  | "issuesMetadata"
  | "issuesCreate"
  | "issuesUpdate"
  | "issuesComment"
  | "issuesSetState"
  | "issuesActivitySince"
>;

function state(value: string): IssueState {
  return value === "closed" ? "closed" : "open";
}

function author(value: WireIssueUser): IssueAuthor {
  return { id: value.node_id, login: value.login, avatar_url: null };
}

function label(value: WireIssueLabel): IssueLabel {
  return { id: value.node_id, name: value.name, color: value.color || null, description: value.description };
}

function milestone(value: WireIssueMilestone | null): IssueMilestone | null {
  return value === null
    ? null
    : {
        number: value.number,
        title: value.title,
        state: state(value.state),
        due_on: value.due_on,
        html_url: value.html_url,
      };
}

export function normalizeIssueSummary(value: WireIssueSummary): Issue {
  return {
    id: value.node_id,
    number: value.number,
    title: value.title,
    body: "",
    state: state(value.state),
    author: author(value.author),
    created_at: value.created_at,
    updated_at: value.updated_at,
    closed_at: value.closed_at,
    url: value.url,
    labels: value.labels.map(label),
    assignees: value.assignees.map((entry) => ({ id: entry.node_id, login: entry.login, avatar_url: null })),
    milestone: milestone(value.milestone),
    comments_count: value.comment_count,
    locked: value.locked,
    state_reason: value.state_reason,
    issue_type: null,
  };
}

export function normalizeIssueDetail(value: WireIssueDetail): Issue {
  return {
    ...normalizeIssueSummary(value),
    body: value.body,
    issue_type: value.issue_type,
  };
}

export function normalizeIssueComment(value: WireIssueComment): IssueComment {
  return {
    id: value.node_id,
    issue_number: value.issue_number,
    author: author(value.author),
    body: value.body,
    created_at: value.created_at,
    updated_at: value.updated_at,
    url: value.url,
  };
}

export function normalizeIssueActivityComment(value: WireIssueActivityComment): IssueComment {
  return { ...normalizeIssueComment(value), issue_title: value.issue_title };
}

export function normalizeIssueListPage(value: WireIssueListPage): IssueListPage {
  return {
    items: value.items.map(normalizeIssueSummary),
    page: value.page,
    per_page: value.per_page,
    has_next: value.has_next,
    has_previous: value.has_prev,
    total_count: value.total_count,
    incomplete_results: value.incomplete_results,
  };
}

export function normalizeIssueCommentPage(value: WireIssueCommentPage): IssueCommentPage {
  return {
    items: value.items.map(normalizeIssueComment),
    page: value.page,
    per_page: value.per_page,
    has_earlier: value.has_prev,
    has_later: value.has_next,
    previous_page: value.has_prev ? Math.max(1, value.page - 1) : null,
    next_page: value.has_next ? value.page + 1 : null,
  };
}

export function normalizeIssueMetadata(value: WireIssueMetadata): IssueMetadata {
  return {
    labels: value.labels.map(label),
    assignees: value.assignees.map((entry) => ({ id: entry.node_id, login: entry.login, avatar_url: null })),
    milestones: value.milestones.map((entry) => milestone(entry)!),
  };
}

export function normalizeIssueIdentity(repoId: string, value: WireIssueIdentity): IssueIdentity {
  return {
    repo_id: repoId,
    executor: value.executor === "device" ? "device" : "local",
    device_id: value.device_id,
    host: value.github_host,
    owner: value.owner,
    repo: value.repo,
    remote: value.remote,
    user_id: value.user.node_id,
    user_login: value.user.login,
  };
}

/**
 * A null next cursor means that stream completed. The client timestamp is the
 * only completion boundary available when Rust has no Link continuation.
 */
export function normalizeIssueActivityBatch(
  value: WireIssueActivityBatch,
  now = Date.now(),
): IssueActivityBatch {
  const issues = value.issues.map(normalizeIssueSummary);
  const comments = value.comments.map(normalizeIssueActivityComment);
  return {
    scanned_at: new Date(now).toISOString(),
    issues,
    comments,
    actual_issue_numbers: [...new Set([
      ...issues.map((issue) => issue.number),
      ...comments.map((comment) => comment.issue_number),
    ])],
    next_issue_cursor: value.next_issue_cursor,
    next_comment_cursor: value.next_comment_cursor,
    issues_complete: value.next_issue_cursor === null,
    comments_complete: value.next_comment_cursor === null,
  };
}

function wireQuery(query: IssueQuery, page: number): WireIssueListQuery {
  return {
    state: query.state,
    search: query.search ?? "",
    labels: query.labels,
    assignees: query.assignees,
    milestone: query.milestone ?? "",
    sort: query.sort,
    direction: query.direction,
    per_page: query.per_page,
    page,
  };
}

/** Defaults to the same persisted GitHub remote override as ForgePanel. */
export function defaultIssueRemote(repoId: string): string {
  const root = repoRootFor(repoId);
  return root === undefined ? "origin" : loadForgeRemote(root) ?? "origin";
}

function unavailable(name: string): never {
  throw new Error(`Issues IPC command ${name} is not available in this build`);
}

/**
 * Normalizing adapter around the backend-owned IPC wrappers. Namespace access
 * keeps this module importable while those wrappers are being added.
 */
export function createIssuesTransport(
  client: IssuesIpcClient = ipc,
  remoteFor: (repoId: string) => string = defaultIssueRemote,
): IssueTransport {
  const detail = async (repoId: string, number: number): Promise<Issue> => {
    const fn = client.issuesDetail;
    if (typeof fn !== "function") throw unavailable("issuesDetail");
    return normalizeIssueDetail(await fn(repoId, remoteFor(repoId), number));
  };

  return {
    issuesIdentity: async (repoId) => {
      const fn = client.issuesIdentity;
      if (typeof fn !== "function") throw unavailable("issuesIdentity");
      return normalizeIssueIdentity(repoId, await fn(repoId, remoteFor(repoId)));
    },
    issuesList: async (repoId, query, page) => {
      const fn = client.issuesList;
      if (typeof fn !== "function") throw unavailable("issuesList");
      return normalizeIssueListPage(await fn(repoId, remoteFor(repoId), wireQuery(query, page)));
    },
    issuesDetail: detail,
    issuesComments: async (repoId, number, page, perPage) => {
      const fn = client.issuesComments;
      if (typeof fn !== "function") throw unavailable("issuesComments");
      return normalizeIssueCommentPage(await fn(repoId, remoteFor(repoId), number, page, perPage));
    },
    issuesMetadata: async (repoId) => {
      const fn = client.issuesMetadata;
      if (typeof fn !== "function") throw unavailable("issuesMetadata");
      return normalizeIssueMetadata(await fn(repoId, remoteFor(repoId)));
    },
    issuesCreate: async (repoId, input) => {
      const fn = client.issuesCreate;
      if (typeof fn !== "function") throw unavailable("issuesCreate");
      return normalizeIssueDetail(await fn(
        repoId,
        remoteFor(repoId),
        input.title,
        input.body ?? "",
        input.labels ?? [],
        input.assignees ?? [],
        input.milestone ?? null,
      ));
    },
    issuesUpdate: async (repoId, number, input) => {
      const fn = client.issuesUpdate;
      if (typeof fn !== "function") throw unavailable("issuesUpdate");
      const needsCurrent =
        input.title === undefined ||
        input.body === undefined ||
        input.labels === undefined ||
        input.assignees === undefined ||
        input.milestone === undefined;
      const current = needsCurrent ? await detail(repoId, number) : null;
      return normalizeIssueDetail(await fn(
        repoId,
        remoteFor(repoId),
        number,
        input.title ?? current!.title,
        input.body ?? current!.body,
        input.labels ?? current!.labels.map((entry) => entry.name),
        input.assignees ?? current!.assignees.map((entry) => entry.login),
        input.milestone === undefined ? current!.milestone?.number ?? null : input.milestone,
      ));
    },
    issuesComment: async (repoId, number, body) => {
      const fn = client.issuesComment;
      if (typeof fn !== "function") throw unavailable("issuesComment");
      return normalizeIssueComment(await fn(repoId, remoteFor(repoId), number, body));
    },
    issuesSetState: async (repoId, number, issueState, comment) => {
      const fn = client.issuesSetState;
      if (typeof fn !== "function") throw unavailable("issuesSetState");
      return normalizeIssueDetail(await fn(repoId, remoteFor(repoId), number, issueState, comment ?? null));
    },
    issuesActivitySince: async (repoId, issueSince, commentSince) => {
      const fn = client.issuesActivitySince;
      if (typeof fn !== "function") throw unavailable("issuesActivitySince");
      return normalizeIssueActivityBatch(await fn(repoId, remoteFor(repoId), issueSince, commentSince));
    },
  };
}

export const issuesTransport = createIssuesTransport();

export function issueAuthorLogin(issue: Issue): string {
  return issue.author?.login ?? "";
}

export function commentAuthorLogin(comment: IssueComment): string {
  return comment.author?.login ?? "";
}

export function isIssueOpen(issue: Issue): boolean {
  return issue.state === "open";
}

/** Stable dedupe/outbox keys; never based on mutable titles or positions. */
export function issueNodeKey(id: string): string {
  return `issue:${id}`;
}

export function commentNodeKey(id: string): string {
  return `comment:${id}`;
}

/** Canonical runtime/persisted identity scope (contains no credentials). */
export function issueIdentityScope(identity: IssueIdentity): string {
  return JSON.stringify([
    identity.executor,
    identity.device_id,
    identity.host.trim().toLowerCase(),
    identity.owner.trim().toLowerCase(),
    identity.repo.trim().toLowerCase(),
    identity.remote.trim(),
    identity.user_id,
    identity.user_login.trim().toLowerCase(),
  ]);
}

/** Shared subscription rate-limit scope: all repos on one executor/device. */
export function issueExecutorScope(identity: IssueIdentity): string {
  return JSON.stringify([
    identity.executor,
    identity.device_id,
    identity.host.trim().toLowerCase(),
  ]);
}

export function issueDisplayName(identity: IssueIdentity): string {
  return `${identity.owner}/${identity.repo}`;
}
