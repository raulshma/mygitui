/**
 * Unit tests for the assistant chat store (M13): gates (master switch,
 * opt-in), the per-root busy slot, happy-path turns (user + assistant
 * bubbles with serving meta), failure/cancel turns (partial text kept),
 * thread persistence (envelope, cap, parse-and-drop), and the pure prompt
 * assembly.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { AiJobStore } from "./jobs.svelte";
import {
  CHAT_MESSAGE_CAP,
  CHAT_MESSAGE_TEXT_CAP,
  CHAT_THREAD_CAP,
  CHAT_STORAGE_KEY,
  ChatStore,
  chatPrompt,
  parseChatThreads,
} from "./chat.svelte";
import type { ChatGitClient, ChatStoreDeps } from "./chat.svelte";
import { AiError } from "./types";
import type { AiResult } from "./types";
import type { FeatureRouter } from "./features";

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

function aiResult(text: string): AiResult {
  return { text, model: "m-1", backend: "opencode", elapsedMs: 42, usage: { totalTokens: 99 } };
}

interface Harness {
  store: ChatStore;
  jobs: AiJobStore;
  storage: Storage;
  router: { generate: ReturnType<typeof vi.fn>; generateStream?: ReturnType<typeof vi.fn> };
  git: ChatGitClient;
}

function makeHarness(opts?: {
  storage?: Storage;
  enabled?: () => boolean;
  isOptedIn?: (repoId: string) => boolean;
  stream?: boolean;
  generateText?: string;
  routerError?: unknown;
  gitError?: unknown;
}): Harness {
  const storage = opts?.storage ?? memStorage();
  const jobs = new AiJobStore({ storage: null });
  const generate = vi.fn(async () => {
    if (opts?.routerError !== undefined) throw opts.routerError;
    return aiResult(opts?.generateText ?? "Here is the answer.");
  });
  const router: Harness["router"] = { generate };
  if (opts?.stream === true) {
    router.generateStream = vi.fn(async (_req, onDelta) => {
      if (opts.routerError !== undefined) throw opts.routerError;
      onDelta("Here ");
      onDelta("is the streamed answer.");
      return aiResult("Here is the streamed answer.");
    });
  }
  const git: ChatGitClient = {
    repoDiff: vi.fn(async () => {
      if (opts?.gitError) throw opts.gitError;
      return [];
    }),
    streamLog: vi.fn(async (_repoId, _filter, onPage) => {
      if (opts?.gitError) throw opts.gitError;
      onPage({
        commits: [
          {
            sha: "abc1234def",
            parents: [],
            author: { name: "A", email: "a@b.c", time: 0, offset_minutes: 0 },
            committer: { name: "A", email: "a@b.c", time: 0, offset_minutes: 0 },
            message: "add thing",
            summary: "add thing",
            refs: [],
          },
        ],
        rows: [],
        next_cursor: null,
        generation: 0,
      });
    }),
  };
  const deps: ChatStoreDeps = {
    storage,
    jobs,
    router: router as FeatureRouter,
    git,
    id: (() => {
      let n = 0;
      return () => `msg-${++n}`;
    })(),
    now: (() => {
      let n = 1_000_000;
      return () => (n += 1_000);
    })(),
  };
  if (opts?.enabled !== undefined) deps.enabled = opts.enabled;
  if (opts?.isOptedIn !== undefined) deps.isOptedIn = opts.isOptedIn;
  return { store: new ChatStore(deps), jobs, storage, router, git };
}

const CONTEXT = { staged: true, commits: false, path: null, pasted: null };

beforeEach(() => {
  vi.clearAllMocks();
});

describe("ChatStore gates", () => {
  it("refuses when the master switch is off (before appending anything)", async () => {
    const { store } = makeHarness({ enabled: () => false });
    await expect(
      store.send({ repoId: "r1", root: "/repo", question: "hi", context: CONTEXT }),
    ).rejects.toMatchObject({ kind: "opt-in" });
    expect(store.conversationFor("/repo")).toHaveLength(0);
  });

  it("refuses without repo opt-in and gathers nothing", async () => {
    const { store, git, router } = makeHarness({ isOptedIn: () => false });
    await expect(
      store.send({ repoId: "r1", root: "/repo", question: "hi", context: CONTEXT }),
    ).rejects.toMatchObject({ kind: "opt-in" });
    expect(git.repoDiff).not.toHaveBeenCalled();
    expect(router.generate).not.toHaveBeenCalled();
  });

  it("refuses an empty question and a concurrent turn on the same root", async () => {
    const { store, router } = makeHarness();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    router.generate.mockImplementationOnce(() => gate.then(() => aiResult("late")));

    const first = store.send({ repoId: "r1", root: "/repo", question: "hi", context: CONTEXT });
    await expect(
      store.send({ repoId: "r1", root: "/repo", question: "again", context: CONTEXT }),
    ).rejects.toMatchObject({ kind: "unknown" });
    release();
    await first;

    await expect(
      store.send({ repoId: "r1", root: "/repo", question: "   ", context: CONTEXT }),
    ).rejects.toMatchObject({ kind: "unknown" });
  });

  it("different roots run concurrently (own busy slot per root)", async () => {
    const { store, router } = makeHarness();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    router.generate.mockImplementation(() => gate.then(() => aiResult("done")));

    const a = store.send({ repoId: "r1", root: "/a", question: "q", context: CONTEXT });
    // /b is a different slot: allowed while /a is in flight.
    const b = store.send({ repoId: "r2", root: "/b", question: "q", context: CONTEXT });
    release();
    await Promise.all([a, b]);
    expect(store.busy["/a"]).toBe(false);
    expect(store.busy["/b"]).toBe(false);
  });
});

describe("ChatStore turns", () => {
  it("a happy path turn appends user + assistant bubbles with meta", async () => {
    const { store, router } = makeHarness({ stream: true });
    const reply = await store.send({
      repoId: "r1",
      root: "/repo",
      repoName: "repo",
      question: "what changed?",
      context: CONTEXT,
    });

    const thread = store.conversationFor("/repo");
    expect(thread).toHaveLength(2);
    expect(thread[0]).toMatchObject({ role: "user", text: "what changed?" });
    // $state proxies: compare by value, not identity.
    expect(thread[1]).toEqual(reply);
    expect(reply).toMatchObject({
      role: "assistant",
      text: "Here is the streamed answer.",
      model: "m-1",
      backend: "opencode",
      usage: { totalTokens: 99 },
    });
    expect(router.generateStream).toHaveBeenCalledTimes(1);
  });

  it("routes through generate when the router cannot stream", async () => {
    const { store, router } = makeHarness();
    await store.send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT });
    expect(router.generate).toHaveBeenCalledTimes(1);
    expect(router.generateStream).toBeUndefined();
  });

  it("a failed turn keeps the question and appends an error bubble", async () => {
    const { store } = makeHarness({ routerError: new AiError("unavailable", "backend down", {}) });
    await expect(
      store.send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT }),
    ).rejects.toMatchObject({ kind: "unavailable" });

    const thread = store.conversationFor("/repo");
    expect(thread).toHaveLength(2);
    expect(thread[0]).toMatchObject({ role: "user", text: "q" });
    expect(thread[1]).toMatchObject({ role: "assistant", error: "backend down" });
  });

  it("a cancelled turn keeps the partial streamed text and flags cancelled", async () => {
    const jobs = new AiJobStore({ storage: null });
    const store = new ChatStore({
      storage: null,
      jobs,
      git: { repoDiff: async () => [], streamLog: async () => {} },
      router: {
        generateStream: (
          req: { signal?: AbortSignal },
          onDelta: (delta: string) => void,
        ) =>
          new Promise<AiResult>((_resolve, reject) => {
            onDelta("Here ");
            // An already-aborted signal never fires "abort" — providers
            // reject immediately when they see `aborted === true`.
            const abort = () => reject(new DOMException("aborted", "AbortError"));
            if (req.signal?.aborted) {
              abort();
              return;
            }
            req.signal?.addEventListener("abort", abort, { once: true });
          }),
      } as unknown as FeatureRouter,
      id: (() => {
        let n = 0;
        return () => `m-${++n}`;
      })(),
    });

    const pending = store.send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT });
    await vi.waitFor(() => store.activeJobId("/repo") !== null);
    store.stop("/repo");
    await expect(pending).rejects.toMatchObject({ kind: "cancelled" });

    const thread = store.conversationFor("/repo");
    // The partial text survives (trimmed), flagged as a cancelled turn.
    expect(thread[1]).toMatchObject({ role: "assistant", text: "Here", cancelled: true });
    expect(store.busy["/repo"]).toBe(false);
    // The job record says cancelled too.
    expect(jobs.history[0]).toMatchObject({ feature: "chat", status: "cancelled" });
  });

  it("stop() cancels the in-flight job for its root", async () => {
    // A signal-respecting mock (what streamText/SSE do on abort).
    const storage = memStorage();
    const jobs = new AiJobStore({ storage: null });
    const store = new ChatStore({
      storage,
      jobs,
      git: { repoDiff: async () => [], streamLog: async () => {} },
      router: {
        generateStream: (req: { signal?: AbortSignal }) =>
          new Promise<AiResult>((_resolve, reject) => {
            const abort = () => reject(new DOMException("aborted", "AbortError"));
            if (req.signal?.aborted) {
              abort();
              return;
            }
            req.signal?.addEventListener("abort", abort, { once: true });
          }),
      } as unknown as FeatureRouter,
      id: () => "m",
    });

    const pending = store.send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT });
    await vi.waitFor(() => store.activeJobId("/repo") !== null);
    store.stop("/repo");
    await expect(pending).rejects.toMatchObject({ kind: "cancelled" });
  });

  it("gathers enabled context chips; a chip failure degrades to a note", async () => {
    const { store, git, router } = makeHarness({ stream: true, gitError: new Error("nope") });
    await store.send({
      repoId: "r1",
      root: "/repo",
      question: "q",
      context: { staged: true, commits: true, path: "f.ts", pasted: "note" },
    });
    // All four context reads hit the git client (staged + commits + file).
    expect(git.repoDiff).toHaveBeenCalledTimes(2); // staged + current file
    expect(git.streamLog).toHaveBeenCalledTimes(1);
    const prompt = router.generateStream!.mock.calls[0]![0].prompt as string;
    expect(prompt).toContain("(not available right now)");
    expect(prompt).toContain("### Pasted notes");
    expect(prompt).toContain("note");
  });

  it("the active file chip reports no working-copy changes for clean paths", async () => {
    const { store, router } = makeHarness({ stream: true });
    await store.send({
      repoId: "r1",
      root: "/repo",
      question: "q",
      context: { staged: false, commits: false, path: "clean.ts", pasted: null },
    });
    const prompt = router.generateStream!.mock.calls[0]![0].prompt as string;
    expect(prompt).toContain("### Current file: `clean.ts`");
    expect(prompt).toContain("(no working-copy changes)");
  });

  it("the turn reports phases + prompt preview into the job store", async () => {
    const { store, jobs } = makeHarness({ stream: true });
    const seen: string[] = [];
    const originalBegin = jobs.begin.bind(jobs);
    jobs.begin = ((...args: Parameters<typeof jobs.begin>) => {
      const ticket = originalBegin(...args);
      expect(args[0]).toMatchObject({ repoId: "r1", repoName: "repo", feature: "chat" });
      const phase = ticket.phase;
      ticket.phase = (p, message) => {
        seen.push(p);
        phase(p, message);
      };
      return ticket;
    }) as typeof jobs.begin;

    await store.send({ repoId: "r1", root: "/repo", repoName: "repo", question: "q", context: CONTEXT });
    expect(seen).toContain("gathering");
    expect(seen).toContain("prompting");
    expect(seen).toContain("generating");
    expect(jobs.history[0]?.promptPreview).toContain("Question: q");
  });
});

describe("ChatStore persistence", () => {
  it("persists threads in the envelope and reloads them", () => {
    const storage = memStorage();
    const first = makeHarness({ storage }).store;
    first.threads["/repo"] = [
      { id: "u1", role: "user", text: "hi", at: 1 },
      {
        id: "a1",
        role: "assistant",
        text: "hello",
        at: 2,
        model: "m",
        backend: "opencode",
        elapsedMs: 5,
        usage: { totalTokens: 3 },
      },
    ];
    // #append persists; direct assignment does not — persist explicitly by
    // round-tripping through clear() + the loaded instance.
    const second = new ChatStore({
      storage,
      id: () => "x",
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    // Direct $state assignment above skipped persistence; emulate a stored
    // write to prove parsing, then check the reload.
    storage.setItem(
      CHAT_STORAGE_KEY,
      JSON.stringify({ v: 1, data: { "/repo": first.threads["/repo"] } }),
    );
    void second;
    const third = new ChatStore({
      storage,
      id: () => "x",
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    expect(third.conversationFor("/repo")).toHaveLength(2);
    expect(third.conversationFor("/repo")[1]).toMatchObject({
      role: "assistant",
      text: "hello",
      model: "m",
    });
  });

  it("send() persists after each bubble (user lands before anything can fail)", async () => {
    const storage = memStorage();
    const { store } = makeHarness({ storage, routerError: new Error("boom") });
    await store
      .send({ repoId: "r1", root: "/repo", question: "kept?", context: CONTEXT })
      .catch(() => {});
    const raw = storage.dump()[CHAT_STORAGE_KEY]!;
    expect(JSON.parse(raw).data["/repo"]).toHaveLength(2);
  });

  it("caps threads at CHAT_MESSAGE_CAP messages", () => {
    const storage = memStorage();
    const flood = Array.from({ length: CHAT_MESSAGE_CAP + 10 }, (_, i) => ({
      id: `m${i}`,
      role: "user" as const,
      text: `m${i}`,
      at: i,
    }));
    storage.setItem(CHAT_STORAGE_KEY, JSON.stringify({ v: 1, data: { "/repo": flood } }));
    const store = new ChatStore({
      storage,
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    expect(store.conversationFor("/repo")).toHaveLength(CHAT_MESSAGE_CAP);
    expect(store.conversationFor("/repo")[0]!.text).toBe("m10");
  });

  it("parseChatThreads drops junk and keeps valid messages", () => {
    const parsed = parseChatThreads({
      "/ok": [
        { id: "u", role: "user", text: "hi", at: 1 },
        { id: "x", role: "wizard", text: "no", at: 2 },
        "junk",
      ],
      "/empty": [],
      bad: "nope",
    });
    expect(parsed).not.toBeNull();
    expect(parsed!["/ok"]).toHaveLength(1);
    expect(parsed!["/empty"]).toBeUndefined();
    expect(parseChatThreads("junk")).toBeNull();
  });

  it("clear() drops the thread from memory and storage", async () => {
    const storage = memStorage();
    const { store } = makeHarness({ storage });
    await store.send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT });
    expect(store.conversationFor("/repo")).toHaveLength(2);
    store.clear("/repo");
    expect(store.conversationFor("/repo")).toHaveLength(0);
    expect(JSON.parse(storage.dump()[CHAT_STORAGE_KEY]!).data["/repo"]).toBeUndefined();
  });
});

describe("chatPrompt (pure)", () => {
  it("assembles repo header, context, transcript and question", () => {
    const prompt = chatPrompt(
      "/repo",
      [
        { id: "1", role: "user", text: "first?", at: 0 },
        { id: "2", role: "assistant", text: "first answer", at: 1 },
        { id: "3", role: "assistant", text: "", at: 2, error: "boom" },
        { id: "4", role: "assistant", text: "partial", at: 3, cancelled: true },
      ],
      "### Staged changes",
      "second?",
    );
    expect(prompt).toContain("Repository: /repo");
    expect(prompt).toContain("user: first?");
    // The valid assistant turn rides along…
    expect(prompt).toContain("assistant: first answer");
    // …but failed/cancelled turns contribute nothing.
    expect(prompt).not.toContain("partial");
    expect(prompt).toContain("### Staged changes");
    expect(prompt.endsWith("Question: second?")).toBe(true);
  });

  it("omits empty sections", () => {
    const prompt = chatPrompt("/repo", [], "(no extra context selected)", "q");
    expect(prompt).toContain("Repository: /repo");
    expect(prompt).not.toContain("Conversation so far");
  });
});

describe("ChatStore failure hygiene + caps (M13 fixes)", () => {
  it("a context-gather failure still reaches exactly one terminal transition", async () => {
    // The gather lazily imports the real IPC client when no git client is
    // injected; make that import reject so #gatherContext throws — the
    // ticket must still terminate or its status-bar row would pin forever.
    vi.doMock("$lib/ipc/client", () => {
      throw new Error("ipc bridge unavailable");
    });
    try {
      const jobs = new AiJobStore({ storage: null });
      const store = new ChatStore({
        storage: memStorage(),
        jobs,
        router: { generate: vi.fn(async () => aiResult("unused")) } as FeatureRouter,
      });
      await expect(
        store.send({ repoId: "r1", root: "/r", question: "q", context: CONTEXT }),
      ).rejects.toThrow();
      expect(Object.keys(jobs.active)).toHaveLength(0);
      expect(jobs.history).toHaveLength(1);
      expect(jobs.history[0]?.status).toBe("error");
      // The error bubble lands so the question stays retryable.
      expect(store.conversationFor("/r").at(-1)?.error).toBeTruthy();
    } finally {
      vi.doUnmock("$lib/ipc/client");
      vi.resetModules();
    }
  });

  it("the deadline fails with the honest timeout, never 'cancelled'", async () => {
    vi.useFakeTimers();
    try {
      const h = makeHarness({ stream: true });
      // The provider shape: an abort rejects as AiError("cancelled") —
      // indistinguishable from a user cancel by error kind alone (this is
      // what opencode.ts / openrouter.ts throw).
      h.router.generateStream = vi.fn(
        (req: { signal?: AbortSignal }): Promise<AiResult> =>
          new Promise<AiResult>((_resolve, reject) => {
            req.signal?.addEventListener(
              "abort",
              () => reject(new AiError("cancelled", "AI request cancelled", {})),
              { once: true },
            );
          }),
      );
      const pending = h.store.send({ repoId: "r1", root: "/r", question: "q", context: CONTEXT });
      const assertion = expect(pending).rejects.toThrow(/did not finish in time/);
      await vi.advanceTimersByTimeAsync(120_000); // CHAT_TIMEOUT_MS
      await assertion;
      expect(h.jobs.history[0]?.status).toBe("error");
      expect(h.jobs.history[0]?.error).toBe("the AI backend did not finish in time");
      expect(h.store.conversationFor("/r").at(-1)?.error).toBe("the AI backend did not finish in time");
    } finally {
      vi.useRealTimers();
    }
  });

  it("a stored answer caps at CHAT_MESSAGE_TEXT_CAP; the returned reply stays whole", async () => {
    const long = "x".repeat(CHAT_MESSAGE_TEXT_CAP + 5_000);
    const h = makeHarness({ generateText: long });
    const reply = await h.store.send({ repoId: "r1", root: "/r", question: "q", context: CONTEXT });
    expect(reply.text).toBe(long);
    const stored = h.store.conversationFor("/r").at(-1);
    expect(stored?.text.length).toBeLessThan(long.length);
    expect(stored?.text.endsWith("[truncated]")).toBe(true);
    // The activity record is capped independently (jobs store), so the
    // thread envelope and the history ring can never carry an uncapped body.
    expect(h.jobs.history[0]?.text?.length).toBe(20_000);
  });

  it("threads cap at CHAT_THREAD_CAP roots (oldest-inserted drop)", async () => {
    const h = makeHarness();
    for (let i = 0; i <= CHAT_THREAD_CAP; i++) {
      await h.store.send({ repoId: `r${i}`, root: `/r${i}`, question: `q${i}`, context: CONTEXT });
    }
    const roots = Object.keys(h.store.threads);
    expect(roots).toHaveLength(CHAT_THREAD_CAP);
    expect(roots).not.toContain("/r0");
    expect(roots.at(-1)).toBe(`/r${CHAT_THREAD_CAP}`);
  });

  it("parseChatThreads caps message text and root count on load", () => {
    const big = "y".repeat(CHAT_MESSAGE_TEXT_CAP + 1_000);
    const threads: Record<string, unknown> = {};
    for (let i = 0; i <= CHAT_THREAD_CAP; i++) {
      threads[`/r${i}`] = [{ id: "1", role: "assistant", text: big, at: 1 }];
    }
    const parsed = parseChatThreads(threads);
    expect(parsed).not.toBeNull();
    expect(Object.keys(parsed!)).toHaveLength(CHAT_THREAD_CAP);
    expect(Object.values(parsed!)[0]?.[0]?.text.endsWith("[truncated]")).toBe(true);
  });
});
