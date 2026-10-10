/**
 * Playwright Tauri mock (M12) — installs a minimal `window.__TAURI_INTERNALS__`
 * BEFORE app code (via `page.addInitScript`) so the SPA boots in a plain
 * Chromium exactly as it does inside a Tauri webview.
 *
 * Shape (mirrors what `@tauri-apps/api` v2 actually touches):
 *   - `invoke(cmd, args, options)` — every command flows through here
 *     (`@tauri-apps/api/core`). Resolves from a per-command mock map
 *     (snake_case payloads mirroring `src/lib/ipc/types.ts`), a generic
 *     fallback (`[]` for known list commands, `null` otherwise), and
 *     records every call in `window.__TAURI_MOCK__.calls` for assertions.
 *   - `transformCallback(callback, once)` — identity-with-registry: stores
 *     the callback and returns an id (used by `Channel` and event handlers).
 *   - `unregisterCallback(id)` — channel cleanup.
 *   - `metadata.currentWindow/.currentWebview` labels — needed by
 *     `getCurrentWebview()` (drag-drop listener) at boot.
 *   - `convertFileSrc(path)` — passthrough.
 *   - `window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener` — the
 *     event plugin's unlisten path.
 *   - Event shims: `plugin:event|listen` / `unlisten` resolve with ids and
 *     register the (transformCallback-registered) handler; tests can push
 *     events with `window.__TAURI_MOCK__.emit(event, payload)`.
 *
 * Streaming commands (`*_stream` taking a `Channel`) deliver one empty page
 * through the channel (`{ index, message }`) and resolve.
 */

import type { Page, TestInfo } from "@playwright/test";

/** Resolve value for a command; `{ __error__: "msg" }` rejects. */
export type CommandMock = unknown;

/** Per-command overrides merged over the built-in defaults. */
export interface TauriMockOptions {
  commands?: Record<string, CommandMock>;
}

/**
 * Installs the mock for every frame of `page` before any app script runs.
 * Returns a helper bound to this page for reading recorded invokes and
 * emitting backend events from the test.
 */
export async function installTauriMock(
  page: Page,
  options: TauriMockOptions = {},
): Promise<TauriMockHandle> {
  await page.addInitScript(BOOTSTRAP, options ?? {});
  return {
    async calls(): Promise<Array<{ cmd: string; args: Record<string, unknown> }>> {
      return page.evaluate(() => window.__TAURI_MOCK__.calls);
    },
    async callsOf(command: string): Promise<Array<Record<string, unknown>>> {
      return page.evaluate(
        (cmd) =>
          window.__TAURI_MOCK__.calls
            .filter((c) => c.cmd === cmd)
            .map((c) => c.args),
        command,
      );
    },
    async emit(event: string, payload: unknown): Promise<void> {
      await page.evaluate(
        ({ event: name, payload: data }) => {
          window.__TAURI_MOCK__.emit(name, data);
        },
        { event, payload },
      );
    },
  };
}

/** Test-side handle over the in-page mock (post-boot). */
export interface TauriMockHandle {
  /** Every recorded invoke, in order. */
  calls(): Promise<Array<{ cmd: string; args: Record<string, unknown> }>>;
  /** Recorded args for one command, in order. */
  callsOf(command: string): Promise<Array<Record<string, unknown>>>;
  /** Delivers a backend event to registered listeners. */
  emit(event: string, payload: unknown): Promise<void>;
}

/** Collects console errors + page errors; use with `expect(errors).toEqual([])`. */
export function trackErrors(page: Page): {
  errors: string[];
  attach: (testInfo: TestInfo) => Promise<void>;
} {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    // Resource-load noise (missing favicon) is not an app error.
    const url = message.location()?.url ?? "";
    if (url.includes("favicon")) return;
    errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(String(error)));
  return {
    errors,
    async attach(testInfo: TestInfo): Promise<void> {
      if (errors.length > 0) {
        await testInfo.attach("console-errors.txt", {
          body: errors.join("\n"),
          contentType: "text/plain",
        });
      }
    },
  };
}

// -- shared fixtures (repo-open + history flows) ----------------------------

/** localStorage key of the recent-repositories list. */
export const RECENT_REPOS_KEY = "mygitui.recent-repos";

/** Seeds localStorage entries before any app script runs (addInitScript). */
export async function seedStorage(
  page: Page,
  entries: Record<string, string>,
): Promise<void> {
  await page.addInitScript((seed) => {
    for (const [key, value] of Object.entries(seed)) {
      window.localStorage.setItem(key, value);
    }
  }, entries);
}

/** localStorage seed listing `paths` as unpinned recent repos (in order). */
export function recentRepos(paths: string[]): Record<string, string> {
  return {
    [RECENT_REPOS_KEY]: JSON.stringify(
      paths.map((path, i) => ({ path, pinned: false, lastOpened: i + 1 })),
    ),
  };
}

/** A newest-first linear log of `count` commits + single-lane graph rows. */
export function linearLog(count: number): {
  commits: Array<Record<string, unknown>>;
  rows: Array<Record<string, unknown>>;
} {
  const commits = Array.from({ length: count }, (_, i) => ({
    sha: i.toString(16).padStart(40, "0"),
    parents: i > 0 ? [(i - 1).toString(16).padStart(40, "0")] : [],
    author: {
      name: `Author ${i}`,
      email: `a${i}@test.com`,
      time: 1700000000 + i,
      offset_minutes: 0,
    },
    committer: {
      name: `Author ${i}`,
      email: `a${i}@test.com`,
      time: 1700000000 + i,
      offset_minutes: 0,
    },
    message: `commit message ${i}\n\nFull description here`,
    summary: `commit message ${i}`,
    refs: i === 0 ? ["HEAD -> main"] : [],
  }));
  const rows = commits.map((c, i) => ({
    sha: c.sha,
    lane: 0,
    edges: i < count - 1 ? [{ from: 0, to: 0 }] : [],
    lane_count: 1,
  }));
  return { commits, rows };
}

/** Standard command overrides for a repo-open + history flow. */
export function historyCommands(opts: {
  commits: Array<Record<string, unknown>>;
  rows: Array<Record<string, unknown>>;
  /** `repo_diff` reply (default: no changed files). */
  diff?: CommandMock;
  describe?: string;
  /** `commit_signature` reply (default: valid). */
  signature?: CommandMock;
}): Record<string, CommandMock> {
  return {
    repo_log_stream: {
      commits: opts.commits,
      rows: opts.rows,
      next_cursor: null,
      generation: 1,
    },
    repo_diff: opts.diff ?? [],
    describe: opts.describe ?? "v1.0.0",
    commit_signature: opts.signature ?? { state: "valid" },
  };
}

/** Boots the home screen and opens the recent repo at `path`. */
export async function openRecentRepo(page: Page, path = "/tmp/repo"): Promise<void> {
  await page.goto("/");
  const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const base = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
  // The card's accessible name is "<folder name> <path>".
  const card = page.getByRole("button", {
    name: new RegExp(`${escape(base)}\\s*${escape(path)}`),
  });
  await card.click();
}

/** The init-script body — runs in the page BEFORE the app bundle. */
function BOOTSTRAP(options: TauriMockOptions): void {
  const commandMocks: Record<string, CommandMock> = {
    // -- core commands (payloads mirror src/lib/ipc/types.ts, snake_case) --
    repo_open: {
      repo_id: "r1",
      root: "/tmp/repo",
      name: "repo",
      bare: false,
      git_dir: "/tmp/repo/.git",
    },
    repo_status: {
      branch: "main",
      head: "0123456789abcdef0123456789abcdef01234567",
      detached: false,
      ahead: 0,
      behind: 0,
      merging: false,
      rebasing: false,
      sequencer: false,
      entries: [
        { path: "src/app.ts", old_path: null, index: "unmodified", worktree: "modified" },
        { path: "notes.md", old_path: null, index: "unmodified", worktree: "untracked" },
      ],
    },
    repo_refs: [],
    cli_args_initial: [],
    stage: null,
    commit: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    signing_info: { active: false, format: "openpgp", key_id: null },
    hooks_list: [],
    bisect_state: {
      active: false,
      bad: "",
      good: null,
      current: null,
      remaining: 0,
      skipped: [],
      orig_head: "",
      orig_branch: null,
      log: [],
    },
    repo_health: {
      git_size_bytes: 0,
      worktree_size_bytes: 0,
      loose_objects: 0,
      packed_objects: null,
      pack_files: 0,
      has_commit_graph: false,
      commit_graph_bytes: 0,
      packed_refs: false,
      last_gc: null,
      fsck_dangling: null,
      fsck_samples: [],
    },
    sparse_info: { enabled: false, cone: true, patterns: [] },
    lfs_status: { installed: false, version: null, tracked_patterns: [] },
    ...options.commands,
  };

  /** Commands whose generic fallback is an empty array. */
  const LIST_COMMANDS = new Set([
    "branches",
    "remotes",
    "tag_list",
    "remote_branches",
    "worktrees",
    "reflog",
    "checkpoints",
    "stash_list",
    "submodules",
    "conflicts",
    "pr_list",
    "pr_checks",
    "commit_activity",
    "contributor_stats",
    "bisect_log",
    "branch_trash_list",
  ]);

  const EMPTY_LOG_PAGE = { commits: [], rows: [], next_cursor: null, generation: 0 };

  const callbacks = new Map<number, (raw: unknown) => void>();
  const listeners = new Map<number, { event: string; handler: number }>();
  let nextCallbackId = 1;
  let nextEventId = 1;
  const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];

  /** Pushes one message through a Channel arg (instance or serialized id).
   *  Delivered on a MICROTASK (never synchronously inside `invoke`) — the
   *  real backend delivers channel pages from its event loop, and running
   *  app effects rely on pages arriving outside the calling effect's stack. */
  function deliverChannel(channel: unknown, message: unknown): void {
    let id: number | null = null;
    if (typeof channel === "string") {
      const match = /^__CHANNEL__:(\d+)$/.exec(channel);
      id = match === null ? null : Number(match[1]);
    } else if (channel && typeof channel === "object" && "id" in channel) {
      id = Number((channel as { id: unknown }).id);
    }
    const callback = id === null ? undefined : callbacks.get(id);
    if (callback) queueMicrotask(() => callback({ index: 0, message }));
  }

  const internals = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main" },
    },
    transformCallback(callback: (raw: unknown) => void, _once?: boolean): number {
      const id = nextCallbackId++;
      callbacks.set(id, callback);
      return id;
    },
    unregisterCallback(id: number): void {
      callbacks.delete(id);
    },
    convertFileSrc(path: string, _protocol?: string): string {
      return path;
    },
    invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
      calls.push({ cmd, args: args ?? {} });

      // Event plugin shims (what @tauri-apps/api/event listen/unlisten call).
      if (cmd === "plugin:event|listen") {
        const event = String((args as { event?: string })?.event ?? "");
        const handler = Number((args as { handler?: number })?.handler ?? 0);
        const eventId = nextEventId++;
        listeners.set(eventId, { event, handler });
        return Promise.resolve(eventId);
      }
      if (cmd === "plugin:event|unlisten") {
        const eventId = Number((args as { eventId?: number })?.eventId ?? 0);
        listeners.delete(eventId);
        return Promise.resolve(null);
      }

      // Streaming: deliver one page through the channel, then resolve.
      if (cmd === "repo_log_stream" || cmd === "repo_file_history") {
        const page = (cmd in commandMocks ? commandMocks[cmd] : EMPTY_LOG_PAGE);
        deliverChannel((args as { on_page?: unknown })?.on_page, page);
        return Promise.resolve(null);
      }
      if (cmd === "repo_diff_stream") {
        deliverChannel((args as { on_page?: unknown })?.on_page, []);
        return Promise.resolve(null);
      }

      if (cmd in commandMocks) {
        const value = commandMocks[cmd];
        if (
          value !== null &&
          typeof value === "object" &&
          "__error__" in (value as Record<string, unknown>)
        ) {
          return Promise.reject(
            new Error(String((value as { __error__: unknown }).__error__)),
          );
        }
        return Promise.resolve(value === undefined ? null : value);
      }
      if (LIST_COMMANDS.has(cmd)) return Promise.resolve([]);
      // Generic fallback: null (callers treat it as "nothing").
      return Promise.resolve(null);
    },
  };

  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = internals;
  (window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(): void {
      /* best-effort no-op */
    },
  };
  (window as unknown as Record<string, unknown>).__TAURI_MOCK__ = {
    calls,
    emit(event: string, payload: unknown): void {
      for (const { event: name, handler } of [...listeners.values()]) {
        if (name !== event) continue;
        const callback = callbacks.get(handler);
        if (callback) callback({ event: name, id: 0, payload });
      }
    },
  };
}
