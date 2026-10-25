/**
 * Shared network verbs for repository sync entry points.
 *
 * The header quick actions and the command palette both fetch/pull/push, so
 * remote selection and option construction live here exactly once. The
 * module is plain TypeScript: it emits no toasts and reads no UI state, which
 * keeps the mapping testable and lets each surface phrase progress/success
 * for its own context.
 *
 * Preconditions are returned as {@link SyncOutcome} skips instead of thrown
 * errors: "no remote configured" / "detached HEAD" are ordinary repo states,
 * not failures. Backend/network errors still reject and belong to the caller.
 */

import { fetchRepo, pullRepo, pushRepo, remotes } from "$lib/ipc/client";
import type {
  FetchOptions,
  NetStats,
  PullOptions,
  PushOptions,
  RemoteInfo,
  RepoId,
} from "$lib/ipc/types";

/** The three sync verbs the quick actions expose. */
export type SyncVerb = "fetch" | "pull" | "push";

/** A sync that ran to completion. */
export interface SyncDone {
  kind: "done";
  /** Remote that was used (the repo's primary remote). */
  remote: string;
  stats: NetStats;
}

/** Why a sync was not attempted. */
export type SyncSkipReason = "no-remote" | "no-branch";

/** A sync that was skipped before touching the network. */
export interface SyncSkipped {
  kind: "skipped";
  reason: SyncSkipReason;
}

/** Result of a sync attempt: completed or precondition-skipped. */
export type SyncOutcome = SyncDone | SyncSkipped;

/**
 * Quick fetches prune deleted remote refs, matching the Remotes panel's
 * default (the palette's plain fetch command passes `false` explicitly).
 */
export const QUICK_FETCH_PRUNE = true;

/**
 * Picks the remote quick actions target: `origin` when configured, else the
 * first remote. `null` means the repo has no remote at all.
 */
export function primaryRemoteName(
  configured: readonly RemoteInfo[],
): string | null {
  return (
    configured.find((remote) => remote.name === "origin") ?? configured[0]
  )?.name ?? null;
}

/** Fetch options for one remote (`refs: []` = every ref). */
export function fetchOptions(remote: string, prune: boolean): FetchOptions {
  return { remote, prune, refs: [], depth: null };
}

/** Safe pull default: fast-forward only, no rebase. */
export function pullOptions(remote: string, branch: string): PullOptions {
  return { remote, branch, ff_only: true, rebase: false };
}

/** Plain single-branch push: no force, no upstream changes, no deletes/tags. */
export function pushOptions(remote: string, branch: string): PushOptions {
  return {
    remote,
    branch,
    force: false,
    force_with_lease: false,
    set_upstream: false,
    refs: [],
    tags: false,
    delete: false,
  };
}

/** Resolves the repo's primary remote name, or `null` when none is set. */
export async function primaryRemote(repoId: RepoId): Promise<string | null> {
  return primaryRemoteName(await remotes(repoId));
}

/**
 * Fetches the repo's primary remote. Skips with `no-remote` instead of
 * erroring when the repository has no remote configured.
 */
export async function fetchDefaultRemote(
  repoId: RepoId,
  prune: boolean = QUICK_FETCH_PRUNE,
): Promise<SyncOutcome> {
  const remote = await primaryRemote(repoId);
  if (remote === null) return { kind: "skipped", reason: "no-remote" };
  const stats = await fetchRepo(repoId, fetchOptions(remote, prune));
  return { kind: "done", remote, stats };
}

/**
 * Pulls `branch` from the repo's primary remote (ff-only). Detached HEAD /
 * an unknown branch and a missing remote are returned as skips.
 */
export async function pullCurrentBranch(
  repoId: RepoId,
  branch: string | null | undefined,
): Promise<SyncOutcome> {
  if (!branch) return { kind: "skipped", reason: "no-branch" };
  const remote = await primaryRemote(repoId);
  if (remote === null) return { kind: "skipped", reason: "no-remote" };
  const stats = await pullRepo(repoId, pullOptions(remote, branch));
  return { kind: "done", remote, stats };
}

/**
 * Pushes `branch` to the repo's primary remote. Detached HEAD / an unknown
 * branch and a missing remote are returned as skips.
 */
export async function pushCurrentBranch(
  repoId: RepoId,
  branch: string | null | undefined,
): Promise<SyncOutcome> {
  if (!branch) return { kind: "skipped", reason: "no-branch" };
  const remote = await primaryRemote(repoId);
  if (remote === null) return { kind: "skipped", reason: "no-remote" };
  const stats = await pushRepo(repoId, pushOptions(remote, branch));
  return { kind: "done", remote, stats };
}

/** "up to date" / "1 ref updated" / "3 refs updated" for a completed sync. */
export function netStatsSummary(stats: NetStats): string {
  const count = stats.updated_refs.length;
  if (count === 0) return "up to date";
  return `${count} ref${count === 1 ? "" : "s"} updated`;
}

/**
 * Success toast text for a completed sync. `branch` is only used by
 * pull/push; fetch names just the remote.
 */
export function syncSuccessMessage(
  verb: SyncVerb,
  done: SyncDone,
  branch: string | null = null,
): string {
  const refs = netStatsSummary(done.stats);
  switch (verb) {
    case "fetch":
      return `Fetched ${done.remote} (${refs})`;
    case "pull":
      return `Pulled ${branch ? `${done.remote}/${branch}` : done.remote} (${refs})`;
    case "push":
      return `Pushed ${branch ? `${branch} to ` : ""}${done.remote} (${refs})`;
  }
}

/** Human-readable reason when a sync was skipped before any network I/O. */
export function syncSkipMessage(
  reason: SyncSkipReason,
  verb: SyncVerb,
): string {
  return reason === "no-remote"
    ? "No remote configured for this repository"
    : `No current branch to ${verb}`;
}
