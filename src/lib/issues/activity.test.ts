import { describe, expect, it, vi } from "vitest";
import {
  IssueActivityStore,
  OUTBOX_LIMIT,
  OUTBOX_MAX_AGE_MS,
  boundIssueSyncState,
  defaultIssueSyncState,
  issueSyncKey,
  parseIssueSyncState,
  readIssueSyncState,
  writeIssueSyncState,
  type IssueActivityEvent,
  type IssueSyncState,
} from "./activity.svelte";
import { MemoryStorage, identity } from "./testFixtures";

const ROOT = "c:/repos/demo";
const NOW = Date.parse("2026-02-01T00:00:00.000Z");

function event(index: number, overrides: Partial<IssueActivityEvent> = {}): IssueActivityEvent {
  const id = `I_${index}`;
  return {
    id: `issue:${id}`,
    kind: "issue",
    issueId: id,
    issueNumber: index + 1,
    commentId: null,
    title: `Issue ${index}`,
    actor: "alice",
    createdAt: new Date(NOW - index * 1_000).toISOString(),
    detectedAt: new Date(NOW - index * 1_000).toISOString(),
    url: `https://github.com/octo/demo/issues/${index + 1}`,
    readAt: null,
    ...overrides,
  };
}

function state(events: IssueActivityEvent[]): IssueSyncState {
  return { ...defaultIssueSyncState(), identity: identity(), newIssues: true, phase: "ready", outbox: events };
}

describe("Issues durable activity store", () => {
  it("restores durable unread counts and supports single/all mark-read", () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, state([event(1), event(2)]), NOW);
    const first = new IssueActivityStore({ storage, now: () => NOW });
    expect(first.unreadCount(ROOT)).toBe(2);
    expect(first.markRead(ROOT, "issue:I_1", NOW + 10)).toBe(true);
    expect(first.unreadCounts()[ROOT]).toBe(1);

    const restarted = new IssueActivityStore({ storage, now: () => NOW + 20 });
    expect(restarted.unreadCount(ROOT)).toBe(1);
    expect(restarted.markAllRead(ROOT, NOW + 20)).toBe(true);
    expect(restarted.unreadCount(ROOT)).toBe(0);
    expect(readIssueSyncState(storage, ROOT).outbox.every((entry) => entry.readAt !== null)).toBe(true);
  });

  it("bounds the outbox by count and age without retaining expired events", () => {
    const storage = new MemoryStorage();
    const expired = event(1_000, { detectedAt: new Date(NOW - OUTBOX_MAX_AGE_MS - 1).toISOString() });
    const recent = Array.from({ length: OUTBOX_LIMIT + 5 }, (_, index) => event(index));
    writeIssueSyncState(storage, ROOT, state([expired, ...recent]), NOW);
    const restored = readIssueSyncState(storage, ROOT);
    expect(restored.outbox).toHaveLength(OUTBOX_LIMIT);
    expect(restored.outbox.some((entry) => entry.id === expired.id)).toBe(false);
    expect(boundIssueSyncState(state([expired]), NOW).outbox).toEqual([]);
  });

  it("drops unknown credential/body fields rather than copying them", () => {
    const polluted = {
      ...state([event(1)]),
      token: "secret-token",
      body: "private issue body",
      outbox: [{ ...event(1), body: "private comment body", token: "nested-token" }],
    };
    const parsed = parseIssueSyncState(polluted);
    expect(parsed).not.toBeNull();
    expect(parsed).not.toHaveProperty("token");
    expect(parsed).not.toHaveProperty("body");
    expect(parsed?.outbox[0]).not.toHaveProperty("body");
    expect(parsed?.outbox[0]).not.toHaveProperty("token");

    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, polluted, NOW);
    const raw = storage.getItem(issueSyncKey(ROOT)) ?? "";
    expect(raw).not.toContain("secret-token");
    expect(raw).not.toContain("private issue body");
    expect(raw).not.toContain("private comment body");
  });

  it("coalesces multiple events into one toast with an Open action", () => {
    const storage = new MemoryStorage();
    const first = event(1);
    const second = event(2, { id: "comment:C_2", kind: "comment", commentId: "C_2", issueId: "I_2", issueNumber: 2 });
    writeIssueSyncState(storage, ROOT, state([first, second]), NOW);
    const showToast = vi.fn();
    const onOpen = vi.fn();
    const store = new IssueActivityStore({ storage, now: () => NOW, showToast });
    store.present(ROOT, store.entries(ROOT), { repoId: "repo-1", repoName: "octo/demo", onOpen });
    expect(showToast).toHaveBeenCalledOnce();
    expect(showToast.mock.calls[0]?.[0]).toContain("1 new issue and 1 comment");
    const options = showToast.mock.calls[0]?.[1];
    expect(options?.action?.label).toBe("Open");
    options?.action?.run();
    expect(onOpen).toHaveBeenCalledWith(first);
    expect(store.unreadCount(ROOT)).toBe(1);
  });

  it("deduplicates storage warnings and clears them after a durable retry", () => {
    const storage = new MemoryStorage();
    const showToast = vi.fn();
    const onRetry = vi.fn();
    const store = new IssueActivityStore({ storage, now: () => NOW, showToast, onRetry });
    storage.failWrites = true;
    expect(store.markAllRead(ROOT)).toBe(false);
    store.warnStorage(ROOT);
    expect(showToast).toHaveBeenCalledOnce();
    showToast.mock.calls[0]?.[1]?.action?.run();
    expect(onRetry).toHaveBeenCalledWith(ROOT);
    storage.failWrites = false;
    store.applyPersisted(ROOT, state([event(1)]));
    store.warnStorage(ROOT);
    expect(showToast).toHaveBeenCalledTimes(2);
  });
});
