/**
 * Unit tests for the AI runes store: config persistence (localStorage,
 * malformed-tolerant), per-repo opt-in gating, run-state management
 * (busy/error/last), the secrets-never-in-localStorage guarantee, and
 * end-to-end routing through the supervisor (fallback included).
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FileDiff } from "$lib/ipc/types";
import { AiStore, localServeCwd, parseAiConfig, AI_STORAGE_KEY } from "./ai.svelte";
import type { AiProbeResult, AiProvider, ProviderSet } from "./provider";
import type { FeatureGitClient } from "./features";
import { OpenCodeProvider } from "./opencode";
import { AiError, defaultAiConfig, resolveAgenticBudget, resolveOpencode, resolveOpencodeMode } from "./types";
import type { AiBackend, AiResult } from "./types";

const secrets = vi.hoisted(() => ({
  secretsGet: vi.fn(async () => null),
  secretsSet: vi.fn(async () => undefined),
  secretsDelete: vi.fn(async () => undefined),
}));

vi.mock("$lib/ipc/client", () => ({
  secretsGet: secrets.secretsGet,
  secretsSet: secrets.secretsSet,
  secretsDelete: secrets.secretsDelete,
  SECRET_KEYS: {
    openrouterApiKey: "openrouter.api-key",
    opencodeServerPassword: "opencode.server.password",
  },
  // Serve-registry spies (the agentic-capability gate must never spawn a
  // server behind an OpenRouter-configured backend).
  repoDiff: vi.fn(),
  streamLog: vi.fn(),
  opencodeDetect: vi.fn(async () => ({ installed: false, path: null, version: null, major: null })),
  opencodeServeStatus: vi.fn(async () => ({ running: false, url: null, port: null, error: null })),
  opencodeServeStart: vi.fn(async () => ({ running: false, url: null, port: null, error: null })),
  opencodeServeStop: vi.fn(async () => ({ running: false, url: null, port: null, error: null })),
}));

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const FILE: FileDiff = {
  path: "src/a.ts",
  old_path: null,
  binary: false,
  is_image: false,
  additions: 1,
  deletions: 0,
  hunks: [
    {
      old_start: 1,
      new_start: 1,
      lines: [
        { old_no: null, new_no: 1, origin: "+", text: "export const ok = true;", highlights: [] },
      ],
    },
  ],
};

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

function mockProvider(id: AiBackend, probe: AiProbeResult = { status: "ok" }) {
  const provider = {
    id,
    probe,
    available: vi.fn(async () => provider.probe.status !== "down"),
    check: vi.fn(async () => provider.probe),
    // Agentic runs (chunk 2) feature-detect this method; tests stub it
    // per-test via mockResolvedValue/mockImplementation.
    runAgentic: vi.fn(),
    listModels: vi.fn(async () => [{ id: `${id}/model-a`, label: id }]),
    generate: vi.fn(async (_req?: { model?: string }): Promise<AiResult> => ({
      text: "feat(ui): add the thing\n\nIt was missing.",
      model: `${id}-model`,
      backend: id,
      elapsedMs: 4,
    })),
  };
  return provider;
}

interface Harness {
  store: AiStore;
  storage: Storage;
  opencode: ReturnType<typeof mockProvider>;
  openrouter: ReturnType<typeof mockProvider>;
  git: FeatureGitClient;
}

function makeHarness(opts?: {
  storage?: Storage;
  opencodeProbe?: AiProbeResult;
  openrouterProbe?: AiProbeResult;
  /** Overrides the store's `isTauri()` desktop check (liveness tests). */
  isTauri?: () => boolean;
}): Harness {
  const storage = opts?.storage ?? memStorage();
  const opencode = mockProvider("opencode", opts?.opencodeProbe);
  const openrouter = mockProvider("openrouter", opts?.openrouterProbe);
  const providers: Partial<ProviderSet> = { opencode, openrouter };
  const store = new AiStore({
    storage,
    providers,
    isTauri: opts?.isTauri,
  });
  const git: FeatureGitClient = {
    repoDiff: vi.fn(async () => [FILE]),
    streamLog: vi.fn(async (_repoId, _filter, onPage) => {
      onPage({ commits: [], rows: [], next_cursor: null, generation: 0 });
    }),
  };
  return { store, storage, opencode, openrouter, git };
}

beforeEach(() => {
  vi.clearAllMocks();
  secrets.secretsGet.mockResolvedValue(null);
});

// ---------------------------------------------------------------------------
// Config persistence
// ---------------------------------------------------------------------------

describe("localServeCwd", () => {
  it("passes local tab roots through as the managed workspace", () => {
    expect(localServeCwd("/home/raulshma/agent-repos/mygitui")).toBe(
      "/home/raulshma/agent-repos/mygitui",
    );
    expect(localServeCwd("C:\\Code\\repo")).toBe("C:\\Code\\repo");
  });

  it("drops ssh:// remote tab roots (managed serve is a local process)", () => {
    expect(
      localServeCwd("ssh://dev-6c4a837b-a7ef-45c2-a5cd-409e856e7916/home/raulshma/agent-repos/mygitui"),
    ).toBeUndefined();
    expect(localServeCwd(undefined)).toBeUndefined();
    expect(localServeCwd(null)).toBeUndefined();
  });
});

describe("AiStore config persistence", () => {
  it("defaults to opencode + fallback allowed, nothing opted in", () => {
    const { store } = makeHarness();
    expect(store.config).toEqual({
      backend: "opencode",
      allowFallback: true,
      repoOptIn: {},
    });
  });

  it("round-trips config through storage", () => {
    const storage = memStorage();
    const first = new AiStore({ storage, providers: {} });
    first.setBackend("openrouter");
    first.setOpenrouterModel("openai/gpt-oss-120b");
    first.setOpencodeUrl("http://localhost:9999");
    first.setAllowFallback(false);
    first.setOptIn("repo-1", true);

    const second = new AiStore({ storage, providers: {} });
    expect(second.config.backend).toBe("openrouter");
    expect(second.config.openrouterModel).toBe("openai/gpt-oss-120b");
    expect(second.config.opencodeUrl).toBe("http://localhost:9999");
    expect(second.config.allowFallback).toBe(false);
    expect(second.isOptedIn("repo-1")).toBe(true);
  });

  it("falls back to defaults on malformed storage content", () => {
    const store = new AiStore({ storage: memStorage({ [AI_STORAGE_KEY]: "{not json" }), providers: {} });
    expect(store.config.backend).toBe("opencode");
    expect(store.config.allowFallback).toBe(true);
  });

  it("drops unknown backend values from storage", () => {
    const config = parseAiConfig(
      JSON.stringify({ backend: "hal9000", repoOptIn: { r: "yes", s: true } }),
    );
    expect(config.backend).toBe("opencode");
    expect(config.repoOptIn).toEqual({ s: true });
  });

  it("parses the opencode mode/enabled fields, dropping unknown modes", () => {
    const config = parseAiConfig(
      JSON.stringify({ backend: "opencode", opencodeMode: "managed", opencodeEnabled: false }),
    );
    expect(config.opencodeMode).toBe("managed");
    expect(config.opencodeEnabled).toBe(false);

    const unknown = parseAiConfig(
      JSON.stringify({ backend: "opencode", opencodeMode: "telepathy" }),
    );
    expect(unknown.opencodeMode).toBeUndefined();
  });

  it("parses the agentic budget caps, dropping invalid entries", () => {
    const config = parseAiConfig(
      JSON.stringify({
        backend: "opencode",
        agenticBudget: { maxWallClockMs: 120_000, maxToolCalls: 10 },
      }),
    );
    expect(config.agenticBudget).toEqual({ maxWallClockMs: 120_000, maxToolCalls: 10 });

    // Zero/negative/non-numeric caps are dropped field-by-field; an object
    // with no valid field stays unset (defaults apply).
    const partial = parseAiConfig(
      JSON.stringify({ backend: "opencode", agenticBudget: { maxToolCalls: 5 } }),
    );
    expect(partial.agenticBudget).toEqual({ maxToolCalls: 5 });

    const invalid = parseAiConfig(
      JSON.stringify({ backend: "opencode", agenticBudget: { maxWallClockMs: 0, maxToolCalls: -1 } }),
    );
    expect(invalid.agenticBudget).toBeUndefined();

    const garbage = parseAiConfig(
      JSON.stringify({ backend: "opencode", agenticBudget: { maxWallClockMs: "forever" } }),
    );
    expect(garbage.agenticBudget).toBeUndefined();
  });

  it("resolves the agentic budget: config overrides, else the defaults", () => {
    expect(resolveAgenticBudget(defaultAiConfig())).toEqual({
      maxWallClockMs: 5 * 60_000,
      maxToolCalls: 40,
    });
    expect(
      resolveAgenticBudget({ ...defaultAiConfig(), agenticBudget: { maxToolCalls: 12 } }),
    ).toEqual({ maxWallClockMs: 5 * 60_000, maxToolCalls: 12 });
    // A stored invalid cap (parse should drop it) still resolves safely.
    expect(
      resolveAgenticBudget({
        ...defaultAiConfig(),
        agenticBudget: { maxWallClockMs: -5, maxToolCalls: 0 },
      }),
    ).toEqual({ maxWallClockMs: 5 * 60_000, maxToolCalls: 40 });
  });

  it("round-trips the agentic budget through storage", () => {
    const storage = memStorage();
    const first = new AiStore({ storage, providers: {} });
    first.setAgenticBudget({ maxWallClockMs: 90_000, maxToolCalls: 25 });
    const second = new AiStore({ storage, providers: {} });
    expect(second.config.agenticBudget).toEqual({ maxWallClockMs: 90_000, maxToolCalls: 25 });
    second.setAgenticBudget(undefined);
    const third = new AiStore({ storage, providers: {} });
    expect(third.config.agenticBudget).toBeUndefined();
  });

  it("resolves the mode: explicit value wins, legacy URL implies attach, else managed", () => {
    expect(
      resolveOpencodeMode({ ...defaultAiConfig(), opencodeMode: "attach" }),
    ).toBe("attach");
    // Legacy configs (pre-M12) only set a manual attach URL.
    expect(resolveOpencodeMode({ ...defaultAiConfig(), opencodeUrl: "http://localhost:9999" })).toBe(
      "attach",
    );
    expect(resolveOpencodeMode(defaultAiConfig())).toBe("managed");
  });

  it("resolves the provider behavior: disabled wins over the mode", () => {
    expect(resolveOpencode({ ...defaultAiConfig(), opencodeEnabled: false })).toBe("disabled");
    expect(resolveOpencode({ ...defaultAiConfig(), opencodeMode: "attach" })).toBe("attach");
    expect(resolveOpencode(defaultAiConfig())).toBe("managed");
  });

  it("round-trips the managed mode through storage", () => {
    const storage = memStorage();
    const first = new AiStore({ storage, providers: {} });
    first.setOpencodeMode("attach");
    first.setOpencodeEnabled(false);
    const second = new AiStore({ storage, providers: {} });
    expect(second.config.opencodeMode).toBe("attach");
    expect(second.config.opencodeEnabled).toBe(false);
  });

  it("setOptIn(false) removes the entry (and persists the removal)", () => {
    const storage = memStorage();
    const store = new AiStore({ storage, providers: {} });
    store.setOptIn("repo-1", true);
    expect(storage.dump()[AI_STORAGE_KEY]).toContain("repo-1");
    store.setOptIn("repo-1", false);
    expect(storage.dump()[AI_STORAGE_KEY]).not.toContain("repo-1");
    expect(store.isOptedIn("repo-1")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Secrets (OS keyring only)
// ---------------------------------------------------------------------------

describe("AiStore secrets", () => {
  it("saveOpenrouterKey writes the keyring, never localStorage", async () => {
    const { store, storage } = makeHarness();
    await store.saveOpenrouterKey("sk-or-v1-hush");
    expect(secrets.secretsSet).toHaveBeenCalledWith("openrouter.api-key", "sk-or-v1-hush");
    expect(JSON.stringify(storage.dump())).not.toContain("sk-or-v1-hush");
  });

  it("saveOpencodePassword writes the keyring, never localStorage", async () => {
    const { store, storage } = makeHarness();
    await store.saveOpencodePassword("hunter2");
    expect(secrets.secretsSet).toHaveBeenCalledWith("opencode.server.password", "hunter2");
    expect(JSON.stringify(storage.dump())).not.toContain("hunter2");
  });

  it("clear* helpers call secretsDelete", async () => {
    const { store } = makeHarness();
    await store.clearOpenrouterKey();
    await store.clearOpencodePassword();
    expect(secrets.secretsDelete).toHaveBeenCalledWith("openrouter.api-key");
    expect(secrets.secretsDelete).toHaveBeenCalledWith("opencode.server.password");
  });
});

// ---------------------------------------------------------------------------
// Opt-in gating + run state
// ---------------------------------------------------------------------------

describe("AiStore opt-in gating and run state", () => {
  it("run refuses before opt-in and gathers nothing", async () => {
    const { store, git, opencode } = makeHarness();
    const err = await store.run("commit-message", { repoId: "r1" }, git).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("opt-in");
    expect(git.repoDiff).not.toHaveBeenCalled();
    expect(opencode.generate).not.toHaveBeenCalled();
  });

  it("a one-shot run runs without persisted opt-in and does not persist it", async () => {
    const { store, git, opencode } = makeHarness();

    const outcome = await store.generateCommitMessage("r1", git, { oneShot: true });
    expect(outcome.message).toEqual({
      subject: "feat(ui): add the thing",
      body: "It was missing.",
    });
    expect(git.repoDiff).toHaveBeenCalledWith("r1", "head", "index");
    expect(opencode.generate).toHaveBeenCalledTimes(1);
    expect(store.isOptedIn("r1")).toBe(false);

    // The next run without consent still refuses.
    await expect(store.run("commit-message", { repoId: "r1" }, git)).rejects.toMatchObject({
      kind: "opt-in",
    });
  });

  it("a successful commit-message run parses subject/body and records state", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);

    const outcome = await store.generateCommitMessage("r1", git);
    expect(outcome.message).toEqual({
      subject: "feat(ui): add the thing",
      body: "It was missing.",
    });
    expect(git.repoDiff).toHaveBeenCalledWith("r1", "head", "index");
    expect(opencode.generate).toHaveBeenCalledWith(
      expect.objectContaining({ sessionKey: "r1" }),
    );

    const state = store.stateFor("r1");
    expect(state.busy).toBe(false);
    expect(state.error).toBeNull();
    expect(state.last?.message?.subject).toBe("feat(ui): add the thing");
  });

  it("opts.model (the run picker's choice) rides the request over the settings default", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);

    await store.run("commit-message", { repoId: "r1" }, git, { model: "openai/gpt-5" });
    expect(opencode.generate).toHaveBeenCalledWith(
      expect.objectContaining({ model: "openai/gpt-5", sessionKey: "r1" }),
    );
    // A blank override runs on the configured default (no model in the request).
    await store.run("commit-message", { repoId: "r1" }, git, { model: "  " });
    const second = opencode.generate.mock.calls[1]?.[0] as { model?: string };
    expect(second?.model).toBeUndefined();
  });

  it("opts.model wins over a ctx.model default", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);

    await store.run(
      "explain-hunk",
      { repoId: "r1", path: "src/a.ts", hunk: "@@ -1 +1 @@\n+ok", model: "ctx/model" },
      git,
      { model: "picker/model" },
    );
    expect(opencode.generate).toHaveBeenCalledWith(
      expect.objectContaining({ model: "picker/model" }),
    );

    // Without a picker choice the ctx.model default still passes through.
    await store.run(
      "explain-hunk",
      { repoId: "r1", path: "src/a.ts", hunk: "@@ -1 +1 @@\n+ok", model: "ctx/model" },
      git,
    );
    expect(opencode.generate).toHaveBeenLastCalledWith(
      expect.objectContaining({ model: "ctx/model" }),
    );
  });

  it("a failed run records the error and rethrows (no auto-retry)", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.generate.mockRejectedValueOnce(new AiError("bad-response", "no text", {}));

    await expect(store.run("commit-message", { repoId: "r1" }, git)).rejects.toMatchObject({
      kind: "bad-response",
    });
    expect(store.stateFor("r1")).toMatchObject({
      busy: false,
      error: "no text",
    });
    expect(opencode.generate).toHaveBeenCalledTimes(1);
  });

  it("refuses a second concurrent run for the same repo", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    opencode.generate.mockImplementationOnce(() => gate.then(() => ({
      text: "feat: x",
      model: "m",
      backend: "opencode" as const,
      elapsedMs: 1,
    })));

    const first = store.run("commit-message", { repoId: "r1" }, git);
    await expect(store.run("commit-message", { repoId: "r1" }, git)).rejects.toMatchObject({
      kind: "unknown",
    });
    release();
    await first;
  });

  it("peekState reads without creating entries (template-safe)", () => {
    const { store } = makeHarness();
    expect(store.peekState("never-ran")).toEqual({ busy: false, error: null, last: null });
    expect(Object.keys(store.generateState)).toEqual([]);
  });

  it("routing falls back to the other backend when the preferred is down", async () => {
    const { store, git, opencode, openrouter } = makeHarness({
      opencodeProbe: { status: "down" },
    });
    store.setOptIn("r1", true);
    const outcome = await store.generateCommitMessage("r1", git);
    expect(outcome.result.backend).toBe("openrouter");
    expect(openrouter.generate).toHaveBeenCalledTimes(1);
    expect(opencode.generate).not.toHaveBeenCalled();
  });

  it("exposes the supervisor with both providers", () => {
    const { store } = makeHarness();
    expect(store.supervisor.provider("opencode").id).toBe("opencode");
    expect(store.supervisor.provider("openrouter").id).toBe("openrouter");
  });
});

// ---------------------------------------------------------------------------
// SSE liveness loop (provider-owned subscribeLiveness; injectable gate)
// ---------------------------------------------------------------------------

/** A scheduler whose timers never fire (hermetic: no hidden retries). */
function deadScheduler(): { setTimeout: () => number; clearTimeout: () => void } {
  return { setTimeout: () => 0, clearTimeout: () => {} };
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

describe("AiStore SSE liveness loop", () => {
  /** Mock opencode provider WITH the liveness capability. */
  function liveMockProvider() {
    const handlers: Array<(down: boolean) => void> = [];
    const provider = {
      ...mockProvider("opencode"),
      subscribeLiveness: vi.fn((handler: (down: boolean) => void) => {
        handlers.push(handler);
        return () => {};
      }),
    };
    return { provider, handlers };
  }

  it("runs only under the injectable desktop gate and never double-subscribes", async () => {
    const { provider: off } = liveMockProvider();
    // Browser/test posture (no override; isTauri() is false in vitest):
    // the loop never wires even though the backend probes ok.
    const offStore = new AiStore({
      storage: memStorage(),
      providers: { opencode: off, openrouter: mockProvider("openrouter") },
    });
    await offStore.supervisor.check("opencode");
    expect(off.subscribeLiveness).not.toHaveBeenCalled();

    // Desktop posture (injected): subscribes on the ok transition.
    const { provider: on } = liveMockProvider();
    const onStore = new AiStore({
      storage: memStorage(),
      providers: { opencode: on, openrouter: mockProvider("openrouter") },
      isTauri: () => true,
      supervisor: { scheduler: deadScheduler(), jitter: () => 0 },
    });
    await onStore.supervisor.check("opencode");
    expect(on.subscribeLiveness).toHaveBeenCalledTimes(1);
    // Further ok emissions while a subscription exists do not re-subscribe.
    await onStore.supervisor.check("opencode");
    expect(on.subscribeLiveness).toHaveBeenCalledTimes(1);
  });

  it("maps handler(false) to a freshness refresh and handler(true) to resubscription", async () => {
    const { provider, handlers } = liveMockProvider();
    const store = new AiStore({
      storage: memStorage(),
      providers: { opencode: provider, openrouter: mockProvider("openrouter") },
      isTauri: () => true,
      supervisor: { scheduler: deadScheduler(), jitter: () => 0 },
    });
    await store.supervisor.check("opencode");
    expect(handlers.length).toBe(1);

    // Decoded event → transport alive → supervisor freshness refreshed.
    const before = store.supervisor.status("opencode").lastCheck ?? 0;
    handlers[0]!(false);
    expect(store.supervisor.status("opencode").lastCheck ?? 0).toBeGreaterThanOrEqual(before);

    // Stream ended → dropped; the next ok transition resubscribes.
    handlers[0]!(true);
    await store.supervisor.check("opencode");
    expect(provider.subscribeLiveness).toHaveBeenCalledTimes(2);
  });

  it("a managed-server restart rotates the password: the next stream attempt carries the new auth header", async () => {
    // Serve registry: the generated password + port are regenerated per restart.
    let registry = {
      running: true,
      url: "http://127.0.0.1:53011",
      error: null as string | null,
      password: "generated-one",
    };
    const serve = {
      status: vi.fn(async () => registry),
      start: vi.fn(async () => registry),
    };
    const streamAuth: string[] = [];
    let streamController: ReadableStreamDefaultController<Uint8Array> | null = null;
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith("/api/info")) {
        // The managed server identifies as 2.x (JSON probe = app health).
        return jsonResponse(200, { version: "2.0.18", pid: 9 });
      }
      // The event stream: record the auth header the provider just built.
      streamAuth.push(new Headers(init?.headers).get("authorization") ?? "");
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          streamController = controller;
          init?.signal?.addEventListener("abort", () =>
            controller.error(new DOMException("aborted", "AbortError")),
          );
        },
      });
      return new Response(stream, { status: 200 });
    }) as unknown as typeof fetch;

    // The REAL OpenCodeProvider is wired in (fake fetch + fake serve
    // bridge), so the whole path — resolution, header building, SSE
    // subscription — runs for real against fakes.
    const opencode = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve as never,
      fetchImpl,
    });
    const store = new AiStore({
      storage: memStorage(),
      providers: { opencode, openrouter: mockProvider("openrouter") },
      isTauri: () => true,
      supervisor: { scheduler: deadScheduler(), jitter: () => 0 },
    });

    // Startup probe → ok → liveness subscribes with the CURRENT password.
    await vi.waitFor(() => expect(streamAuth.length).toBe(1));
    expect(streamAuth[0]).toBe(`Basic ${btoa("opencode:generated-one")}`);

    // Restart: the server dies (stream errors) and comes back on a new
    // port with a freshly generated password in the registry.
    registry = {
      running: true,
      url: "http://127.0.0.1:53012",
      error: null,
      password: "generated-two",
    };
    streamController!.error(new Error("server went away"));

    // The stream end reports down; the next ok transition resubscribes —
    // through a FRESH connection(), so the rotated password is used
    // instead of the one frozen into the previous subscription.
    await store.supervisor.check("opencode");
    await vi.waitFor(() => expect(streamAuth.length).toBe(2));
    expect(streamAuth[1]).toBe(`Basic ${btoa("opencode:generated-two")}`);
  });
});

// ---------------------------------------------------------------------------
// M13: master switch + per-feature gating + chat store wiring
// ---------------------------------------------------------------------------

describe("AiStore master switch and per-feature gates", () => {
  it("run refuses while the master switch is off, works after re-enable", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);
    store.setEnabled(false);
    await expect(store.run("commit-message", { repoId: "r1" }, git)).rejects.toMatchObject({
      kind: "opt-in",
    });
    expect(opencode.generate).not.toHaveBeenCalled();
    // isEnabled is opt-out: re-enabling deletes the flag and runs again.
    expect(store.isEnabled()).toBe(false);
    store.setEnabled(true);
    expect(store.isEnabled()).toBe(true);
    await store.run("commit-message", { repoId: "r1" }, git);
    expect(opencode.generate).toHaveBeenCalledTimes(1);
  });

  it("setEnabled(false) cancels in-flight jobs", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    opencode.generate.mockImplementationOnce(
      () =>
        new Promise<AiResult>((_resolve, reject) => {
          // Reject on cancel — what a real SDK does with an aborted signal.
          void gate.then(() => reject(new DOMException("aborted", "AbortError")));
        }),
    );
    const pending = store.run("commit-message", { repoId: "r1" }, git);
    store.setEnabled(false); // cancels all jobs
    release();
    await expect(pending).rejects.toMatchObject({ kind: "cancelled" });
    expect(store.jobs.history[0]).toMatchObject({ status: "cancelled" });
  });

  it("per-feature toggles gate only their own kind", async () => {
    const { store, git } = makeHarness();
    store.setOptIn("r1", true);
    store.setFeatureEnabled("commit-message", false);
    expect(store.isFeatureEnabled("commit-message")).toBe(false);
    expect(store.isFeatureEnabled("review-staged")).toBe(true);
    await expect(store.run("commit-message", { repoId: "r1" }, git)).rejects.toMatchObject({
      kind: "opt-in",
    });
    // Re-enable via deletion (default-on semantics).
    store.setFeatureEnabled("commit-message", true);
    expect(store.isFeatureEnabled("commit-message")).toBe(true);
  });

  it("the status-bar toggle persists without disabling AI", () => {
    const storage = memStorage();
    const store = new AiStore({ storage, providers: {} });
    expect(store.isStatusBarEnabled()).toBe(true);
    store.setStatusBarEnabled(false);
    expect(store.isStatusBarEnabled()).toBe(false);
    expect(store.isEnabled()).toBe(true);
    const second = new AiStore({ storage, providers: {} });
    expect(second.isStatusBarEnabled()).toBe(false);
    expect(second.isEnabled()).toBe(true);
  });
});

describe("AiStore chat wiring", () => {
  it("exposes a chat store sharing the same storage and gates", async () => {
    const { store } = makeHarness();
    store.setEnabled(false);
    await expect(
      store.sendChat({ repoId: "r1", root: "/repo", question: "hi", context: { staged: false, commits: false, path: null, pasted: null } }),
    ).rejects.toMatchObject({ kind: "opt-in" });
    store.setEnabled(true);
    await expect(
      store.sendChat({ repoId: "r1", root: "/repo", question: "hi", context: { staged: false, commits: false, path: null, pasted: null } }),
    ).rejects.toMatchObject({ kind: "opt-in" }); // not opted in yet
    store.setOptIn("r1", true);
    const reply = await store.sendChat({
      repoId: "r1",
      root: "/repo",
      question: "hi",
      context: { staged: false, commits: false, path: null, pasted: null },
    });
    expect(reply.role).toBe("assistant");
    // The turn minted a "chat" job (status bar + activity visibility).
    expect(store.jobs.history[0]).toMatchObject({ feature: "chat", status: "done" });
    // The thread is keyed by root (survives repoId churn across launches).
    expect(store.chat.conversationFor("/repo")).toHaveLength(2);
  });

  it("chat turns do not take the one-feature-at-a-time gate", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    opencode.generate.mockImplementationOnce(() => gate.then(() => ({
      text: "feat: x",
      model: "m",
      backend: "opencode" as const,
      elapsedMs: 1,
    })));
    // A feature run in flight…
    const featureRun = store.run("commit-message", { repoId: "r1" }, git);
    // …does not block a chat turn on the same repo.
    const chatRun = store.sendChat({
      repoId: "r1",
      root: "/repo",
      question: "q",
      context: { staged: false, commits: false, path: null, pasted: null },
    });
    await chatRun;
    release();
    await featureRun;
    expect(store.chat.conversationFor("/repo")).toHaveLength(2);
  });
});

describe("AiStore streaming preference (registry-driven)", () => {
  it("stream-preferring features hook deltas; structured ones stay one-shot", async () => {
    const { store, git, opencode } = makeHarness();
    store.setOptIn("r1", true);
    // The provider CAN stream — the feature registry decides per kind.
    const stream = vi.fn(async (_req, onDelta: (d: string) => void) => {
      onDelta("kebab-case-name ");
      return {
        text: "kebab-case-name",
        model: "opencode-model",
        backend: "opencode" as const,
        elapsedMs: 4,
      };
    });
    (opencode as unknown as { generateStream: unknown }).generateStream = stream;

    // branch-name carries no streaming preference → one-shot generate (the
    // parsed answer is the deliverable; streamed deltas were never rendered).
    await store.run("branch-name", { repoId: "r1", description: "add the thing" }, git);
    expect(opencode.generate).toHaveBeenCalledTimes(1);
    expect(stream).not.toHaveBeenCalled();

    // review-staged prefers streaming → generateStream with deltas.
    await store.run("review-staged", { repoId: "r1" }, git);
    expect(stream).toHaveBeenCalledTimes(1);
    expect(opencode.generate).toHaveBeenCalledTimes(1); // unchanged
  });
});

// ---------------------------------------------------------------------------
// Agentic runs (chunk 2): job-bound live runtime via AiStore.runAgentic
// ---------------------------------------------------------------------------

import { AgenticRunController } from "./agenticRun.svelte";
import type { AgenticRunResult } from "./opencode";

/** A fake runAgentic result (the shape the provider's method returns). */
function fakeResult(controller: AgenticRunController, overrides?: Partial<AgenticRunResult>): AgenticRunResult {
  return {
    text: "done",
    model: "opencode-model",
    usage: { totalTokens: 30 },
    steps: [...controller.steps],
    budgetStopped: false,
    budgetReason: null,
    approvals: { ...controller.summary },
    controller,
    sessionID: "sess-1",
    elapsedMs: 42,
    ...overrides,
  };
}

describe("AiStore.runAgentic (chunk 2)", () => {
  it("refuses without opt-in and without the master switch (same gates as run)", async () => {
    const { store } = makeHarness();
    await expect(
      store.runAgentic({ repoId: "r1", prompt: "x", feature: "chat" }),
    ).rejects.toMatchObject({ kind: "opt-in" });

    const { store: disabled } = makeHarness();
    disabled.setOptIn("r1", true);
    disabled.setEnabled(false);
    await expect(
      disabled.runAgentic({ repoId: "r1", prompt: "x", feature: "chat" }),
    ).rejects.toMatchObject({ kind: "opt-in" });
  });

  it("refuses when the opencode provider cannot serve agentic runs", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    // Strip the capability (an OpenRouter-like provider): feature-detect
    // routes the refusal BEFORE a job starts.
    delete (opencode as unknown as { runAgentic?: unknown }).runAgentic;
    await expect(
      store.runAgentic({ repoId: "r1", prompt: "x", feature: "chat" }),
    ).rejects.toMatchObject({ kind: "unavailable" });
    expect(store.jobs.history).toHaveLength(0);
  });

  it("starts a job, binds the controller, streams deltas and settles done", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.runAgentic.mockImplementation(
      async (req: { onController?: (c: AgenticRunController) => void; onDelta?: (d: string) => void }) => {
        const controller = new AgenticRunController({
          sessionID: "sess-1",
          reply: async () => undefined,
          interrupt: async () => undefined,
        });
        req.onController?.(controller);
        controller.handleToolCalled({ id: "t1", input: { command: "git status" }, executed: true });
        controller.handleToolFinished("t1", true);
        req.onDelta?.("partial ");
        req.onDelta?.("answer");
        return fakeResult(controller, { text: "partial answer" });
      },
    );
    const result = await store.runAgentic({ repoId: "r1", prompt: "explore", feature: "chat" });
    expect(result).toMatchObject({
      text: "partial answer",
      backend: "opencode",
      model: "opencode-model",
    });
    // The run was served as a first-class job with the agentic snapshot.
    const record = store.jobs.history[0];
    expect(record).toMatchObject({
      repoId: "r1",
      feature: "chat",
      status: "done",
      backend: "opencode",
      model: "opencode-model",
      text: "partial answer",
    });
    expect(record?.agentic?.steps).toEqual([
      expect.objectContaining({ id: "t1", tool: "shell", args: "git status", status: "ok" }),
    ]);
    expect(record?.usage).toMatchObject({ totalTokens: 30 });
    expect(store.jobs.activeJob("r1", "chat")).toBeNull();
  });

  it("cancel settles the job as cancelled with the partial text kept", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.runAgentic.mockImplementation(
      async (req: {
        signal?: AbortSignal;
        onController?: (c: AgenticRunController) => void;
        onDelta?: (d: string) => void;
      }) => {
        const controller = new AgenticRunController({
          sessionID: "sess-1",
          reply: async () => undefined,
          interrupt: async () => undefined,
        });
        req.onController?.(controller);
        req.onDelta?.("partial before cancel");
        // The user cancels through the store (status-bar/panel path); the
        // provider reacts to the abort by settling cancelled.
        const job = store.jobs.activeJob("r1", "chat");
        store.jobs.cancel(job!.id);
        await new Promise<void>((resolve) => {
          if (req.signal?.aborted) resolve();
          else req.signal?.addEventListener("abort", () => resolve(), { once: true });
        });
        throw new AiError("cancelled", "AI request cancelled", { backend: "opencode" });
      },
    );
    await expect(
      store.runAgentic({ repoId: "r1", prompt: "explore", feature: "chat" }),
    ).rejects.toMatchObject({ kind: "cancelled" });
    const record = store.jobs.history[0];
    expect(record).toMatchObject({ status: "cancelled", text: "partial before cancel" });
    expect(record?.agentic).not.toBeNull();
  });

  it("failures land in the job record with the safe error message", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.runAgentic.mockRejectedValue(new AiError("bad-response", "opencode exploded", {}));
    await expect(
      store.runAgentic({ repoId: "r1", prompt: "x", feature: "chat" }),
    ).rejects.toMatchObject({ kind: "bad-response" });
    expect(store.jobs.history[0]).toMatchObject({
      status: "error",
      error: "opencode exploded",
    });
  });

  it("the bound controller is reachable through the live job while running", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    let release: (() => void) | null = null;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    opencode.runAgentic.mockImplementation(
      async (req: {
        signal?: AbortSignal;
        onServing?: (backend: string, model: string | null) => void;
        onController?: (c: AgenticRunController) => void;
      }) => {
        const controller = new AgenticRunController({
          sessionID: "sess-1",
          reply: async () => undefined,
          interrupt: async () => undefined,
        });
        req.onServing?.("opencode", "opencode-model");
        req.onController?.(controller);
        controller.handlePermissionAsk({
          id: "ask-1",
          action: "shell",
          resources: ["npm test"],
        });
        await gate;
        return fakeResult(controller);
      },
    );
    const pending = store.runAgentic({ repoId: "r1", prompt: "x", feature: "chat" });
    // Give the glue a tick to begin the job, then assert the live state.
    await new Promise((resolve) => setTimeout(resolve, 0));
    const job = store.jobs.activeJob("r1", "chat");
    expect(job?.agentic?.controller.pending).toHaveLength(1);
    // The serving seam feeds the live row's "via <model>" like single-shot.
    expect(job?.model).toBe("opencode-model");
    expect(store.jobs.activeJob("r1", "chat")).not.toBeNull();
    (release as unknown as (() => void) | undefined)?.();
    await pending.catch(() => {});
  });
});

describe("AiStore.runAgentic — chat orchestration (chunk 3)", () => {
  /** A full AgenticRunResult for mockResolvedValue. */
  function runResult(sessionID: string): Record<string, unknown> {
    return {
      text: "agentic answer",
      model: "m-1",
      usage: { totalTokens: 30 },
      steps: [],
      budgetStopped: false,
      budgetReason: null,
      approvals: { asked: 0, autoRejected: 0, approved: 0, rejected: 0, cancelled: 0, expired: 0 },
      controller: {},
      sessionID,
      elapsedMs: 9,
    };
  }

  it("runAgentic adopts a caller-owned ticket (chat) without settling it", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.runAgentic.mockResolvedValue(runResult("sess-adopt"));

    const ticket = store.jobs.begin({ repoId: "r1", feature: "chat" });
    const result = await store.runAgentic({
      repoId: "r1",
      prompt: "x",
      feature: "chat",
      ticket,
      sessionID: "sess-bind",
      model: "m-1",
    });
    // The session that SERVED the run is reported (retry-aware) and the
    // caller's session binding request rode through.
    expect(result.sessionID).toBe("sess-adopt");
    expect(opencode.runAgentic).toHaveBeenCalledWith(
      expect.objectContaining({ sessionID: "sess-bind", model: "m-1" }),
    );
    // Adopted: the caller owns begin + settle — the job is still active.
    expect(store.jobs.active[ticket.id]).toBeDefined();
    expect(store.jobs.history).toHaveLength(0);
    ticket.done({ backend: "opencode", model: "m-1", elapsedMs: 1, usage: null, text: "out" });
    expect(store.jobs.active[ticket.id]).toBeUndefined();
  });

  it("agentic chat persists the thread binding and reuses it (full chain)", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.runAgentic.mockResolvedValue(runResult("sess-9"));

    const reply = await store.sendChat({
      repoId: "r1",
      root: "/repo",
      question: "What changed?",
      context: { staged: false, commits: false, path: null, pasted: null },
      agentic: true,
    });
    expect(reply.mode).toBe("agentic");
    expect(store.chat.sessionFor("/repo")).toBe("sess-9");
    expect(opencode.runAgentic).toHaveBeenCalledTimes(1);
    expect(opencode.runAgentic).toHaveBeenCalledWith(
      expect.objectContaining({ sessionID: undefined, system: expect.any(String) }),
    );

    // The next agentic message reuses the binding (switch-then-prompt
    // semantics live in the provider).
    await store.sendChat({
      repoId: "r1",
      root: "/repo",
      question: "and now?",
      context: { staged: false, commits: false, path: null, pasted: null },
      agentic: true,
    });
    expect(opencode.runAgentic).toHaveBeenLastCalledWith(
      expect.objectContaining({ sessionID: "sess-9" }),
    );
    expect(store.jobs.history).toHaveLength(2);
  });

  it("chat without the agentic chip rides the router (single-shot, marked)", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    opencode.runAgentic.mockResolvedValue(runResult("sess-never"));
    opencode.generate.mockResolvedValue({
      text: "single-shot answer",
      model: "m",
      backend: "opencode",
      elapsedMs: 3,
    });
    const reply = await store.sendChat({
      repoId: "r1",
      root: "/repo",
      question: "q",
      context: { staged: false, commits: false, path: null, pasted: null },
    });
    expect(reply.mode).toBe("single-shot");
    expect(opencode.runAgentic).not.toHaveBeenCalled();
    expect(store.chat.sessionFor("/repo")).toBeNull();
  });
});

describe("AiStore.agenticAvailable / runAgenticWithFallback (chunk 4)", () => {
  /** Wire a connection capability onto the mock provider. */
  function withConnection(opencode: ReturnType<typeof mockProvider>, serverKind: "v1" | "v2" | "throws"): void {
    (opencode as unknown as { connection?: unknown }).connection =
      serverKind === "throws"
        ? vi.fn(async () => {
            throw new Error("down");
          })
        : vi.fn(async () => ({ baseUrl: "http://x", authHeader: null, serverKind }));
  }

  it("agenticAvailable probes capability + a 2.x server", async () => {
    const { store, opencode } = makeHarness();
    // No runAgentic capability on the provider:
    delete (opencode as unknown as { runAgentic?: unknown }).runAgentic;
    withConnection(opencode, "v2");
    expect(await store.agenticAvailable()).toBe(false);

    // Capability present, no connection capability (probe impossible):
    const { store: store2, opencode: opencode2 } = makeHarness();
    expect(await store2.agenticAvailable()).toBe(false);

    // Capability + v2 server:
    const { store: store3, opencode: opencode3 } = makeHarness();
    withConnection(opencode3, "v2");
    expect(await store3.agenticAvailable()).toBe(true);

    // 1.x server:
    const { store: store4, opencode: opencode4 } = makeHarness();
    withConnection(opencode4, "v1");
    expect(await store4.agenticAvailable()).toBe(false);

    // Probe failure (server down):
    const { store: store5, opencode: opencode5 } = makeHarness();
    withConnection(opencode5, "throws");
    expect(await store5.agenticAvailable()).toBe(false);
  });

  it("runAgenticWithFallback runs agentic when a v2 path exists", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    withConnection(opencode, "v2");
    opencode.runAgentic.mockResolvedValue({
      text: "agentic diagnosis",
      model: "m-1",
      backend: "opencode",
      elapsedMs: 7,
      sessionID: "sess-t",
    });
    const fallback = vi.fn(async (): Promise<AiResult> => ({ text: "single", model: "m", backend: "opencode", elapsedMs: 1 }));
    const { result, mode } = await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "diagnose p",
      feature: "diagnose-workflow",
      fallback,
    });
    expect(mode).toBe("agentic");
    expect(result.text).toBe("agentic diagnosis");
    // The mode marker rides the serving session (agentic-only field).
    expect(result.sessionID).toBe("sess-t");
    expect(fallback).not.toHaveBeenCalled();
    expect(opencode.runAgentic).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: "diagnose p", sessionKey: "r1" }),
    );
  });

  it("a pre-start refusal falls back silently (single-shot runs)", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    withConnection(opencode, "v2");
    // The probe passed, but the dispatch refuses pre-start (e.g. the
    // server flipped to 1.x between the probe and the run).
    opencode.runAgentic.mockRejectedValue(
      new AiError("unavailable", "agentic runs need a 2.x opencode server — this one is 1.x", {
        backend: "opencode",
      }),
    );
    const fallback = vi.fn(async (): Promise<AiResult> => ({ text: "single", model: "m", backend: "opencode", elapsedMs: 1 }));
    const { result, mode } = await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "p",
      feature: "diagnose-action",
      fallback,
    });
    expect(mode).toBe("single-shot");
    expect(result.text).toBe("single");
    expect(fallback).toHaveBeenCalledTimes(1);
  });

  it("a post-start failure surfaces honestly (no fallback re-run)", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    withConnection(opencode, "v2");
    opencode.runAgentic.mockImplementation(async (req: { onController?: (c: unknown) => void }) => {
      // A real controller: the glue binds it and snapshots its state.
      const controller = new AgenticRunController({
        sessionID: "sess-x",
        reply: async () => undefined,
        interrupt: async () => undefined,
      });
      req.onController?.(controller);
      throw new AiError("unavailable", "dead stream", { backend: "opencode" });
    });
    const fallback = vi.fn(async (): Promise<AiResult> => ({ text: "single", model: "m", backend: "opencode", elapsedMs: 1 }));
    await expect(
      store.runAgenticWithFallback({
        repoId: "r1",
        prompt: "p",
        feature: "diagnose-workflow",
        fallback,
      }),
    ).rejects.toMatchObject({ kind: "unavailable" });
    expect(fallback).not.toHaveBeenCalled();
  });

  it("a false probe skips the agentic attempt entirely (no doomed ticket)", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    withConnection(opencode, "v1");
    const fallback = vi.fn(async (): Promise<AiResult> => ({ text: "single", model: "m", backend: "opencode", elapsedMs: 1 }));
    const { result, mode } = await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "p",
      feature: "diagnose-workflow",
      fallback,
    });
    expect(mode).toBe("single-shot");
    expect(result.text).toBe("single");
    expect(opencode.runAgentic).not.toHaveBeenCalled();
  });
});

describe("Agentic diagnose — fix round (chunk 4 review)", () => {
  /** Wires a v2 connection onto the mock provider. */
  function withV2(opencode: ReturnType<typeof mockProvider>): void {
    (opencode as unknown as { connection?: unknown }).connection = vi.fn(async () => ({
      baseUrl: "http://x",
      authHeader: null,
      serverKind: "v2" as const,
    }));
  }

  it("one-shot consent covers an agentic diagnose run (not persisted)", async () => {
    const { store, opencode } = makeHarness();
    withV2(opencode);
    opencode.runAgentic.mockResolvedValue({
      text: "agentic diagnosis",
      model: "m-1",
      backend: "opencode",
      elapsedMs: 5,
      sessionID: "sess-t",
    });
    // Not opted in + "Just once": the agentic run proceeds…
    const { result, mode } = await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "p",
      feature: "diagnose-workflow",
      oneShot: true,
      fallback: async (): Promise<AiResult> => ({
        text: "single",
        model: "m",
        backend: "opencode",
        elapsedMs: 1,
      }),
    });
    expect(mode).toBe("agentic");
    expect(result.text).toBe("agentic diagnosis");
    // …and the consent is NOT persisted.
    expect(store.isOptedIn("r1")).toBe(false);
  });

  it("no one-shot consent without opt-in still refuses (unchanged)", async () => {
    const { store, opencode } = makeHarness();
    withV2(opencode);
    await expect(
      store.runAgenticWithFallback({
        repoId: "r1",
        prompt: "p",
        feature: "diagnose-workflow",
        fallback: async (): Promise<AiResult> => ({
          text: "single",
          model: "m",
          backend: "opencode",
          elapsedMs: 1,
        }),
      }),
    ).rejects.toMatchObject({ kind: "opt-in" });
    expect(opencode.runAgentic).not.toHaveBeenCalled();
  });

  it("a pre-start refusal settles the agentic ticket cancelled (one visible row)", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    withV2(opencode);
    // The probe passed but the dispatch refuses pre-start (server flipped
    // to 1.x between probe and run).
    opencode.runAgentic.mockRejectedValue(
      new AiError("unavailable", "agentic runs need a 2.x opencode server — this one is 1.x", {
        backend: "opencode",
      }),
    );
    // The single-shot fallback is ai.run-shaped: its own ticket, settled done.
    const fallback = vi.fn(async (): Promise<AiResult> => {
      const ticket = store.jobs.begin({ repoId: "r1", feature: "diagnose-workflow" });
      ticket.done({ backend: "opencode", model: "m", elapsedMs: 2, usage: null, text: "single" });
      return { text: "single", model: "m", backend: "opencode", elapsedMs: 2 };
    });
    const { mode } = await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "p",
      feature: "diagnose-workflow",
      fallback,
    });
    expect(mode).toBe("single-shot");
    const statuses = store.jobs.history.map((record) => record.status).sort();
    expect(statuses).toEqual(["cancelled", "done"]); // no "error" row
  });

  it("backend=openrouter gates the capability off (no server spawn)", async () => {
    const { store, opencode } = makeHarness();
    store.setBackend("openrouter");
    // Even with a capable provider and a v2 connection wired:
    withV2(opencode);
    expect(await store.agenticAvailable()).toBe(false);
    // The managed serve registry was never touched (no spawn, no probe).
    const ipc = await import("$lib/ipc/client");
    expect(ipc.opencodeServeStart).not.toHaveBeenCalled();
    expect(ipc.opencodeServeStatus).not.toHaveBeenCalled();
  });

  it("the busy slot wraps the agentic attempt and clears on every exit path", async () => {
    const { store, opencode } = makeHarness();
    store.setOptIn("r1", true);
    withV2(opencode);

    // Success path:
    opencode.runAgentic.mockResolvedValueOnce({
      text: "ok",
      model: "m",
      backend: "opencode",
      elapsedMs: 3,
      sessionID: "sess-t",
    });
    await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "p",
      feature: "diagnose-workflow",
      fallback: async (): Promise<AiResult> => ({
        text: "single",
        model: "m",
        backend: "opencode",
        elapsedMs: 1,
      }),
    });
    expect(store.peekState("r1").busy).toBe(false);

    // Post-start failure path:
    opencode.runAgentic.mockImplementationOnce(async (req: { onController?: (c: unknown) => void }) => {
      const controller = new AgenticRunController({
        sessionID: "sess-x",
        reply: async () => undefined,
        interrupt: async () => undefined,
      });
      req.onController?.(controller);
      throw new AiError("unavailable", "dead stream", { backend: "opencode" });
    });
    await expect(
      store.runAgenticWithFallback({
        repoId: "r1",
        prompt: "p",
        feature: "diagnose-workflow",
        fallback: async (): Promise<AiResult> => ({
          text: "single",
          model: "m",
          backend: "opencode",
          elapsedMs: 1,
        }),
      }),
    ).rejects.toMatchObject({ kind: "unavailable" });
    expect(store.peekState("r1").busy).toBe(false);

    // Fallback path:
    opencode.runAgentic.mockRejectedValueOnce(
      new AiError("unavailable", "agentic runs need a 2.x opencode server — this one is 1.x", {
        backend: "opencode",
      }),
    );
    await store.runAgenticWithFallback({
      repoId: "r1",
      prompt: "p",
      feature: "diagnose-workflow",
      fallback: async (): Promise<AiResult> => ({
        text: "single",
        model: "m",
        backend: "opencode",
        elapsedMs: 1,
      }),
    });
    expect(store.peekState("r1").busy).toBe(false);

    // Concurrency: a busy slot refuses the next run (single-shot parity).
    store.stateFor("r1").busy = true;
    await expect(
      store.runAgenticWithFallback({
        repoId: "r1",
        prompt: "p",
        feature: "diagnose-workflow",
        fallback: async (): Promise<AiResult> => ({
          text: "single",
          model: "m",
          backend: "opencode",
          elapsedMs: 1,
        }),
      }),
    ).rejects.toMatchObject({ kind: "unknown" });
    store.stateFor("r1").busy = false;
  });
});
