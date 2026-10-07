/**
 * Unit tests for the typed IPC client (`$lib/ipc/client`). The Tauri runtime
 * is never present in jsdom, so every test here exercises either an injected
 * mock transport or the documented non-Tauri fallbacks.
 */

import { afterEach, describe, expect, it } from "vitest";
import {
  closeRepo,
  IpcError,
  onRepoChanged,
  openRepo,
  repoBlame,
  repoDiff,
  repoRefs,
  repoStatus,
  resetChannelFactory,
  resetTransport,
  setChannelFactory,
  setTransport,
  streamDiff,
  streamFileHistory,
  streamLog,
  type ChannelLike,
  type Transport,
} from "$lib/ipc/client";
import type {
  BlameLine,
  FileDiff,
  LogPage,
  RepoInfo,
  RepoStatus,
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
  return call.args.channel as ChannelLike<T>;
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
    expect(typeof (calls[0]?.args.channel as ChannelLike<LogPage>).onmessage).toBe(
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
