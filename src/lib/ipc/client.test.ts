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
  fetchRepo,
  hooksList,
  IpcError,
  onAuthRequest,
  onCloneProgress,
  onOpProgress,
  onRepoChanged,
  openRepo,
  pickFolder,
  pullRepo,
  pushRepo,
  readFile,
  remotes,
  remoteAdd,
  remoteRemove,
  remoteSetUrl,
  repoBlame,
  repoDiff,
  repoRefs,
  repoStatus,
  resetChannelFactory,
  resetTransport,
  setChannelFactory,
  setTransport,
  SECRET_KEYS,
  secretsDelete,
  secretsGet,
  secretsSet,
  signingInfo,
  stage,
  stageAll,
  streamDiff,
  streamFileHistory,
  streamLog,
  tagCreate,
  tagDelete,
  type ChannelLike,
  type Transport,
} from "$lib/ipc/client";
import type {
  BlameLine,
  BranchInfo,
  CommitOptions,
  FetchOptions,
  FileDiff,
  HookInfo,
  LogPage,
  NetStats,
  PullOptions,
  PushOptions,
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
};

const REPO_STATUS: RepoStatus = {
  branch: "main",
  head: "abc123def456",
  detached: false,
  ahead: 2,
  behind: 1,
  merging: false,
  rebasing: false,
  sequencer: false,
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
      message: "Initial commit\n",
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

  it("repoDiff passes sides verbatim and omits optional paths", async () => {
    const { transport, calls } = mockTransport(() => [FILE_DIFF]);
    setTransport(transport);

    await repoDiff("repo-1", "head", "worktree");
    await repoDiff("repo-1", { commit: "abc123" }, "index", ["src/a.ts"]);

    expect(calls[0]?.args).toEqual({
      repo_id: "repo-1",
      old: "head",
      new: "worktree",
    });
    expect(calls[1]?.args).toEqual({
      repo_id: "repo-1",
      old: { commit: "abc123" },
      new: "index",
      paths: ["src/a.ts"],
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

  it("streamDiff delivers file pages and forwards optional paths", async () => {
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
      paths: ["src/a.ts"],
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
// M3 conflict-editor file access (E1, appended)
// ---------------------------------------------------------------------------

describe("ipc client repo_read_file wrapper", () => {
  it("readFile sends repo_id + path and returns byte arrays verbatim", async () => {
    // `<<<<<<< HEAD\n` as UTF-8 bytes — what the conflict editor consumes.
    const bytes = [0x3c, 0x3c, 0x3c, 0x3c, 0x3c, 0x3c, 0x3c, 0x20, 0x48, 0x0a];
    const { transport, calls } = mockTransport(() => bytes);
    setTransport(transport);

    await expect(readFile("repo-1", "src/a.ts")).resolves.toEqual(bytes);

    expect(calls).toEqual([
      { command: "repo_read_file", args: { repo_id: "repo-1", path: "src/a.ts" } },
    ]);
  });

  it("readFile resolves an empty array for a missing file (documented fallback)", async () => {
    const { transport } = mockTransport(() => []);
    setTransport(transport);

    await expect(readFile("repo-1", "gone.txt")).resolves.toEqual([]);
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
