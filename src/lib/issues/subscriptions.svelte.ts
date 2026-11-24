/**
 * Visible-only Issues subscription coordinator.
 *
 * One PollScheduler lane per subscribed open repository, durable root-keyed
 * baselines/cursors/seen IDs/outbox, active-first coalesced catch-up, and a
 * shared per-executor rate-limit gate. Repo eviction removes only runtime
 * state/lanes; durable preferences and sync state deliberately survive.
 */

import { describeError } from "$lib/errors";
import { registerRepoEvictor } from "$lib/state/repoEvictors";
import type { StorageLike } from "$lib/state/persist";
import { getActiveId, getTabs, repoRootFor } from "$lib/stores/tabs.svelte";
import {
  PollScheduler,
  pollScheduler,
  type Clock,
  type VisibilitySource,
} from "$lib/stores/pollScheduler.svelte";
import {
  IssueActivityStore,
  boundIssueSyncState,
  defaultIssueSyncState,
  defaultSyncStorage,
  issueActivity,
  readIssueSyncState,
  routeIssueActivityEvent,
  writeIssueSyncState,
  type IssueSyncError,
  type IssueSyncPhase,
  type IssueSyncState,
} from "./activity.svelte";
import { activityOverlapCursor, commitActivityScan } from "./activityScan";
import {
  issueDisplayName,
  issueExecutorScope,
  issueIdentityScope,
  type IssueIdentity,
  type IssueTransport,
  issuesTransport,
} from "./model";
import { normalizeIssueRoot } from "./prefs";

export const ACTIVE_ISSUE_POLL_MS = 60_000;
export const INACTIVE_ISSUE_POLL_MS = 5 * 60_000;
export const SUBSCRIPTION_IDENTITY_TTL_MS = 5 * 60_000;

export interface SubscriptionPatch {
  newIssues?: boolean;
  newComments?: boolean;
}

export interface SubscriptionTab {
  id: string;
  root: string;
  deviceId?: string | null;
}

export interface SubscriptionTabsSource {
  list(): SubscriptionTab[];
  activeId(): string | null;
  rootFor(repoId: string): string | undefined;
}

export interface SubscriptionEventTarget {
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

export interface SubscriptionRateGate {
  run<T>(scope: string, work: () => Promise<T>): Promise<T>;
}

export interface SubscriptionCoordinatorOptions {
  tabs?: SubscriptionTabsSource;
  scheduler?: PollScheduler;
  transport?: IssueTransport;
  activity?: IssueActivityStore;
  storage?: StorageLike | null;
  visibility?: VisibilitySource;
  eventTarget?: SubscriptionEventTarget | null;
  gate?: SubscriptionRateGate;
  clock?: Clock;
  now?: () => number;
}

interface Lane {
  root: string;
  intervalMs: number;
  unsubscribe: () => void;
}

class HiddenSubscription extends Error {}
class IssueStorageError extends Error {}

function defaultVisibility(): VisibilitySource {
  return {
    isHidden: () =>
      typeof document === "undefined" ? false : document.visibilityState === "hidden",
  };
}

function defaultEventTarget(): SubscriptionEventTarget | null {
  return typeof document === "undefined" ? null : document;
}

function defaultClock(): Clock {
  return {
    setTimeout: (handler, ms) => setTimeout(handler, ms),
    clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
  };
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
}

function numeric(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim() !== "" && Number.isFinite(Number(value))) {
    return Number(value);
  }
  return null;
}

/** Retry-After seconds/HTTP-date or GitHub reset epoch, in milliseconds. */
export function rateLimitDelayMs(error: unknown, now = Date.now()): number {
  const root = record(error);
  if (root === null) return 0;
  const nested = record(root.failure) ?? record(root.detail);
  const source = nested ?? root;
  const status = numeric(source.status ?? source.status_code ?? source.http_status ?? root.status);
  const kind = typeof source.kind === "string" ? source.kind : typeof root.kind === "string" ? root.kind : "";
  if (kind !== "RateLimited" && status !== 403 && status !== 429) return 0;

  let retry = 0;
  const retryRaw = source.retry_after ?? source.retryAfter ?? source.retry_after_seconds;
  const retryNumber = numeric(retryRaw);
  if (retryNumber !== null) retry = retryNumber * 1_000;
  else if (typeof retryRaw === "string") retry = Math.max(0, Date.parse(retryRaw) - now);

  let reset = 0;
  const resetRaw =
    source.rate_limit_reset ??
    source.rate_limit_reset_at ??
    source.rateLimitReset ??
    source.reset_at ??
    source.resetAt ??
    root.rate_limit_reset;
  const resetNumber = numeric(resetRaw);
  if (resetNumber !== null) {
    const epochMs = resetNumber < 1_000_000_000_000 ? resetNumber * 1_000 : resetNumber;
    reset = Math.max(0, epochMs - now);
  } else if (typeof resetRaw === "string") reset = Math.max(0, Date.parse(resetRaw) - now);

  const delay = Math.max(retry, reset, 0);
  return Number.isFinite(delay) ? delay : 0;
}

/** Serializes work per executor/device and honors a shared reset deadline. */
export class ExecutorRateLimitGate implements SubscriptionRateGate {
  #tails = new Map<string, Promise<void>>();
  #blockedUntil = new Map<string, number>();
  #now: () => number;
  #clock: Clock;

  constructor(options: { now?: () => number; clock?: Clock } = {}) {
    this.#now = options.now ?? Date.now;
    this.#clock = options.clock ?? defaultClock();
  }

  async run<T>(scope: string, work: () => Promise<T>): Promise<T> {
    const previous = this.#tails.get(scope) ?? Promise.resolve();
    const result = previous.catch(() => {}).then(async () => {
      const blocked = this.#blockedUntil.get(scope) ?? 0;
      const wait = blocked - this.#now();
      if (wait > 0) await new Promise<void>((resolve) => this.#clock.setTimeout(resolve, wait));
      return work();
    });
    const tail = result.then(
      () => {},
      () => {},
    );
    this.#tails.set(scope, tail);
    try {
      return await result;
    } catch (error) {
      const delay = rateLimitDelayMs(error, this.#now());
      if (delay > 0) {
        this.#blockedUntil.set(scope, Math.max(this.#blockedUntil.get(scope) ?? 0, this.#now() + delay));
      }
      throw error;
    } finally {
      if (this.#tails.get(scope) === tail) this.#tails.delete(scope);
    }
  }
}

/** Application-wide gate; the scope key keeps executors/devices independent. */
export const executorRateLimitGate = new ExecutorRateLimitGate();

export function issueLaneKey(repoId: string): string {
  return `issues:${encodeURIComponent(repoId)}`;
}

function classifyFailure(error: unknown): { phase: IssueSyncPhase; lastError: IssueSyncError } {
  const value = record(error);
  const kind = typeof value?.kind === "string" ? value.kind : "";
  const status = numeric(value?.status ?? value?.status_code ?? value?.http_status);
  if (kind === "NotAuthed" || status === 401) return { phase: "auth-error", lastError: "auth-error" };
  if (kind === "Offline") return { phase: "offline", lastError: "offline" };
  if (kind === "RateLimited" || status === 403 || status === 429 || rateLimitDelayMs(error) > 0) {
    return { phase: "paused", lastError: "rate-limited" };
  }
  return { phase: "paused", lastError: "request-failed" };
}

export class IssueSubscriptionCoordinator {
  #tabs: SubscriptionTabsSource;
  #scheduler: PollScheduler;
  #transport: IssueTransport;
  #activity: IssueActivityStore;
  #storage: StorageLike | null;
  #visibility: VisibilitySource;
  #eventTarget: SubscriptionEventTarget | null;
  #gate: SubscriptionRateGate;
  #clock: Clock;
  #now: () => number;

  #started = false;
  #unregisterEvictor: (() => void) | null = null;
  #removeListeners: (() => void) | null = null;
  #lanes = new Map<string, Lane>();
  #repoRoots = new Map<string, string>();
  #generations = new Map<string, number>();
  #identityCache = new Map<string, { identity: IssueIdentity; expiresAt: number }>();
  #pollRuns = new Map<string, Promise<void>>();
  #continuations = new Map<string, unknown>();
  #storageErrors = new Set<string>();
  #manual = new Set<string>();
  #queue = new Set<string>();
  #draining: Promise<void> | null = null;
  #states = $state<Record<string, IssueSyncState>>({});

  constructor(options: SubscriptionCoordinatorOptions = {}) {
    this.#tabs = options.tabs ?? { list: getTabs, activeId: getActiveId, rootFor: repoRootFor };
    this.#scheduler = options.scheduler ?? pollScheduler;
    this.#transport = options.transport ?? issuesTransport;
    this.#activity = options.activity ?? issueActivity;
    this.#storage = "storage" in options ? (options.storage ?? null) : defaultSyncStorage();
    this.#visibility = options.visibility ?? defaultVisibility();
    this.#eventTarget = "eventTarget" in options ? options.eventTarget ?? null : defaultEventTarget();
    this.#gate = options.gate ?? executorRateLimitGate;
    this.#clock = options.clock ?? defaultClock();
    this.#now = options.now ?? Date.now;
  }

  /** Idempotent application-start entry point. */
  start(): void {
    if (this.#started) return;
    this.#started = true;
    this.#unregisterEvictor = registerRepoEvictor((repoId) => this.evict(repoId));
    if (this.#eventTarget !== null) {
      const onVisibility = (): void => this.#onVisible();
      const onFocus = (): void => this.#onVisible();
      this.#eventTarget.addEventListener("visibilitychange", onVisibility);
      this.#eventTarget.addEventListener("focus", onFocus);
      this.#removeListeners = () => {
        this.#eventTarget?.removeEventListener("visibilitychange", onVisibility);
        this.#eventTarget?.removeEventListener("focus", onFocus);
      };
    }
    this.syncLanes();
    void this.catchUp();
  }

  /** Reconciles open tabs, enablement, and active/inactive cadence. */
  syncLanes(): void {
    if (!this.#started) return;
    const desired = new Map<string, SubscriptionTab>();
    for (const tab of this.#tabs.list()) {
      if (desired.has(tab.id)) continue;
      const root = normalizeIssueRoot(tab.root);
      if (root === "") continue;
      desired.set(tab.id, { ...tab, root });
      this.#repoRoots.set(tab.id, root);
      this.#loadState(root);
    }

    for (const [repoId, lane] of [...this.#lanes]) {
      const tab = desired.get(repoId);
      const state = tab === undefined ? null : this.#loadState(tab.root);
      if (tab === undefined || state === null || (!state.newIssues && !state.newComments) || lane.root !== tab.root) {
        this.#removeLane(repoId);
      }
    }

    const activeId = this.#tabs.activeId();
    for (const [repoId, tab] of desired) {
      const state = this.#loadState(tab.root);
      if (!state.newIssues && !state.newComments) continue;
      const intervalMs = repoId === activeId ? ACTIVE_ISSUE_POLL_MS : INACTIVE_ISSUE_POLL_MS;
      const current = this.#lanes.get(repoId);
      if (current?.root === tab.root && current.intervalMs === intervalMs) continue;
      this.#removeLane(repoId);
      const unsubscribe = this.#scheduler.register(issueLaneKey(repoId), {
        intervalMs,
        work: () => this.#poll(repoId, tab.root, this.#manual.has(repoId)),
      });
      this.#lanes.set(repoId, { root: tab.root, intervalMs, unsubscribe });
    }
  }

  /** Visible, coalesced catch-up; the active repository is always first. */
  catchUp(): Promise<void> {
    if (!this.#started || this.#visibility.isHidden()) return Promise.resolve();
    this.syncLanes();
    const activeId = this.#tabs.activeId();
    const ordered = [...this.#lanes.keys()].sort((a, b) => {
      if (a === activeId) return -1;
      if (b === activeId) return 1;
      return this.#tabs.list().findIndex((tab) => tab.id === a) - this.#tabs.list().findIndex((tab) => tab.id === b);
    });
    for (const repoId of ordered) this.#queue.add(repoId);
    if (this.#draining !== null) return this.#draining;
    const draining = this.#drainQueue();
    this.#draining = draining;
    void draining.finally(() => {
      if (this.#draining === draining) this.#draining = null;
    });
    return draining;
  }

  /** Manual retry/catch-up. Hidden windows intentionally perform no work. */
  async refresh(repoId: string): Promise<void> {
    if (!this.#started || this.#visibility.isHidden()) return;
    const root = this.#tabs.rootFor(repoId);
    if (root === undefined) return;
    const key = normalizeIssueRoot(root);
    const state = this.#loadState(key);
    if (!state.newIssues && !state.newComments) return;
    this.#storageErrors.delete(key);
    if (state.phase === "storage-error") {
      const retry = {
        ...state,
        phase: state.baselineIssues || state.baselineComments ? "baseline-pending" as const : "ready" as const,
        lastError: null,
      };
      try {
        this.#save(key, retry);
      } catch {
        this.#states = { ...this.#states, [key]: { ...state, phase: "storage-error", lastError: "storage-error" } };
        this.#storageErrors.add(key);
        this.#activity.warnStorage(key);
        return;
      }
    }
    this.syncLanes();
    if (!this.#lanes.has(repoId)) return;
    // A startup/focus catch-up may already own this lane. Wait for it so a
    // user-initiated refresh is not silently dropped by PollScheduler.
    const existing = this.#pollRuns.get(repoId);
    if (existing !== undefined) await existing.catch(() => {});
    if (!this.#started || this.#visibility.isHidden() || !this.#lanes.has(repoId)) return;
    this.#manual.add(repoId);
    try {
      await this.#scheduler.refresh(issueLaneKey(repoId));
    } finally {
      this.#manual.delete(repoId);
    }
  }

  /** Persists independent toggles; first enable establishes a fresh baseline. */
  async setSubscription(repoId: string, patch: SubscriptionPatch): Promise<void> {
    const rawRoot = this.#tabs.rootFor(repoId);
    if (rawRoot === undefined) return;
    const root = normalizeIssueRoot(rawRoot);
    const prior = this.#loadState(root);
    const wasOff = !prior.newIssues && !prior.newComments;
    const next: IssueSyncState = {
      ...prior,
      newIssues: typeof patch.newIssues === "boolean" ? patch.newIssues : prior.newIssues,
      newComments: typeof patch.newComments === "boolean" ? patch.newComments : prior.newComments,
    };
    const nowOff = !next.newIssues && !next.newComments;
    if (wasOff && !nowOff) {
      next.phase = "baseline-pending";
      next.identity = null;
      next.issueCursor = null;
      next.commentCursor = null;
      next.seenIssueIds = [];
      next.seenCommentIds = [];
      next.baselineIssues = true;
      next.baselineComments = true;
      next.lastError = null;
    } else if (nowOff) {
      next.phase = "paused";
      next.baselineIssues = false;
      next.baselineComments = false;
    }
    try {
      this.#save(root, next);
    } catch {
      this.#states = { ...this.#states, [root]: { ...next, phase: "storage-error", lastError: "storage-error" } };
      this.#storageErrors.add(root);
      this.#activity.warnStorage(root);
      throw new IssueStorageError("Issue subscription preferences could not be saved");
    }
    this.#storageErrors.delete(root);
    this.syncLanes();
    if (!nowOff && !this.#visibility.isHidden()) void this.catchUp();
  }

  /** Current durable snapshot hydrated for an open root (reactive). */
  state(root: string): IssueSyncState {
    const key = normalizeIssueRoot(root);
    return this.#states[key] ?? defaultIssueSyncState();
  }

  lane(repoId: string) {
    return this.#scheduler.lanes[issueLaneKey(repoId)];
  }

  /** Stops timers/listeners and invalidates in-flight work; durability remains. */
  stop(): void {
    if (!this.#started) return;
    this.#started = false;
    this.#removeListeners?.();
    this.#removeListeners = null;
    this.#unregisterEvictor?.();
    this.#unregisterEvictor = null;
    for (const repoId of [...this.#lanes.keys()]) {
      this.#generations.set(repoId, (this.#generations.get(repoId) ?? 0) + 1);
      this.#removeLane(repoId);
    }
    for (const timer of this.#continuations.values()) this.#clock.clearTimeout(timer);
    this.#continuations.clear();
    this.#queue.clear();
    this.#identityCache.clear();
    this.#repoRoots.clear();
    this.#manual.clear();
    this.#storageErrors.clear();
    this.#states = {};
  }

  /** Repo close/tab close: drop caches and lane only, never durable state. */
  evict(repoId: string): void {
    this.#generations.set(repoId, (this.#generations.get(repoId) ?? 0) + 1);
    this.#removeLane(repoId);
    this.#queue.delete(repoId);
    this.#manual.delete(repoId);
    this.#identityCache.delete(repoId);
    const timer = this.#continuations.get(repoId);
    if (timer !== undefined) {
      this.#clock.clearTimeout(timer);
      this.#continuations.delete(repoId);
    }
    const root = this.#repoRoots.get(repoId);
    this.#repoRoots.delete(repoId);
    if (root !== undefined) {
      const states = { ...this.#states };
      delete states[root];
      this.#states = states;
    }
  }

  #onVisible(): void {
    if (!this.#started || this.#visibility.isHidden()) return;
    this.syncLanes();
    void this.catchUp();
  }

  #loadState(root: string): IssueSyncState {
    const key = normalizeIssueRoot(root);
    const cached = this.#states[key];
    if (cached !== undefined) return cached;
    const loaded = boundIssueSyncState(readIssueSyncState(this.#storage, key), this.#now());
    this.#states = { ...this.#states, [key]: loaded };
    return loaded;
  }

  #reloadState(root: string): IssueSyncState {
    const loaded = boundIssueSyncState(readIssueSyncState(this.#storage, root), this.#now());
    this.#states = { ...this.#states, [root]: loaded };
    return loaded;
  }

  #save(root: string, state: IssueSyncState): void {
    const next = boundIssueSyncState(state, this.#now());
    try {
      writeIssueSyncState(this.#storage, root, next, this.#now());
    } catch (error) {
      throw new IssueStorageError(describeError(error));
    }
    this.#states = { ...this.#states, [root]: next };
    this.#activity.applyPersisted(root, next);
    this.#storageErrors.delete(root);
  }

  #removeLane(repoId: string): void {
    const lane = this.#lanes.get(repoId);
    if (lane !== undefined) lane.unsubscribe();
    this.#lanes.delete(repoId);
    this.#scheduler.unregister(issueLaneKey(repoId));
  }

  async #drainQueue(): Promise<void> {
    while (this.#started && this.#queue.size > 0 && !this.#visibility.isHidden()) {
      const activeId = this.#tabs.activeId();
      const repoId = [...this.#queue].sort((a, b) => {
        if (a === activeId) return -1;
        if (b === activeId) return 1;
        return 0;
      })[0]!;
      // Keep the entry queued until its run settles so concurrent catch-up
      // calls coalesce with work that is already in flight.
      await this.#scheduler.refresh(issueLaneKey(repoId));
      this.#queue.delete(repoId);
    }
  }

  #poll(repoId: string, root: string, manual: boolean): Promise<void> {
    const existing = this.#pollRuns.get(repoId);
    if (existing !== undefined) return existing;
    const run = this.#runPoll(repoId, root, manual).finally(() => {
      if (this.#pollRuns.get(repoId) === run) this.#pollRuns.delete(repoId);
    });
    this.#pollRuns.set(repoId, run);
    return run;
  }

  async #runPoll(repoId: string, root: string, manual: boolean): Promise<void> {
    const generation = this.#generations.get(repoId) ?? 0;
    const tab = this.#tabs.list().find((entry) => entry.id === repoId);
    if (!this.#started || this.#visibility.isHidden() || tab === undefined || normalizeIssueRoot(tab.root) !== root) {
      throw new HiddenSubscription();
    }
    let prior = this.#loadState(root);
    if (!prior.newIssues && !prior.newComments) return;
    if (prior.phase === "storage-error" && !manual) throw new IssueStorageError("Issue activity storage needs a retry");

    try {
      const identity = await this.#identity(repoId, tab, root, generation);
      if (!this.#current(repoId, root, generation)) return;
      const identityKey = issueIdentityScope(identity);
      if (prior.identity === null || issueIdentityScope(prior.identity) !== identityKey) {
        prior = {
          ...prior,
          identity,
          phase: "baseline-pending",
          issueCursor: null,
          commentCursor: null,
          seenIssueIds: [],
          seenCommentIds: [],
          baselineIssues: true,
          baselineComments: true,
          lastSuccessAt: null,
          lastError: null,
          outbox: [],
        };
        this.#save(root, prior);
      } else {
        // Pick up mark-read/local-mutation writes made outside this coordinator.
        prior = this.#loadState(root);
      }

      const issueSince = prior.baselineIssues && prior.issueCursor === null
        ? null
        : activityOverlapCursor(prior.issueCursor);
      const commentSince = prior.baselineComments && prior.commentCursor === null
        ? null
        : activityOverlapCursor(prior.commentCursor);
      const batch = await this.#gate.run(issueExecutorScope(identity), async () => {
        if (!this.#current(repoId, root, generation)) throw new HiddenSubscription();
        return this.#transport.issuesActivitySince(repoId, issueSince, commentSince);
      });
      if (!this.#current(repoId, root, generation)) return;
      // Merge mark-read/local-mutation writes that landed during the request.
      prior = this.#reloadState(root);

      const result = commitActivityScan(prior, batch, {
        now: this.#now(),
        baselineIssues: prior.baselineIssues,
        baselineComments: prior.baselineComments,
        persist: (next) => this.#save(root, next),
      });
      if (!result.persisted) {
        this.#states = { ...this.#states, [root]: { ...prior, phase: "storage-error", lastError: "storage-error" } };
        this.#storageErrors.add(root);
        this.#activity.warnStorage(root);
        throw new IssueStorageError("Issue activity cursor could not be saved");
      }
      if (result.events.length > 0 && !this.#visibility.isHidden()) {
        this.#activity.present(root, result.events, {
          repoId,
          repoName: issueDisplayName(identity),
          onOpen: (event) => routeIssueActivityEvent(event, repoId, this.#now()),
        });
      }
      if (result.continuation && !this.#visibility.isHidden()) this.#scheduleContinuation(repoId, root);
    } catch (error) {
      if (error instanceof HiddenSubscription) return;
      this.#recordFailure(repoId, root, generation, error);
      throw error;
    }
  }

  async #identity(
    repoId: string,
    tab: SubscriptionTab,
    root: string,
    generation: number,
  ): Promise<IssueIdentity> {
    const cached = this.#identityCache.get(repoId);
    if (cached !== undefined && cached.expiresAt > this.#now()) return cached.identity;
    const executor = tab.deviceId ? "device" : "local";
    const preliminaryScope = JSON.stringify([executor, tab.deviceId ?? null]);
    const identity = await this.#gate.run(preliminaryScope, async () => {
      if (!this.#current(repoId, root, generation)) throw new HiddenSubscription();
      return this.#transport.issuesIdentity(repoId);
    });
    this.#identityCache.set(repoId, {
      identity,
      expiresAt: this.#now() + SUBSCRIPTION_IDENTITY_TTL_MS,
    });
    return identity;
  }

  #current(repoId: string, root: string, generation: number): boolean {
    return (
      this.#started &&
      (this.#generations.get(repoId) ?? 0) === generation &&
      !this.#visibility.isHidden() &&
      normalizeIssueRoot(this.#tabs.rootFor(repoId) ?? "") === root
    );
  }

  #recordFailure(repoId: string, root: string, generation: number, error: unknown): void {
    if (!this.#started || (this.#generations.get(repoId) ?? 0) !== generation) return;
    const state = this.#loadState(root);
    const failure = error instanceof IssueStorageError
      ? { phase: "storage-error" as const, lastError: "storage-error" as const }
      : classifyFailure(error);
    const next = { ...state, ...failure };
    this.#states = { ...this.#states, [root]: next };
    try {
      this.#save(root, next);
    } catch {
      this.#storageErrors.add(root);
      this.#activity.warnStorage(root);
    }
  }

  #scheduleContinuation(repoId: string, root: string): void {
    if (this.#continuations.has(repoId)) return;
    const timer = this.#clock.setTimeout(() => {
      this.#continuations.delete(repoId);
      if (!this.#started || this.#visibility.isHidden() || !this.#lanes.has(repoId)) return;
      void this.#scheduler.refresh(issueLaneKey(repoId));
    }, 0);
    this.#continuations.set(repoId, timer);
  }
}

export const issueSubscriptions = new IssueSubscriptionCoordinator();
