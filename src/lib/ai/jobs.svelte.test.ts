/**
 * Unit tests for the AI job store (M13): the ticket lifecycle (phase /
 * serving / delta / usage / prompt preview / terminal transitions), cancel
 * semantics, the capped persisted history ring, and aggregate usage.
 */

import { describe, expect, it } from "vitest";
import {
  AiJobStore,
  JOB_HISTORY_CAP,
  JOB_HISTORY_KEY,
  JOB_PROMPT_PREVIEW_CAP,
  JOB_STREAM_CAP,
  JOB_TEXT_CAP,
  parseJobHistory,
} from "./jobs.svelte";
import type { AiJobRecord } from "./jobs.svelte";

function memStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return {
    getItem: (key: string) => data[key] ?? null,
    setItem: (key: string, value: string) => {
      data[key] = value;
    },
    removeItem: (key: string) => {
      delete data[key];
    },
    dump: () => ({ ...data }),
  };
}

type Storage = ReturnType<typeof memStorage>;

/** Deterministic ids + clock for reproducible history order. */
let counter = 0;
function makeStore(storage: Storage | null = memStorage()) {
  let now = 1_000_000;
  counter = 0;
  const store = new AiJobStore({
    storage,
    id: () => `job-${++counter}`,
    now: () => (now += 100),
  });
  return { store, storage };
}

function record(overrides: Partial<AiJobRecord> | Record<string, unknown> = {}): AiJobRecord {
  counter += 1;
  return {
    id: `rec-${counter}`,
    repoId: "r1",
    repoName: "repo",
    feature: "commit-message",
    status: "done",
    startedAt: 1_000,
    elapsedMs: 500,
    backend: "opencode",
    model: "m",
    usage: { inputTokens: 10, outputTokens: 20, totalTokens: 30 },
    error: null,
    text: "out",
    promptPreview: "prompt",
    ...(overrides as Partial<AiJobRecord>),
  };
}

describe("AiJobStore lifecycle", () => {
  it("begin adds a queued job and the ticket advances phases", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", repoName: "repo", feature: "commit-message" });
    expect(store.active[ticket.id]).toMatchObject({
      id: ticket.id,
      repoId: "r1",
      repoName: "repo",
      feature: "commit-message",
      phase: "queued",
      message: null,
    });

    ticket.phase("gathering", "reading staged diff");
    expect(store.active[ticket.id]).toMatchObject({ phase: "gathering", message: "reading staged diff" });

    ticket.serving("opencode", "m-1");
    ticket.delta("he");
    ticket.delta("llo");
    ticket.usage({ totalTokens: 12 });
    expect(store.active[ticket.id]).toMatchObject({
      backend: "opencode",
      model: "m-1",
      streamedText: "hello",
      usage: { totalTokens: 12 },
    });

    ticket.done({
      backend: "opencode",
      model: "m-1",
      elapsedMs: 300,
      usage: { totalTokens: 12 },
      text: "hello",
    });
    expect(store.active[ticket.id]).toBeUndefined();
    expect(store.history[0]).toMatchObject({
      feature: "commit-message",
      status: "done",
      text: "hello",
      elapsedMs: 300,
    });
  });

  it("promptPreview is capped and lands in the history record", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "review-staged" });
    ticket.promptPreview("x".repeat(5_000));
    ticket.done({ backend: "opencode", model: null, elapsedMs: 1, usage: null, text: null });
    const record = store.history[0]!;
    expect(record.promptPreview?.length).toBe(2_000);
  });

  it("delta caps the live stream buffer (runaway model safety valve)", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "chat" });
    ticket.delta("a".repeat(600_000));
    expect(store.active[ticket.id]?.streamedText).toBeNull(); // over cap → dropped
    ticket.phase("generating");
    // A fresh small delta after an oversized one still lands (the store keeps
    // the last-good buffer rather than wedging).
    ticket.delta("ok");
    expect(store.active[ticket.id]?.streamedText).toBe("ok");
  });

  it("cancelled keeps the partial stream and moves to history", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "chat" });
    ticket.serving("openrouter", "or-1");
    ticket.delta("partial ");
    ticket.delta("answer");
    ticket.cancelled();
    expect(store.active[ticket.id]).toBeUndefined();
    expect(store.history[0]).toMatchObject({
      status: "cancelled",
      text: "partial answer",
      backend: "openrouter",
    });
  });

  it("failed records the safe error message", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "explain-hunk" });
    ticket.failed("backend did not finish in time");
    expect(store.history[0]).toMatchObject({ status: "error", error: "backend did not finish in time" });
  });

  it("late terminal calls after finish are ignored", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "commit-message" });
    ticket.done({ backend: "opencode", model: null, elapsedMs: 1, usage: null, text: "a" });
    ticket.failed("late");
    expect(store.history).toHaveLength(1);
  });
});

describe("AiJobStore cancel", () => {
  it("cancel aborts the ticket signal and flags the request", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "chat" });
    expect(ticket.signal.aborted).toBe(false);
    expect(store.cancelRequested(ticket.id)).toBe(false);
    store.cancel(ticket.id);
    expect(ticket.signal.aborted).toBe(true);
    expect(store.cancelRequested(ticket.id)).toBe(true);
  });

  it("cancelAll aborts every active job", () => {
    const { store } = makeStore();
    const a = store.begin({ repoId: "r1", feature: "chat" });
    const b = store.begin({ repoId: "r2", feature: "review-staged" });
    store.cancelAll();
    expect(a.signal.aborted).toBe(true);
    expect(b.signal.aborted).toBe(true);
  });

  it("finishing clears the cancel flag", () => {
    const { store } = makeStore();
    const ticket = store.begin({ repoId: "r1", feature: "chat" });
    store.cancel(ticket.id);
    ticket.cancelled();
    expect(store.cancelRequested(ticket.id)).toBe(false);
  });
});

describe("AiJobStore queries", () => {
  it("activeForRepo / activeJob filter by repo and feature", () => {
    const { store } = makeStore();
    store.begin({ repoId: "r1", feature: "chat" });
    store.begin({ repoId: "r1", feature: "review-staged" });
    store.begin({ repoId: "r2", feature: "chat" });

    expect(store.activeForRepo("r1")).toHaveLength(2);
    expect(store.activeJob("r1", "chat")?.feature).toBe("chat");
    expect(store.activeJob("r1", "review-staged")?.feature).toBe("review-staged");
    expect(store.activeJob("r2")?.repoId).toBe("r2");
    expect(store.activeJob("nope")).toBeNull();
  });

  it("clearHistory empties memory and storage", () => {
    const storage = memStorage();
    const store = makeStore(storage).store;
    const ticket = store.begin({ repoId: "r1", feature: "chat" });
    ticket.done({ backend: "opencode", model: null, elapsedMs: 1, usage: null, text: "a" });
    expect(store.history).toHaveLength(1);
    store.clearHistory();
    expect(store.history).toHaveLength(0);
    expect(JSON.parse(storage.dump()[JOB_HISTORY_KEY]!).data).toEqual([]);
  });
});

describe("AiJobStore history persistence", () => {
  it("persists history in the versioned envelope and reloads it", () => {
    const storage = memStorage();
    const first = makeStore(storage).store;
    const ticket = first.begin({ repoId: "r1", repoName: "repo", feature: "commit-message" });
    ticket.promptPreview("the prompt");
    ticket.done({ backend: "opencode", model: "m", elapsedMs: 10, usage: { totalTokens: 3 }, text: "out" });

    const raw = storage.dump()[JOB_HISTORY_KEY]!;
    expect(JSON.parse(raw).v).toBe(1);
    const second = new AiJobStore({ storage });
    expect(second.history).toHaveLength(1);
    expect(second.history[0]).toMatchObject({
      feature: "commit-message",
      status: "done",
      text: "out",
      promptPreview: "the prompt",
    });
  });

  it("caps the history ring at JOB_HISTORY_CAP entries (newest first)", () => {
    const storage = memStorage();
    const flood = Array.from({ length: JOB_HISTORY_CAP + 30 }, (_, i) =>
      record({ id: `rec-${i}`, startedAt: i }),
    );
    storage.setItem(JOB_HISTORY_KEY, JSON.stringify({ v: 1, data: flood.reverse() }));
    const store = new AiJobStore({ storage });
    expect(store.history).toHaveLength(JOB_HISTORY_CAP);
    // Newest (highest startedAt) first.
    expect(store.history[0]!.id).toBe(`rec-${JOB_HISTORY_CAP + 29}`);
  });

  it("parseJobHistory drops malformed records but keeps valid ones", () => {
    const parsed = parseJobHistory([
      record(),
      { id: "bad" },
      null,
      record({ id: "bad-status", status: "wat" }),
      record({ id: "bad-time", startedAt: "nope" }),
    ]);
    expect(parsed).not.toBeNull();
    // Only the fully-valid record survives; the rest drop silently.
    expect(parsed).toHaveLength(1);
    expect(parsed![0]!.id).toMatch(/^rec-/);
    expect(parseJobHistory("junk")).toBeNull();
    expect(parseJobHistory(null)).toBeNull();
  });

  it("a missing storage never throws (persistence disabled)", () => {
    const { store } = makeStore(null);
    const ticket = store.begin({ repoId: "r1", feature: "chat" });
    ticket.done({ backend: "opencode", model: null, elapsedMs: 1, usage: null, text: "a" });
    expect(store.history).toHaveLength(1);
  });
});

describe("AiJobStore usageSince", () => {
  it("aggregates runs and tokens after the cutoff, per feature", () => {
    const storage = memStorage();
    storage.setItem(
      JOB_HISTORY_KEY,
      JSON.stringify({
        v: 1,
        data: [
          record({ id: "old", startedAt: 500, feature: "commit-message" }),
          record({
            id: "new-1",
            startedAt: 2_000,
            feature: "chat",
            usage: { inputTokens: 10, outputTokens: 5, totalTokens: 15, costUsd: 0.01 },
          }),
          record({
            id: "new-2",
            startedAt: 3_000,
            feature: "chat",
            usage: { totalTokens: 25 },
            status: "error",
          }),
          record({ id: "new-3", startedAt: 3_500, feature: "review-staged", usage: null }),
        ],
      }),
    );
    const store = new AiJobStore({ storage });
    const summary = store.usageSince(1_000);
    expect(summary.runs).toBe(3);
    expect(summary.totalTokens).toBe(40);
    expect(summary.inputTokens).toBe(10);
    expect(summary.outputTokens).toBe(5);
    expect(summary.costUsd).toBeCloseTo(0.01);
    expect(summary.byFeature.chat).toEqual({ runs: 2, totalTokens: 40 });
    expect(summary.byFeature["review-staged"]).toEqual({ runs: 1, totalTokens: 0 });
    // Everything before the cutoff is excluded.
    expect(store.usageSince(10_000).runs).toBe(0);
  });
});

describe("text caps on every terminal path", () => {
  it("caps the response text at JOB_TEXT_CAP for done, cancelled AND failed", () => {
    const { store } = makeStore(null);

    const done = store.begin({ repoId: "r1", feature: "review-staged" });
    done.done({
      backend: "opencode",
      model: "m",
      elapsedMs: 1,
      usage: null,
      text: "x".repeat(JOB_TEXT_CAP + 5_000),
    });

    const failed = store.begin({ repoId: "r1", feature: "review-staged" });
    failed.delta("y".repeat(JOB_STREAM_CAP));
    failed.failed("boom");

    const cancelled = store.begin({ repoId: "r1", feature: "review-staged" });
    cancelled.delta("z".repeat(JOB_STREAM_CAP));
    cancelled.cancelled();

    // 200 history records × an uncapped 500k stream would blow the
    // localStorage quota — every record carries at most JOB_TEXT_CAP.
    for (const record of store.history) {
      expect(record.text?.length).toBe(JOB_TEXT_CAP);
    }
  });

  it("parseJobHistory caps oversized stored text (payloads from pre-cap builds)", () => {
    const huge = "x".repeat(JOB_TEXT_CAP + 10_000);
    const parsed = parseJobHistory([
      {
        id: "job-1",
        repoId: "r1",
        feature: "review-staged",
        status: "error",
        startedAt: 1,
        elapsedMs: 2,
        text: huge,
        promptPreview: "p".repeat(JOB_PROMPT_PREVIEW_CAP + 500),
      },
    ]);
    expect(parsed?.[0]?.text?.length).toBe(JOB_TEXT_CAP);
    expect(parsed?.[0]?.promptPreview?.length).toBe(JOB_PROMPT_PREVIEW_CAP);
  });
});
