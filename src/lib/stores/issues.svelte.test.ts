import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetRepoEvictors, runRepoEvictors } from "$lib/state/repoEvictors";
import { issueIdentityScope } from "$lib/issues/model";
import type { IssueCommentPage, IssueListPage } from "$lib/issues/model";
import { comment, identity, issue, transport } from "$lib/issues/testFixtures";
import {
  ISSUE_LIST_CACHE_LIMIT,
  ISSUE_LIST_TTL_MS,
  IssueMutationError,
  IssuesStore,
} from "./issues.svelte";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function listPage(items = [issue()], page = 1): IssueListPage {
  return { items, page, per_page: 30, has_next: false, has_previous: page > 1, total_count: items.length, incomplete_results: false };
}

function commentPage(page: number | null, items = [comment()], overrides: Partial<IssueCommentPage> = {}): IssueCommentPage {
  return {
    items,
    page,
    per_page: 30,
    has_earlier: page !== null && page > 1,
    has_later: false,
    previous_page: page !== null && page > 1 ? page - 1 : null,
    next_page: null,
    ...overrides,
  };
}

describe("Issues store", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-01T00:00:00.000Z"));
    resetRepoEvictors();
  });
  afterEach(() => {
    vi.useRealTimers();
    resetRepoEvictors();
  });

  it("normalizes queries, caches identity, and resets pages when the query changes", async () => {
    const issuesIdentity = vi.fn(async () => identity());
    const issuesList = vi.fn(async (_repo: string, _query: unknown, page: number) => listPage([issue({ number: page })], page));
    const store = new IssuesStore({ transport: transport({ issuesIdentity, issuesList }), storage: null });
    await store.loadList("repo-1", { labels: [" bug ", "bug", "help"] }, 2);
    await store.loadList("repo-1", { labels: ["help", "bug"] }, 2);
    expect(issuesIdentity).toHaveBeenCalledOnce();
    expect(issuesList).toHaveBeenCalledOnce();
    expect(issuesList.mock.calls[0]?.[1]).toMatchObject({ labels: ["bug", "help"] });
    expect(issuesList.mock.calls[0]?.[2]).toBe(2);
    await store.loadList("repo-1", { labels: ["help", "bug"], state: "closed" }, 4);
    expect(issuesList.mock.calls[1]?.[2]).toBe(1);
    expect(store.identityFor("repo-1")).toEqual(identity());
    expect(issueIdentityScope(store.identityFor("repo-1")!)).toContain("user-self");
  });

  it("caches list pages independently", async () => {
    const issuesList = vi.fn(async (_repo: string, _query: unknown, page: number) =>
      listPage([issue({ number: page })], page),
    );
    const store = new IssuesStore({ transport: transport({ issuesList }), storage: null });
    await store.loadList("repo-1", {}, 1);
    await store.loadList("repo-1", {}, 2);
    await store.loadList("repo-1", {}, 1);
    expect(issuesList).toHaveBeenCalledTimes(2);
    expect(store.list("repo-1", {}, 1)).toMatchObject({ state: "ready", data: { page: 1 } });
    expect(store.list("repo-1", {}, 2)).toMatchObject({ state: "ready", data: { page: 2 } });
  });

  it("honors list TTL", async () => {
    const issuesList = vi.fn(async () => listPage([issue({ title: "initial" })]));
    const store = new IssuesStore({ transport: transport({ issuesList }), storage: null });
    await store.loadList("repo-1");
    await store.loadList("repo-1");
    expect(issuesList).toHaveBeenCalledOnce();
    vi.advanceTimersByTime(ISSUE_LIST_TTL_MS + 1);
    await store.loadList("repo-1");
    expect(issuesList).toHaveBeenCalledTimes(2);
  });

  it("suppresses an older forced response", async () => {
    const first = deferred<IssueListPage>();
    const second = deferred<IssueListPage>();
    const issuesList = vi.fn()
      .mockResolvedValueOnce(listPage([issue({ title: "initial" })]))
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const store = new IssuesStore({ transport: transport({ issuesList }), storage: null });
    await store.loadList("repo-1");
    const older = store.loadList("repo-1", {}, 1, { force: true, silent: true });
    const newer = store.loadList("repo-1", {}, 1, { force: true, silent: true });
    await vi.waitFor(() => expect(issuesList).toHaveBeenCalledTimes(3));
    second.resolve(listPage([issue({ title: "newest" })]));
    await newer;
    first.resolve(listPage([issue({ title: "stale" })]));
    expect(await older).toBeNull();
    expect(store.list("repo-1")).toMatchObject({ state: "ready", data: { items: [{ title: "newest" }] } });
  });

  it("loads the final comment page and prepends bounded earlier pages", async () => {
    const pages = new Map<number | null, IssueCommentPage>([
      [null, commentPage(3, [comment({ id: "C_3" })])],
      [2, commentPage(2, [comment({ id: "C_2" })])],
      [1, commentPage(1, [comment({ id: "C_1" })], { has_earlier: false, previous_page: null })],
    ]);
    const issuesComments = vi.fn(async (_repo: string, _number: number, page: number | null) => pages.get(page)!);
    const store = new IssuesStore({ transport: transport({ issuesComments }), storage: null });
    const latest = await store.loadLatestComments("repo-1", 1);
    expect(issuesComments).toHaveBeenNthCalledWith(1, "repo-1", 1, null, 30);
    expect(latest?.pages.map((page) => page.page)).toEqual([3]);
    const earlier = await store.loadEarlierComments("repo-1", 1);
    expect(earlier?.pages.map((page) => page.page)).toEqual([2, 3]);
    expect(earlier?.pages.flatMap((page) => page.items.map((entry) => entry.id))).toEqual(["C_2", "C_3"]);
  });

  it("patches details/comments and invalidates affected lists after mutations", async () => {
    const created = issue({ id: "I_new", number: 2, title: "Created" });
    const updated = issue({ title: "Updated", updated_at: "2026-02-01T01:00:00.000Z" });
    const newComment = comment({ id: "C_new", created_at: "2026-02-01T02:00:00.000Z" });
    const closed = { ...updated, state: "closed" as const };
    const issuesList = vi.fn(async () => listPage());
    const issuesDetail = vi.fn(async () => issue());
    const issuesComments = vi.fn(async () => commentPage(1));
    const store = new IssuesStore({
      storage: null,
      transport: transport({ issuesList, issuesDetail, issuesComments, issuesCreate: async () => created, issuesUpdate: async () => updated, issuesComment: async () => newComment, issuesSetState: async () => closed }),
    });
    await store.loadList("repo-1");
    await store.loadDetail("repo-1", 1);
    await store.loadLatestComments("repo-1", 1);

    await store.create("repo-1", { title: "Created" });
    expect(store.detail("repo-1", 2)).toMatchObject({ state: "ready", data: { title: "Created" } });
    expect(store.list("repo-1").state).toBe("idle");

    await store.loadList("repo-1");
    await store.update("repo-1", 1, { title: "Updated" });
    expect(store.detail("repo-1", 1)).toMatchObject({ state: "ready", data: { title: "Updated" } });
    expect(store.list("repo-1").state).toBe("idle");

    await store.loadList("repo-1");
    await store.comment("repo-1", 1, "local comment");
    expect(store.comments("repo-1", 1)).toMatchObject({ state: "ready", data: { pages: [{ items: [{ id: "C_1" }, { id: "C_new" }] }] } });
    expect(store.detail("repo-1", 1)).toMatchObject({ state: "ready", data: { comments_count: 1 } });
    expect(store.list("repo-1").state).toBe("idle");

    await store.setState("repo-1", 1, "closed", "done");
    expect(store.detail("repo-1", 1)).toMatchObject({ state: "ready", data: { state: "closed" } });
  });

  it("tracks per-issue busy/error state and clears it after failures", async () => {
    const pending = deferred<ReturnType<typeof issue>>();
    const issuesUpdate = vi.fn()
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValueOnce(issue({ title: "ok" }));
    const store = new IssuesStore({ transport: transport({ issuesUpdate }), storage: null });
    const mutation = store.update("repo-1", 1, { title: "waiting" });
    await vi.waitFor(() => expect(store.busy("repo-1", 1)?.operation).toBe("update"));
    await expect(store.update("repo-1", 1, {})).rejects.toBeInstanceOf(IssueMutationError);
    pending.reject(new Error("write failed"));
    await expect(mutation).rejects.toThrow("write failed");
    expect(store.busy("repo-1", 1)).toBeNull();
    expect(store.mutationError("repo-1", 1)).toContain("write failed");
    await store.update("repo-1", 1, { title: "ok" });
    expect(store.mutationError("repo-1", 1)).toBeNull();
  });

  it("evicts in-memory entries and invalidates in-flight reads without deleting durable sync state", async () => {
    const pending = deferred<ReturnType<typeof issue>>();
    const issuesDetail = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValueOnce(issue({ title: "reopened" }));
    const store = new IssuesStore({ transport: transport({ issuesDetail }), storage: null });
    const load = store.loadDetail("repo-1", 1);
    await vi.waitFor(() => expect(issuesDetail).toHaveBeenCalledOnce());
    runRepoEvictors("repo-1");
    pending.resolve(issue({ title: "must not land" }));
    expect(await load).toBeNull();
    expect(store.detail("repo-1", 1).state).toBe("idle");
    await store.loadDetail("repo-1", 1);
    expect(store.detail("repo-1", 1)).toMatchObject({ state: "ready", data: { title: "reopened" } });
  });

  it("bounds the detail cache", async () => {
    const store = new IssuesStore({
      storage: null,
      transport: transport({ issuesDetail: async (_repo, number) => issue({ number }) }),
    });
    for (let number = 1; number <= ISSUE_LIST_CACHE_LIMIT + 1; number += 1) await store.loadDetail("repo-1", number);
    expect(store.detail("repo-1", 1).state).toBe("idle");
    expect(store.detail("repo-1", ISSUE_LIST_CACHE_LIMIT + 1).state).toBe("ready");
  });
});
