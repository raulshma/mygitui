/**
 * Issues read/mutation surface: AsyncMap caches with TTL/read-through,
 * newest-response guards, explicit bounds, cache patching, and repo eviction.
 */

import { describeError } from "$lib/errors";
import { repoRootFor } from "$lib/stores/tabs.svelte";
import { AsyncMap, type AsyncData } from "$lib/state/async.svelte";
import { registerRepoEvictor } from "$lib/state/repoEvictors";
import {
  defaultSyncStorage,
  markLocalActivitySeen,
  issueActivity,
} from "$lib/issues/activity.svelte";
import {
  issueIdentityScope,
  type Issue,
  type IssueComment,
  type IssueCommentPage,
  type IssueCreateInput,
  type IssueIdentity,
  type IssueListPage,
  type IssueMetadata,
  type IssueQuery,
  type IssueState,
  type IssueTransport,
  type IssueUpdateInput,
  issuesTransport,
} from "$lib/issues/model";
import {
  DEFAULT_ISSUE_QUERY,
  issueCachePrefix,
  issueCommentsCacheKey,
  issueDetailCacheKey,
  issueIdentityCacheKey,
  issueListCacheKey,
  issueMetadataCacheKey,
  issueMatchesQuery,
  normalizeIssuePage,
  normalizeIssueQuery,
  resolveIssuePage,
  sortIssues,
  type IssuePagePosition,
} from "$lib/issues/query";

export const ISSUE_LIST_TTL_MS = 60_000;
export const ISSUE_DETAIL_TTL_MS = 30_000;
export const ISSUE_COMMENTS_TTL_MS = 30_000;
export const ISSUE_IDENTITY_TTL_MS = 5 * 60_000;
export const ISSUE_METADATA_TTL_MS = 5 * 60_000;
export const ISSUE_LIST_CACHE_LIMIT = 20;
export const ISSUE_DETAIL_CACHE_LIMIT = 20;
export const ISSUE_COMMENT_PAGE_LIMIT = 5;
export const ISSUE_COMMENT_THREAD_LIMIT = 20;

export interface IssueCommentThread {
  issueNumber: number;
  pages: IssueCommentPage[];
}

export type IssueMutationOperation = "create" | "update" | "comment" | "setState";

export interface IssueBusyState {
  operation: IssueMutationOperation;
  issueNumber: number | null;
}

export class IssueMutationError extends Error {
  readonly code: "busy" | "mutation-failed";

  constructor(code: "busy" | "mutation-failed", message: string) {
    super(message);
    this.name = "IssueMutationError";
    this.code = code;
  }
}

export interface IssuesStoreOptions {
  transport?: IssueTransport;
  rootFor?: (repoId: string) => string | undefined;
  storage?: ReturnType<typeof defaultSyncStorage>;
}

interface ListMeta {
  repoId: string;
  scope: string;
  query: IssueQuery;
  page: number;
}

export class IssuesStore {
  readonly #transport: IssueTransport;
  readonly #rootFor: (repoId: string) => string | undefined;
  readonly #storage: ReturnType<typeof defaultSyncStorage>;

  readonly #identities = new AsyncMap<IssueIdentity>();
  readonly #lists = new AsyncMap<IssueListPage>();
  readonly #details = new AsyncMap<Issue>();
  readonly #comments = new AsyncMap<IssueCommentThread>();
  readonly #metadata = new AsyncMap<IssueMetadata>();

  #identityOrder: string[] = [];
  #listOrder: string[] = [];
  #detailOrder: string[] = [];
  #commentOrder: string[] = [];
  #metadataOrder: string[] = [];
  #listMeta = new Map<string, ListMeta>();
  #pagePositions = new Map<string, IssuePagePosition>();
  #identityScopes = new Map<string, string>();
  #repos = new Set<string>();

  #busy = $state<Record<string, IssueBusyState>>({});
  #mutationErrors = $state<Record<string, string | null>>({});

  constructor(options: IssuesStoreOptions = {}) {
    this.#transport = options.transport ?? issuesTransport;
    this.#rootFor = options.rootFor ?? repoRootFor;
    this.#storage =
      options.storage !== undefined ? options.storage : defaultSyncStorage();
    registerRepoEvictor((repoId) => this.evict(repoId));
  }

  identity(repoId: string): AsyncData<IssueIdentity> {
    return this.#identities.get(issueIdentityCacheKey(repoId));
  }

  identityFor(repoId: string): IssueIdentity | null {
    const entry = this.identity(repoId);
    return entry.state === "ready" ? entry.data : null;
  }

  list(
    repoId: string,
    query: Partial<IssueQuery> = DEFAULT_ISSUE_QUERY,
    page = 1,
  ): AsyncData<IssueListPage> {
    const identity = this.identityFor(repoId);
    if (identity === null) return { state: "idle" };
    return this.#lists.get(issueListCacheKey(repoId, issueIdentityScope(identity), query, page));
  }

  detail(repoId: string, number: number): AsyncData<Issue> {
    const identity = this.identityFor(repoId);
    if (identity === null) return { state: "idle" };
    return this.#details.get(issueDetailCacheKey(repoId, issueIdentityScope(identity), number));
  }

  comments(repoId: string, number: number): AsyncData<IssueCommentThread> {
    const identity = this.identityFor(repoId);
    if (identity === null) return { state: "idle" };
    return this.#comments.get(issueCommentsCacheKey(repoId, issueIdentityScope(identity), number));
  }

  metadata(repoId: string): AsyncData<IssueMetadata> {
    const identity = this.identityFor(repoId);
    if (identity === null) return { state: "idle" };
    return this.#metadata.get(issueMetadataCacheKey(repoId) + ":" + encodeURIComponent(issueIdentityScope(identity)));
  }

  busy(repoId: string, issueNumber: number | null = null): IssueBusyState | null {
    return this.#busy[this.#busyKey(repoId, issueNumber)] ?? null;
  }

  mutationError(repoId: string, issueNumber: number | null = null): string | null {
    return this.#mutationErrors[this.#busyKey(repoId, issueNumber)] ?? null;
  }

  async loadIdentity(repoId: string, force = false): Promise<IssueIdentity | null> {
    this.#repos.add(repoId);
    const key = issueIdentityCacheKey(repoId);
    const fetch = (): Promise<IssueIdentity> => this.#transport.issuesIdentity(repoId);
    const identity = force
      ? await this.#identities.run(key, fetch, { silent: this.#identities.get(key).state === "loading" })
      : await this.#identities.ensure(key, fetch, ISSUE_IDENTITY_TTL_MS);
    if (identity !== null) {
      this.#touch(this.#identityOrder, key);
      this.#bound(this.#identityOrder, this.#identities, ISSUE_LIST_CACHE_LIMIT);
      const scope = issueIdentityScope(identity);
      if (this.#identityScopes.get(repoId) !== scope) {
        this.#identityScopes.set(repoId, scope);
        this.#pagePositions.delete(repoId);
      }
    }
    return identity;
  }

  async loadList(
    repoId: string,
    query: Partial<IssueQuery> = DEFAULT_ISSUE_QUERY,
    requestedPage = 1,
    options: { force?: boolean; silent?: boolean } = {},
  ): Promise<IssueListPage | null> {
    const identity = await this.loadIdentity(repoId);
    if (identity === null) return null;
    const scope = issueIdentityScope(identity);
    const previous = this.#pagePositions.get(repoId);
    const position = resolveIssuePage(previous ?? null, query, requestedPage);
    const key = issueListCacheKey(repoId, scope, query, position.page);
    const normalized = normalizeIssueQuery(query);
    this.#listMeta.set(key, { repoId, scope, query: normalized, page: position.page });
    const fetch = (): Promise<IssueListPage> =>
      this.#transport.issuesList(repoId, normalized, position.page);
    const page = options.force
      ? await this.#lists.run(key, fetch, { silent: options.silent })
      : await this.#lists.ensure(key, fetch, ISSUE_LIST_TTL_MS);
    if (page !== null) {
      this.#touch(this.#listOrder, key);
      this.#bound(this.#listOrder, this.#lists, ISSUE_LIST_CACHE_LIMIT, (evicted) => {
        this.#listMeta.delete(evicted);
      });
      this.#pagePositions.set(repoId, { queryKey: position.queryKey, page: page.page });
    }
    return page;
  }

  async loadDetail(
    repoId: string,
    number: number,
    options: { force?: boolean; silent?: boolean } = {},
  ): Promise<Issue | null> {
    const identity = await this.loadIdentity(repoId);
    if (identity === null) return null;
    const key = issueDetailCacheKey(repoId, issueIdentityScope(identity), number);
    const fetch = (): Promise<Issue> => this.#transport.issuesDetail(repoId, number);
    const issue = options.force
      ? await this.#details.run(key, fetch, { silent: options.silent })
      : await this.#details.ensure(key, fetch, ISSUE_DETAIL_TTL_MS);
    if (issue !== null) {
      this.#touch(this.#detailOrder, key);
      this.#bound(this.#detailOrder, this.#details, ISSUE_DETAIL_CACHE_LIMIT);
    }
    return issue;
  }

  /** A null page asks the backend to resolve the final Link page. */
  async loadLatestComments(repoId: string, number: number, perPage = 30): Promise<IssueCommentThread | null> {
    const identity = await this.loadIdentity(repoId);
    if (identity === null) return null;
    const key = issueCommentsCacheKey(repoId, issueIdentityScope(identity), number);
    const fetch = async (): Promise<IssueCommentThread> => {
      const page = await this.#transport.issuesComments(repoId, number, null, perPage);
      return { issueNumber: number, pages: [page] };
    };
    const thread = await this.#comments.ensure(key, fetch, ISSUE_COMMENTS_TTL_MS);
    if (thread !== null) {
      this.#touch(this.#commentOrder, key);
      this.#bound(this.#commentOrder, this.#comments, ISSUE_COMMENT_THREAD_LIMIT);
    }
    return thread;
  }

  async loadEarlierComments(repoId: string, number: number): Promise<IssueCommentThread | null> {
    const identity = await this.loadIdentity(repoId);
    if (identity === null) return null;
    const key = issueCommentsCacheKey(repoId, issueIdentityScope(identity), number);
    const current = this.#comments.get(key);
    if (current.state !== "ready") return this.loadLatestComments(repoId, number);
    const first = [...current.data.pages].sort((a, b) => (a.page ?? 0) - (b.page ?? 0))[0];
    if (first === undefined || (!first.has_earlier && first.previous_page === null)) return current.data;
    const pageNumber = first.previous_page ?? Math.max(1, (first.page ?? 1) - 1);
    const fetch = async (): Promise<IssueCommentThread> => {
      const earlier = await this.#transport.issuesComments(repoId, number, pageNumber, first.per_page);
      const pages = [...current.data.pages.filter((page) => page.page !== earlier.page), earlier]
        .sort((a, b) => (a.page ?? 0) - (b.page ?? 0))
        .slice(-ISSUE_COMMENT_PAGE_LIMIT);
      return { issueNumber: number, pages };
    };
    const thread = await this.#comments.run(key, fetch);
    if (thread !== null) {
      this.#touch(this.#commentOrder, key);
      this.#bound(this.#commentOrder, this.#comments, ISSUE_COMMENT_THREAD_LIMIT);
    }
    return thread;
  }

  async loadMetadata(repoId: string): Promise<IssueMetadata | null> {
    const identity = await this.loadIdentity(repoId);
    if (identity === null) return null;
    const key = issueMetadataCacheKey(repoId) + ":" + encodeURIComponent(issueIdentityScope(identity));
    const metadata = await this.#metadata.ensure(
      key,
      () => this.#transport.issuesMetadata(repoId),
      ISSUE_METADATA_TTL_MS,
    );
    if (metadata !== null) {
      this.#touch(this.#metadataOrder, key);
      this.#bound(this.#metadataOrder, this.#metadata, ISSUE_LIST_CACHE_LIMIT);
    }
    return metadata;
  }

  async create(repoId: string, input: IssueCreateInput): Promise<Issue> {
    const identity = await this.#mutationIdentity(repoId, null, "create");
    const issue = await this.#mutate(repoId, null, "create", () =>
      this.#transport.issuesCreate(repoId, input),
    );
    this.#details.set(issueDetailCacheKey(repoId, issueIdentityScope(identity), issue.number), issue);
    this.#touch(this.#detailOrder, issueDetailCacheKey(repoId, issueIdentityScope(identity), issue.number));
    this.#bound(this.#detailOrder, this.#details, ISSUE_DETAIL_CACHE_LIMIT);
    this.#invalidateLists(repoId, issueIdentityScope(identity), issue, true);
    this.#markSeen(repoId, "issue", issue.id);
    return issue;
  }

  async update(repoId: string, number: number, input: IssueUpdateInput): Promise<Issue> {
    const identity = await this.#mutationIdentity(repoId, number, "update");
    const issue = await this.#mutate(repoId, number, "update", () =>
      this.#transport.issuesUpdate(repoId, number, input),
    );
    this.#details.set(issueDetailCacheKey(repoId, issueIdentityScope(identity), issue.number), issue);
    this.#touch(this.#detailOrder, issueDetailCacheKey(repoId, issueIdentityScope(identity), issue.number));
    this.#bound(this.#detailOrder, this.#details, ISSUE_DETAIL_CACHE_LIMIT);
    this.#invalidateLists(repoId, issueIdentityScope(identity), issue, false);
    this.#markSeen(repoId, "issue", issue.id);
    return issue;
  }

  async comment(repoId: string, number: number, body: string): Promise<IssueComment> {
    const identity = await this.#mutationIdentity(repoId, number, "comment");
    const comment = await this.#mutate(repoId, number, "comment", () =>
      this.#transport.issuesComment(repoId, number, body),
    );
    this.#patchComment(repoId, issueIdentityScope(identity), number, comment);
    this.#markSeen(repoId, "comment", comment.id);
    return comment;
  }

  async setState(
    repoId: string,
    number: number,
    state: IssueState,
    comment?: string | null,
  ): Promise<Issue> {
    const identity = await this.#mutationIdentity(repoId, number, "setState");
    const issue = await this.#mutate(repoId, number, "setState", () =>
      this.#transport.issuesSetState(repoId, number, state, comment),
    );
    this.#details.set(issueDetailCacheKey(repoId, issueIdentityScope(identity), issue.number), issue);
    this.#touch(this.#detailOrder, issueDetailCacheKey(repoId, issueIdentityScope(identity), issue.number));
    this.#bound(this.#detailOrder, this.#details, ISSUE_DETAIL_CACHE_LIMIT);
    this.#invalidateLists(repoId, issueIdentityScope(identity), issue, false);
    this.#markSeen(repoId, "issue", issue.id);
    return issue;
  }

  /** In-memory only. Durable preferences/cursors/outbox are intentionally kept. */
  evict(repoId: string): void {
    this.#identities.resetPrefix(issueCachePrefix(repoId));
    this.#lists.resetPrefix(issueCachePrefix(repoId));
    this.#details.resetPrefix(issueCachePrefix(repoId));
    this.#comments.resetPrefix(issueCachePrefix(repoId));
    this.#metadata.resetPrefix(issueCachePrefix(repoId));
    this.#identityOrder = this.#identityOrder.filter((key) => !key.startsWith(issueCachePrefix(repoId)));
    this.#listOrder = this.#listOrder.filter((key) => !key.startsWith(issueCachePrefix(repoId)));
    this.#detailOrder = this.#detailOrder.filter((key) => !key.startsWith(issueCachePrefix(repoId)));
    this.#commentOrder = this.#commentOrder.filter((key) => !key.startsWith(issueCachePrefix(repoId)));
    this.#metadataOrder = this.#metadataOrder.filter((key) => !key.startsWith(issueCachePrefix(repoId)));
    for (const key of [...this.#listMeta.keys()]) {
      if (key.startsWith(issueCachePrefix(repoId))) this.#listMeta.delete(key);
    }
    this.#pagePositions.delete(repoId);
    this.#identityScopes.delete(repoId);
    this.#repos.delete(repoId);
    this.#busy = Object.fromEntries(Object.entries(this.#busy).filter(([key]) => !key.startsWith(`${repoId}:`)));
    this.#mutationErrors = Object.fromEntries(Object.entries(this.#mutationErrors).filter(([key]) => !key.startsWith(`${repoId}:`)));
  }

  reset(): void {
    for (const repoId of [...this.#repos]) this.evict(repoId);
    this.#listMeta.clear();
    this.#pagePositions.clear();
    this.#busy = {};
    this.#mutationErrors = {};
  }

  #busyKey(repoId: string, issueNumber: number | null): string {
    return `${repoId}:${issueNumber ?? "new"}`;
  }

  async #mutationIdentity(
    repoId: string,
    number: number | null,
    operation: IssueMutationOperation,
  ): Promise<IssueIdentity> {
    const key = this.#busyKey(repoId, number);
    if (this.#busy[key] !== undefined) {
      throw new IssueMutationError("busy", "Another issue mutation is still in progress");
    }
    this.#busy = { ...this.#busy, [key]: { operation, issueNumber: number } };
    this.#mutationErrors = { ...this.#mutationErrors, [key]: null };
    try {
      const identity = await this.loadIdentity(repoId);
      if (identity === null) throw new Error("Could not resolve the GitHub Issues identity");
      return identity;
    } catch (err) {
      delete this.#busy[key];
      this.#mutationErrors = { ...this.#mutationErrors, [key]: describeError(err) };
      throw err;
    }
  }

  async #mutate<T>(
    repoId: string,
    number: number | null,
    operation: IssueMutationOperation,
    fn: () => Promise<T>,
  ): Promise<T> {
    const key = this.#busyKey(repoId, number);
    try {
      return await fn();
    } catch (err) {
      this.#mutationErrors = { ...this.#mutationErrors, [key]: describeError(err) };
      throw err;
    } finally {
      const next = { ...this.#busy };
      delete next[key];
      this.#busy = next;
    }
  }

  #invalidateLists(repoId: string, scope: string, issue: Issue, created: boolean): void {
    for (const [key, meta] of this.#listMeta) {
      if (meta.repoId !== repoId || meta.scope !== scope) continue;
      const entry = this.#lists.get(key);
      if (entry.state !== "ready") continue;
      const contained = entry.data.items.some((item) => item.id === issue.id || item.number === issue.number);
      const matches = issueMatchesQuery(issue, meta.query);
      if (contained || (created && matches) || matches) {
        this.#lists.reset(key);
        this.#listOrder = this.#listOrder.filter((entryKey) => entryKey !== key);
        this.#listMeta.delete(key);
      }
    }
  }

  #patchComment(repoId: string, scope: string, number: number, comment: IssueComment): void {
    const commentKey = issueCommentsCacheKey(repoId, scope, number);
    const thread = this.#comments.get(commentKey);
    if (thread.state === "ready") {
      const pages = thread.data.pages.map((page) => {
        if (page.has_later || page.next_page !== null) return page;
        const items = [...page.items.filter((item) => item.id !== comment.id), comment]
          .sort((a, b) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id));
        return { ...page, items };
      });
      if (pages.every((page) => page.items.every((item) => item.id !== comment.id))) {
        pages[pages.length - 1] = {
          ...pages[pages.length - 1]!,
          items: [...pages[pages.length - 1]!.items, comment],
        };
      }
      this.#comments.set(commentKey, { issueNumber: number, pages });
    }
    const detailKey = issueDetailCacheKey(repoId, scope, number);
    const detail = this.#details.get(detailKey);
    if (detail.state === "ready") {
      this.#details.set(detailKey, {
        ...detail.data,
        comments_count: detail.data.comments_count + 1,
        updated_at: comment.updated_at,
      });
    }
    // Comment counts/updated ordering can affect any list containing the issue.
    for (const [key, meta] of this.#listMeta) {
      if (meta.repoId !== repoId || meta.scope !== scope) continue;
      const entry = this.#lists.get(key);
      if (entry.state !== "ready") continue;
      const items = entry.data.items.map((issue) =>
        issue.number === number
          ? { ...issue, comments_count: issue.comments_count + 1, updated_at: comment.updated_at }
          : issue,
      );
      this.#lists.set(key, { ...entry.data, items: sortIssues(items, meta.query) });
      if (meta.query.sort === "updated" || meta.query.sort === "comments") {
        this.#lists.reset(key);
        this.#listOrder = this.#listOrder.filter((entryKey) => entryKey !== key);
        this.#listMeta.delete(key);
      }
    }
  }

  #markSeen(repoId: string, kind: "issue" | "comment", id: string): void {
    const root = this.#rootFor(repoId);
    if (root === undefined || root === "") return;
    try {
      markLocalActivitySeen(this.#storage, root, kind, id);
    } catch {
      issueActivity.warnStorage(root);
    }
  }

  #touch(order: string[], key: string): void {
    this.#touchOrder(order, key);
  }

  #touchOrder(order: string[], key: string): void {
    const index = order.indexOf(key);
    if (index !== -1) order.splice(index, 1);
    order.push(key);
  }

  #bound<T>(
    order: string[],
    map: AsyncMap<T>,
    limit: number,
    onEvict?: (key: string) => void,
  ): void {
    while (order.length > limit) {
      const oldest = order.shift();
      if (oldest !== undefined) {
        map.reset(oldest);
        onEvict?.(oldest);
      }
    }
  }
}

export const issuesStore = new IssuesStore();
