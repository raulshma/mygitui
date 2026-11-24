/** Pure Issues query normalization, cache keys, pagination, and matching. */

import type {
  Issue,
  IssueDirection,
  IssueQuery,
  IssueSort,
  IssueState,
} from "./model";

export const DEFAULT_ISSUE_QUERY: IssueQuery = {
  state: "open",
  labels: [],
  assignees: [],
  milestone: null,
  sort: "updated",
  direction: "desc",
  per_page: 30,
  search: null,
};

const ISSUE_STATES = new Set<IssueState | "all">(["open", "closed", "all"]);
const ISSUE_SORTS = new Set<IssueSort>(["created", "updated", "comments"]);
const ISSUE_DIRECTIONS = new Set<IssueDirection>(["asc", "desc"]);

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.filter((v): v is string => typeof v === "string").map((v) => v.trim()).filter(Boolean))].sort(
    (a, b) => a.localeCompare(b),
  );
}

/** Clamps a positive, finite page number to an integer. */
export function normalizeIssuePage(page: number): number {
  return Number.isFinite(page) ? Math.max(1, Math.floor(page)) : 1;
}

/** Returns a fresh, deterministic query; invalid fields re-default. */
export function normalizeIssueQuery(query: Partial<IssueQuery> = {}): IssueQuery {
  const state = ISSUE_STATES.has(query.state as IssueState | "all")
    ? (query.state as IssueState | "all")
    : DEFAULT_ISSUE_QUERY.state;
  const sort = ISSUE_SORTS.has(query.sort as IssueSort)
    ? (query.sort as IssueSort)
    : DEFAULT_ISSUE_QUERY.sort;
  const direction = ISSUE_DIRECTIONS.has(query.direction as IssueDirection)
    ? (query.direction as IssueDirection)
    : DEFAULT_ISSUE_QUERY.direction;
  const perPage =
    typeof query.per_page === "number" && Number.isFinite(query.per_page)
      ? Math.min(100, Math.max(1, Math.floor(query.per_page)))
      : DEFAULT_ISSUE_QUERY.per_page;
  const search =
    typeof query.search === "string" ? query.search.trim().replace(/\s+/g, " ") || null : null;
  const milestone =
    typeof query.milestone === "string" && query.milestone.trim() !== ""
      ? query.milestone.trim()
      : null;
  return { state, labels: stringList(query.labels), assignees: stringList(query.assignees), milestone, sort, direction, per_page: perPage, search };
}

/** Stable serialization with a fixed property/array order. */
export function issueQueryKey(query: Partial<IssueQuery> = {}): string {
  const q = normalizeIssueQuery(query);
  return JSON.stringify([
    q.state,
    q.labels,
    q.assignees,
    q.milestone,
    q.sort,
    q.direction,
    q.per_page,
    q.search,
  ]);
}

/** All cache entries for one session repo id share this unambiguous prefix. */
export function issueCachePrefix(repoId: string): string {
  return `issues:${encodeURIComponent(repoId)}:`;
}

export function issueIdentityCacheKey(repoId: string): string {
  return `${issueCachePrefix(repoId)}identity`;
}

export function issueMetadataCacheKey(repoId: string): string {
  return `${issueCachePrefix(repoId)}metadata`;
}

export function issueListCacheKey(
  repoId: string,
  scope: string,
  query: Partial<IssueQuery>,
  page = 1,
): string {
  return `${issueCachePrefix(repoId)}list:${encodeURIComponent(scope)}:${encodeURIComponent(issueQueryKey(query))}:${normalizeIssuePage(page)}`;
}

export function issueDetailCacheKey(repoId: string, scope: string, number: number): string {
  return `${issueCachePrefix(repoId)}detail:${encodeURIComponent(scope)}:${normalizeIssuePage(number)}`;
}

export function issueCommentsCacheKey(repoId: string, scope: string, number: number): string {
  return `${issueCachePrefix(repoId)}comments:${encodeURIComponent(scope)}:${normalizeIssuePage(number)}`;
}

export interface IssuePagePosition {
  queryKey: string;
  page: number;
}

/** Any query change starts at page one, regardless of a caller's old page. */
export function resolveIssuePage(
  previous: IssuePagePosition | null,
  query: Partial<IssueQuery>,
  requestedPage: number,
): IssuePagePosition {
  const queryKey = issueQueryKey(query);
  return {
    queryKey,
    page: previous === null || previous.queryKey === queryKey
      ? normalizeIssuePage(requestedPage)
      : 1,
  };
}

/** Local cache-membership test; the backend remains the search authority. */
export function issueMatchesQuery(issue: Issue, input: Partial<IssueQuery>): boolean {
  const query = normalizeIssueQuery(input);
  if (query.state !== "all" && issue.state !== query.state) return false;
  const labels = new Set(issue.labels.map((label) => label.name));
  if (query.labels.some((label) => !labels.has(label))) return false;
  const assignees = new Set(issue.assignees.map((assignee) => assignee.login));
  if (query.assignees.length > 0 && !query.assignees.some((assignee) => assignees.has(assignee))) return false;
  if (query.milestone !== null && issue.milestone?.title !== query.milestone) return false;
  if (query.search !== null) {
    const needle = query.search.toLocaleLowerCase();
    const haystack = `${issue.title}\n${issue.body}`.toLocaleLowerCase();
    if (!haystack.includes(needle)) return false;
  }
  return true;
}

/** Sorts a patched list snapshot without mutating the input. */
export function sortIssues(issues: Issue[], query: Partial<IssueQuery>): Issue[] {
  const q = normalizeIssueQuery(query);
  const sign = q.direction === "asc" ? 1 : -1;
  return [...issues].sort((a, b) => {
    let comparison = 0;
    if (q.sort === "comments") comparison = a.comments_count - b.comments_count;
    else if (q.sort === "created") comparison = a.created_at.localeCompare(b.created_at);
    else comparison = a.updated_at.localeCompare(b.updated_at);
    return comparison * sign || a.number - b.number;
  });
}
