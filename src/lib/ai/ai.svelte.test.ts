/**
 * Unit tests for the AI runes store: config persistence (localStorage,
 * malformed-tolerant), per-repo opt-in gating, run-state management
 * (busy/error/last), the secrets-never-in-localStorage guarantee, and
 * end-to-end routing through the supervisor (fallback included).
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FileDiff } from "$lib/ipc/types";
import { AiStore, parseAiConfig, AI_STORAGE_KEY } from "./ai.svelte";
import type { AiProbeResult, AiProvider, ProviderSet } from "./provider";
import type { FeatureGitClient } from "./features";
import { AiError } from "./types";
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
  repoDiff: vi.fn(),
  streamLog: vi.fn(),
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
    listModels: vi.fn(async () => [{ id: `${id}/model-a`, label: id }]),
    generate: vi.fn(async (): Promise<AiResult> => ({
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
}): Harness {
  const storage = opts?.storage ?? memStorage();
  const opencode = mockProvider("opencode", opts?.opencodeProbe);
  const openrouter = mockProvider("openrouter", opts?.openrouterProbe);
  const providers: Partial<ProviderSet> = { opencode, openrouter };
  const store = new AiStore({ storage, providers });
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
