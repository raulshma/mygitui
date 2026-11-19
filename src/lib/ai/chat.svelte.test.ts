/**
 * Unit tests for the assistant chat store (M13): gates (master switch,
 * opt-in), the per-root busy slot, happy-path turns (user + assistant
 * bubbles with serving meta), failure/cancel turns (partial text kept),
 * thread persistence (envelope, cap, parse-and-drop), and the pure prompt
 * assembly.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { AgenticRunController } from "./agenticRun.svelte";
import { AiJobStore } from "./jobs.svelte";
import {
  CHAT_MESSAGE_CAP,
  CHAT_MESSAGE_TEXT_CAP,
  CHAT_SESSIONS_KEY,
  CHAT_THREAD_CAP,
  CHAT_STORAGE_KEY,
  ChatStore,
  chatPrompt,
  parseChatSessions,
  parseChatThreads,
} from "./chat.svelte";
import type { ChatAgenticRunner, ChatGitClient, ChatStoreDeps } from "./chat.svelte";
import { AiError, isRetryableWithoutTools } from "./types";
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
  /** The agentic runner seam (absent unless `withAgentic`). */
  agentic?: ReturnType<typeof vi.fn>;
  deleteSession: ReturnType<typeof vi.fn>;
}

function makeHarness(opts?: {
  storage?: Storage;
  enabled?: () => boolean;
  isOptedIn?: (repoId: string) => boolean;
  stream?: boolean;
  generateText?: string;
  routerError?: unknown;
  gitError?: unknown;
  /** Wires the agentic runner seam (tests stub the returned vi.fn). */
  withAgentic?: boolean;
  /** The per-repo model override reader. */
  model?: (repoId: string) => string | null;
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
  const deleteSession = vi.fn(async () => undefined);
  if (opts?.gitError) deleteSession.mockRejectedValue(new Error("server dead"));
  const agentic = opts?.withAgentic === true ? vi.fn() : undefined;
  if (agentic !== undefined) deps.agentic = agentic as never;
  if (opts?.model !== undefined) deps.model = opts.model;
  deps.deleteSession = deleteSession;
  const harness: Harness = { store: new ChatStore(deps), jobs, storage, router, git, deleteSession };
  if (agentic !== undefined) harness.agentic = agentic;
  return harness;
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
    first.threads["/repo"] = {
      opencodeSessionId: "sess-bound",
      messages: [
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
      ],
    };
    // Direct $state assignment skips #persist; emulate what it writes:
    // the OLD-format messages envelope + the sibling bindings envelope.
    storage.setItem(
      CHAT_STORAGE_KEY,
      JSON.stringify({
        v: 1,
        data: { "/repo": first.threads["/repo"].messages },
      }),
    );
    storage.setItem(
      CHAT_SESSIONS_KEY,
      JSON.stringify({ v: 1, data: { "/repo": "sess-bound" } }),
    );
    // #append persists; direct assignment does not — persist explicitly by
    // round-tripping through clear() + the loaded instance.
    const second = new ChatStore({
      storage,
      id: () => "x",
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    void second;
    const third = new ChatStore({
      storage,
      id: () => "x",
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    expect(third.conversationFor("/repo")).toHaveLength(2);
    expect(third.sessionFor("/repo")).toBe("sess-bound");
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
    // The messages payload stays OLD-FORMAT (bare arrays) even after the
    // agentic work — old builds' read-modify-write round-trips untouched.
    const dumped = JSON.parse(raw).data["/repo"];
    expect(Array.isArray(dumped)).toBe(true);
    expect(dumped).toHaveLength(2);
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

// ---------------------------------------------------------------------------
// Agentic chat (chunk 3): thread-bound sessions, chip orchestration, markers
// ---------------------------------------------------------------------------

import { CHAT_SYSTEM, agenticPrompt } from "./chat.svelte";
import type { ChatAgenticInput } from "./chat.svelte";

/** A minimal successful runner result. */
function runnerResult(text = "agentic answer", sessionID = "sess-9"): AiResult & { sessionID: string } {
  return { text, model: "m-1", backend: "opencode", elapsedMs: 8, sessionID };
}

describe("Agentic chat (chunk 3)", () => {
  function makeAgenticHarness() {
    const h = makeHarness({ withAgentic: true });
    h.agentic!.mockResolvedValue(runnerResult());
    return h as ReturnType<typeof makeHarness> & {
      agentic: ReturnType<typeof vi.fn>;
    };
  }

  it("agentic send skips the gather, binds the session, marks the turn", async () => {
    const h = makeAgenticHarness();
    const reply = await h.store.send({
      repoId: "r1",
      root: "/repo",
      question: "What changed?",
      context: { staged: true, commits: true, path: null, pasted: null },
      agentic: true,
    });
    // The agent reads the repo itself: no client-side gather.
    expect(h.git.repoDiff).not.toHaveBeenCalled();
    expect(h.git.streamLog).not.toHaveBeenCalled();
    // The runner got the task (no repo header, no transcript, no context blocks).
    expect(h.agentic).toHaveBeenCalledTimes(1);
    const input = h.agentic.mock.calls[0]![0] as ChatAgenticInput;
    expect(input.prompt).toBe("What changed?");
    expect(input.system).toBe(CHAT_SYSTEM);
    expect(input.sessionID).toBeUndefined(); // fresh session
    // The binding persists from the result; the turn records its mode.
    expect(h.store.sessionFor("/repo")).toBe("sess-9");
    expect(reply.mode).toBe("agentic");
    expect(reply.text).toBe("agentic answer");
    // Single-shot never ran.
    expect(h.router.generate).not.toHaveBeenCalled();
  });

  it("pasted notes are part of an agentic message (always flow through)", async () => {
    const h = makeAgenticHarness();
    await h.store.send({
      repoId: "r1",
      root: "/repo",
      question: "Review this",
      context: { staged: false, commits: false, path: null, pasted: "my paste" },
      agentic: true,
    });
    const input = h.agentic.mock.calls[0]![0] as ChatAgenticInput;
    expect(input.prompt).toBe("Review this\n\n### Pasted notes\n\nmy paste");
  });

  it("reuses the thread's binding on the next agentic message", async () => {
    const h = makeAgenticHarness();
    await h.store.send({
      repoId: "r1", root: "/repo", question: "one", context: CONTEXT, agentic: true,
    });
    await h.store.send({
      repoId: "r1", root: "/repo", question: "two", context: CONTEXT, agentic: true,
    });
    expect(h.agentic).toHaveBeenCalledTimes(2);
    const first = h.agentic.mock.calls[0]![0] as ChatAgenticInput;
    const second = h.agentic.mock.calls[1]![0] as ChatAgenticInput;
    expect(first.sessionID).toBeUndefined();
    expect(second.sessionID).toBe("sess-9");
    // The second message is user text only — no system re-send.
    expect(second.prompt).toBe("two");
  });

  it("the per-repo model choice rides the runner", async () => {
    const h = makeHarness({ withAgentic: true, model: () => "anthropic/claude-opus-4" });
    const agentic = h.agentic as ReturnType<typeof vi.fn>;
    agentic.mockResolvedValue(runnerResult());
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    expect((agentic.mock.calls[0]![0] as ChatAgenticInput).model).toBe("anthropic/claude-opus-4");
  });

  it("clear() fires the session DELETE best-effort and resets the binding", async () => {
    const h = makeAgenticHarness();
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    expect(h.store.sessionFor("/repo")).toBe("sess-9");
    h.store.clear("/repo");
    expect(h.deleteSession).toHaveBeenCalledWith("sess-9");
    expect(h.store.sessionFor("/repo")).toBeNull();

    // A dead server must not make thread deletion fail.
    h.deleteSession.mockRejectedValueOnce(new Error("server dead"));
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q2", context: CONTEXT, agentic: true,
    });
    expect(() => h.store.clear("/repo")).not.toThrow();
    expect(h.store.sessionFor("/repo")).toBeNull();
  });

  it("stop() cancels a running agentic turn and keeps the partial text", async () => {
    const h = makeAgenticHarness();
    h.agentic!.mockImplementation(async (input: ChatAgenticInput) => {
      input.onDelta?.("partial before cancel ");
      await new Promise<void>((resolve) => {
        if (input.ticket.signal.aborted) resolve();
        else input.ticket.signal.addEventListener("abort", () => resolve(), { once: true });
      });
      throw new AiError("cancelled", "AI request cancelled", { backend: "opencode" });
    });
    const pending = h.store
      .send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true })
      .catch((err: unknown) => err);
    await vi.waitFor(() => expect(h.store.busy["/repo"]).toBe(true));
    h.store.stop("/repo");
    await expect(pending).resolves.toMatchObject({ kind: "cancelled" });
    const last = h.store.conversationFor("/repo").at(-1);
    expect(last).toMatchObject({ role: "assistant", cancelled: true, mode: "agentic" });
    expect(last?.text).toBe("partial before cancel");
    expect(h.store.busy["/repo"]).toBe(false);
  });

  it("chip off runs the single-shot path (byte-identical) with the mode marker", async () => {
    const h = makeAgenticHarness();
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT,
    });
    expect(h.agentic).not.toHaveBeenCalled();
    expect(h.router.generate).toHaveBeenCalledTimes(1);
    const reply = h.store.conversationFor("/repo").at(-1);
    expect(reply?.mode).toBe("single-shot");
    // No binding written: the thread stays unbound.
    expect(h.store.sessionFor("/repo")).toBeNull();
  });

  it("chip on without a runner seam degrades to single-shot (ADR 0001)", async () => {
    const h = makeHarness(); // no agentic dep wired
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    expect(h.router.generate).toHaveBeenCalledTimes(1);
    const reply = h.store.conversationFor("/repo").at(-1);
    expect(reply?.mode).toBe("single-shot");
    expect(h.store.sessionFor("/repo")).toBeNull();
  });

  it("chip on + runner refuses (no v2 server) degrades to single-shot", async () => {
    const h = makeAgenticHarness();
    h.agentic!.mockRejectedValue(
      new AiError("unavailable", "agentic runs need a 2.x opencode server — this one is 1.x", {
        backend: "opencode",
      }),
    );
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    // The turn completed as today: single-shot ran, gather included.
    expect(h.git.repoDiff).toHaveBeenCalledTimes(1);
    expect(h.router.generate).toHaveBeenCalledTimes(1);
    const reply = h.store.conversationFor("/repo").at(-1);
    expect(reply?.mode).toBe("single-shot");
    expect(h.store.sessionFor("/repo")).toBeNull();
  });

  it("a mid-run agentic failure does NOT degrade (output already streamed)", async () => {
    const h = makeAgenticHarness();
    h.agentic!.mockImplementation(async (input: ChatAgenticInput) => {
      input.onDelta?.("partial ");
      throw new AiError("unavailable", "opencode request failed: fetch failed", {
        backend: "opencode",
      });
    });
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    }).catch(() => {});
    expect(h.router.generate).not.toHaveBeenCalled();
    const failed = h.store.conversationFor("/repo").at(-1);
    expect(failed).toMatchObject({ role: "assistant", mode: "agentic" });
    expect(failed?.error).toBeTruthy();
    expect(failed?.text).toBe("partial");
  });

  it("persisted session ids validate and survive a reload", async () => {
    const storage = memStorage();
    const h = makeHarness({ storage, withAgentic: true });
    (h.agentic as ReturnType<typeof vi.fn>).mockResolvedValue(runnerResult());
    await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    const reloaded = new ChatStore({
      storage,
      id: () => "x",
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    expect(reloaded.sessionFor("/repo")).toBe("sess-9");
  });
});

describe("agenticPrompt (chunk 3)", () => {
  it("passes the question through without notes", () => {
    expect(agenticPrompt("What changed?", null)).toBe("What changed?");
    expect(agenticPrompt("What changed?", "   ")).toBe("What changed?");
  });

  it("appends pasted notes as part of the message", () => {
    expect(agenticPrompt("Review this", "my paste")).toBe(
      "Review this\n\n### Pasted notes\n\nmy paste",
    );
  });
});

describe("chat parse — mode marker + sibling session bindings (chunk 3 review)", () => {
  it("the mode marker validates in the messages payload", () => {
    const parsed = parseChatThreads({
      "/ok": [
        { id: "a", role: "assistant", text: "x", at: 1, mode: "agentic" },
        { id: "b", role: "assistant", text: "y", at: 2, mode: "weird" },
        { id: "c", role: "assistant", text: "z", at: 3 },
      ],
    });
    expect(parsed?.["/ok"]).toEqual([
      expect.objectContaining({ id: "a", mode: "agentic" }),
      expect.objectContaining({ id: "b", mode: undefined }),
      expect.objectContaining({ id: "c", mode: undefined }),
    ]);
  });

  it("the messages payload stays OLD-FORMAT (bare arrays) for old builds", () => {
    // L5: an old build's read-modify-write of mygitui.ai.chat must round-
    // trip through the new store's parse and persist untouched.
    const storage = memStorage({
      [CHAT_STORAGE_KEY]: JSON.stringify({
        v: 1,
        data: { "/repo": [{ id: "a", role: "user", text: "x", at: 1 }] },
      }),
    });
    const store = new ChatStore({
      storage,
      id: () => "x",
      git: { repoDiff: async () => [], streamLog: async () => {} },
    });
    expect(store.conversationFor("/repo")).toHaveLength(1);
    const dump = JSON.parse(storage.dump()[CHAT_STORAGE_KEY]!);
    expect(Array.isArray(dump.data["/repo"])).toBe(true);
    expect(dump.data["/repo"]).toHaveLength(1);
  });

  it("parseChatSessions validates the sibling bindings envelope", () => {
    const parsed = parseChatSessions({
      "/ok": "sess-1",
      "/junk": 42,
      "/empty": "",
      bad: "nope",
      "/nested": { id: "sess-2" },
    });
    // Non-string values and empty ids drop; any non-empty root + id pair
    // is kept (roots are opaque repo paths — same rule as the threads).
    expect(parsed).toEqual({ "/ok": "sess-1", bad: "nope" });
    expect(parseChatSessions("junk")).toBeNull();
    expect(parseChatSessions(null)).toBeNull();
  });
});

describe("Agentic chat — fix round (chunk 3 review)", () => {
  /** A harness with the agentic seam pre-wired to a per-test vi.fn. */
  function makeAgenticHarness() {
    const h = makeHarness({ withAgentic: true });
    h.agentic!.mockResolvedValue(runnerResult());
    return h as ReturnType<typeof makeHarness> & { agentic: ReturnType<typeof vi.fn> };
  }

  function makeFailingRunner(shape: "backstop" | "transport" | "recreate-failed") {
    return async (input: ChatAgenticInput): Promise<never> => {
      // All three post-start shapes dispatch (onController fires) before
      // the failure: a tools-only run hitting the dead-stream backstop, a
      // mid-run SSE transport drop, and a failed session-lost re-create.
      input.onController?.({} as AgenticRunController);
      const message =
        shape === "backstop"
          ? "OpenCode did not finish generating in time."
          : shape === "transport"
            ? "opencode request failed: fetch failed"
            : "opencode session.create failed: server gone";
      throw new AiError("unavailable", message, { backend: "opencode" });
    };
  }

  it.each([
    ["dead-stream backstop", "backstop"],
    ["mid-run SSE transport drop", "transport"],
    ["failed session-lost re-create", "recreate-failed"],
  ] as const)(
    "post-start unavailable (%s) surfaces honestly — NO single-shot re-run",
    async (_label, shape) => {
      const h = makeAgenticHarness();
      h.agentic!.mockImplementation(makeFailingRunner(shape));
      await h.store
        .send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true })
        .catch(() => {});
      // MEDIUM 4: the run dispatched — re-executing the whole request
      // single-shot would hide (and duplicate) the agentic work.
      expect(h.router.generate).not.toHaveBeenCalled();
      const failed = h.store.conversationFor("/repo").at(-1);
      expect(failed).toMatchObject({ role: "assistant", mode: "agentic" });
      expect(failed?.error).toBeTruthy();
    },
  );

  it("clear() mid-run: the turn's fresh session still binds (no orphan)", async () => {
    const h = makeAgenticHarness();
    let release: (() => void) | null = null;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    h.agentic!.mockImplementation(async (input: ChatAgenticInput) => {
      input.onController?.({} as AgenticRunController);
      await gate;
      return runnerResult("late answer", "sess-new");
    });
    const pending = h.store
      .send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true })
      .catch((err: unknown) => err);
    await vi.waitFor(() => expect(h.agentic).toHaveBeenCalled());
    h.store.clear("/repo");
    expect(h.store.sessionFor("/repo")).toBeNull();
    (release as unknown as () => void)?.();
    await pending;
    // The reply recreates the thread, and the fresh session binds to it —
    // the recreated null binding must not overwrite the id (L6).
    expect(h.store.conversationFor("/repo").at(-1)?.text).toBe("late answer");
    expect(h.store.sessionFor("/repo")).toBe("sess-new");
  });
});

describe("ChatStore per-feature gate (chunk 5)", () => {
  function makeGatedHarness(enabled: boolean) {
    const h = makeHarness({ withAgentic: true });
    (h.agentic as ReturnType<typeof vi.fn>).mockResolvedValue(runnerResult());
    const base = h as unknown as { store: ChatStore };
    // Wrap the store's deps gate: rebuild with the per-feature gate wired.
    const store = new ChatStore({
      storage: memStorage(),
      jobs: h.jobs,
      router: { generate: async () => aiResult("single") } as FeatureRouter,
      agentic: h.agentic as ChatAgenticRunner,
      id: () => "m-x",
      now: () => 2_000_000,
      isFeatureEnabled: () => enabled,
    });
    base.store = store;
    return { store: base.store, agentic: h.agentic, git: h.git };
  }

  it("chat mode off refuses the send at the store level (no agentic dispatch)", async () => {
    const { store, agentic } = makeGatedHarness(false);
    await expect(
      store.send({
        repoId: "r1",
        root: "/repo",
        question: "q",
        context: CONTEXT,
        agentic: true,
      }),
    ).rejects.toMatchObject({ kind: "opt-in", message: "Assistant is turned off in AI settings" });
    expect(agentic).not.toHaveBeenCalled();
    // A gate refusal lands nothing (the user bubble appends after the gates).
  });

  it("chat mode on sends normally with the gate wired", async () => {
    const { store, agentic } = makeGatedHarness(true);
    await store.send({
      repoId: "r1",
      root: "/repo",
      question: "q",
      context: CONTEXT,
      agentic: true,
    });
    expect(agentic).toHaveBeenCalledTimes(1);
    expect(store.conversationFor("/repo")).toHaveLength(2);
  });
});

describe("Agentic chat — budget stops and single-shot retry (chunk 6)", () => {
  /** A harness with the agentic seam pre-wired to a per-test vi.fn. */
  function makeAgenticHarness() {
    const h = makeHarness({ withAgentic: true });
    h.agentic!.mockResolvedValue(runnerResult());
    return h as ReturnType<typeof makeHarness> & { agentic: ReturnType<typeof vi.fn> };
  }

  it("a budget-stopped agentic turn is a distinct partial reply (not cancel/error)", async () => {
    const h = makeAgenticHarness();
    h.agentic!.mockImplementation(async (input: ChatAgenticInput) => {
      // A real controller at its tool-call cap, as the budget settle does.
      const controller = new AgenticRunController({
        sessionID: "sess-9",
        budget: { maxToolCalls: 1 },
        reply: async () => undefined,
        interrupt: async () => undefined,
      });
      input.onController?.(controller);
      // The real glue binds the controller to the ticket — mirror it so
      // the job snapshot carries the agentic outcome.
      h.jobs.bindAgentic(input.ticket.id, controller);
      controller.handleToolCalled({ id: "t1", input: { command: "ls" }, executed: true });
      return {
        ...runnerResult("partial findings"),
        budgetStopped: true,
        budgetToolCalls: 1,
        steps: [...controller.steps],
      };
    });
    const reply = await h.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    expect(reply).toMatchObject({
      role: "assistant",
      mode: "agentic",
      text: "partial findings",
      budgetStopped: true,
      budgetToolCalls: 1,
    });
    // NOT an error or cancellation:
    expect(reply.error).toBeUndefined();
    expect(reply.cancelled).toBeUndefined();
    expect(reply.retryable).toBeUndefined();
    const record = h.jobs.history[0];
    expect(record?.status).toBe("done");
    expect(record?.agentic?.budgetStopped).toBe(true);
    // Round-trips through storage:
    const parsed = parseChatThreads(JSON.parse(JSON.stringify({ "/repo": [reply] })));
    expect(parsed?.["/repo"]?.[0]).toMatchObject({ budgetStopped: true, budgetToolCalls: 1 });
  });

  it("a post-start unavailable failure marks the turn retryable", async () => {
    const h = makeAgenticHarness();
    h.agentic!.mockImplementation(async (input: ChatAgenticInput) => {
      input.onController?.({} as never);
      input.onDelta?.("partial ");
      throw new AiError("unavailable", "opencode request failed: fetch failed", {
        backend: "opencode",
      });
    });
    await h.store
      .send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true })
      .catch(() => {});
    const failed = h.store.conversationFor("/repo").at(-1);
    expect(failed).toMatchObject({ role: "assistant", mode: "agentic", retryable: true });
    expect(failed?.error).toBeTruthy();
  });

  it("a pre-start refusal (opt-in) is not retryable", async () => {
    const h = makeAgenticHarness();
    h.agentic!.mockRejectedValue(
      new AiError("opt-in", "AI features are not enabled for this repository", {}),
    );
    await h.store
      .send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true })
      .catch(() => {});
    const failed = h.store.conversationFor("/repo").at(-1);
    expect(failed).toMatchObject({ role: "assistant", mode: "agentic" });
    expect(failed?.retryable).toBeUndefined();
  });

  it("the forced single-shot retry bypasses the seam and keeps the thread binding", async () => {
    const h = makeAgenticHarness();
    // Bind the thread to a session through a successful agentic turn.
    await h.store.send({
      repoId: "r1", root: "/repo", question: "first", context: CONTEXT, agentic: true,
    });
    expect(h.store.sessionFor("/repo")).toBe("sess-9");
    const agenticCalls = h.agentic!.mock.calls.length;

    // The panels' "Retry without tools" passes agentic:false for that turn
    // only — the single-shot router serves it and the binding is untouched.
    const retry = await h.store.send({
      repoId: "r1", root: "/repo", question: "first", context: CONTEXT, agentic: false,
    });
    expect(h.agentic!.mock.calls.length).toBe(agenticCalls);
    expect(h.router.generate).toHaveBeenCalledTimes(1);
    expect(retry.mode).toBe("single-shot");
    expect(retry.retryable).toBeUndefined();
    expect(h.store.sessionFor("/repo")).toBe("sess-9");

    // The binding still serves the NEXT agentic send.
    await h.store.send({
      repoId: "r1", root: "/repo", question: "third", context: CONTEXT, agentic: true,
    });
    expect((h.agentic!.mock.calls.at(-1)![0] as ChatAgenticInput).sessionID).toBe("sess-9");
  });

  it("non-retryable failures carry no marker (budget stops resolve; others differ)", async () => {
    const h = makeAgenticHarness();
    // Ordinary agentic failure:
    h.agentic!.mockRejectedValue(new AiError("bad-response", "opencode exploded", {}));
    await h.store
      .send({ repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true })
      .catch(() => {});
    expect(h.store.conversationFor("/repo").at(-1)?.retryable).toBeUndefined();

    // A budget stop RESOLVES (partial output + banner) — never retryable.
    const h2 = makeAgenticHarness();
    h2.agentic!.mockResolvedValue({
      ...runnerResult("partial"),
      budgetStopped: true,
      budgetToolCalls: 3,
    });
    await h2.store.send({
      repoId: "r1", root: "/repo", question: "q", context: CONTEXT, agentic: true,
    });
    expect(h2.store.conversationFor("/repo").at(-1)?.retryable).toBeUndefined();
  });

  it("parse validates the new turn markers (junk dropped)", () => {
    const parsed = parseChatThreads({
      "/r": [
        { id: "a", role: "assistant", text: "x", at: 1, budgetStopped: true, budgetToolCalls: 4, retryable: true },
        { id: "b", role: "assistant", text: "y", at: 2, budgetStopped: "yes", budgetToolCalls: "many", retryable: "sure" },
        { id: "c", role: "assistant", text: "z", at: 3, budgetStopped: true, budgetToolCalls: -4 },
      ],
    });
    expect(parsed?.["/r"]).toEqual([
      expect.objectContaining({ budgetStopped: true, budgetToolCalls: 4, retryable: true }),
      expect.objectContaining({ budgetStopped: undefined, budgetToolCalls: undefined, retryable: undefined }),
      expect.objectContaining({ budgetStopped: true, budgetToolCalls: 0 }),
    ]);
  });
});

describe("diagnoseExecutionMode (chunk 6)", () => {
  it("auto + not forced → agentic; everything else → single-shot", async () => {
    const { diagnoseExecutionMode } = await import("./features");
    expect(diagnoseExecutionMode("auto", false)).toBe("agentic");
    expect(diagnoseExecutionMode("auto", true)).toBe("single-shot");
    expect(diagnoseExecutionMode("single-shot", false)).toBe("single-shot");
    expect(diagnoseExecutionMode("single-shot", true)).toBe("single-shot");
    expect(diagnoseExecutionMode("off", false)).toBe("single-shot");
  });
});

describe("isRetryableWithoutTools (chunk 6)", () => {
  it("only a POST-start unavailable-class failure arms the tools-free retry", () => {
    const unavailable = new AiError("unavailable", "fetch failed", {});
    expect(isRetryableWithoutTools(unavailable, true)).toBe(true);
    // Pre-start refusal: already fell back; nothing to retry.
    expect(isRetryableWithoutTools(unavailable, false)).toBe(false);
    // Other kinds never arm it (opt-in refusals, cancellations, bad output).
    for (const kind of ["opt-in", "cancelled", "bad-response", "unknown"] as const) {
      expect(isRetryableWithoutTools(new AiError(kind, "x", {}), true)).toBe(false);
    }
    expect(isRetryableWithoutTools(new Error("fetch failed"), true)).toBe(false);
    expect(isRetryableWithoutTools(null, true)).toBe(false);
  });
});
