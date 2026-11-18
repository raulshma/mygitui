/**
 * Unit tests for the typed IPC client (`$lib/ipc/client`). The Tauri runtime
 * is never present in jsdom, so every test here exercises either an injected
 * mock transport or the documented non-Tauri fallbacks.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  authRespond,
  branchCreate,
  branchDelete,
  branchIsMerged,
  branchRename,
  branchSwitch,
  branches,
  cloneRepo,
  closeRepo,
  commit,
  commitMessage,
  conflictResolve,
  discard,
  fetchRepo,
  hooksList,
  IpcError,
  lfsLock,
  lfsLocks,
  lfsUnlock,
  noteAdd,
  noteRemove,
  commitNotes,
  onAuthRequest,
  onCloneProgress,
  onOpProgress,
  onRepoChanged,
  openRepo,
  openWithCustom,
  pickFolder,
  pullRepo,
  pushRepo,
  readBlob,
  readFile,
  remotes,
  remoteAdd,
  remoteRemove,
  remoteSetUrl,
  repoBlame,
  repoBundle,
  repoClean,
  repoDiff,
  repoRefs,
  repoStatus,
  resetChannelFactory,
  resetTransport,
  rebaseSkip,
  setChannelFactory,
  setTransport,
  SECRET_KEYS,
  secretsDelete,
  secretsGet,
  secretsSet,
  signingInfo,
  stage,
  stageAll,
  stashApply,
  stashPush,
  stashRename,
  stashShow,
  streamDiff,
  streamFileHistory,
  streamLog,
  submoduleAdd,
  submoduleDeinit,
  tagCreate,
  tagDelete,
  worktreeAdd,
  worktreeLock,
  worktreeUnlock,
  type ChannelLike,
  type Transport,
} from "$lib/ipc/client";
import type {
  BlameLine,
  BlameQuery,
  BranchInfo,
  CommitOptions,
  FetchOptions,
  FileDiff,
  HookInfo,
  LfsLock,
  LogPage,
  NetStats,
  NoteInfo,
  PullOptions,
  PushOptions,
  RebaseState,
  RemoteInfo,
  RepoInfo,
  RepoStatus,
  SigningInfo,
  StageRequest,
} from "$lib/ipc/types";

// ---------------------------------------------------------------------------
// Fixtures (snake_case, exactly what the Rust side serializes)
// ---------------------------------------------------------------------------

const REPO_INFO: RepoInfo = {
  repo_id: "repo-1",
  root: "C:/repos/mygitui",
  name: "mygitui",
  bare: false,
  git_dir: "C:/repos/mygitui/.git",
  device_id: null,
};

const REPO_STATUS: RepoStatus = {
  branch: "main",
  head: "abc123def456",
  detached: false,
  ahead: 2,
  behind: 1,
  merging: false,
  rebasing: false,
  sequencer: false,  bisecting: false,
  entries: [
    { path: "src/a.ts", old_path: null, index: "modified", worktree: "unmodified" },
    { path: "src/b.ts", old_path: null, index: "unmodified", worktree: "modified" },
  ],
};

const LOG_PAGE: LogPage = {
  commits: [
    {
      sha: "abc123",
      parents: ["def456"],
      author: { name: "Ada", email: "ada@example.com", time: 1_700_000_000, offset_minutes: 60 },
      committer: { name: "Ada", email: "ada@example.com", time: 1_700_000_000, offset_minutes: 60 },
      summary: "Initial commit",
      refs: ["HEAD -> main"],
    },
  ],
  rows: [{ sha: "abc123", lane: 0, edges: [], lane_count: 1 }],
  next_cursor: "cursor-1",
  generation: 3,
};

const EMPTY_LOG_PAGE: LogPage = {
  commits: [],
  rows: [],
  next_cursor: null,
  generation: 0,
};

const FILE_DIFF: FileDiff = {
  path: "src/a.ts",
  old_path: null,
  binary: false,
  is_image: false,
  additions: 3,
  deletions: 1,
  hunks: [
    {
      old_start: 1,
      new_start: 1,
      lines: [
        { old_no: 1, new_no: 1, origin: " ", text: "context", highlights: [] },
        { old_no: null, new_no: 2, origin: "+", text: "added", highlights: [[0, 5]] },
      ],
    },
  ],
};

const BLAME: BlameLine[] = [
  {
    line_no: 1,
    sha: "abc123",
    signature: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    final_sha: "abc123",
    final_signature: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    text: "hello",
    ignored: false,
  },
];

// ---------------------------------------------------------------------------
// Mock transport helpers
// ---------------------------------------------------------------------------

interface RecordedCall {
  command: string;
  args: Record<string, unknown>;
}

/** A transport that records every call and delegates to `handle`. */
function mockTransport(
  handle: (call: RecordedCall) => unknown = () => undefined,
): { transport: Transport; calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const transport: Transport = async (command, args) => {
    const call = { command, args: args ?? {} };
    calls.push(call);
    return handle(call);
  };
  return { transport, calls };
}

/** Extracts the channel a streaming command received. */
function channelOf<T>(call: RecordedCall): ChannelLike<T> {
  return call.args.on_page as ChannelLike<T>;
}

afterEach(() => {
  resetTransport();
  resetChannelFactory();
});

// ---------------------------------------------------------------------------
// Command wrappers (mock transport)
// ---------------------------------------------------------------------------

describe("ipc client command wrappers", () => {
  it("openRepo maps the JSON response to a typed RepoInfo (snake_case passthrough)", async () => {
    const { transport, calls } = mockTransport(() => REPO_INFO);
    setTransport(transport);

    const info = await openRepo("C:/repos/mygitui");

    expect(info).toEqual(REPO_INFO); // no camelCase conversion anywhere
    expect(info.repo_id).toBe("repo-1");
    expect(calls).toEqual([
      { command: "repo_open", args: { path: "C:/repos/mygitui" } },
    ]);
  });

  it("closeRepo sends the repo_id argument in snake_case", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await closeRepo("repo-1");

    expect(calls).toEqual([
      { command: "repo_close", args: { repo_id: "repo-1" } },
    ]);
  });

  it("repoStatus returns the typed status snapshot", async () => {
    const { transport, calls } = mockTransport(() => REPO_STATUS);
    setTransport(transport);

    const status = await repoStatus("repo-1");

    expect(status).toEqual(REPO_STATUS);
    expect(status.entries).toHaveLength(2);
    expect(calls[0]?.args).toEqual({ repo_id: "repo-1" });
  });

  it("repoStatus forwards an incremental paths batch; omitted paths = full rescan", async () => {
    const { transport, calls } = mockTransport(() => REPO_STATUS);
    setTransport(transport);

    await repoStatus("repo-1", ["src/a.ts", "src/b.ts"]);

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      paths: ["src/a.ts", "src/b.ts"],
    });
  });

  it("commitMessage resolves the repo_commit_message payload verbatim", async () => {
    const { transport, calls } = mockTransport(() => "subject\n\nbody");
    setTransport(transport);

    await expect(commitMessage("repo-1", "abc123")).resolves.toBe(
      "subject\n\nbody",
    );
    expect(calls[0]?.command).toBe("repo_commit_message");
    expect(calls[0]?.args).toEqual({ repo_id: "repo-1", sha: "abc123" });
  });

  it("repoRefs returns name→sha pairs", async () => {
    const refs: [string, string][] = [
      ["refs/heads/main", "abc123"],
      ["refs/tags/v0.1.0", "def456"],
    ];
    const { transport } = mockTransport(() => refs);
    setTransport(transport);

    await expect(repoRefs("repo-1")).resolves.toEqual(refs);
  });

  it("repoBlame omits `from` when not given and includes it when given", async () => {
    const { transport, calls } = mockTransport(() => BLAME);
    setTransport(transport);

    await repoBlame("repo-1", "src/a.ts");
    await repoBlame("repo-1", "src/a.ts", "abc123");

    expect(calls[0]?.args).toEqual({ repo_id: "repo-1", path: "src/a.ts" });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      path: "src/a.ts",
      from: "abc123",
    });
  });

  it("repoBlame forwards the optional BlameQuery verbatim (null keeps it off)", async () => {
    const { transport, calls } = mockTransport(() => BLAME);
    setTransport(transport);

    const query: BlameQuery = {
      from: "def456",
      oldest: "aaa111",
      ignore_whitespace: true,
      first_parent: false,
      ignore_revs: true,
    };
    await repoBlame("repo-1", "src/a.ts", "def456", query);
    await repoBlame("repo-1", "src/a.ts", "def456", null);

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      path: "src/a.ts",
      from: "def456",
      query,
    });
    // A null query stays off the wire (legacy shape unchanged).
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      path: "src/a.ts",
      from: "def456",
    });
  });

  it("repoDiff passes sides verbatim and omits the query without options", async () => {
    const { transport, calls } = mockTransport(() => [FILE_DIFF]);
    setTransport(transport);

    await repoDiff("repo-1", "head", "worktree");
    await repoDiff("repo-1", { commit: "abc123" }, "index", ["src/a.ts"]);
    await repoDiff("repo-1", "head", "worktree", ["src/a.ts"], true);

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      old: "head",
      new: "worktree",
    });
    // Paths ride inside the optional DiffQuery, never as a top-level arg.
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      old: { commit: "abc123" },
      new: "index",
      query: { paths: ["src/a.ts"], ignore_whitespace: false },
    });
    expect(calls[2]?.args).toEqual({
      repo_id: "repo-1",
      old: "head",
      new: "worktree",
      query: { paths: ["src/a.ts"], ignore_whitespace: true },
    });
  });
});

// ---------------------------------------------------------------------------
// Streaming commands (mock transport drives the channel)
// ---------------------------------------------------------------------------

describe("ipc client streaming wrappers", () => {
  it("streamLog delivers pages through the channel in order", async () => {
    const { transport, calls } = mockTransport((call) => {
      const channel = channelOf<LogPage>(call);
      channel.onmessage(LOG_PAGE);
      channel.onmessage(EMPTY_LOG_PAGE);
    });
    setTransport(transport);

    const pages: LogPage[] = [];
    await streamLog("repo-1", { regex: false, refs: [], follow: false }, (page) =>
      pages.push(page),
    );

    expect(pages).toEqual([LOG_PAGE, EMPTY_LOG_PAGE]);
    expect(calls[0]?.command).toBe("repo_log_stream");
    expect(calls[0]?.args.repo_id).toBe("repo-1");
    expect(calls[0]?.args.filter).toEqual({ regex: false, refs: [], follow: false });
    expect(typeof (calls[0]?.args.on_page as ChannelLike<LogPage>).onmessage).toBe(
      "function",
    );
  });

  it("streamDiff delivers file pages and forwards options inside the query", async () => {
    const { transport, calls } = mockTransport((call) => {
      channelOf<FileDiff[]>(call).onmessage([FILE_DIFF]);
    });
    setTransport(transport);

    const pages: FileDiff[][] = [];
    await streamDiff("repo-1", "head", "worktree", (files) => pages.push(files), [
      "src/a.ts",
    ]);

    expect(pages).toEqual([[FILE_DIFF]]);
    expect(calls[0]?.command).toBe("repo_diff_stream");
    expect(calls[0]?.args).toMatchObject({
      repo_id: "repo-1",
      old: "head",
      new: "worktree",
      query: { paths: ["src/a.ts"], ignore_whitespace: false },
    });

    // No options at all: the query arg stays off the invoke entirely.
    await streamDiff("repo-1", "head", "worktree", () => {});
    expect(calls[1]?.args).not.toHaveProperty("query");

    // Ignore-whitespace rides the query alongside the paths.
    await streamDiff(
      "repo-1",
      "head",
      "worktree",
      () => {},
      ["src/a.ts"],
      "repo-diff",
      true,
    );
    expect(calls[2]?.args).toMatchObject({
      consumer_id: "repo-diff",
      query: { paths: ["src/a.ts"], ignore_whitespace: true },
    });
  });

  it("streamFileHistory sends the path and forwards log pages", async () => {
    const { transport, calls } = mockTransport((call) => {
      channelOf<LogPage>(call).onmessage(LOG_PAGE);
    });
    setTransport(transport);

    const pages: LogPage[] = [];
    await streamFileHistory("repo-1", "src/a.ts", (page) => pages.push(page));

    expect(pages).toEqual([LOG_PAGE]);
    expect(calls[0]?.command).toBe("repo_file_history");
    expect(calls[0]?.args).toMatchObject({ repo_id: "repo-1", path: "src/a.ts" });
  });

  it("stream wrappers forward the optional consumer_id as the stream slot", async () => {
    const { transport, calls } = mockTransport((call) => {
      if (call.command === "repo_diff_stream") {
        channelOf<FileDiff[]>(call).onmessage([FILE_DIFF]);
      } else {
        channelOf<LogPage>(call).onmessage(EMPTY_LOG_PAGE);
      }
    });
    setTransport(transport);

    await streamLog("repo-1", { regex: false, refs: [], follow: false }, () => {}, "history");
    await streamFileHistory("repo-1", "src/a.ts", () => {}, "file-history");
    await streamDiff("repo-1", "head", "worktree", () => {}, ["src/a.ts"], "repo-diff");

    expect(calls[0]?.command).toBe("repo_log_stream");
    expect(calls[0]?.args.consumer_id).toBe("history");
    expect(calls[1]?.command).toBe("repo_file_history");
    expect(calls[1]?.args.consumer_id).toBe("file-history");
    expect(calls[2]?.command).toBe("repo_diff_stream");
    expect(calls[2]?.args.consumer_id).toBe("repo-diff");
  });

  it("streaming uses an injected channel factory when provided", async () => {
    const created: Array<ChannelLike<LogPage>> = [];
    setChannelFactory(<T,>(onMessage: (m: T) => void): ChannelLike<T> => {
      const ch: ChannelLike<T> = { onmessage: onMessage };
      created.push(ch as unknown as ChannelLike<LogPage>);
      return ch;
    });
    const { transport } = mockTransport((call) => {
      channelOf<LogPage>(call).onmessage(LOG_PAGE);
    });
    setTransport(transport);

    const pages: LogPage[] = [];
    await streamLog("repo-1", { regex: false, refs: [], follow: false }, (p) =>
      pages.push(p),
    );

    expect(created).toHaveLength(1);
    expect(pages).toEqual([LOG_PAGE]);
  });
});

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

describe("ipc client error handling", () => {
  it("rejects with IpcError carrying the command and message (Error cause)", async () => {
    const { transport } = mockTransport(() => {
      throw new Error("not a git repository");
    });
    setTransport(transport);

    const err = await openRepo("C:/not-a-repo").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    const ipcErr = err as IpcError;
    expect(ipcErr.command).toBe("repo_open");
    expect(ipcErr.message).toBe("not a git repository");
    expect(ipcErr.name).toBe("IpcError");
  });

  it("rejects with IpcError when the transport throws a raw string", async () => {
    const { transport } = mockTransport(() => {
      throw "boom";
    });
    setTransport(transport);

    const err = await repoStatus("repo-1").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("repo_status");
    expect((err as IpcError).message).toBe("boom");
  });

  it("streaming failures normalize to IpcError too", async () => {
    const { transport } = mockTransport(() => {
      throw new Error("backend exploded");
    });
    setTransport(transport);

    const err = await streamLog("repo-1", { regex: false, refs: [], follow: false }, () => {}).catch(
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("repo_log_stream");
  });
});

// ---------------------------------------------------------------------------
// Non-Tauri fallbacks (no transport injected — jsdom has no Tauri runtime)
// ---------------------------------------------------------------------------

describe("ipc client non-Tauri behavior", () => {
  it("stream commands resolve immediately with a single empty page (no throw)", async () => {
    const logPages: LogPage[] = [];
    await expect(
      streamLog("repo-1", { regex: false, refs: [], follow: false }, (p) =>
        logPages.push(p),
      ),
    ).resolves.toBeUndefined();
    expect(logPages).toEqual([EMPTY_LOG_PAGE]);

    const diffPages: FileDiff[][] = [];
    await expect(
      streamDiff("repo-1", "head", "worktree", (files) => diffPages.push(files)),
    ).resolves.toBeUndefined();
    expect(diffPages).toEqual([[]]);
  });

  it("non-stream commands reject with IpcError outside Tauri", async () => {
    const err = await openRepo("C:/repos/mygitui").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("repo_open");
    expect((err as IpcError).message).toContain("Tauri runtime");
  });

  it("onRepoChanged resolves with a callable no-op unlisten outside Tauri", async () => {
    let fired = 0;
    const unlisten = await onRepoChanged(() => {
      fired += 1;
    });
    expect(typeof unlisten).toBe("function");
    expect(() => unlisten()).not.toThrow();
    expect(fired).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Clone + dialog wrappers (B4, appended)
// ---------------------------------------------------------------------------

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

describe("ipc client clone + dialog wrappers", () => {
  it("cloneRepo sends url/destination and omits depth unless given", async () => {
    const { transport, calls } = mockTransport(() => "C:/repos/cloned");
    setTransport(transport);

    await expect(cloneRepo("https://host/x.git", "C:/repos/cloned")).resolves.toBe(
      "C:/repos/cloned",
    );
    await cloneRepo("https://host/x.git", "C:/repos/cloned", 1);

    expect(calls[0]).toMatchObject({
      command: "repo_clone",
      args: { url: "https://host/x.git", destination: "C:/repos/cloned" },
    });
    expect(calls[0]?.args.depth).toBeUndefined();
    expect(calls[1]?.args).toEqual({
      url: "https://host/x.git",
      destination: "C:/repos/cloned",
      depth: 1,
    });
  });

  it("cloneRepo normalizes failures to IpcError", async () => {
    const { transport } = mockTransport(() => {
      throw new Error("destination exists and is not empty");
    });
    setTransport(transport);

    const err = await cloneRepo("https://host/x.git", "C:/occupied").catch(
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("repo_clone");
    expect((err as IpcError).message).toBe("destination exists and is not empty");
  });

  it("onCloneProgress resolves with a no-op unlisten outside Tauri", async () => {
    const events: unknown[] = [];
    const unlisten = await onCloneProgress((event) => events.push(event));
    expect(typeof unlisten).toBe("function");
    expect(() => unlisten()).not.toThrow();
    expect(events).toEqual([]);
  });

  it("pickFolder returns null outside Tauri", async () => {
    await expect(pickFolder()).resolves.toBeNull();
  });

  it("pickFolder forwards to the dialog plugin inside Tauri and nulls on cancel", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const mockOpen = vi.mocked(open);
    // Pretend the Tauri runtime is present (isTauri just checks the flag).
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    try {
      mockOpen.mockResolvedValue("C:/repos/picked");
      await expect(pickFolder()).resolves.toBe("C:/repos/picked");
      expect(mockOpen).toHaveBeenCalledWith(
        expect.objectContaining({ directory: true, multiple: false }),
      );

      mockOpen.mockResolvedValue(null);
      await expect(pickFolder()).resolves.toBeNull();

      mockOpen.mockRejectedValue(new Error("dialog blew up"));
      await expect(pickFolder()).resolves.toBeNull();
    } finally {
      delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    }
  });
});

// ---------------------------------------------------------------------------
// M2 mutation wrappers (appended)
// ---------------------------------------------------------------------------

const STAGE_REQUEST: StageRequest = {
  targets: [{ file: "src/a.ts" }, { hunk: { path: "src/b.ts", hunk: 1 } }],
  unstage: false,
};

const COMMIT_OPTIONS: CommitOptions = {
  message: "fix: thing",
  amend: false,
  no_verify: false,
  allow_empty: false,
  author: null,
};

const SIGNING_INFO: SigningInfo = { active: true, format: "ssh", key_id: "SHA256:abc" };

const HOOKS: HookInfo[] = [
  { kind: "pre-commit", present: true, executable: true },
  { kind: "commit-msg", present: true, executable: false },
  { kind: "pre-push", present: false, executable: false },
];

const BRANCHES: BranchInfo[] = [
  { name: "main", sha: "abc123def4567890", upstream: "origin/main", ahead: 1, behind: 0, gone: false, is_head: true },
  { name: "feature/x", sha: "def456abc1237890", upstream: "origin/feature/x", ahead: 0, behind: 3, gone: true, is_head: false },
];

const REMOTES: RemoteInfo[] = [
  { name: "origin", url: "https://host/origin.git", push_url: null },
  { name: "upstream", url: "https://host/up.git", push_url: "git@host:up.git" },
];

const FETCH_OPTIONS: FetchOptions = { remote: "origin", prune: true, refs: [], depth: null };
const PULL_OPTIONS: PullOptions = { remote: "origin", branch: "main", ff_only: true, rebase: false };
const PUSH_OPTIONS: PushOptions = {
  remote: "origin",
  branch: "main",
  force: false,
  force_with_lease: false,
  set_upstream: false,
  refs: [],
  tags: false,
  delete: false,
  atomic: false,
};

const NET_STATS: NetStats = {
  received_bytes: 4_096,
  objects: 12,
  updated_refs: [["refs/heads/main", "def456"]],
};

describe("ipc client M2 mutation wrappers", () => {
  it("stage sends the request verbatim under the `request` key", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await stage("repo-1", STAGE_REQUEST);

    expect(calls).toEqual([
      { command: "stage", args: { repo_id: "repo-1", request: STAGE_REQUEST } },
    ]);
  });

  it("stageAll forwards the unstage flag", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await stageAll("repo-1", false);
    await stageAll("repo-1", true);

    expect(calls[0]).toEqual({
      command: "stage_all",
      args: { repo_id: "repo-1", unstage: false },
    });
    expect(calls[1]).toEqual({
      command: "stage_all",
      args: { repo_id: "repo-1", unstage: true },
    });
  });

  it("commit resolves with the new sha and passes options verbatim", async () => {
    const { transport, calls } = mockTransport(() => "0123456789abcdef");
    setTransport(transport);

    await expect(commit("repo-1", COMMIT_OPTIONS)).resolves.toBe("0123456789abcdef");
    expect(calls[0]).toEqual({
      command: "commit",
      args: { repo_id: "repo-1", options: COMMIT_OPTIONS },
    });
  });

  it("signingInfo and hooksList return typed payloads", async () => {
    const { transport, calls } = mockTransport((call) =>
      call.command === "signing_info" ? SIGNING_INFO : HOOKS,
    );
    setTransport(transport);

    await expect(signingInfo("repo-1")).resolves.toEqual(SIGNING_INFO);
    await expect(hooksList("repo-1")).resolves.toEqual(HOOKS);
    expect(calls.map((c) => c.command)).toEqual(["signing_info", "hooks_list"]);
    expect(calls[0]?.args).toEqual({ repo_id: "repo-1" });
    expect(calls[1]?.args).toEqual({ repo_id: "repo-1" });
  });

  it("branches returns the typed branch list", async () => {
    const { transport } = mockTransport(() => BRANCHES);
    setTransport(transport);

    await expect(branches("repo-1")).resolves.toEqual(BRANCHES);
  });

  it("branchCreate omits `from` when not given and includes it when given", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await branchCreate("repo-1", "feature/y", true);
    await branchCreate("repo-1", "feature/z", false, "abc123");

    expect(calls[0]).toEqual({
      command: "branch_create",
      args: { repo_id: "repo-1", name: "feature/y", checkout: true },
    });
    expect(calls[1]).toEqual({
      command: "branch_create",
      args: { repo_id: "repo-1", name: "feature/z", checkout: false, from: "abc123" },
    });
  });

  it("branchSwitch / branchDelete forward the force flag", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await branchSwitch("repo-1", "feature/x", false);
    await branchDelete("repo-1", "feature/x", true);

    expect(calls[0]).toEqual({
      command: "branch_switch",
      args: { repo_id: "repo-1", name: "feature/x", force: false },
    });
    expect(calls[1]).toEqual({
      command: "branch_delete",
      args: { repo_id: "repo-1", name: "feature/x", force: true },
    });
  });

  it("branchRename maps oldName/newName onto the reserved-word keys `old`/`new`", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await branchRename("repo-1", "old-name", "new-name");

    expect(calls).toEqual([
      {
        command: "branch_rename",
        args: { repo_id: "repo-1", old: "old-name", new: "new-name" },
      },
    ]);
  });

  it("branchIsMerged returns the merged flag", async () => {
    const { transport, calls } = mockTransport(() => true);
    setTransport(transport);

    await expect(branchIsMerged("repo-1", "feature/x", "main")).resolves.toBe(true);
    expect(calls[0]).toEqual({
      command: "branch_is_merged",
      args: { repo_id: "repo-1", name: "feature/x", into: "main" },
    });
  });

  it("tagCreate omits optional target/message; tagDelete sends the name", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await tagCreate("repo-1", "v1.0.0");
    await tagCreate("repo-1", "v2.0.0", "abc123", "release two");
    await tagDelete("repo-1", "v1.0.0");

    expect(calls[0]?.args).toEqual({ repo_id: "repo-1", name: "v1.0.0" });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      name: "v2.0.0",
      target: "abc123",
      message: "release two",
    });
    expect(calls[2]).toEqual({ command: "tag_delete", args: { repo_id: "repo-1", name: "v1.0.0" } });
  });

  it("remotes returns the typed remote list", async () => {
    const { transport } = mockTransport(() => REMOTES);
    setTransport(transport);

    await expect(remotes("repo-1")).resolves.toEqual(REMOTES);
  });

  it("remoteAdd / remoteRemove / remoteSetUrl send their args in snake_case", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await remoteAdd("repo-1", "origin", "https://host/o.git");
    await remoteRemove("repo-1", "upstream");
    await remoteSetUrl("repo-1", "origin", "git@host:o.git", true);
    await remoteSetUrl("repo-1", "origin", "https://host/o.git", false);

    expect(calls[0]).toEqual({
      command: "remote_add",
      args: { repo_id: "repo-1", name: "origin", url: "https://host/o.git" },
    });
    expect(calls[1]).toEqual({
      command: "remote_remove",
      args: { repo_id: "repo-1", name: "upstream" },
    });
    expect(calls[2]).toEqual({
      command: "remote_set_url",
      args: { repo_id: "repo-1", name: "origin", url: "git@host:o.git", push: true },
    });
    expect(calls[3]?.args).toMatchObject({ push: false });
  });

  it("fetchRepo / pullRepo / pushRepo send options verbatim and resolve NetStats", async () => {
    const { transport, calls } = mockTransport(() => NET_STATS);
    setTransport(transport);

    await expect(fetchRepo("repo-1", FETCH_OPTIONS)).resolves.toEqual(NET_STATS);
    await expect(pullRepo("repo-1", PULL_OPTIONS)).resolves.toEqual(NET_STATS);
    await expect(pushRepo("repo-1", PUSH_OPTIONS)).resolves.toEqual(NET_STATS);

    expect(calls.map((c) => c.command)).toEqual(["fetch", "pull", "push"]);
    expect(calls[0]?.args).toEqual({ repo_id: "repo-1", options: FETCH_OPTIONS });
    expect(calls[1]?.args).toEqual({ repo_id: "repo-1", options: PULL_OPTIONS });
    expect(calls[2]?.args).toEqual({ repo_id: "repo-1", options: PUSH_OPTIONS });
  });

  it("M2 wrapper failures normalize to IpcError", async () => {
    const { transport } = mockTransport(() => {
      throw new Error("nothing to commit");
    });
    setTransport(transport);

    const err = await commit("repo-1", COMMIT_OPTIONS).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("commit");
    expect((err as IpcError).message).toBe("nothing to commit");
  });

  it("authRespond includes op_id + store and omits empty credentials", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await authRespond("op-1", "user", "pass", true);
    await authRespond("op-2"); // cancel: no creds, store false

    expect(calls[0]).toEqual({
      command: "auth_respond",
      args: { op_id: "op-1", username: "user", password: "pass", store: true },
    });
    expect(calls[1]).toEqual({
      command: "auth_respond",
      args: { op_id: "op-2", store: false },
    });
  });

  it("onOpProgress and onAuthRequest resolve with no-op unlistens outside Tauri", async () => {
    let ops = 0;
    let auths = 0;
    const unOps = await onOpProgress(() => {
      ops += 1;
    });
    const unAuth = await onAuthRequest(() => {
      auths += 1;
    });
    expect(typeof unOps).toBe("function");
    expect(typeof unAuth).toBe("function");
    expect(() => {
      unOps();
      unAuth();
    }).not.toThrow();
    expect(ops).toBe(0);
    expect(auths).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// discard / clean checkpoint flag (the "checkpoints for git changes" setting)
// ---------------------------------------------------------------------------

describe("ipc client checkpoint-flag wrappers", () => {
  const TARGETS = [{ file: "src/a.ts" }];

  it("discard omits the checkpoint field by default (backend default = on)", async () => {
    const { transport, calls } = mockTransport(() => null);
    setTransport(transport);

    await expect(discard("repo-1", TARGETS)).resolves.toBeNull();
    expect(calls).toEqual([
      { command: "discard", args: { repo_id: "repo-1", targets: TARGETS } },
    ]);
  });

  it("discard forwards the checkpoint flag and resolves the warning payload", async () => {
    const { transport, calls } = mockTransport((call) =>
      call.args.checkpoint === false ? "committer identity not configured" : null,
    );
    setTransport(transport);

    await expect(discard("repo-1", TARGETS, { checkpoint: true })).resolves.toBeNull();
    await expect(
      discard("repo-1", TARGETS, { checkpoint: false }),
    ).resolves.toBe("committer identity not configured");
    expect(calls[0]).toEqual({
      command: "discard",
      args: { repo_id: "repo-1", targets: TARGETS, checkpoint: true },
    });
    expect(calls[1]).toEqual({
      command: "discard",
      args: { repo_id: "repo-1", targets: TARGETS, checkpoint: false },
    });
  });

  it("repoClean omits the checkpoint field by default and forwards it when given", async () => {
    const { transport, calls } = mockTransport(() => 2);
    setTransport(transport);

    await expect(repoClean("repo-1", ["junk.log"], "pre-clean")).resolves.toBe(2);
    await expect(
      repoClean("repo-1", ["junk.log"], "pre-clean", { checkpoint: false }),
    ).resolves.toBe(2);
    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      paths: ["junk.log"],
      checkpoint_reason: "pre-clean",
    });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      paths: ["junk.log"],
      checkpoint_reason: "pre-clean",
      checkpoint: false,
    });
  });
});

// ---------------------------------------------------------------------------
// M3 conflict-editor file access (E1, appended)
// ---------------------------------------------------------------------------

describe("conflict_resolve custom content (base64 wire format)", () => {
  it("encodes customContent as base64 (same wire format as repo_write_file) and omits it when absent", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await conflictResolve("repo-1", "src/a.txt", "both", new Uint8Array([1, 2, 3]));
    await conflictResolve("repo-1", "src/a.txt", "ours");

    expect(calls[0]?.command).toBe("conflict_resolve");
    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      path: "src/a.txt",
      resolution: "both",
      custom_content_b64: "AQID",
    });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      path: "src/a.txt",
      resolution: "ours",
    });
  });
});

describe("ipc client repo_read_file wrapper", () => {
  it("readFile sends repo_id + path and returns the bytes as a Uint8Array", async () => {
    // `<<<<<<< HEAD\n` as UTF-8 bytes — what the conflict editor consumes.
    const bytes = [0x3c, 0x3c, 0x3c, 0x3c, 0x3c, 0x3c, 0x3c, 0x20, 0x48, 0x0a];
    const { transport, calls } = mockTransport(() => bytes);
    setTransport(transport);

    // Production returns an ArrayBuffer (raw IPC response) — normalized to
    // Uint8Array here; test transports hand back the historical number[].
    await expect(readFile("repo-1", "src/a.ts")).resolves.toEqual(
      Uint8Array.from(bytes),
    );

    expect(calls).toEqual([
      { command: "repo_read_file", args: { repo_id: "repo-1", path: "src/a.ts" } },
    ]);
  });

  it("readFile resolves an empty array for a missing file (documented fallback)", async () => {
    const { transport } = mockTransport(() => []);
    setTransport(transport);

    await expect(readFile("repo-1", "gone.txt")).resolves.toEqual(
      Uint8Array.of(),
    );
  });

  it("readFile normalizes failures (path escape, not a file) to IpcError", async () => {
    const { transport } = mockTransport(() => {
      throw "path must not contain `..`";
    });
    setTransport(transport);

    const err = await readFile("repo-1", "../secrets").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("repo_read_file");
    expect((err as IpcError).message).toContain("..");
  });
});

describe("ipc client repo_blob wrapper", () => {
  it("readBlob sends repo_id + side + path and returns the bytes as a Uint8Array", async () => {
    // A PNG magic number — what the image-diff loader wraps into a Blob.
    const bytes = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    const { transport, calls } = mockTransport(() => bytes);
    setTransport(transport);

    await expect(
      readBlob("repo-1", { commit: "abc123" }, "assets/logo.png"),
    ).resolves.toEqual(Uint8Array.from(bytes));

    expect(calls).toEqual([
      {
        command: "repo_blob",
        args: { repo_id: "repo-1", side: { commit: "abc123" }, path: "assets/logo.png" },
      },
    ]);
  });

  it("readBlob passes plain side strings through verbatim", async () => {
    const { transport, calls } = mockTransport(() => []);
    setTransport(transport);

    await expect(readBlob("repo-1", "index", "a.png")).resolves.toEqual(
      Uint8Array.of(),
    );
    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      side: "index",
      path: "a.png",
    });
  });

  it("readBlob normalizes failures (missing path, oversized) to IpcError", async () => {
    const { transport } = mockTransport(() => {
      throw "`gone.png` not found in the working tree";
    });
    setTransport(transport);

    const err = await readBlob("repo-1", "worktree", "gone.png").catch(
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("repo_blob");
  });
});

// ---------------------------------------------------------------------------
// M6 (lane H1, appended): OS-keyring secrets wrappers
// ---------------------------------------------------------------------------

describe("ipc client secrets_* wrappers", () => {
  it("secretsGet sends the key and resolves the stored value verbatim", async () => {
    const { transport, calls } = mockTransport(() => "sk-or-v1-abc");
    setTransport(transport);

    await expect(secretsGet(SECRET_KEYS.openrouterApiKey)).resolves.toBe("sk-or-v1-abc");

    expect(calls).toEqual([
      { command: "secrets_get", args: { key: "openrouter.api-key" } },
    ]);
  });

  it("secretsGet resolves null when no entry exists", async () => {
    const { transport } = mockTransport(() => null);
    setTransport(transport);

    await expect(secretsGet("opencode.server.password")).resolves.toBeNull();
  });

  it("secretsSet sends key + value (snake_case command name)", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await expect(
      secretsSet(SECRET_KEYS.opencodeServerPassword, "hunter2"),
    ).resolves.toBeUndefined();

    expect(calls).toEqual([
      { command: "secrets_set", args: { key: "opencode.server.password", value: "hunter2" } },
    ]);
  });

  it("secretsDelete sends the key and is idempotent (no entry = ok)", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await expect(secretsDelete(SECRET_KEYS.openrouterApiKey)).resolves.toBeUndefined();

    expect(calls).toEqual([
      { command: "secrets_delete", args: { key: "openrouter.api-key" } },
    ]);
  });

  it("SECRET_KEYS pins the keyring entry names", () => {
    expect(SECRET_KEYS.openrouterApiKey).toBe("openrouter.api-key");
    expect(SECRET_KEYS.opencodeServerPassword).toBe("opencode.server.password");
  });
});

// ---------------------------------------------------------------------------
// External apps ("Open with…") — appended with the custom-app support
// ---------------------------------------------------------------------------

describe("ipc client open-with wrappers", () => {
  it("openWithCustom sends the full custom-app spec in snake_case", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await expect(
      openWithCustom("C:/repos/mygitui", {
        id: "custom:abc-123",
        name: "Neovim",
        command: "kitty {path}",
      }),
    ).resolves.toBeUndefined();

    expect(calls).toEqual([
      {
        command: "open_with_custom",
        args: {
          repo_root: "C:/repos/mygitui",
          app_id: "custom:abc-123",
          name: "Neovim",
          command: "kitty {path}",
        },
      },
    ]);
  });

  it("openWithCustom normalizes failures (binary not found) to IpcError", async () => {
    const { transport } = mockTransport(() => {
      throw "failed to launch `kitty /repos/x`: No such file or directory";
    });
    setTransport(transport);

    const err = await openWithCustom("/repos/x", {
      id: "custom:abc-123",
      name: "Neovim",
      command: "kitty {path}",
    }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(IpcError);
    expect((err as IpcError).command).toBe("open_with_custom");
    expect((err as IpcError).message).toContain("No such file or directory");
  });
});

// ---------------------------------------------------------------------------
// Phase 5 wrappers (appended): stash extras, blame query, rebase skip,
// bundle, notes, LFS locks, worktree locks, submodule add/deinit
// ---------------------------------------------------------------------------

describe("ipc client phase-5 wrappers", () => {
  it("stashPush forwards optional paths; empty/null = arg omitted", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await stashPush("repo-1", "wip", false, true, ["src/a.ts", "src/b.ts"]);
    await stashPush("repo-1", undefined, true, false, null);
    await stashPush("repo-1", undefined, false, true, []);

    expect(calls[0]).toEqual({
      command: "stash_push",
      args: {
        repo_id: "repo-1",
        message: "wip",
        keep_index: false,
        include_untracked: true,
        paths: ["src/a.ts", "src/b.ts"],
      },
    });
    // null and [] both mean "whole worktree" — the arg stays off the wire.
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      message: undefined,
      keep_index: true,
      include_untracked: false,
    });
    expect(calls[2]?.args).toEqual({
      repo_id: "repo-1",
      message: undefined,
      keep_index: false,
      include_untracked: true,
    });
  });

  it("stashShow resolves the stash's FileDiff list", async () => {
    const { transport, calls } = mockTransport(() => [FILE_DIFF]);
    setTransport(transport);

    await expect(stashShow("repo-1", 0)).resolves.toEqual([FILE_DIFF]);
    expect(calls[0]).toEqual({ command: "stash_show", args: { repo_id: "repo-1", index: 0 } });
  });

  it("stashRename sends index + message", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await stashRename("repo-1", 0, "renamed wip");
    expect(calls[0]).toEqual({
      command: "stash_rename",
      args: { repo_id: "repo-1", index: 0, message: "renamed wip" },
    });
  });

  it("stashApply forwards pop and the optional restore_index (--index) flag", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await stashApply("repo-1", 2, true);
    await stashApply("repo-1", 0, false, true);

    expect(calls[0]?.args).toEqual({ repo_id: "repo-1", index: 2, pop: true });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      index: 0,
      pop: false,
      restore_index: true,
    });
  });

  it("rebaseSkip resolves the resulting RebaseState", async () => {
    const state: RebaseState = {
      active: true,
      plan: [],
      current: 1,
      paused_for_edit: false,
      paused_for_conflict: false,
      paused_for_exec: false,
      exec_error: null,
      rewritten: [],
    };
    const { transport, calls } = mockTransport(() => state);
    setTransport(transport);

    await expect(rebaseSkip("repo-1")).resolves.toEqual(state);
    expect(calls[0]).toEqual({ command: "rebase_skip", args: { repo_id: "repo-1" } });
  });

  it("repoBundle sends out_path, refs and the all flag", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await repoBundle("repo-1", "C:/out/mygitui-2026.bundle", ["refs/heads/main"], true);
    expect(calls[0]).toEqual({
      command: "repo_bundle",
      args: {
        repo_id: "repo-1",
        out_path: "C:/out/mygitui-2026.bundle",
        refs: ["refs/heads/main"],
        all: true,
      },
    });
  });

  it("commitNotes resolves the NoteInfo list verbatim", async () => {
    const notes: NoteInfo[] = [{ ref: "refs/notes/commits", text: "reviewed by ada" }];
    const { transport, calls } = mockTransport(() => notes);
    setTransport(transport);

    await expect(commitNotes("repo-1", "abc123")).resolves.toEqual(notes);
    expect(calls[0]).toEqual({
      command: "commit_notes",
      args: { repo_id: "repo-1", sha: "abc123" },
    });
  });

  it("noteAdd / noteRemove omit note_ref when unset or empty", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await noteAdd("repo-1", "abc123", "hello note");
    await noteAdd("repo-1", "abc123", "custom", "refs/notes/reviews");
    await noteRemove("repo-1", "abc123");
    await noteRemove("repo-1", "abc123", "refs/notes/reviews");

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      sha: "abc123",
      text: "hello note",
    });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      sha: "abc123",
      text: "custom",
      note_ref: "refs/notes/reviews",
    });
    expect(calls[2]?.args).toEqual({ repo_id: "repo-1", sha: "abc123" });
    expect(calls[3]?.args).toEqual({
      repo_id: "repo-1",
      sha: "abc123",
      note_ref: "refs/notes/reviews",
    });
  });

  it("lfsLocks / lfsLock / lfsUnlock pass paths verbatim", async () => {
    const locks: LfsLock[] = [
      { id: "1", path: "assets/model.psd", owner: "ada" },
    ];
    const { transport, calls } = mockTransport((call) =>
      call.command === "lfs_locks" ? locks : "locked assets/model.psd",
    );
    setTransport(transport);

    await expect(lfsLocks("repo-1")).resolves.toEqual(locks);
    await expect(lfsLock("repo-1", "assets/model.psd")).resolves.toBe(
      "locked assets/model.psd",
    );
    await expect(lfsUnlock("repo-1", "assets/model.psd")).resolves.toBe(
      "locked assets/model.psd",
    );

    expect(calls[0]?.command).toBe("lfs_locks");
    expect(calls[1]?.args).toEqual({ repo_id: "repo-1", path: "assets/model.psd" });
    expect(calls[2]?.args).toEqual({ repo_id: "repo-1", path: "assets/model.psd" });
  });

  it("worktreeAdd forwards the optional from_remote; lock/unlock carry reason", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await worktreeAdd("repo-1", "C:/wt/x", undefined, "feature", "origin");
    await worktreeLock("repo-1", "C:/wt/x", "wip do not touch");
    await worktreeLock("repo-1", "C:/wt/x");
    await worktreeUnlock("repo-1", "C:/wt/x");

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      path: "C:/wt/x",
      branch: undefined,
      new_branch: "feature",
      from_remote: "origin",
    });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      path: "C:/wt/x",
      reason: "wip do not touch",
    });
    // Empty/omitted reason stays off the wire.
    expect(calls[2]?.args).toEqual({ repo_id: "repo-1", path: "C:/wt/x" });
    expect(calls[3]?.args).toEqual({ repo_id: "repo-1", path: "C:/wt/x" });
  });

  it("submoduleAdd carries name only when set; submoduleDeinit sends the path", async () => {
    const { transport, calls } = mockTransport(() => undefined);
    setTransport(transport);

    await submoduleAdd("repo-1", "https://host/lib.git", "vendor/lib");
    await submoduleAdd("repo-1", "https://host/lib.git", "vendor/lib", "lib");
    await submoduleDeinit("repo-1", "vendor/lib");

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      url: "https://host/lib.git",
      path: "vendor/lib",
    });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      url: "https://host/lib.git",
      path: "vendor/lib",
      name: "lib",
    });
    expect(calls[2]).toEqual({
      command: "submodule_deinit",
      args: { repo_id: "repo-1", path: "vendor/lib" },
    });
  });
});
