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
 *   - `plugin:app|version` — what `getVersion()` (@tauri-apps/api/app)
 *     invokes; mocked with the real package.json version so the About
 *     surfaces show the shipping version in e2e runs too.
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
 *   - `window.__TAURI_MOCK__.commands` — the live command-mock map; the
 *     handle's `setCommand` replaces entries mid-test (init-script options
 *     are serialized at install, so Node-side mutations never reach it).
 *
 * Streaming commands (`*_stream` taking a `Channel`) deliver one empty page
 * through the channel (`{ index, message }`) and resolve. A `{ pages: [...] }`
 * override streams a multi-page walk; `resolveBeforeLastPage: true` resolves
 * the command before the last page lands (Tauri's large-payload fetch path);
 * `pageGapMs: N` spaces the pages N ms apart (and delays the first page by N,
 * default 0) so tests can land `emit`-ed events in the middle of a walk.
 * Channel indices increment per channel — the real `Channel` class queues
 * out-of-order messages forever, so a constant index strands every page
 * after the first.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { Page, TestInfo } from "@playwright/test";
import type { ForgeError } from "../src/lib/ipc/gen/ForgeError";
import type { IssueComment } from "../src/lib/ipc/gen/IssueComment";
import type { IssueSummary } from "../src/lib/ipc/gen/IssueSummary";
import type {
  IssueBackendSnapshot,
  IssuesCommand,
  IssuesMockOptions,
  NewMockComment,
  NewMockIssue,
} from "./issues-fixtures";

declare global {
  interface Window {
    __TAURI_MOCK__: {
      calls: Array<{ cmd: string; args: Record<string, unknown> }>;
      commands: Record<string, CommandMock>;
      emit(event: string, payload: unknown): void;
    };
  }
}

/** The shipping version, read once from package.json (never drifts). */
const APP_VERSION: string = (
  JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")) as
    { version?: string }
).version ?? "0.0.0";

/** Resolve value for a command; `{ __error__: "msg" }` rejects. */
export type CommandMock = unknown;

/** Per-command overrides merged over the built-in defaults. */
export interface TauriMockOptions {
  commands?: Record<string, CommandMock>;
  /** `plugin:app|version` reply (default: the real package.json version). */
  version?: string;
  /**
   * Optional stateful Issues backend. With no fixture, every `issues_*`
   * command still returns its correct empty wire shape instead of `null`/`[]`.
   */
  issues?: IssuesMockOptions;
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
  // The version default is applied here (Node side, reads package.json);
  // explicit per-test overrides win.
  await page.addInitScript(BOOTSTRAP, { version: APP_VERSION, ...options });
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
    async setCommand(command: string, value: CommandMock): Promise<void> {
      await page.evaluate(
        ({ command: cmd, value: v }) => {
          window.__TAURI_MOCK__.commands[cmd] = v;
        },
        { command, value },
      );
    },
    addIssue(input: NewMockIssue): Promise<IssueSummary> {
      return issueControl<NewMockIssue, IssueSummary>(page, "addIssue", input);
    },
    addComment(input: NewMockComment): Promise<IssueComment> {
      return issueControl<NewMockComment, IssueComment>(page, "addComment", input);
    },
    addPullRequestComment(input: NewMockComment): Promise<IssueComment> {
      return issueControl<NewMockComment, IssueComment>(
        page,
        "addPullRequestComment",
        input,
      );
    },
    editComment(nodeId: string, body: string): Promise<IssueComment> {
      return issueControl<{ nodeId: string; body: string }, IssueComment>(
        page,
        "editComment",
        { nodeId, body },
      );
    },
    setIssueFailure(command: IssuesCommand, error: ForgeError | null): Promise<void> {
      return issueControl<{ command: IssuesCommand; error: ForgeError | null }, void>(
        page,
        "setIssueFailure",
        { command, error },
      );
    },
    setIssueLatency(latencyMs: number | null): Promise<void> {
      return issueControl<number | null, void>(page, "setIssueLatency", latencyMs);
    },
    issueSnapshot(): Promise<IssueBackendSnapshot> {
      return issueControl<undefined, IssueBackendSnapshot>(page, "snapshot", undefined);
    },
    resetIssueState(): Promise<void> {
      return issueControl<undefined, void>(page, "reset", undefined);
    },
    clearCalls(): Promise<void> {
      return page.evaluate(() => {
        window.__TAURI_MOCK__.calls.length = 0;
      });
    },
  };
}

async function issueControl<A, R>(
  page: Page,
  method: string,
  argument: A,
): Promise<R> {
  return page.evaluate(
    ({ method: name, argument: arg }) => {
      const mock = window.__TAURI_MOCK__ as unknown as {
        issues?: Record<string, (value?: unknown) => unknown>;
      };
      const control = mock.issues;
      const run = control?.[name];
      if (!run) throw new Error(`Issues mock control is unavailable: ${name}`);
      return run(arg) as R;
    },
    { method, argument },
  );
}

/** Test-side handle over the in-page mock (post-boot). */
export interface TauriMockHandle {
  /** Every recorded invoke, in order. */
  calls(): Promise<Array<{ cmd: string; args: Record<string, unknown> }>>;
  /** Recorded args for one command, in order. */
  callsOf(command: string): Promise<Array<Record<string, unknown>>>;
  /** Delivers a backend event to registered listeners. */
  emit(event: string, payload: unknown): Promise<void>;
  /**
   * Replaces one command's mock value IN THE PAGE. Init-script options are
   * serialized at install time, so mutating the Node-side object does
   * nothing — tests that change what a command returns mid-run (a restarted
   * log stream serving a different page) go through here.
   */
  setCommand(command: string, value: CommandMock): Promise<void>;
  /** Adds a server-side issue after the activity baseline has been taken. */
  addIssue(input: NewMockIssue): Promise<IssueSummary>;
  /** Adds a server-side issue comment after the activity baseline. */
  addComment(input: NewMockComment): Promise<IssueComment>;
  /** Adds a PR comment; issue commands/activity must never expose it. */
  addPullRequestComment(input: NewMockComment): Promise<IssueComment>;
  /** Changes updated_at without changing created_at (must not notify). */
  editComment(nodeId: string, body: string): Promise<IssueComment>;
  /** Overrides one Issues command with a structured ForgeError. */
  setIssueFailure(command: IssuesCommand, error: ForgeError | null): Promise<void>;
  setIssueLatency(latencyMs: number | null): Promise<void>;
  issueSnapshot(): Promise<IssueBackendSnapshot>;
  resetIssueState(): Promise<void>;
  clearCalls(): Promise<void>;
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

/** localStorage key of the last open-tab session (launch restore). */
export const SESSION_KEY = "mygitui.session";

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
  /** `repo_diff` reply; also the single `repo_diff_stream` page the commit
   * detail streams (default: no changed files). */
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
    // CommitDetail streams its diff (a plain array mock = one page).
    repo_diff_stream: opts.diff ?? [],
    describe: opts.describe ?? "v1.0.0",
    commit_signature: opts.signature ?? { state: "valid" },
  };
}

/** Boots the home screen and opens the recent repo at `path`. */
export async function openRecentRepo(page: Page, path = "/tmp/repo"): Promise<void> {
  await page.goto("/");
  const base = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
  // The Overview row's explicit "Open <name>" action opens the tab (row
  // clicks expand the inline diffs). Wait for the scan to land first.
  const open = page.getByRole("button", { name: `Open ${base}` });
  await open.waitFor({ state: "visible" });
  await open.click();
  await page
    .locator(".overview")
    .waitFor({ state: "detached" })
    .catch(() => {});
}

/**
 * Activates a panel from the activity bar. Pinned panels (Status, Branches,
 * Remotes, Terminal) are clicked directly on the rail; panels in the
 * "More panels" overflow menu (Actions, GitHub, Stats, Repo, etc.) are
 * opened via the menu.
 */
export async function openPanel(page: Page, name: string): Promise<void> {
  const railBtn = page.locator(".activity-bar").getByRole("button", { name, exact: true });
  if (await railBtn.isVisible()) {
    await railBtn.click();
    return;
  }
  const moreBtn = page.locator(".activity-bar").getByRole("button", { name: "More panels" });
  await moreBtn.click();
  await page.getByRole("menuitem", { name: new RegExp(`^${name}`) }).click();
}

/** The init-script body — runs in the page BEFORE the app bundle. */
function BOOTSTRAP(options: TauriMockOptions): void {
  // The real backend mints a fresh repo_id per process launch (see
  // `repo::RepoManager::next_repo_id` — timestamp + pid + counter). A
  // relaunch therefore presents a DIFFERENT id for the same repo. Tests
  // model that by setting `e2e-repo-id` in sessionStorage (survives reload
  // within one browser context) before the reload.
  let seededRepoId: string | null = null;
  try {
    seededRepoId = sessionStorage.getItem("e2e-repo-id");
  } catch {
    seededRepoId = null;
  }
  const commandMocks: Record<string, CommandMock> = {
    // -- core commands (payloads mirror src/lib/ipc/types.ts, snake_case) --
    // App version (getVersion() invokes `plugin:app|version`; item 65).
    "plugin:app|version": options.version,
    repo_open: {
      repo_id: seededRepoId ?? "r1",
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
    // The Overview's per-repo scan (repo_summary): synthesized per path so
    // seeded recents render as rows. Path conventions the specs rely on:
    //   *missing* → the scan fails (dimmed row + Remove action)
    //   *ahead*   → ahead: 2, behind: 0 (Needs push filter)
    //   *behind*  → behind: 3 (Needs pull filter)
    //   *dirty*   → staged + unstaged + untracked files
    //   *clean*   → empty working copy
    // repo_summary is overridden per test via the invoke patch pattern
    // (see recent_repos.spec.ts / overview.spec.ts).
    repo_summary: null,
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
    "issue_list",
    "notifications_list",
    "workflow_runs",
    "workflow_list",
    "project_detect",
    "commit_activity",
    "contributor_stats",
    "bisect_log",
    "branch_trash_list",
    "device_list",
    "device_scan_repos",
  ]);

  const EMPTY_LOG_PAGE = { commits: [], rows: [], next_cursor: null, generation: 0 };

  /** Preserve structured ForgeError objects; keep legacy string failures. */
  function mockRejection(value: unknown): unknown {
    const error =
      value !== null && typeof value === "object" && "__error__" in value
        ? (value as { __error__: unknown }).__error__
        : value;
    return typeof error === "string" || error === null || error === undefined
      ? new Error(String(error))
      : error;
  }

  /**
   * Synthesizes one `repo_summary` reply for `path` (the Overview's scan
   * tier). The path segments steer the shape (see the repo_summary note in
   * the command map above) so specs can seed recents that need push, are
   * dirty, or have vanished.
   */
  function fakeRepoSummary(path: string): CommandMock {
    const lower = path.toLowerCase();
    if (lower.includes("missing")) {
      return { __error__: "repository not found" };
    }
    const name = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
    const dirty = !lower.includes("clean");
    const entries: Array<Record<string, unknown>> = [];
    if (lower.includes("dirty")) {
      entries.push(
        { path: "src/staged.ts", old_path: null, index: "modified", worktree: "unmodified" },
        { path: "dirty.ts", old_path: null, index: "unmodified", worktree: "modified" },
        { path: "notes/new.md", old_path: null, index: "unmodified", worktree: "untracked" },
      );
    } else if (dirty) {
      entries.push(
        { path: "src/app.ts", old_path: null, index: "unmodified", worktree: "modified" },
        { path: "notes.md", old_path: null, index: "unmodified", worktree: "untracked" },
      );
    }
    const ahead = lower.includes("ahead") ? 2 : 0;
    const behind = lower.includes("behind") ? 3 : 0;
    return {
      root: path,
      name,
      branch: lower.includes("detached") ? null : "main",
      detached: lower.includes("detached"),
      ahead,
      behind,
      conflicts: 0,
      staged: entries.filter((e) => e.index !== "unmodified").length,
      unstaged: entries.filter(
        (e) => e.worktree !== "unmodified" && e.worktree !== "untracked",
      ).length,
      untracked: entries.filter((e) => e.worktree === "untracked").length,
      entries,
      last_commit: {
        sha: "0123456789abcdef0123456789abcdef01234567",
        summary: `last commit of ${name}`,
        timestamp: 1_700_000_000,
      },
    };
  }

  /**
   * Keeps `repo_status` in sync with `stage` / `stage_all` so the UI's
   * staged-change gating behaves like the real backend (the smoke flow
   * stages a file and then commits it).
   */
  const statusMock = commandMocks.repo_status as
    | { entries?: Array<Record<string, unknown>> }
    | null
    | undefined;

  function targetPath(target: unknown): string | null {
    if (!target || typeof target !== "object") return null;
    const t = target as {
      file?: unknown;
      hunk?: { path?: unknown };
      lines?: { path?: unknown };
    };
    if (typeof t.file === "string") return t.file;
    if (typeof t.hunk?.path === "string") return t.hunk.path;
    if (typeof t.lines?.path === "string") return t.lines.path;
    return null;
  }

  // -- workspace (file browser) fake FS --------------------------------------
  // Keys are repo-relative paths (forward slashes) → file bytes. Dirs are
  // derived from the file keys, so create/rename/delete flows stay in sync.

  const WORKSPACE_COMMANDS = new Set([
    "repo_list_dir",
    "repo_stat_file",
    "repo_read_file",
    "repo_write_file",
    "repo_create_entry",
    "repo_rename_entry",
    "repo_delete_entry",
    "repo_file_index",
  ]);

  const fakeFiles = new Map<string, number[]>([
    ["README.md", Array.from(new TextEncoder().encode("hello world\n"))],
    ["src/app.ts", Array.from(new TextEncoder().encode("export const app = 1;\n"))],
    ["notes.md", Array.from(new TextEncoder().encode("# notes\n"))],
  ]);
  const fakeDirs = new Set<string>(["src"]);
  const FAKE_MTIME = 1_700_000_000_000;

  function fakeBasename(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx === -1 ? path : path.slice(idx + 1);
  }

  function fakeKind(path: string): string {
    return /\.(png|jpg|jpeg|gif|webp|bmp|svg|ico|avif)$/i.test(path)
      ? "image"
      : "text";
  }

  function fakeList(dir: string): Array<Record<string, unknown>> {
    const names = new Map<string, string>(); // name → kind
    if (dir === "") {
      for (const d of fakeDirs) names.set(d, "dir");
    }
    for (const path of fakeFiles.keys()) {
      if (dir === "") {
        if (!path.includes("/")) names.set(path, "file");
      } else if (path.startsWith(`${dir}/`)) {
        const rest = path.slice(dir.length + 1);
        if (!rest.includes("/")) names.set(rest, "file");
      }
    }
    const entries = [...names.entries()].map(([name, kind]) => {
      const path = dir === "" ? name : `${dir}/${name}`;
      return {
        name,
        path,
        kind,
        size: kind === "file" ? (fakeFiles.get(path)?.length ?? 0) : null,
      };
    });
    // Dirs first, then case-insensitive name order (mirrors the backend).
    entries.sort((a, b) => {
      const da = a.kind === "dir" ? 0 : 1;
      const db = b.kind === "dir" ? 0 : 1;
      if (da !== db) return da - db;
      return String(a.name).localeCompare(String(b.name), undefined, {
        sensitivity: "base",
      });
    });
    return entries;
  }

  /** Handle one workspace command; `undefined` = not a workspace command. */
  function applyWorkspace(
    cmd: string,
    args: Record<string, unknown>,
  ): unknown {
    const path = String(args.path ?? "");
    switch (cmd) {
      case "repo_list_dir":
        if (path !== "" && !fakeDirs.has(path)) {
          return { __error__: `\`${path}\` is not a directory` };
        }
        return {
          path,
          entries: fakeList(path),
          truncated: false,
        };
      case "repo_stat_file": {
        const bytes = fakeFiles.get(path);
        if (!bytes) return { __error__: `\`${path}\`: not found` };
        return {
          path,
          size: bytes.length,
          kind: fakeKind(path),
          readonly: false,
          mtime_ms: FAKE_MTIME,
        };
      }
      case "repo_read_file":
        return fakeFiles.get(path) ?? [];
      case "repo_write_file": {
        if (!fakeFiles.has(path) && !fakeDirs.has(path)) {
          const dir = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
          if (dir !== "") fakeDirs.add(dir);
        }
        const content = Array.isArray(args.content)
          ? (args.content as number[])
          : [];
        fakeFiles.set(path, content);
        return {
          path,
          size: content.length,
          kind: fakeKind(path),
          readonly: false,
          mtime_ms: FAKE_MTIME + 1,
        };
      }
      case "repo_create_entry": {
        if (args.kind === "dir") {
          fakeDirs.add(path);
        } else {
          fakeFiles.set(path, []);
        }
        return null;
      }
      case "repo_rename_entry": {
        const to = String(args.new_path ?? "");
        if (fakeFiles.has(path)) {
          fakeFiles.set(to, fakeFiles.get(path)!);
          fakeFiles.delete(path);
        } else if (fakeDirs.has(path)) {
          fakeDirs.delete(path);
          fakeDirs.add(to);
          for (const [key, value] of [...fakeFiles.entries()]) {
            if (key.startsWith(`${path}/`)) {
              fakeFiles.set(`${to}${key.slice(path.length)}`, value);
              fakeFiles.delete(key);
            }
          }
        }
        return null;
      }
      case "repo_delete_entry": {
        fakeFiles.delete(path);
        fakeDirs.delete(path);
        for (const key of [...fakeFiles.keys()]) {
          if (key.startsWith(`${path}/`)) fakeFiles.delete(key);
        }
        return { id: "ckpt-1" };
      }
      case "repo_file_index": {
        const entries: Array<Record<string, unknown>> = [];
        for (const d of fakeDirs) {
          if (d !== "") entries.push({ path: d, is_dir: true, size: null });
        }
        for (const f of fakeFiles.keys()) {
          entries.push({ path: f, is_dir: false, size: null });
        }
        return { entries, truncated: false };
      }
      default:
        return undefined;
    }
  }

  function applyStage(args: Record<string, unknown> | undefined): void {
    const entries = statusMock?.entries;
    if (!Array.isArray(entries)) return;
    const request = args?.request as
      | { unstage?: boolean; targets?: unknown[] }
      | undefined;
    const targets = Array.isArray(request?.targets) ? request.targets : [];
    const paths = new Set(targets.map(targetPath).filter((p): p is string => p !== null));
    const unstage = request?.unstage === true;
    for (const entry of entries) {
      const path = entry.path;
      if (typeof path !== "string" || !paths.has(path)) continue;
      const index = typeof entry.index === "string" ? entry.index : "unmodified";
      const worktree =
        typeof entry.worktree === "string" ? entry.worktree : "unmodified";
      if (unstage) {
        // Put the index state back into the working tree.
        entry.index = "unmodified";
        entry.worktree =
          index === "unmodified"
            ? worktree
            : index === "added"
              ? "untracked"
              : index;
      } else {
        // Promote the working-tree state into the index.
        entry.index = worktree === "untracked" ? "added" : worktree;
        entry.worktree = "unmodified";
      }
    }
  }

  function applyStageAll(args: Record<string, unknown> | undefined): void {
    const entries = statusMock?.entries;
    if (!Array.isArray(entries)) return;
    const unstage = args?.unstage === true;
    for (const entry of entries) {
      const index = typeof entry.index === "string" ? entry.index : "unmodified";
      const worktree =
        typeof entry.worktree === "string" ? entry.worktree : "unmodified";
      if (unstage) {
        if (index === "unmodified") continue;
        entry.index = "unmodified";
        entry.worktree =
          index === "added" ? "untracked" : index;
      } else {
        if (worktree === "unmodified") continue;
        entry.index = worktree === "untracked" ? "added" : worktree;
        entry.worktree = "unmodified";
      }
    }
  }

  // -- stateful GitHub Issues backend ---------------------------------------

  const ISSUE_COMMANDS = new Set<string>([
    "issues_identity",
    "issues_list",
    "issues_detail",
    "issues_comments",
    "issues_metadata",
    "issues_create",
    "issues_update",
    "issues_comment",
    "issues_set_state",
    "issues_activity_since",
  ]);
  const ISSUE_STATE_KEY = "e2e-issues-backend";

  function cloneWire<T>(value: T): T {
    // Real IPC serializes values. Cloning here prevents the SPA from mutating
    // the fake server's stored rows by retaining an object reference.
    return JSON.parse(JSON.stringify(value)) as T;
  }

  function forgeFailure(
    kind: "NoGh" | "NotAuthed" | "AlreadyExists" | "NotRepo" | "RateLimited" | "Offline" | "Other",
    message: string,
    httpStatus: number | null = null,
  ): ForgeError {
    return {
      kind,
      message,
      url: null,
      http_status: httpStatus,
      request_id: `e2e-${kind.toLowerCase()}`,
      retry_after_seconds: null,
      rate_limit_reset_at: null,
    };
  }

  const defaultIssueIdentity = {
    executor: "local",
    device_id: null,
    github_host: "github.com",
    owner: "acme",
    repo: "repo",
    remote: "git@github.com:acme/repo.git",
    user: { node_id: "U_SELF", login: "octocat" },
  };
  const issueIdentity = cloneWire(options.issues?.identity ?? defaultIssueIdentity);
  const issueMetadata = cloneWire(
    options.issues?.metadata ?? { labels: [], assignees: [], milestones: [] },
  );
  const initialIssueState = {
    issues: cloneWire(options.issues?.issues ?? []),
    details: cloneWire(options.issues?.details ?? {}),
    comments: cloneWire(options.issues?.comments ?? []),
    pullRequestComments: cloneWire(options.issues?.pullRequestComments ?? []),
  };
  let issueState = cloneWire(initialIssueState);
  let issueLatency = Math.max(0, options.issues?.latencyMs ?? 0);
  const activityPageSize =
    options.issues?.activityPageSize === undefined
      ? null
      : Math.min(100, Math.max(1, Math.floor(options.issues.activityPageSize)));
  let issueFailures: Partial<Record<IssuesCommand, ForgeError>> = {
    ...(options.issues?.errors ?? {}),
  };
  let injectedCommentId = 0;

  try {
    const persisted = sessionStorage.getItem(ISSUE_STATE_KEY);
    if (persisted !== null) {
      const parsed = JSON.parse(persisted) as typeof issueState;
      if (
        Array.isArray(parsed.issues) &&
        parsed.details !== null &&
        typeof parsed.details === "object" &&
        Array.isArray(parsed.comments) &&
        Array.isArray(parsed.pullRequestComments)
      ) {
        issueState = parsed;
      }
    }
  } catch {
    // Storage can be unavailable in hardened webviews; the in-memory backend
    // remains fully functional and only restart persistence is lost.
  }

  function persistIssueState(): void {
    try {
      sessionStorage.setItem(ISSUE_STATE_KEY, JSON.stringify(issueState));
    } catch {
      // Best effort (the production feature has its own storage fallback).
    }
  }

  type MockIssue = (typeof issueState.issues)[number];
  type MockDetail = (typeof issueState.details)[number];
  type MockComment = (typeof issueState.comments)[number];

  function mockUser(login: string): { node_id: string; login: string } {
    return (
      issueMetadata.assignees.find((entry) => entry.login === login) ?? {
        node_id: `U_${login.toUpperCase()}`,
        login,
      }
    );
  }

  function issueByNumber(number: number): MockIssue | undefined {
    return issueState.issues.find((issue) => issue.number === number);
  }

  function detailByNumber(number: number): MockDetail | undefined {
    return issueState.details[number];
  }

  function touchIssue(issue: MockIssue, at: string): void {
    issue.updated_at = at;
    const detail = detailByNumber(issue.number);
    if (detail) detail.updated_at = at;
  }

  function stringArray(value: unknown): string[] {
    return Array.isArray(value)
      ? value.filter((entry): entry is string => typeof entry === "string")
      : [];
  }

  function activityCursor(cursor: unknown): { page: number; since: number } {
    if (typeof cursor !== "string") return { page: 1, since: 0 };
    if (cursor.startsWith("/")) {
      const params = new URLSearchParams(cursor.split("?")[1] ?? "");
      const since = Date.parse(params.get("since") ?? "");
      return {
        page: Math.max(1, Math.floor(Number(params.get("page")) || 1)),
        since: Number.isFinite(since) ? since : 0,
      };
    }
    const since = Date.parse(cursor);
    return { page: 1, since: Number.isFinite(since) ? since : 0 };
  }

  function activityLink(
    endpoint: "issues" | "issues/comments",
    page: number,
    cursor: unknown,
  ): string {
    const params = new URLSearchParams({
      page: String(page),
      per_page: String(activityPageSize),
    });
    if (typeof cursor === "string" && !cursor.startsWith("/")) params.set("since", cursor);
    return `/repos/${issueIdentity.owner}/${issueIdentity.repo}/${endpoint}?${params}`;
  }

  function resolveLabels(names: string[]): Array<(typeof issueMetadata.labels)[number]> {
    return names
      .map((name) => issueMetadata.labels.find((label) => label.name === name))
      .filter((label): label is (typeof issueMetadata.labels)[number] => label !== undefined);
  }

  function resolveAssignees(logins: string[]): Array<(typeof issueMetadata.assignees)[number]> {
    return logins.map(mockUser);
  }

  function resolveMilestone(value: number | string | null | undefined) {
    if (value === null || value === undefined || value === "") return null;
    return (
      issueMetadata.milestones.find(
        (milestone) =>
          milestone.number === Number(value) || milestone.title === String(value),
      ) ?? null
    );
  }

  function appendIssueComment(
    issue: MockIssue,
    body: string,
    authorLogin: string,
    createdAt: string,
  ): MockComment {
    injectedCommentId += 1;
    const comment: MockComment = {
      node_id: `C_E2E_${Date.now()}_${injectedCommentId}`,
      issue_number: issue.number,
      author: mockUser(authorLogin),
      body,
      created_at: createdAt,
      updated_at: createdAt,
      url: `https://github.com/${issueIdentity.owner}/${issueIdentity.repo}/issues/${issue.number}#issuecomment-C_E2E_${Date.now()}_${injectedCommentId}`,
    };
    issueState.comments.push(comment);
    issue.comment_count += 1;
    const detail = detailByNumber(issue.number);
    if (detail) detail.comment_count += 1;
    touchIssue(issue, createdAt);
    return comment;
  }

  function makeIssue(input: NewMockIssue): MockIssue {
    const now = new Date().toISOString();
    const createdAt = input.createdAt ?? now;
    const updatedAt = input.updatedAt ?? createdAt;
    const number = Math.max(0, ...issueState.issues.map((issue) => issue.number)) + 1;
    const milestone = resolveMilestone(input.milestone);
    const issue: MockIssue = {
      node_id: input.nodeId ?? `I_E2E_${number}`,
      number,
      title: input.title,
      state: input.state ?? "open",
      author: mockUser(input.author ?? issueIdentity.user.login),
      created_at: createdAt,
      updated_at: updatedAt,
      closed_at: input.state === "closed" ? updatedAt : null,
      url: `https://github.com/${issueIdentity.owner}/${issueIdentity.repo}/issues/${number}`,
      labels: resolveLabels(input.labels ?? []),
      assignees: resolveAssignees(input.assignees ?? []),
      milestone,
      comment_count: 0,
      locked: false,
      state_reason: input.state === "closed" ? "completed" : null,
    };
    issueState.issues.push(issue);
    issueState.details[number] = {
      ...cloneWire(issue),
      body: input.body ?? "",
      issue_type: {
        name: issue.labels.some((label) => label.name === "bug") ? "Bug" : "Task",
      },
    };
    persistIssueState();
    return issue;
  }

  function makeComment(input: NewMockComment, pullRequest = false): MockComment {
    const issue = issueByNumber(input.issueNumber);
    if (!pullRequest && !issue) {
      throw forgeFailure("NotRepo", `Issue #${input.issueNumber} was not found`, 404);
    }
    const now = new Date().toISOString();
    const createdAt = input.createdAt ?? now;
    const updatedAt = input.updatedAt ?? createdAt;
    const nodeId = input.nodeId ?? `C_E2E_${Date.now()}_${++injectedCommentId}`;
    const number = pullRequest ? input.issueNumber : issue!.number;
    const comment: MockComment = {
      node_id: nodeId,
      issue_number: number,
      author: mockUser(input.author ?? "alice"),
      body: input.body,
      created_at: createdAt,
      updated_at: updatedAt,
      url: `https://github.com/${issueIdentity.owner}/${issueIdentity.repo}/issues/${number}#issuecomment-${nodeId}`,
    };
    if (pullRequest) {
      issueState.pullRequestComments.push(comment);
    } else {
      issue!.comment_count += 1;
      const detail = detailByNumber(number);
      if (detail) detail.comment_count += 1;
      touchIssue(issue!, updatedAt);
      issueState.comments.push(comment);
    }
    persistIssueState();
    return comment;
  }

  function configuredIssueFailure(
    command: IssuesCommand,
    args: Record<string, unknown>,
  ): ForgeError | null {
    const direct = issueFailures[command];
    if (direct) return direct;
    if (command === "issues_list") {
      const query = args.query as { search?: unknown } | undefined;
      const search = typeof query?.search === "string" ? query.search.trim().toLowerCase() : "";
      return options.issues?.searchErrors?.[search] ?? null;
    }
    return null;
  }

  function applyIssues(cmd: string, args: Record<string, unknown>): unknown {
    const command = cmd as IssuesCommand;
    const failure = configuredIssueFailure(command, args);
    if (failure) throw failure;

    switch (command) {
      case "issues_identity":
        return issueIdentity;
      case "issues_metadata":
        return issueMetadata;
      case "issues_list": {
        const query = (args.query ?? {}) as {
          state?: unknown;
          search?: unknown;
          labels?: unknown;
          assignees?: unknown;
          milestone?: unknown;
          sort?: unknown;
          direction?: unknown;
          per_page?: unknown;
          page?: unknown;
        };
        const state = typeof query.state === "string" ? query.state : "open";
        const search = typeof query.search === "string" ? query.search.trim().toLowerCase() : "";
        const labels = stringArray(query.labels).map((label) => label.toLowerCase());
        const assignees = stringArray(query.assignees);
        const milestone = typeof query.milestone === "string" ? query.milestone : "";
        const sort = typeof query.sort === "string" ? query.sort : "updated";
        const direction = query.direction === "asc" ? 1 : -1;
        const perPage = Math.min(100, Math.max(1, Number(query.per_page) || 30));
        const page = Math.max(1, Math.floor(Number(query.page) || 1));
        let items = issueState.issues.filter((issue) => {
          if (state !== "all" && issue.state !== state) return false;
          if (!labels.every((label) => issue.labels.some((entry) => entry.name.toLowerCase() === label))) {
            return false;
          }
          if (assignees.length > 0 && !assignees.some((assignee) => issue.assignees.some((entry) => entry.login === assignee))) {
            return false;
          }
          if (
            milestone !== "" &&
            issue.milestone?.title !== milestone &&
            String(issue.milestone?.number ?? "") !== milestone
          ) {
            return false;
          }
          if (search !== "") {
            const haystack = `${issue.number} ${issue.title} ${detailByNumber(issue.number)?.body ?? ""}`.toLowerCase();
            if (!haystack.includes(search)) return false;
          }
          return true;
        });
        items = [...items].sort((left, right) => {
          let compared = 0;
          if (sort === "created") {
            compared = Date.parse(left.created_at) - Date.parse(right.created_at);
          } else if (sort === "comments") {
            compared = left.comment_count - right.comment_count;
          } else if (sort === "number") {
            compared = left.number - right.number;
          } else {
            compared = Date.parse(left.updated_at) - Date.parse(right.updated_at);
          }
          if (compared === 0) compared = left.number - right.number;
          return compared * direction;
        });
        const total = items.length;
        const start = (page - 1) * perPage;
        return {
          items: items.slice(start, start + perPage),
          page,
          per_page: perPage,
          has_next: start + perPage < total,
          has_prev: page > 1 && start < total,
          total_count: search === "" ? total : null,
          incomplete_results:
            search !== "" &&
            (options.issues?.incompleteSearches ?? []).some((entry) => entry.toLowerCase() === search),
        };
      }
      case "issues_detail": {
        const number = Number(args.number);
        const detail = detailByNumber(number);
        if (!detail) throw forgeFailure("NotRepo", `Issue #${number} was not found`, 404);
        return detail;
      }
      case "issues_comments": {
        const number = Number(args.number);
        if (!issueByNumber(number)) {
          throw forgeFailure("NotRepo", `Issue #${number} was not found`, 404);
        }
        const perPage = Math.min(100, Math.max(1, Number(args.per_page) || 30));
        const comments = issueState.comments
          .filter((comment) => comment.issue_number === number)
          .sort((left, right) => Date.parse(left.created_at) - Date.parse(right.created_at));
        const total = comments.length;
        const page =
          args.page === null || args.page === undefined
            ? Math.max(1, Math.ceil(total / perPage))
            : Math.min(Math.max(1, Math.floor(Number(args.page) || 1)), Math.max(1, Math.ceil(total / perPage)));
        const start = (page - 1) * perPage;
        return {
          items: comments.slice(start, start + perPage),
          page,
          per_page: perPage,
          has_next: start + perPage < total,
          has_prev: page > 1 && start < total,
          total_count: total,
        };
      }
      case "issues_create": {
        const title = String(args.title ?? "").trim();
        if (title === "") throw forgeFailure("Other", "Validation Failed: title cannot be blank", 422);
        const issue = makeIssue({
          title,
          body: String(args.body ?? ""),
          labels: stringArray(args.labels),
          assignees: stringArray(args.assignees),
          milestone: typeof args.milestone === "number" ? args.milestone : null,
        });
        return detailByNumber(issue.number)!;
      }
      case "issues_update": {
        const number = Number(args.number);
        const issue = issueByNumber(number);
        const detail = detailByNumber(number);
        if (!issue || !detail) throw forgeFailure("NotRepo", `Issue #${number} was not found`, 404);
        const title = String(args.title ?? "").trim();
        if (title === "") throw forgeFailure("Other", "Validation Failed: title cannot be blank", 422);
        const now = new Date().toISOString();
        issue.title = title;
        issue.labels = resolveLabels(stringArray(args.labels));
        issue.assignees = resolveAssignees(stringArray(args.assignees));
        issue.milestone = resolveMilestone(typeof args.milestone === "number" ? args.milestone : null);
        detail.title = title;
        detail.body = String(args.body ?? "");
        detail.labels = cloneWire(issue.labels);
        detail.assignees = cloneWire(issue.assignees);
        detail.milestone = cloneWire(issue.milestone);
        touchIssue(issue, now);
        persistIssueState();
        return detail;
      }
      case "issues_comment": {
        const number = Number(args.number);
        const issue = issueByNumber(number);
        if (!issue) throw forgeFailure("NotRepo", `Issue #${number} was not found`, 404);
        const body = String(args.body ?? "").trim();
        if (body === "") throw forgeFailure("Other", "Validation Failed: body cannot be blank", 422);
        return appendIssueComment(issue, body, issueIdentity.user.login, new Date().toISOString());
      }
      case "issues_set_state": {
        const number = Number(args.number);
        const issue = issueByNumber(number);
        const detail = detailByNumber(number);
        if (!issue || !detail) throw forgeFailure("NotRepo", `Issue #${number} was not found`, 404);
        if (args.state !== "open" && args.state !== "closed") {
          throw forgeFailure("Other", "Issue state must be open or closed", 422);
        }
        const now = new Date().toISOString();
        issue.state = args.state;
        detail.state = args.state;
        issue.state_reason = args.state === "closed" ? "completed" : null;
        detail.state_reason = issue.state_reason;
        issue.closed_at = args.state === "closed" ? now : null;
        detail.closed_at = issue.closed_at;
        const comment = typeof args.comment === "string" ? args.comment.trim() : "";
        if (comment !== "") appendIssueComment(issue, comment, issueIdentity.user.login, now);
        touchIssue(issue, now);
        persistIssueState();
        return detail;
      }
      case "issues_activity_since": {
        const issueWindow = activityCursor(args.issue_since);
        const commentWindow = activityCursor(args.comment_since);
        const allIssues = issueState.issues
          .filter((issue) => Date.parse(issue.updated_at) >= issueWindow.since)
          .sort((left, right) => Date.parse(left.updated_at) - Date.parse(right.updated_at));
        const allComments = issueState.comments
          .filter((comment) => Date.parse(comment.updated_at) >= commentWindow.since)
          .sort((left, right) => Date.parse(left.created_at) - Date.parse(right.created_at))
          .map((comment) => ({
            ...comment,
            // The Rust fallback resolves this title when the issue is absent
            // from the independently paginated issue lane.
            issue_title: issueByNumber(comment.issue_number)?.title ?? `Issue #${comment.issue_number}`,
          }));
        const issueStart = (issueWindow.page - 1) * (activityPageSize ?? allIssues.length);
        const commentStart = (commentWindow.page - 1) * (activityPageSize ?? allComments.length);
        const issues = allIssues.slice(issueStart, issueStart + (activityPageSize ?? allIssues.length));
        const comments = allComments.slice(
          commentStart,
          commentStart + (activityPageSize ?? allComments.length),
        );
        const nextIssueCursor =
          activityPageSize !== null && issueStart + activityPageSize < allIssues.length
            ? activityLink("issues", issueWindow.page + 1, args.issue_since)
            : null;
        const nextCommentCursor =
          activityPageSize !== null && commentStart + activityPageSize < allComments.length
            ? activityLink("issues/comments", commentWindow.page + 1, args.comment_since)
            : null;
        // pullRequestComments deliberately never enter this response: the real
        // Rust gh query excludes PR rows before the JS normalization layer.
        return {
          issues,
          comments,
          next_issue_cursor: nextIssueCursor,
          next_comment_cursor: nextCommentCursor,
          complete: nextIssueCursor === null && nextCommentCursor === null,
        };
      }
      default:
        return null;
    }
  }

  const issueControls = {
    addIssue(input: NewMockIssue): MockIssue {
      return cloneWire(makeIssue(input));
    },
    addComment(input: NewMockComment): MockComment {
      return cloneWire(makeComment(input));
    },
    addPullRequestComment(input: NewMockComment): MockComment {
      return cloneWire(makeComment(input, true));
    },
    editComment(input: { nodeId: string; body: string }): MockComment {
      const comment = [...issueState.comments, ...issueState.pullRequestComments].find(
        (entry) => entry.node_id === input.nodeId,
      );
      if (!comment) throw forgeFailure("NotRepo", `Comment ${input.nodeId} was not found`, 404);
      comment.body = input.body;
      comment.updated_at = new Date().toISOString();
      persistIssueState();
      return cloneWire(comment);
    },
    setIssueFailure(input: { command: IssuesCommand; error: ForgeError | null }): void {
      if (input.error === null) delete issueFailures[input.command];
      else issueFailures[input.command] = input.error;
    },
    setIssueLatency(latencyMs: number | null): void {
      issueLatency = Math.max(0, latencyMs ?? 0);
    },
    snapshot(): typeof issueState {
      return cloneWire(issueState);
    },
    reset(): void {
      issueState = cloneWire(initialIssueState);
      issueFailures = { ...(options.issues?.errors ?? {}) };
      issueLatency = Math.max(0, options.issues?.latencyMs ?? 0);
      injectedCommentId = 0;
      try {
        sessionStorage.removeItem(ISSUE_STATE_KEY);
      } catch {
        // Best effort.
      }
    },
  };

  const callbacks = new Map<number, (raw: unknown) => void>();
  const channelIndex = new Map<number, number>();
  const listeners = new Map<number, { event: string; handler: number }>();
  let nextCallbackId = 1;
  let nextEventId = 1;
  const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];

  /** Pushes one message through a Channel arg (instance or serialized id).
   *  Delivered on a MICROTASK (never synchronously inside `invoke`) — the
   *  real backend delivers channel pages from its event loop, and running
   *  app effects rely on pages arriving outside the calling effect's stack.
   *  Indices increment per channel: the real `Channel` class only processes
   *  `index === nextMessageIndex` and queues the rest forever, so a constant
   *  index would strand every page after the first. */
  function deliverChannel(channel: unknown, message: unknown): void {
    let id: number | null = null;
    if (typeof channel === "string") {
      const match = /^__CHANNEL__:(\d+)$/.exec(channel);
      id = match === null ? null : Number(match[1]);
    } else if (channel && typeof channel === "object" && "id" in channel) {
      id = Number((channel as { id: unknown }).id);
    }
    const callback = id === null ? undefined : callbacks.get(id);
    if (!callback || id === null) return;
    const index = channelIndex.get(id) ?? 0;
    channelIndex.set(id, index + 1);
    queueMicrotask(() => callback({ index, message }));
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
      channelIndex.delete(id);
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

      // Streaming: deliver one page through the channel, then resolve. The
      // page lands on a microtask BEFORE the invoke promise settles — the
      // real backend resolves these commands only when the walk ends, i.e.
      // after every channel page, and the history store's abort detector
      // treats a resolve with no accepted final page as a truncated walk.
      // A `{ pages: [...] }` mock value delivers a multi-page walk (one page
      // per macrotask, resolve after the last) like the real 500-commit
      // pagination; a plain page object keeps the single-page shape.
      // `resolveBeforeLastPage: true` models Tauri's large-payload fetch
      // path: pages over 8 KB reach the JS Channel through an async fetch
      // round-trip, so the command promise resolves while the last page is
      // still in flight (the store must not read that as a truncation).
      if (cmd === "repo_log_stream" || cmd === "repo_file_history") {
        const mock = cmd in commandMocks ? commandMocks[cmd] : EMPTY_LOG_PAGE;
        const shape = mock as {
          pages?: unknown;
          resolveBeforeLastPage?: unknown;
          pageGapMs?: unknown;
        };
        const pages = Array.isArray(shape?.pages)
          ? (shape.pages as unknown[])
          : [mock];
        const lateLast =
          shape?.resolveBeforeLastPage === true && pages.length > 1;
        const gap = typeof shape?.pageGapMs === "number" ? Math.max(0, shape.pageGapMs) : 0;
        const channel = (args as { on_page?: unknown })?.on_page;
        return new Promise((resolve) => {
          const step = (i: number): void => {
            if (i === pages.length - 1) {
              if (lateLast) {
                setTimeout(() => {
                  resolve(null);
                  setTimeout(() => deliverChannel(channel, pages[i]), 0);
                }, 0);
              } else {
                deliverChannel(channel, pages[i]);
                setTimeout(() => resolve(null), 0);
              }
              return;
            }
            deliverChannel(channel, pages[i]);
            setTimeout(() => step(i + 1), gap);
          };
          setTimeout(() => step(0), gap);
        });
      }
      if (cmd === "repo_diff_stream") {
        const mock = cmd in commandMocks ? commandMocks[cmd] : [];
        // `{ pages: [...] }` delivers the diff as several file pages (one
        // per macrotask) like the real backend's multi-page streams —
        // CommitDetail/PopoutDiff accumulate them onto one files array.
        // A plain array mock keeps the single-page shape.
        const pages = Array.isArray((mock as { pages?: unknown })?.pages)
          ? ((mock as { pages: unknown[] }).pages as unknown[])
          : [mock];
        const gap =
          typeof (mock as { pageGapMs?: unknown })?.pageGapMs === "number"
            ? Math.max(0, (mock as { pageGapMs: number }).pageGapMs)
            : 0;
        const channel = (args as { on_page?: unknown })?.on_page;
        return new Promise((resolve) => {
          const step = (i: number): void => {
            deliverChannel(channel, pages[i]);
            if (i === pages.length - 1) {
              setTimeout(() => resolve(null), 0);
              return;
            }
            setTimeout(() => step(i + 1), gap);
          };
          setTimeout(() => step(0), gap);
        });
      }

      if (cmd === "stage") applyStage(args);
      if (cmd === "stage_all") applyStageAll(args);

      // Overview scan: synthesized per path (see fakeRepoSummary).
      if (cmd === "repo_summary") {
        const path = String((args as { path?: unknown })?.path ?? "");
        const value = fakeRepoSummary(path);
        if (
          value !== null &&
          typeof value === "object" &&
          "__error__" in (value as Record<string, unknown>)
        ) {
          return Promise.reject(mockRejection(value));
        }
        return Promise.resolve(value);
      }

      // Workspace file-browser commands: a small mutable fake workdir so
      // list/stat/read/write flows behave like the real backend.
      if (cmd.startsWith("repo_list_dir") || WORKSPACE_COMMANDS.has(cmd)) {
        const result = applyWorkspace(cmd, args ?? {});
        if (result !== undefined) return Promise.resolve(result);
      }

      // The commit detail pane fetches the full message of the shown
      // commit (`repo_commit_message`); answer from the mocked log fixture,
      // accepting abbreviated shas like the real backend resolves them.
      if (cmd === "repo_commit_message") {
        const sha = String((args as { sha?: string })?.sha ?? "");
        const log = commandMocks.repo_log_stream as
          | { commits?: Array<Record<string, unknown>> }
          | undefined;
        const hit = log?.commits?.find(
          (c) =>
            c.sha === sha ||
            (typeof c.sha === "string" && c.sha.startsWith(sha)),
        );
        return Promise.resolve(typeof hit?.message === "string" ? hit.message : null);
      }

      if (cmd in commandMocks) {
        const value = commandMocks[cmd];
        if (
          value !== null &&
          typeof value === "object" &&
          "__error__" in (value as Record<string, unknown>)
        ) {
          return Promise.reject(mockRejection(value));
        }
        return Promise.resolve(value === undefined ? null : value);
      }

      if (ISSUE_COMMANDS.has(cmd)) {
        let value: unknown;
        try {
          value = applyIssues(cmd, args ?? {});
        } catch (error) {
          if (issueLatency === 0) return Promise.reject(error);
          return new Promise((_resolve, reject) => {
            setTimeout(() => reject(error), issueLatency);
          });
        }
        if (issueLatency === 0) return Promise.resolve(cloneWire(value));
        return new Promise((resolve) => {
          setTimeout(() => resolve(cloneWire(value)), issueLatency);
        });
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
    commands: commandMocks,
    issues: issueControls,
    emit(event: string, payload: unknown): void {
      for (const { event: name, handler } of [...listeners.values()]) {
        if (name !== event) continue;
        const callback = callbacks.get(handler);
        if (callback) callback({ event: name, id: 0, payload });
      }
    },
  };
}
