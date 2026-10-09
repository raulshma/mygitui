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
  AuthRequest,
  BlameLine,
  BisectLogEntry,
  BisectMark,
  BisectState,
  BranchInfo,
  BranchTrashEntry,
  CheckpointInfo,
  CommitOptions,
  CommitSignature,
  ConflictFile,
  ConflictResolution,
  DiffSide,
  FetchOptions,
  FileDiff,
  HookInfo,
  LfsStatus,
  LogFilter,
  LogPage,
  MergeOptions,
  MergeResult,
  MergetoolInfo,
  MergetoolResult,
  NetStats,
  OpProgress,
  PreviewInfo,
  PullOptions,
  PushOptions,
  RebaseState,
  RebaseStep,
  ReflogEntry,
  RemoteBranchInfo,
  RemoteInfo,
  RepoHealth,
  RepoId,
  RepoInfo,
  RepoStatus,
  ResetKind,
  SigningInfo,
  SparseInfo,
  StageRequest,
  StageTarget,
  StashInfo,
  TagInfo,
  WorktreeInfo,
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
  // Backend stream commands declare the channel arg as `on_page`.
  await call<void>(command, { ...args, on_page: channel });
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

/**
 * Save-file picker for exports (M11 archive). Returns the chosen path or
 * `null` on cancel / non-Tauri. Never throws.
 */
export async function pickSaveFile(
  title: string,
  defaultName: string,
  filters: { name: string; extensions: string[] }[],
): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    const dialog = await import("@tauri-apps/plugin-dialog");
    const path = await dialog.save({ title, defaultPath: defaultName, filters });
    return typeof path === "string" && path !== "" ? path : null;
  } catch (err) {
    console.warn("[ipc] save picker failed:", err);
    return null;
  }
}

// ---------- M11: repo health / maintenance / sparse / LFS / archive ----------

/** Health snapshot (fs-only sizes + object counts). */
export function repoHealth(repoId: RepoId): Promise<RepoHealth> {
  return call<RepoHealth>("repo_health", { repo_id: repoId });
}

/** Runs one maintenance op (`gc`|`prune`|`commit_graph`|`pack_refs`|`count_objects`). */
export function maintenanceRun(repoId: RepoId, op: string): Promise<string> {
  return call<string>("maintenance_run", { repo_id: repoId, op });
}

/** `git archive --format <format> <spec>` into `destination`. */
export function archiveSpec(
  repoId: RepoId,
  spec: string,
  format: string,
  destination: string,
): Promise<void> {
  return call<void>("archive", { repo_id: repoId, spec, format, destination });
}

/** Sparse-checkout state (cone mode). */
export function sparseInfo(repoId: RepoId): Promise<SparseInfo> {
  return call<SparseInfo>("sparse_info", { repo_id: repoId });
}

/** Applies sparse-checkout patterns (cone; empty list = full checkout). */
export function sparseApply(
  repoId: RepoId,
  patterns: string[],
  add: boolean,
): Promise<void> {
  return call<void>("sparse_apply", { repo_id: repoId, patterns, add });
}

/** `git lfs` availability + tracked patterns. */
export function lfsStatus(repoId: RepoId): Promise<LfsStatus> {
  return call<LfsStatus>("lfs_status", { repo_id: repoId });
}

/** Runs `git lfs pull|push|install|fetch`. */
export function lfsRun(repoId: RepoId, subcommand: string): Promise<string> {
  return call<string>("lfs_run", { repo_id: repoId, subcommand });
}

/** Blobless (partial) clone via the git CLI (`--filter=blob:none`). */
export function cloneBlobless(
  url: string,
  destination: string,
  depth?: number,
): Promise<string> {
  const args: Record<string, unknown> = { url, destination };
  if (depth !== undefined) args.depth = depth;
  return call<string>("clone_blobless", args);
}

// ---------------------------------------------------------------------------
// M2: mutations (appended; docs/contracts.md "Commands (M2)")
// ---------------------------------------------------------------------------

/** Stages or unstages the files/hunks/line-ranges in `request`. */
export function stage(repoId: RepoId, request: StageRequest): Promise<void> {
  return call<void>("stage", { repo_id: repoId, request });
}

/** Stages (`unstage=false`) or unstages everything at once. */
export function stageAll(repoId: RepoId, unstage: boolean): Promise<void> {
  return call<void>("stage_all", { repo_id: repoId, unstage });
}

/** Commits the index; resolves with the new commit sha. */
export function commit(repoId: RepoId, options: CommitOptions): Promise<string> {
  return call<string>("commit", { repo_id: repoId, options });
}

/** Whether commits are signed and with what (ssh / openpgp / ...). */
export function signingInfo(repoId: RepoId): Promise<SigningInfo> {
  return call<SigningInfo>("signing_info", { repo_id: repoId });
}

/** Git hooks that exist under `.git/hooks` and their executable bit. */
export function hooksList(repoId: RepoId): Promise<HookInfo[]> {
  return call<HookInfo[]>("hooks_list", { repo_id: repoId });
}

/** Local branches with upstream tracking state and the HEAD marker. */
export function branches(repoId: RepoId): Promise<BranchInfo[]> {
  return call<BranchInfo[]>("branches", { repo_id: repoId });
}

/** Creates a branch (optionally starting at `from`) and checks it out. */
export function branchCreate(
  repoId: RepoId,
  name: string,
  checkout: boolean,
  from?: string,
): Promise<void> {
  const args: Record<string, unknown> = { repo_id: repoId, name, checkout };
  if (from !== undefined && from !== "") args.from = from;
  return call<void>("branch_create", args);
}

/** Checks out `name`; `force` discards local modifications. */
export function branchSwitch(
  repoId: RepoId,
  name: string,
  force: boolean,
): Promise<void> {
  return call<void>("branch_switch", { repo_id: repoId, name, force });
}

/** True when `name` is fully merged into `into` (safe delete). */
export function branchIsMerged(
  repoId: RepoId,
  name: string,
  into: string,
): Promise<boolean> {
  return call<boolean>("branch_is_merged", {
    repo_id: repoId,
    name,
    into,
  });
}

/** Deletes a branch; `force` removes it even when unmerged. */
export function branchDelete(
  repoId: RepoId,
  name: string,
  force: boolean,
): Promise<void> {
  return call<void>("branch_delete", { repo_id: repoId, name, force });
}

/** Renames a branch (`old` → `new`). */
export function branchRename(
  repoId: RepoId,
  oldName: string,
  newName: string,
): Promise<void> {
  // `new` is a reserved word, so it is set as a computed property key.
  const args: Record<string, unknown> = {
    repo_id: repoId,
    old: oldName,
    new: newName,
  };
  return call<void>("branch_rename", args);
}

/** Creates a (lightweight unless `message` given) tag at `target`/HEAD. */
export function tagCreate(
  repoId: RepoId,
  name: string,
  target?: string,
  message?: string,
): Promise<void> {
  const args: Record<string, unknown> = { repo_id: repoId, name };
  if (target !== undefined && target !== "") args.target = target;
  if (message !== undefined && message !== "") args.message = message;
  return call<void>("tag_create", args);
}

/** Deletes a tag. */
export function tagDelete(repoId: RepoId, name: string): Promise<void> {
  return call<void>("tag_delete", { repo_id: repoId, name });
}

/** Configured remotes with fetch and (optional) push URLs. */
export function remotes(repoId: RepoId): Promise<RemoteInfo[]> {
  return call<RemoteInfo[]>("remotes", { repo_id: repoId });
}

/** Adds a remote. */
export function remoteAdd(
  repoId: RepoId,
  name: string,
  url: string,
): Promise<void> {
  return call<void>("remote_add", { repo_id: repoId, name, url });
}

/** Removes a remote (tracking branches go with it). */
export function remoteRemove(repoId: RepoId, name: string): Promise<void> {
  return call<void>("remote_remove", { repo_id: repoId, name });
}

/** Sets a remote's fetch URL, or its push URL when `push` is true. */
export function remoteSetUrl(
  repoId: RepoId,
  name: string,
  url: string,
  push: boolean,
): Promise<void> {
  return call<void>("remote_set_url", {
    repo_id: repoId,
    name,
    url,
    push,
  });
}

/** Fetches a remote (optionally pruning); resolves with transfer stats. */
export function fetchRepo(
  repoId: RepoId,
  options: FetchOptions,
): Promise<NetStats> {
  return call<NetStats>("fetch", { repo_id: repoId, options });
}

/** Pulls a branch (ff-only and/or rebase per `options`). */
export function pullRepo(
  repoId: RepoId,
  options: PullOptions,
): Promise<NetStats> {
  return call<NetStats>("pull", { repo_id: repoId, options });
}

/** Pushes a branch (force / set-upstream per `options`). */
export function pushRepo(
  repoId: RepoId,
  options: PushOptions,
): Promise<NetStats> {
  return call<NetStats>("push", { repo_id: repoId, options });
}

/**
 * Answers a pending `auth-request`: the credentials for `op_id`, or empty
 * (cancel). `store=true` asks the backend to persist them in the keyring.
 */
export function authRespond(
  opId: string,
  username?: string,
  password?: string,
  store = false,
): Promise<void> {
  const args: Record<string, unknown> = { op_id: opId, store };
  if (username !== undefined && username !== "") args.username = username;
  if (password !== undefined && password !== "") args.password = password;
  return call<void>("auth_respond", args);
}

/**
 * Subscribes to `op-progress` events (per-repo serial op queue; see
 * `OpProgress`). Same contract as {@link onRepoChanged}: never throws,
 * no-op unlisten outside Tauri.
 */
export async function onOpProgress(
  cb: (event: OpProgress) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<OpProgress>("op-progress", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

/**
 * Subscribes to `auth-request` events (engine needs credentials for an
 * in-flight network op; answer via {@link authRespond}). Same contract as
 * {@link onRepoChanged}: never throws, no-op unlisten outside Tauri.
 */
export async function onAuthRequest(
  cb: (event: AuthRequest) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<AuthRequest>("auth-request", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

// ---------- M3: power + safety wrappers ----------

/**
 * M3 wrappers funnel through the same injectable transport as the M1/M2
 * commands: `call` honors `setTransport` (tests) and normalizes failures
 * into `IpcError`.
 */
function invokeTauri<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return call<T>(command, args);
}

export async function mergeBranch(
  repoId: string,
  refName: string,
  opts: MergeOptions,
): Promise<MergeResult> {
  return invokeTauri("merge_branch", {
    repo_id: repoId,
    ref_name: refName,
    opts,
  });
}

/** Deletes a tag (alias of {@link tagDelete} with the M3-era name). */
export function tagList(repoId: RepoId): Promise<TagInfo[]> {
  return call<TagInfo[]>("tag_list", { repo_id: repoId });
}

/** Creates a signed annotated tag via the git CLI (`git tag -s`). */
export function tagCreateSigned(
  repoId: RepoId,
  name: string,
  message: string,
  target?: string,
): Promise<void> {
  const args: Record<string, unknown> = { repo_id: repoId, name, message };
  if (target !== undefined && target !== "") args.target = target;
  return call<void>("tag_create_signed", args);
}

/** Remote-tracking branches across all remotes. */
export function remoteBranches(repoId: RepoId): Promise<RemoteBranchInfo[]> {
  return call<RemoteBranchInfo[]>("remote_branches", { repo_id: repoId });
}

/**
 * Creates + checks out a local branch tracking `<remote>/<name>`
 * (`git switch <name>`); returns the local branch name.
 */
export function branchCheckoutRemote(
  repoId: RepoId,
  remote: string,
  name: string,
  newLocal?: string,
): Promise<string> {
  const args: Record<string, unknown> = { repo_id: repoId, remote, name };
  if (newLocal !== undefined && newLocal !== "") args.new_local = newLocal;
  return call<string>("branch_checkout_remote", args);
}

/** Configured external merge tools (pure config read). */
export function mergetoolInfo(repoId: RepoId): Promise<MergetoolInfo> {
  return call<MergetoolInfo>("mergetool_info", { repo_id: repoId });
}

/** Launches `git mergetool` for one conflicted path. */
export function mergetoolRun(
  repoId: RepoId,
  path: string,
  tool?: string,
): Promise<MergetoolResult> {
  const args: Record<string, unknown> = { repo_id: repoId, path };
  if (tool !== undefined && tool !== "") args.tool = tool;
  return call<MergetoolResult>("mergetool_run", args);
}

/** Discards local changes (files or hunks); checkpoints first. */
export function discard(
  repoId: RepoId,
  targets: StageTarget[],
): Promise<void> {
  return call<void>("discard", { repo_id: repoId, targets });
}

// ---------- M10: bisect / describe / autosquash ----------

/** Starts a bisect (bad = HEAD when omitted); HEAD detaches on the probe. */
export function bisectStart(
  repoId: RepoId,
  bad?: string,
  good?: string,
): Promise<BisectState> {
  const args: Record<string, unknown> = { repo_id: repoId };
  if (bad) args.bad = bad;
  if (good) args.good = good;
  return call<BisectState>("bisect_start", args);
}

/** Current bisect state (inactive singleton when none is running). */
export function bisectState(repoId: RepoId): Promise<BisectState> {
  return call<BisectState>("bisect_state", { repo_id: repoId });
}

/** Marks the checked-out probe good/bad/skip; returns the next state. */
export function bisectMark(
  repoId: RepoId,
  mark: BisectMark,
): Promise<BisectState> {
  return call<BisectState>("bisect_mark", { repo_id: repoId, mark });
}

/** Ends the bisect and restores the original branch/HEAD. */
export function bisectReset(repoId: RepoId): Promise<void> {
  return call<void>("bisect_reset", { repo_id: repoId });
}

/** `git describe --tags` for a commit-ish (short-sha fallback). */
export function describe(repoId: RepoId, spec: string): Promise<string> {
  return call<string>("describe", { repo_id: repoId, spec });
}

/** Builds an autosquash plan for `base..HEAD` (feeds `rebaseStart`). */
export function autosquashPlan(
  repoId: RepoId,
  base: string,
): Promise<RebaseStep[]> {
  return call<RebaseStep[]>("autosquash_plan", { repo_id: repoId, base });
}

export async function mergeAbort(repoId: string): Promise<void> {
  return invokeTauri("merge_abort", { repo_id: repoId });
}

export async function sequencerAbort(repoId: string): Promise<void> {
  return invokeTauri("sequencer_abort", { repo_id: repoId });
}

export async function conflicts(repoId: string): Promise<ConflictFile[]> {
  return invokeTauri("conflicts", { repo_id: repoId });
}

export async function conflictResolve(
  repoId: string,
  path: string,
  resolution: ConflictResolution,
  customContent?: Uint8Array,
): Promise<void> {
  return invokeTauri("conflict_resolve", {
    repo_id: repoId,
    path,
    resolution,
    custom_content: customContent ? Array.from(customContent) : undefined,
  });
}

export async function cherryPick(repoId: string, shas: string[]): Promise<MergeResult> {
  return invokeTauri("cherry_pick", { repo_id: repoId, shas });
}

export async function revertCommits(repoId: string, shas: string[]): Promise<MergeResult> {
  return invokeTauri("revert", { repo_id: repoId, shas });
}

export async function resetRepo(repoId: string, kind: ResetKind, to: string): Promise<void> {
  return invokeTauri("reset", { repo_id: repoId, kind, to });
}

export async function rebaseStart(repoId: string, plan: RebaseStep[], onto?: string): Promise<RebaseState> {
  return invokeTauri("rebase_start", { repo_id: repoId, plan, onto: onto ?? undefined });
}

export async function rebaseState(repoId: string): Promise<RebaseState> {
  return invokeTauri("rebase_state", { repo_id: repoId });
}

export async function rebaseContinue(repoId: string): Promise<RebaseState> {
  return invokeTauri("rebase_continue", { repo_id: repoId });
}

export async function rebaseAbort(repoId: string): Promise<void> {
  return invokeTauri("rebase_abort", { repo_id: repoId });
}

export async function stashList(repoId: string): Promise<StashInfo[]> {
  return invokeTauri("stash_list", { repo_id: repoId });
}

export async function stashPush(
  repoId: string,
  message?: string,
  keepIndex = false,
  includeUntracked = true,
): Promise<void> {
  return invokeTauri("stash_push", {
    repo_id: repoId,
    message: message ?? undefined,
    keep_index: keepIndex,
    include_untracked: includeUntracked,
  });
}

export async function stashApply(repoId: string, index: number, pop = false): Promise<void> {
  return invokeTauri("stash_apply", { repo_id: repoId, index, pop });
}

export async function stashDrop(repoId: string, index: number): Promise<void> {
  return invokeTauri("stash_drop", { repo_id: repoId, index });
}

export async function stashBranch(repoId: string, name: string, index: number): Promise<void> {
  return invokeTauri("stash_branch", { repo_id: repoId, name, index });
}

export async function worktrees(repoId: string): Promise<WorktreeInfo[]> {
  return invokeTauri("worktrees", { repo_id: repoId });
}

export async function worktreeAdd(
  repoId: string,
  path: string,
  branch?: string,
  newBranch?: string,
): Promise<void> {
  return invokeTauri("worktree_add", {
    repo_id: repoId,
    path,
    branch: branch ?? undefined,
    new_branch: newBranch ?? undefined,
  });
}

export async function worktreeRemove(repoId: string, name: string, force = false): Promise<void> {
  return invokeTauri("worktree_remove", { repo_id: repoId, name, force });
}

export async function reflog(repoId: string, name?: string): Promise<ReflogEntry[]> {
  return invokeTauri("reflog", { repo_id: repoId, name: name ?? undefined });
}

export async function checkpointCreate(repoId: string, reason: string): Promise<CheckpointInfo> {
  return invokeTauri("checkpoint_create", { repo_id: repoId, reason });
}

export async function checkpoints(repoId: string): Promise<CheckpointInfo[]> {
  return invokeTauri("checkpoints", { repo_id: repoId });
}

export async function checkpointRestore(repoId: string, id: string): Promise<void> {
  return invokeTauri("checkpoint_restore", { repo_id: repoId, id });
}

export async function checkpointGc(repoId: string, olderThanDays: number): Promise<number> {
  return invokeTauri("checkpoint_gc", { repo_id: repoId, older_than_days: olderThanDays });
}

export async function opsPreview(
  repoId: string,
  kind: string,
  params: Record<string, unknown>,
): Promise<PreviewInfo> {
  return invokeTauri("ops_preview", { repo_id: repoId, kind, params });
}

export async function guardCheckpoint(repoId: string, reason: string): Promise<CheckpointInfo> {
  return invokeTauri("guard_checkpoint", { repo_id: repoId, reason });
}

// ---------- M3 E1: conflict editor file access ----------

/**
 * Raw bytes of one workdir file (`path` is workdir-relative, forward or
 * back slashes). A path that escapes the repository root or points at a
 * non-file rejects; a *missing* file resolves to an empty byte array. The
 * conflict editor uses this to load the marker-bearing workdir copy of a
 * conflicted path (there is no raw-file command in the M1 contract).
 */
export function readFile(repoId: string, path: string): Promise<number[]> {
  return invokeTauri<number[]>("repo_read_file", { repo_id: repoId, path });
}

// ---------------------------------------------------------------------------
// M4 (lane F3): submodules + gitignore quick-add (appended).
// ---------------------------------------------------------------------------

import type { SubmoduleInfo } from "./types";

/** A curated builtin `.gitignore` template (`gitignore_templates` reply). */
export interface GitignoreTemplate {
  name: string;
  description: string;
  /** One newline-joined `.gitignore` snippet (no trailing newline). */
  patterns: string;
}

/** Lists every submodule with checked-out vs recorded state. */
export function submodules(repoId: string): Promise<SubmoduleInfo[]> {
  return call<SubmoduleInfo[]>("submodules", { repo_id: repoId });
}

/**
 * `git submodule update [--init] [--recursive] <path>` for one submodule.
 * `init` also initializes uninitialized submodules; `recursive` descends
 * into nested submodules (depth-capped).
 */
export function submoduleUpdate(
  repoId: string,
  path: string,
  init = false,
  recursive = false,
): Promise<void> {
  return call<void>("submodule_update", {
    repo_id: repoId,
    path,
    init,
    recursive,
  });
}

/**
 * Rewrites the submodule URL from `.gitmodules` into the repository config
 * (`git submodule sync`); `path` omitted/empty syncs all submodules.
 */
export function submoduleSync(repoId: string, path?: string): Promise<void> {
  return call<void>("submodule_sync", {
    repo_id: repoId,
    path: path ?? undefined,
  });
}

/** Appends one pattern to the repo-root `.gitignore` (creates if missing). */
export function gitignoreAdd(repoId: string, pattern: string): Promise<void> {
  return call<void>("gitignore_add", { repo_id: repoId, pattern });
}

/** The curated builtin gitignore templates (no repository needed). */
export function gitignoreTemplates(): Promise<GitignoreTemplate[]> {
  return call<GitignoreTemplate[]>("gitignore_templates", {});
}

/** Applies a builtin gitignore template by name (idempotent per line). */
export function gitignoreApplyTemplate(
  repoId: string,
  name: string,
): Promise<void> {
  return call<void>("gitignore_apply_template", { repo_id: repoId, name });
}

// ---------------------------------------------------------------------------
// M4 (lane F4): git clean execution + custom shell actions (appended;
// docs/contracts.md "Commands (M4)" + "Events (M4)").
// ---------------------------------------------------------------------------

import type { ActionOutputEvent } from "./types";

/**
 * Deletes the given workdir-relative untracked paths (as listed by
 * `opsPreview(repoId, "clean", { dirs: true })`). A checkpoint of the full
 * workdir state is created FIRST (`checkpoint_reason`, conventionally
 * "pre-clean"), so the deletion is undoable via checkpoints. Resolves with
 * the number of paths actually removed (paths that vanished in the meantime
 * are skipped).
 */
export function repoClean(
  repoId: string,
  paths: string[],
  checkpointReason: string,
): Promise<number> {
  return invokeTauri<number>("repo_clean", {
    repo_id: repoId,
    paths,
    checkpoint_reason: checkpointReason,
  });
}

/**
 * Runs a user-defined shell action with cwd = the repo's workdir root.
 * Output streams back as `action-output` events (subscribe via
 * {@link onActionOutput}, identify runs by the returned run id). Rejects on
 * validation failure or when the repo already has the maximum number of
 * concurrent actions running.
 */
export function actionRun(
  repoId: string,
  name: string,
  command: string,
): Promise<string> {
  return invokeTauri<string>("action_run", { repo_id: repoId, name, command });
}

/**
 * Kills a running action; resolves `false` when the run is unknown or
 * already finished (cancel is best-effort, never an error).
 */
export function actionCancel(runId: string): Promise<boolean> {
  return invokeTauri<boolean>("action_cancel", { run_id: runId });
}

/**
 * Subscribes to `action-output` events (emitted by an in-flight
 * `action_run`; identify yours by `run_id`). Same contract as
 * {@link onRepoChanged}: never throws, no-op unlisten outside Tauri.
 */
export async function onActionOutput(
  cb: (event: ActionOutputEvent) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<ActionOutputEvent>("action-output", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

// ---------------------------------------------------------------------------
// M6 (lane H1): OS-keyring secrets (appended; src-tauri keyring_store.rs).
// Used exclusively for AI credentials — API keys / server passwords must
// never land in localStorage (see $lib/ai). Secret values are also never
// logged by the backend; error strings describe storage failures only.
// ---------------------------------------------------------------------------

/**
 * Known secret keys used by the app. Keep in lockstep with callers; the
 * backend treats them as opaque usernames under the single `"mygitui"`
 * keyring service.
 */
export const SECRET_KEYS = {
  /** OpenRouter API key (`openrouter.api-key`). */
  openrouterApiKey: "openrouter.api-key",
  /** opencode server basic-auth password (`opencode.server.password`). */
  opencodeServerPassword: "opencode.server.password",
} as const;

/** Reads a secret from the OS keyring; `null` when no entry exists. */
export function secretsGet(key: string): Promise<string | null> {
  return call<string | null>("secrets_get", { key });
}

/** Creates or overwrites a secret in the OS keyring. */
export function secretsSet(key: string, value: string): Promise<void> {
  return call<void>("secrets_set", { key, value });
}

/**
 * Deletes a secret from the OS keyring. Idempotent: deleting a key that
 * does not exist resolves normally.
 */
export function secretsDelete(key: string): Promise<void> {
  return call<void>("secrets_delete", { key });
}

// ---------------------------------------------------------------------------
// M5 (lane G2): terminal PTY (appended; everything above is byte-identical).
//
// One long-lived shell session per repository: `pty_create` spawns the
// process (cwd = the repo workdir) and resolves its session id; output and
// exit stream back as `pty-output` / `pty-exit` events identified by that
// id. Same transport + no-Tauri guards as every other command here (tests
// inject a mock; outside Tauri the commands reject with `IpcError` and the
// event subscriptions resolve with a no-op unlisten).
// ---------------------------------------------------------------------------

/** Payload of the `pty-output` event (raw terminal bytes for one session). */
export interface PtyOutputEvent {
  session_id: string;
  /** Chunk of terminal output (UTF-8 decoded, escape sequences intact). */
  data: string;
}

/** Payload of the `pty-exit` event (the shell process finished). */
export interface PtyExitEvent {
  session_id: string;
  /** Process exit code (`null` when it was killed / could not be read). */
  exit_code: number | null;
}

/**
 * Spawns the shell for `repoId` (cwd = the repository workdir), sized to
 * `rows`×`cols` when given. Resolves with the new pty session id.
 */
export function ptyCreate(
  repoId: string,
  rows?: number,
  cols?: number,
): Promise<string> {
  const args: Record<string, unknown> = { repo_id: repoId };
  if (rows !== undefined) args.rows = rows;
  if (cols !== undefined) args.cols = cols;
  return call<string>("pty_create", args);
}

/** Writes raw keystroke bytes to a pty session. */
export function ptyWrite(sessionId: string, data: string): Promise<void> {
  return call<void>("pty_write", { session_id: sessionId, data });
}

/** Notifies the backend that the terminal was resized. */
export function ptyResize(
  sessionId: string,
  rows: number,
  cols: number,
): Promise<void> {
  return call<void>("pty_resize", { session_id: sessionId, rows, cols });
}

/**
 * Kills a pty session (the whole process tree). Resolves even when the
 * session is unknown or already gone (cancel is best-effort, never an
 * error).
 */
export function ptyKill(sessionId: string): Promise<void> {
  return call<void>("pty_kill", { session_id: sessionId });
}

/**
 * Subscribes to `pty-output` events (identify yours by `session_id`). Same
 * contract as {@link onRepoChanged}: never throws, no-op unlisten outside
 * Tauri.
 */
export async function onPtyOutput(
  cb: (event: PtyOutputEvent) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<PtyOutputEvent>("pty-output", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

/**
 * Subscribes to `pty-exit` events (the shell process for `session_id`
 * finished). Same contract as {@link onRepoChanged}: never throws, no-op
 * unlisten outside Tauri.
 */
export async function onPtyExit(
  cb: (event: PtyExitEvent) => void,
): Promise<() => void> {
  const noop = (): void => {};
  if (!isTauri()) return noop;
  try {
    return await listen<PtyExitEvent>("pty-exit", (event) =>
      cb(event.payload),
    );
  } catch {
    return noop;
  }
}

// ---------------------------------------------------------------------------
// M6 (lane H2): forge — gh-assisted GitHub flow (appended;
// docs/contracts.md "Commands (M6 forge)"). All GitHub traffic goes through
// the user's `gh` CLI backend-side; these wrappers mirror `forge.rs`.
// ---------------------------------------------------------------------------

/** Result of `forge_status`: is the user's `gh` CLI usable at all? */
export interface ForgeStatus {
  /** `gh` was found on PATH and ran. */
  available: boolean;
  /** Parsed `gh --version` number ("2.63.3"); empty when unavailable. */
  version: string;
  /** `gh auth status` exited 0 (only meaningful when `available`). */
  authed: boolean;
}

/** Owner/repo/branch context of one open repository (`forge_context`). */
export interface ForgeContext {
  owner: string;
  repo: string;
  /** Current branch (backend errors on detached HEAD instead). */
  branch: string;
  /** The `origin` URL the pair was derived from. */
  remote_url: string;
}

/** `pr_create` success payload. */
export interface PrCreated {
  url: string;
  number: number;
}

/** One PR of `pr_list` (`state`: verbatim gh value OPEN/MERGED/CLOSED). */
export interface PrInfo {
  number: number;
  title: string;
  head_ref_name: string;
  base_ref_name: string;
  state: string;
  is_draft: boolean;
  url: string;
  created_at: string | null;
}

/** Canonical CI check state (`pr_checks`). */
export type CheckState = "pass" | "fail" | "pending" | "skipping";

/** One CI check of `pr_checks`. */
export interface CheckInfo {
  name: string;
  state: CheckState;
}

/** `pr_create` rejection kinds (contracts.md `ForgeError.kind`). */
export type ForgeFailureKind = "NoGh" | "NotAuthed" | "AlreadyExists" | "Other";

/** Structured `pr_create` rejection (serialized object, not a string). */
export interface ForgeFailure {
  kind: ForgeFailureKind;
  message: string;
  /** For `AlreadyExists`: the existing PR's URL when parseable. */
  url: string | null;
}

/** Type guard for the serialized `ForgeError` rejection value. */
export function isForgeFailure(err: unknown): err is ForgeFailure {
  return (
    typeof err === "object" &&
    err !== null &&
    typeof (err as { kind?: unknown }).kind === "string" &&
    typeof (err as { message?: unknown }).message === "string"
  );
}

/** {@link IpcError} carrying the structured forge failure fields. */
export class ForgeIpcError extends IpcError {
  readonly kind: ForgeFailureKind;
  readonly url: string | null;

  constructor(command: string, failure: ForgeFailure) {
    super(command, failure.message);
    this.name = "ForgeIpcError";
    this.kind = failure.kind;
    this.url = failure.url;
  }
}

/**
 * `call` variant that preserves the structured `ForgeError` rejection of the
 * forge commands: a rejection shaped `{kind, message, url}` becomes a
 * {@link ForgeIpcError}; anything else normalizes to a plain `IpcError`.
 */
async function callForge<T>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  const transport = transportOverride ?? defaultTransport;
  try {
    return (await transport(command, args)) as T;
  } catch (err) {
    if (isForgeFailure(err)) throw new ForgeIpcError(command, err);
    throw toIpcError(command, err);
  }
}

/** Detects `gh` on PATH and whether the user is authenticated. */
export function forgeStatus(): Promise<ForgeStatus> {
  return call<ForgeStatus>("forge_status", {});
}

/** Owner/repo/branch context of one open repo (errors on non-GitHub origin). */
export function forgeContext(repoId: RepoId): Promise<ForgeContext> {
  return call<ForgeContext>("forge_context", { repo_id: repoId });
}

/**
 * Creates a PR via `gh pr create`. Rejects with {@link ForgeIpcError};
 * `kind "AlreadyExists"` carries the existing PR's URL.
 */
export function prCreate(
  repoId: RepoId,
  base: string,
  title: string,
  body: string,
  draft: boolean,
): Promise<PrCreated> {
  return callForge<PrCreated>("pr_create", {
    repo_id: repoId,
    base,
    title,
    body,
    draft,
  });
}

/** Open PRs of the repo (`gh pr list --json … --limit 50`). */
export function prList(repoId: RepoId): Promise<PrInfo[]> {
  return call<PrInfo[]>("pr_list", { repo_id: repoId });
}

/** CI checks of one PR, states canonicalized to pass/fail/pending/skipping. */
export function prChecks(repoId: RepoId, number: number): Promise<CheckInfo[]> {
  return call<CheckInfo[]>("pr_checks", { repo_id: repoId, number });
}

// ---------------------------------------------------------------------------
// M7 (stats lane): contribution statistics (contracts.md "Commands (M7)").
// Pure reads on the backend; logic in `src-tauri/src/engine/stats.rs`.
// ---------------------------------------------------------------------------

/** One day of commit activity (`day` is "YYYY-MM-DD", committer local date). */
export interface DayCount {
  day: string;
  count: number;
}

/** One author identity aggregated over the stats window. */
export interface Contributor {
  name: string;
  email: string;
  count: number;
  /** First active day, "YYYY-MM-DD". */
  first_day: string;
  /** Last active day, "YYYY-MM-DD". */
  last_day: string;
}

/**
 * Commits per local day inside the `maxDays` window, day ascending; days
 * with zero commits are omitted. `author` (when set) is a case-insensitive
 * substring matched against the author name OR email.
 */
export function commitActivity(
  repoId: RepoId,
  maxDays: number,
  author?: string | null,
): Promise<DayCount[]> {
  return call<DayCount[]>("commit_activity", {
    repo_id: repoId,
    max_days: maxDays,
    author: author ?? null,
  });
}

/** Per-author rollups (identity = exact name+email pair), count descending. */
export function contributorStats(
  repoId: RepoId,
  maxDays: number,
): Promise<Contributor[]> {
  return call<Contributor[]>("contributor_stats", {
    repo_id: repoId,
    max_days: maxDays,
  });
}

// ---------------------------------------------------------------------------
// M12: signature verify / branch trash / worktree prune / bisect log / ops
// ---------------------------------------------------------------------------

/**
 * Verification status of one commit's signature (`git verify-commit`).
 * `signed: false` for unsigned commits; `valid: null` when signed but not
 * verifiable here (missing key / no `gpg.ssh.allowedSignersFile`).
 */
export function commitSignature(
  repoId: RepoId,
  sha: string,
): Promise<CommitSignature> {
  return call<CommitSignature>("commit_signature", { repo_id: repoId, sha });
}

/** Branches preserved under `refs/mygitui/trash/*`, newest first. */
export function branchTrashList(repoId: RepoId): Promise<BranchTrashEntry[]> {
  return call<BranchTrashEntry[]>("branch_trash_list", { repo_id: repoId });
}

/** Restore a trashed branch (optionally renamed); returns the branch name. */
export function branchTrashRestore(
  repoId: RepoId,
  id: string,
  newName?: string,
): Promise<string> {
  return call<string>("branch_trash_restore", {
    repo_id: repoId,
    id,
    new_name: newName ?? null,
  });
}

/** `git worktree prune` — returns the number of pruned worktrees. */
export function worktreePrune(repoId: RepoId): Promise<number> {
  return call<number>("worktree_prune", { repo_id: repoId });
}

/** Recorded mark history for a live bisect, oldest first. */
export function bisectLog(repoId: RepoId): Promise<BisectLogEntry[]> {
  return call<BisectLogEntry[]>("bisect_log", { repo_id: repoId });
}

/**
 * Best-effort cancel of a queued/running mutation op. Queued ops are
 * dropped; running ops stop at the next cooperative checkpoint. Returns
 * false when the op already finished.
 */
export function opCancel(repoId: RepoId, opId: string): Promise<boolean> {
  return call<boolean>("op_cancel", { repo_id: repoId, op_id: opId });
}

/**
 * argv captured at first launch (`mygitui <path>`), consumed once.
 * Second-launch arguments arrive via the `cli-args` event instead.
 */
export async function cliArgsInitial(): Promise<string[]> {
  if (!isTauri()) return [];
  return call<string[]>("cli_args_initial", {});
}
