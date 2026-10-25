<script lang="ts" module>
  /**
   * Header sync quick actions - Fetch / Pull / Push in one compact group.
   *
   * Counts ride on the verbs themselves (behind on Pull, ahead on Push), so
   * the header keeps one dense control instead of separate read-only chips.
   * Clicking a verb runs the same shared operation as the command palette
   * (`$lib/git/sync`), and busy state comes from the existing per-repo op
   * store - including ops started from a panel or the palette.
   *
   * Feedback rules:
   * - a local in-flight flag gives instant disabled/spinner feedback before
   *   the backend's first op-progress event arrives and blocks double clicks;
   * - post-completion status refresh is delegated to the owner (`onMutated`);
   * - only the active verb spins; the group is disabled while any fetch/
   *   pull/push runs because the backend serializes them per repo anyway.
   */

  import type { SyncVerb } from "$lib/git/sync";

  const VERB_LABELS: Record<SyncVerb, string> = {
    fetch: "Fetch",
    pull: "Pull",
    push: "Push",
  };

  function isSyncVerb(kind: string): kind is SyncVerb {
    return kind === "fetch" || kind === "pull" || kind === "push";
  }

  /** "1 commit" / "3 commits" for tooltips and labels. */
  function commitCount(count: number): string {
    return `${count} commit${count === 1 ? "" : "s"}`;
  }

  /** Counts stay readable instead of stretching the header group. */
  function badgeCount(count: number): string {
    return count > 99 ? "99+" : String(count);
  }
</script>

<script lang="ts">
  import type { RepoStatus } from "$lib/ipc/types";
  import {
    fetchDefaultRemote,
    pullCurrentBranch,
    pushCurrentBranch,
    syncSkipMessage,
    syncSuccessMessage,
    type SyncOutcome,
  } from "$lib/git/sync";
  import { opsFor } from "$lib/stores/ops.svelte";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import Spinner from "$lib/components/m3/Spinner.svelte";
  import iconFetch from "@ktibow/iconset-material-symbols/cloud-download";
  import iconPull from "@ktibow/iconset-material-symbols/arrow-downward";
  import iconPush from "@ktibow/iconset-material-symbols/arrow-upward";

  let {
    repoId,
    status,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Latest status snapshot; `null` while the first fetch is in flight. */
    status: RepoStatus | null;
    /** Called after a successful sync (RepoView refreshes the status). */
    onMutated?: () => void;
  } = $props();

  /** Set while this component's invoke is awaiting the backend. */
  let pending = $state<SyncVerb | null>(null);

  /** Sync ops already running for this repo (any entry point). */
  const activeOps = $derived(
    opsFor(repoId).filter((op) => isSyncVerb(op.kind)),
  );
  const netBusy = $derived(activeOps.length > 0);
  const activeVerb = $derived(
    (activeOps[0]?.kind ?? null) as SyncVerb | null,
  );

  const busy = $derived(netBusy || pending !== null);
  const runningVerb = $derived<SyncVerb | null>(pending ?? activeVerb);

  const branch = $derived(status?.branch ?? null);
  const branchLabel = $derived(branch ?? "current branch");
  /** Pull/push need a branch; fetch works even before status loads. */
  const canUseBranch = $derived(branch !== null && status?.detached !== true);

  const ahead = $derived(status?.ahead ?? 0);
  const behind = $derived(status?.behind ?? 0);

  const fetchTitle = "Fetch from the primary remote (prune)";
  const pullIdleTitle = $derived.by(() => {
    if (status === null) return "Waiting for repository status";
    if (!canUseBranch) return "Pull is unavailable without a current branch";
    return behind > 0
      ? `Pull ${commitCount(behind)} into ${branchLabel}`
      : `Pull ${branchLabel} from the remote`;
  });
  const pushIdleTitle = $derived.by(() => {
    if (status === null) return "Waiting for repository status";
    if (!canUseBranch) return "Push is unavailable without a current branch";
    return ahead > 0
      ? `Push ${commitCount(ahead)} from ${branchLabel}`
      : `Push ${branchLabel} to the remote`;
  });

  const fetchLabel = "Fetch from remote";
  const pullLabel = $derived(
    `Pull ${branchLabel}${behind > 0 ? `, ${behind} behind` : ""}`,
  );
  const pushLabel = $derived(
    `Push ${branchLabel}${ahead > 0 ? `, ${ahead} ahead` : ""}`,
  );

  /** Busy verbs explain the wait without changing the accessible name. */
  function displayTitle(verb: SyncVerb, idle: string): string {
    if (runningVerb === verb) return `${VERB_LABELS[verb]} in progress…`;
    if (busy) return "Wait for the current git operation to finish";
    return idle;
  }

  function actionFor(verb: SyncVerb): Promise<SyncOutcome> {
    switch (verb) {
      case "fetch":
        return fetchDefaultRemote(repoId);
      case "pull":
        return pullCurrentBranch(repoId, branch);
      case "push":
        return pushCurrentBranch(repoId, branch);
    }
  }

  async function run(verb: SyncVerb): Promise<void> {
    if (busy) return;
    if (verb !== "fetch" && !canUseBranch) return;

    pending = verb;
    try {
      const outcome = await actionFor(verb);
      if (outcome.kind === "skipped") {
        toast(syncSkipMessage(outcome.reason, verb), { kind: "error" });
        return;
      }
      toast(syncSuccessMessage(verb, outcome, branch), { kind: "success" });
      onMutated?.();
    } catch (err) {
      toast(
        `${VERB_LABELS[verb]} failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      pending = null;
    }
  }
</script>

<div
  class="sync-actions"
  class:busy
  role="group"
  aria-label="Git sync actions"
  aria-busy={busy}
>
  <button
    class="qa-btn"
    type="button"
    aria-label={fetchLabel}
    aria-busy={runningVerb === "fetch"}
    title={displayTitle("fetch", fetchTitle)}
    disabled={busy}
    onclick={() => void run("fetch")}
  >
    <span class="verb-icon">
      {#if runningVerb === "fetch"}
        <Spinner size={14} />
      {:else}
        <Icon icon={iconFetch} size={15} />
      {/if}
    </span>
  </button>

  <button
    class="qa-btn"
    type="button"
    aria-label={pullLabel}
    aria-busy={runningVerb === "pull"}
    title={displayTitle("pull", pullIdleTitle)}
    disabled={busy || !canUseBranch}
    onclick={() => void run("pull")}
  >
    <span class="verb-icon">
      {#if runningVerb === "pull"}
        <Spinner size={14} />
      {:else}
        <Icon icon={iconPull} size={15} />
      {/if}
    </span>
    {#if behind > 0}
      <span class="count behind" aria-hidden="true">{badgeCount(behind)}</span>
    {/if}
  </button>

  <button
    class="qa-btn"
    type="button"
    aria-label={pushLabel}
    aria-busy={runningVerb === "push"}
    title={displayTitle("push", pushIdleTitle)}
    disabled={busy || !canUseBranch}
    onclick={() => void run("push")}
  >
    <span class="verb-icon">
      {#if runningVerb === "push"}
        <Spinner size={14} />
      {:else}
        <Icon icon={iconPush} size={15} />
      {/if}
    </span>
    {#if ahead > 0}
      <span class="count ahead" aria-hidden="true">{badgeCount(ahead)}</span>
    {/if}
  </button>
</div>

<style>
  .sync-actions {
    display: inline-flex;
    align-items: stretch;
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    overflow: hidden;
    background: var(--m3-surface-container, transparent);
  }

  .qa-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.2rem;
    min-width: 1.65rem;
    height: 1.55rem;
    padding: 0 0.3rem;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
    transition:
      background-color 0.12s ease,
      color 0.12s ease;
  }

  .qa-btn + .qa-btn {
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .sync-actions:not(.busy) .qa-btn:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .qa-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .qa-btn:disabled {
    color: var(--app-disabled-fg, var(--m3-on-surface-variant));
    cursor: not-allowed;
  }

  .verb-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 14px;
    height: 14px;
  }

  .count {
    flex: none;
    min-width: 0.85rem;
    padding: 0 0.22rem;
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.6rem;
    font-weight: 600;
    line-height: 0.95rem;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .count.ahead {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-primary));
  }

  .count.behind {
    background: var(--m3-tertiary-container, var(--m3-surface-container-high));
    color: var(--m3-on-tertiary-container, var(--m3-tertiary));
  }

  @media (max-width: 760px) {
    .count {
      display: none;
    }
  }
</style>
