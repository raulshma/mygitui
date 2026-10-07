/**
 * Typed IPC client for the mygitui backend (M1).
 *
 * Every Tauri command goes through this module — UI code never calls
 * `invoke()` directly. Command names, argument names (snake_case, exactly as
 * documented in docs/contracts.md) and payload shapes mirror the Rust side
 * (`src-tauri/src/engine/types.rs` ↔ `src/lib/ipc/types.ts`); values are
 * passed through verbatim (no camelCase conversion anywhere).
 *
 * Testability / portability:
 *   - All commands flow through one injectable {@link Transport} (tests
 *     inject a mock; the default wraps `@tauri-apps/api/core.invoke`).
 *   - Streaming commands push pages over a channel-like object. The default
 *     factory uses the real tauri `Channel` class inside a Tauri webview and
 *     a plain object everywhere else, so constructing a channel never throws
 *     outside Tauri.
 *   - Outside a Tauri webview the default transport rejects non-stream
 *     commands with an {@link IpcError}, while streams resolve immediately
 *     after delivering a single empty page (documented non-Tauri behavior —
 *     no throw, UI renders an empty result).
 */

import { invoke, Channel } from "@tauri-apps/api/core";
import type { InvokeOptions } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "$lib/entry/dragdrop";
import type {
  BlameLine,
  DiffSide,
  FileDiff,
  LogFilter,
  LogPage,
  RepoId,
  RepoInfo,
  RepoStatus,
} from "./types";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/** Uniform failure for any backend command: which command failed and why. */
export class IpcError extends Error {
  readonly command: string;

  constructor(command: string, message: string) {
    super(message);
    this.name = "IpcError";
    this.command = command;
  }
}

/** Coerces any thrown value into an `IpcError` (already-IpcError passes through). */
function toIpcError(command: string, err: unknown): IpcError {
  if (err instanceof IpcError) return err;
  let message = "unknown IPC error";
  if (typeof err === "string") {
    message = err;
  } else if (
    typeof err === "object" &&
    err !== null &&
    typeof (err as { message?: unknown }).message === "string"
  ) {
    message = (err as { message: string }).message;
  }
  return new IpcError(command, message);
}

// ---------------------------------------------------------------------------
// Injectable transport (command invocation)
// ---------------------------------------------------------------------------

/** Signature of `@tauri-apps/api/core.invoke` (what a mock must satisfy). */
export type Transport = (
  command: string,
  args?: Record<string, unknown>,
  options?: InvokeOptions,
) => Promise<unknown>;

/** Real transport: Tauri `invoke`, guarded for non-Tauri environments. */
function defaultTransport(
  command: string,
  args?: Record<string, unknown>,
): Promise<unknown> {
  if (!isTauri()) {
    return Promise.reject(
      new IpcError(command, "Tauri runtime is not available"),
    );
  }
  return invoke(command, args);
}

let transportOverride: Transport | null = null;

/** Replaces the transport (tests). Pass `null` to restore the default. */
export function setTransport(transport: Transport | null): void {
  transportOverride = transport;
}

/** Restores the real (guarded) transport. */
export function resetTransport(): void {
  transportOverride = null;
}

/** Runs a command through the active transport, normalizing failures. */
async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const transport = transportOverride ?? defaultTransport;
  try {
    return (await transport(command, args)) as T;
  } catch (err) {
    throw toIpcError(command, err);
  }
}

// ---------------------------------------------------------------------------
// Streaming channels
// ---------------------------------------------------------------------------

/** Minimal surface of the tauri `Channel` class this client relies on. */
export interface ChannelLike<T> {
  onmessage: (message: T) => void;
}

/** Creates stream channels; overridable for tests. */
export type ChannelFactory = <T>(
  onMessage: (message: T) => void,
) => ChannelLike<T>;

function defaultChannelFactory<T>(
  onMessage: (message: T) => void,
): ChannelLike<T> {
  // `new Channel()` touches window.__TAURI_INTERNALS__ in its constructor,
  // which does not exist outside a Tauri webview — fall back to a plain
  // object (only ever used by mocked transports / tests in that mode).
  if (isTauri()) return new Channel<T>(onMessage);
  return { onmessage: onMessage };
}

let channelFactoryOverride: ChannelFactory | null = null;

/** Replaces the channel factory (tests). Pass `null` to restore the default. */
export function setChannelFactory(factory: ChannelFactory | null): void {
  channelFactoryOverride = factory;
}

/** Restores the real channel factory. */
export function resetChannelFactory(): void {
  channelFactoryOverride = null;
}

/**
 * Shared body of every streaming command: builds a channel wired to
 * `onPage`, sends it as the `channel` argument, and awaits completion.
 * Non-Tauri default mode delivers one empty page and resolves (no throw).
 */
async function runStream<T>(
  command: string,
  args: Record<string, unknown>,
  emptyPage: T,
  onPage: (page: T) => void,
): Promise<void> {
  const usingDefaults = transportOverride === null && channelFactoryOverride === null;
  if (usingDefaults && !isTauri()) {
    onPage(emptyPage);
    return;
  }
  const factory = channelFactoryOverride ?? defaultChannelFactory;
  const channel = factory<T>((page) => onPage(page));
  await call<void>(command, { ...args, channel });
}

const EMPTY_LOG_PAGE: LogPage = {
  commits: [],
  rows: [],
  next_cursor: null,
  generation: 0,
};

// ---------------------------------------------------------------------------
// Commands (docs/contracts.md — names and args are the contract)
// ---------------------------------------------------------------------------

/** Opens (or focuses) a repository and starts its watcher. */
export function openRepo(path: string): Promise<RepoInfo> {
  return call<RepoInfo>("repo_open", { path });
}

/** Closes a repository, stopping its watcher and dropping caches. */
export function closeRepo(repoId: RepoId): Promise<void> {
  return call<void>("repo_close", { repo_id: repoId });
}

/** Snapshot of branch state + working-copy entries. */
export function repoStatus(repoId: RepoId): Promise<RepoStatus> {
  return call<RepoStatus>("repo_status", { repo_id: repoId });
}

/** All refs as `[name, sha]` pairs. */
export function repoRefs(repoId: RepoId): Promise<[string, string][]> {
  return call<[string, string][]>("repo_refs", { repo_id: repoId });
}

/** Line-by-line blame for one file (optionally starting at a commit). */
export function repoBlame(
  repoId: RepoId,
  path: string,
  from?: string,
): Promise<BlameLine[]> {
  const args: Record<string, unknown> = { repo_id: repoId, path };
  if (from !== undefined) args.from = from;
  return call<BlameLine[]>("repo_blame", args);
}

/** One-shot diff for small/medium changes; larger diffs should stream. */
export function repoDiff(
  repoId: RepoId,
  oldSide: DiffSide,
  newSide: DiffSide,
  paths?: string[],
): Promise<FileDiff[]> {
  // `new` is a reserved word, so it can only appear as a property key.
  const args: Record<string, unknown> = { repo_id: repoId, old: oldSide, new: newSide };
  if (paths !== undefined) args.paths = paths;
  return call<FileDiff[]>("repo_diff", args);
}

/** Streams a diff as pages of ~50 files. */
export function streamDiff(
  repoId: RepoId,
  oldSide: DiffSide,
  newSide: DiffSide,
  onFilePage: (files: FileDiff[]) => void,
  paths?: string[],
): Promise<void> {
  const args: Record<string, unknown> = { repo_id: repoId, old: oldSide, new: newSide };
  if (paths !== undefined) args.paths = paths;
  return runStream<FileDiff[]>(
    "repo_diff_stream",
    args,
    [],
    onFilePage,
  );
}

/** Streams the commit log as `LogPage`s of up to 500 commits. */
export function streamLog(
  repoId: RepoId,
  filter: LogFilter,
  onPage: (page: LogPage) => void,
): Promise<void> {
  return runStream<LogPage>("repo_log_stream", { repo_id: repoId, filter }, EMPTY_LOG_PAGE, onPage);
}

/** Streams the history of a single file, following renames. */
export function streamFileHistory(
  repoId: RepoId,
  path: string,
  onPage: (page: LogPage) => void,
): Promise<void> {
  return runStream<LogPage>(
    "repo_file_history",
    { repo_id: repoId, path },
    EMPTY_LOG_PAGE,
    onPage,
  );
}

// ---------------------------------------------------------------------------
// Events (backend → frontend)
// ---------------------------------------------------------------------------

/** Payload of the `repo-changed` watcher event. */
export interface RepoChangedEvent {
  repo_id: string;
  paths: string[];
  head_moved: boolean;
  full: boolean;
}

/**
 * Subscribes to `repo-changed` watcher events. Resolves with an unlisten
 * function; outside a Tauri webview (or if registration fails) resolves with
 * a no-op unlisten — this function never throws.
 */
export async function onRepoChanged(
  cb: (event: RepoChangedEvent) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<RepoChangedEvent>("repo-changed", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

// ---------------------------------------------------------------------------
// Clone (B4) — repo_clone command, clone-progress events, folder picker
// (appended; everything above is byte-identical)
// ---------------------------------------------------------------------------

/**
 * Payload of the `clone-progress` event emitted by `repo_clone`.
 * `total`/`objects` count git objects (libgit2 exposes no total-bytes
 * estimate); `received` is the byte counter.
 */
export interface CloneProgressEvent {
  /** URL of the clone this progress belongs to. */
  url: string;
  /** Bytes received so far. */
  received: number;
  /** Total objects the remote announced (0 = not yet known). */
  total: number;
  /** Objects received so far. */
  objects: number;
}

/**
 * Clones `url` into `destination` (created if missing, must be empty if it
 * exists). `depth` 1 performs a shallow clone. Resolves with the cloned
 * repository path on success.
 */
export function cloneRepo(
  url: string,
  destination: string,
  depth?: number,
): Promise<string> {
  const args: Record<string, unknown> = { url, destination };
  if (depth !== undefined) args.depth = depth;
  return call<string>("repo_clone", args);
}

/**
 * Subscribes to `clone-progress` events (emitted by an in-flight
 * `repo_clone`; identify yours by `url`). Same contract as
 * {@link onRepoChanged}: never throws, no-op unlisten outside Tauri.
 */
export async function onCloneProgress(
  cb: (event: CloneProgressEvent) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<CloneProgressEvent>("clone-progress", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

/**
 * Opens the native folder picker (@tauri-apps/plugin-dialog) and resolves
 * the chosen directory, or `null` when the user cancelled / the picker is
 * unavailable (non-Tauri). Never throws.
 */
export async function pickFolder(): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    const dialog = await import("@tauri-apps/plugin-dialog");
    const selection = await dialog.open({
      directory: true,
      multiple: false,
      title: "Choose a repository folder",
    });
    if (typeof selection === "string") return selection;
    if (Array.isArray(selection)) {
      const first = selection[0];
      return typeof first === "string" ? first : null;
    }
    return null;
  } catch (err) {
    console.warn("[ipc] folder picker failed:", err);
    return null;
  }
}
