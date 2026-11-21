/**
 * GitHub footer chips store (E1, plan §9.2–9.3).
 *
 * The ONE bridge between the poll scheduler, the forge store's snapshot
 * cache and the indication state machine: it owns the
 * `forge:snapshot:<repoId>` scheduler lanes (60 s for the ACTIVE repo —
 * user-configurable with a 30 s floor — and 5 m for inactive-open repos;
 * hidden windows spawn nothing because the scheduler pauses), derives each
 * repo's chip states through `indication.svelte.ts`, fires the failure
 * toasts (§9.2: failures only by default, deep-linking to the panel) and
 * exposes the repo-tab attention badges ("as of Xm ago").
 *
 * `start()` is lazy and module-owned (the watchHealth idiom): the first
 * StatusBar mount probes `gh` status and registers the per-repo evictor;
 * lane registration itself is driven by {@link syncLanes}, which the
 * StatusBar calls from a `$effect` tracking the open tabs + the active tab
 * + the cadence setting. Tab switching promotes the repo to the active
 * cadence and forces one refresh ({@link promote}).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { untrack } from "svelte";
import {
  appSettings,
  FORGE_INACTIVE_SNAPSHOT_MS,
  FORGE_SNAPSHOT_SECONDS_MIN,
} from "$lib/state/appSettings.svelte";
import { registerRepoEvictor } from "$lib/state/repoEvictors";
import { pollScheduler, type PollLaneState } from "$lib/stores/pollScheduler.svelte";
import { forgeStore } from "$lib/stores/forge.svelte";
import { tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { toast } from "$lib/toast";
import { loadForgeRemote } from "./remotePrefs";
import {
  deriveChips,
  isFailure,
  repoDegraded,
  type ForgeChips,
  type ForgeChipState,
} from "./indication.svelte";

/** Scheduler lane key of one repo's snapshot lane. */
export function snapshotLaneKey(repoId: string): string {
  return `forge:snapshot:${repoId}`;
}

/** The active-repo cadence in ms from the setting (0 = off). */
export function activeSnapshotIntervalMs(seconds: number): number {
  const safe = Number.isFinite(seconds) ? Math.round(seconds) : 60;
  if (safe <= 0) return 0;
  return Math.max(FORGE_SNAPSHOT_SECONDS_MIN, safe) * 1000;
}

/** What the tab strip needs for one repo's attention badge. */
export interface TabAttention {
  /** Any chip is attention-worthy (failing/degraded/warning/unpushed). */
  attention: boolean;
  /** Attention-worthy chip count. */
  count: number;
  /** The worst chip's short text (tooltip line). */
  text: string;
  /** Wall-clock of the last successful snapshot fetch (`null` = none yet). */
  fetchedAtMs: number | null;
}

export class ForgeChipsStore {
  /** Registered snapshot lanes per repo id (diffed to avoid lane churn). */
  #registered = new Map<string, { intervalMs: number }>();
  /** Repos currently failure-toasted (reset when nothing fails anymore). */
  #failureToasted = new Set<string>();
  #evictorUnregister: (() => void) | null = null;
  #statusStarted = false;

  /**
   * Idempotent start: probes `gh` status once and registers the per-repo
   * evictor (tab close drops the snapshot cache + the lane). Safe to call
   * from every mounting surface.
   */
  start(): void {
    if (this.#statusStarted) return;
    this.#statusStarted = true;
    untrack(() => {
      if (forgeStore.status === null && !forgeStore.statusLoading) {
        void forgeStore.refreshStatus();
      }
    });
    this.#evictorUnregister = registerRepoEvictor((repoId) => {
      this.clear(repoId);
    });
  }

  /** Drops one repo's lane, caches and toast bookkeeping (tab close). */
  clear(repoId: string): void {
    pollScheduler.unregister(snapshotLaneKey(repoId));
    this.#registered.delete(repoId);
    this.#failureToasted.delete(repoId);
    untrack(() => forgeStore.dropSnapshot(repoId));
  }

  /**
   * Syncs the snapshot lanes with the open tabs (called from a reactive
   * context — the caller's `$effect` tracks `tabStore.tabs`, `activeId` and
   * the cadence setting; everything else here is untracked). The ACTIVE
   * repo gets the configurable cadence (0 = off), inactive-open repos the
   * fixed 5-minute rate. Device tabs poll the same way — their gh runs on
   * the device, so the snapshot works there like anywhere else.
   */
  syncLanes(): void {
    const activeIntervalMs = untrack(() =>
      activeSnapshotIntervalMs(appSettings.forgeSnapshotSeconds),
    );
    const wanted = new Map<string, number>();
    for (const tab of tabStore.tabs) {
      const isActive = tab.id === tabStore.activeId;
      const intervalMs = isActive ? activeIntervalMs : FORGE_INACTIVE_SNAPSHOT_MS;
      if (intervalMs > 0) wanted.set(tab.id, intervalMs);
    }

    // Re-register only when a lane's cadence actually changed (register
    // replaces the lane and would restart every timer otherwise).
    for (const [repoId, intervalMs] of wanted) {
      const current = this.#registered.get(repoId);
      if (current && current.intervalMs === intervalMs) continue;
      this.#registered.set(repoId, { intervalMs });
      pollScheduler.register(snapshotLaneKey(repoId), {
        intervalMs,
        work: () => this.#snapshotWork(repoId),
      });
    }
    // Closed repos lose their lane (the evictor covers the cache).
    for (const repoId of [...this.#registered.keys()]) {
      if (!wanted.has(repoId)) {
        pollScheduler.unregister(snapshotLaneKey(repoId));
        this.#registered.delete(repoId);
      }
    }
  }

  /**
   * Tab-switch promotion (§9.3): re-runs {@link syncLanes} (the caller's
   * effect does) and forces one refresh of the newly active repo.
   */
  promote(repoId: string): void {
    void pollScheduler.refresh(snapshotLaneKey(repoId));
  }

  /** Manual refresh (popover "Refresh now"). Bypasses the hidden pause. */
  refresh(repoId: string): Promise<void> {
    return pollScheduler.refresh(snapshotLaneKey(repoId));
  }

  /** The repo's scheduler lane state (`undefined` when no lane is active). */
  laneFor(repoId: string): PollLaneState | undefined {
    return pollScheduler.lanes[snapshotLaneKey(repoId)];
  }

  /**
   * One repo's three chip states + the worst-state collapse (§9.2). Reads
   * the forge store + the scheduler lanes reactively — callers re-derive
   * whenever any input moves. Device repos read their device's gh status
   * (the snapshot itself runs there, so a device gh outage degrades the
   * chips like a local outage does).
   */
  chipsFor(repoId: string, nowMs: number = Date.now()): ForgeChips {
    const status = forgeStore.statusFor(repoId);
    const failureKind = forgeStore.snapshotFailureKindFor(repoId);
    // Stale-on-error: a failed refresh keeps the last-known chips visible.
    const snapshot = forgeStore.snapshotStaleFor(repoId);
    const state = forgeStore.snapshotStateFor(repoId);
    const lane = this.laneFor(repoId);

    const degraded = repoDegraded(status, failureKind, snapshot);
    const degradedMessage = forgeStore.snapshotErrorFor(repoId) ?? "";
    const retryInMs =
      degraded === "rate-limited" && lane?.erroring ? lane.backoffMs : null;
    const loading =
      degraded === null &&
      snapshot === null &&
      (state === "idle" || state === "loading");

    const chips = deriveChips(
      {
        loading,
        degraded,
        degradedMessage,
        retryInMs,
      },
      snapshot,
      nowMs,
    );
    return chips;
  }

  /** The repo's worst chip (`null` never happens for a derived repo). */
  worstFor(repoId: string, nowMs: number = Date.now()): ForgeChipState | null {
    return this.chipsFor(repoId, nowMs).worst?.state ?? null;
  }

  /**
   * Repo-tab attention badge inputs (`null` = no badge — the repo never
   * attempted a snapshot).
   */
  attentionFor(repoId: string, nowMs: number = Date.now()): TabAttention | null {
    const tab = this.#tabFor(repoId);
    if (tab === null) return null;
    if (!this.#registered.has(repoId)) return null;
    const chips = this.chipsFor(repoId, nowMs);
    const worst = chips.worst;
    if (worst === null) return null;
    return {
      attention: chips.attentionCount > 0,
      count: chips.attentionCount,
      text: worst.state.text,
      fetchedAtMs: forgeStore.snapshotFetchedAtFor(repoId),
    };
  }

  // -- internals -------------------------------------------------------------

  #tabFor(repoId: string): Tab | null {
    return tabStore.tabs.find((tab) => tab.id === repoId) ?? null;
  }

  /** The persisted per-repo remote override (`null` = default `origin`). */
  #remoteFor(repoId: string): string | null {
    const tab = this.#tabFor(repoId);
    return tab === null ? null : loadForgeRemote(tab.root);
  }

  /**
   * The lane's work: refresh the snapshot, then classify. Rejects when the
   * fetch failed (the scheduler's exponential backoff engages — 403 ⇒
   * "retry in Xm" on the chip), then inspects the fresh state for toasts.
   */
  async #snapshotWork(repoId: string): Promise<void> {
    await forgeStore.refreshSnapshotStrict(repoId, {
      remote: untrack(() => this.#remoteFor(repoId)),
    });
    untrack(() => this.#maybeToast(repoId));
  }

  /**
   * §9.2 toasts: failures only by default (`forgeFailureToasts`); activity
   * transitions opt-in (`forgeActivityToasts`). One toast per failing
   * episode — the flag resets when nothing fails anymore.
   */
  #maybeToast(repoId: string): void {
    const chips = this.chipsFor(repoId);
    const failures: string[] = [];
    if (isFailure(chips.pr)) failures.push(`PR ${chips.pr.text}`);
    if (isFailure(chips.checks)) failures.push(`checks ${chips.checks.text}`);
    if (isFailure(chips.runs)) failures.push(`run ${chips.runs.text}`);
    const activity: string[] = [];
    if (appSettings.forgeActivityToasts) {
      if (chips.pr.kind === "ok") activity.push(`PR ${chips.pr.text} open`);
      if (chips.runs.kind === "running") activity.push(`run ${chips.runs.text} running`);
    }

    const tab = this.#tabFor(repoId);
    const name = tab?.name ?? "repository";
    if (failures.length > 0 && appSettings.forgeFailureToasts) {
      if (!this.#failureToasted.has(repoId)) {
        this.#failureToasted.add(repoId);
        toast(`GitHub on ${name}: ${failures.join(" · ")}`, { kind: "error" });
      }
    } else if (failures.length === 0) {
      this.#failureToasted.delete(repoId);
    }
    if (activity.length > 0) {
      toast(`GitHub on ${name}: ${activity.join(" · ")}`);
    }
  }
}

/** The application-wide chips store. */
export const forgeChips = new ForgeChipsStore();
