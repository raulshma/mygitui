<script lang="ts" module>
  /**
   * Header sync quick actions — one "Sync" split button.
   *
   * The main segment runs the contextual verb (pull when behind, push when
   * ahead, fetch otherwise) and shows ahead/behind counts as `↓n ↑n`; the
   * chevron opens an explicit Fetch / Pull / Push menu. Counts ride on the
   * button itself, so the header keeps one compact control instead of three
   * unlabeled icons. Verbs pinned from the menu (the trailing pin action,
   * stored app-wide in `headerPins`) get their own always-visible icon
   * segments in the same group, so a favorite verb skips the menu entirely.
   * Clicking a verb runs the same shared operation as the
   * command palette (`$lib/git/sync`), and busy state comes from the
   * existing per-repo op store — including ops started from a panel or the
   * palette.
   *
   * Feedback rules:
   * - a local in-flight flag gives instant disabled/spinner feedback before
   *   the backend's first op-progress event arrives and blocks double clicks;
   * - post-completion status refresh is delegated to the owner (`onMutated`);
   * - only the running verb is announced; the whole control is disabled
   *   while any fetch/pull/push runs because the backend serializes them
   *   per repo anyway.
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
  import { headerPins } from "$lib/stores/headerPins.svelte";
  import { toast } from "$lib/toast";
  import {
    showMenuAt,
    showMenuAtElement,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import Spinner from "$lib/components/m3/Spinner.svelte";
  import iconFetch from "@ktibow/iconset-material-symbols/cloud-download";
  import iconPull from "@ktibow/iconset-material-symbols/arrow-downward";
  import iconPush from "@ktibow/iconset-material-symbols/arrow-upward";
  import iconSync from "@ktibow/iconset-material-symbols/sync";
  import iconChevronDown from "@ktibow/iconset-material-symbols/expand-more";
  import iconPin from "@ktibow/iconset-material-symbols/push-pin";

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

  /**
   * The verb the main segment runs: the most needed action first — pull
   * incoming commits, then push outgoing ones, otherwise refresh the
   * remote state. The menu always offers all three explicitly.
   */
  const primaryVerb = $derived<SyncVerb>(
    behind > 0 ? "pull" : ahead > 0 ? "push" : "fetch",
  );

  /**
   * Pinned verbs in the menu's fixed order (fetch/pull/push) regardless of
   * pin order, so the segment row never reshuffles when the list changes.
   */
  const pinnedVerbs = $derived(
    (["fetch", "pull", "push"] as const).filter((verb) =>
      headerPins.pins.includes(verb),
    ),
  );

  const fetchIdleTitle = "Fetch from the primary remote (prune)";
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

  const primaryTitle = $derived.by(() => {
    if (runningVerb !== null) {
      return `${VERB_LABELS[runningVerb]} in progress…`;
    }
    if (busy) return "Wait for the current git operation to finish";
    if (status === null) return "Waiting for repository status";
    if (primaryVerb === "fetch") return fetchIdleTitle;
    return primaryVerb === "pull" ? pullIdleTitle : pushIdleTitle;
  });

  const primaryAriaLabel = $derived.by(() => {
    if (runningVerb !== null) return `${VERB_LABELS[runningVerb]} in progress…`;
    if (primaryVerb === "fetch") return "Sync with remote (fetch)";
    if (primaryVerb === "pull") return `Sync with remote (pull ${branchLabel}${behind > 0 ? `, ${behind} behind` : ""})`;
    return `Sync with remote (push ${branchLabel}${ahead > 0 ? `, ${ahead} ahead` : ""})`;
  });

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

  /** Chevron menu: the three verbs with their counts spelled out. */
  function openSyncMenu(event: MouseEvent): void {
    showMenuAtElement(event.currentTarget as HTMLElement, [
      {
        id: "fetch",
        label: "Fetch",
        icon: iconFetch,
        disabled: busy,
        run: () => void run("fetch"),
        action: pinAction("fetch"),
      },
      {
        id: "pull",
        label: behind > 0 ? `Pull (${behind} behind)` : "Pull",
        icon: iconPull,
        disabled: busy || !canUseBranch,
        run: () => void run("pull"),
        action: pinAction("pull"),
      },
      {
        id: "push",
        label: ahead > 0 ? `Push (${ahead} ahead)` : "Push",
        icon: iconPush,
        disabled: busy || !canUseBranch,
        run: () => void run("push"),
        action: pinAction("push"),
      },
    ]);
  }

  /** Trailing menu affordance toggling the verb's pinned segment. */
  function pinAction(verb: SyncVerb) {
    return {
      label: headerPins.has(verb) ? "Unpin from header" : "Pin to header",
      icon: iconPin,
      run: () => headerPins.toggle(verb),
    };
  }

  /** Right-click on a pinned segment unpins it (mirrors the rail buttons). */
  function openPinContext(event: MouseEvent, verb: SyncVerb): void {
    showMenuAt(event, [
      {
        id: `unpin-${verb}`,
        label: "Unpin from header",
        icon: iconPin,
        run: () => headerPins.unpin(verb),
      },
    ]);
  }

  function verbIcon(verb: SyncVerb) {
    return verb === "fetch"
      ? iconFetch
      : verb === "pull"
        ? iconPull
        : iconPush;
  }

  /** Segment tooltip: running → progress, busy → wait, else the idle title. */
  function pinTitle(verb: SyncVerb): string {
    if (runningVerb === verb) return `${VERB_LABELS[verb]} in progress…`;
    if (busy) return "Wait for the current git operation to finish";
    return verb === "fetch"
      ? fetchIdleTitle
      : verb === "pull"
        ? pullIdleTitle
        : pushIdleTitle;
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
    class="sync-main"
    type="button"
    aria-label={primaryAriaLabel}
    aria-busy={runningVerb !== null}
    title={primaryTitle}
    disabled={busy || (primaryVerb !== "fetch" && !canUseBranch)}
    onclick={() => void run(primaryVerb)}
  >
    <span class="verb-icon">
      {#if runningVerb !== null}
        <Spinner size={14} />
      {:else}
        <Icon icon={iconSync} size={15} />
      {/if}
    </span>
    <span class="sync-word">Sync</span>
    {#if behind > 0 && runningVerb === null}
      <span class="count behind" title={`${behind} behind`}>
        <Icon icon={iconPull} size={10} />
        {badgeCount(behind)}
      </span>
    {/if}
    {#if ahead > 0 && runningVerb === null}
      <span class="count ahead" title={`${ahead} ahead`}>
        <Icon icon={iconPush} size={10} />
        {badgeCount(ahead)}
      </span>
    {/if}
  </button>

  {#each pinnedVerbs as verb (verb)}
    <button
      class="sync-pin-btn"
      type="button"
      data-sync-pin={verb}
      aria-label={pinTitle(verb)}
      aria-busy={runningVerb === verb}
      title={pinTitle(verb)}
      disabled={busy || (verb !== "fetch" && !canUseBranch)}
      onclick={() => void run(verb)}
      oncontextmenu={(event) => openPinContext(event, verb)}
    >
      <span class="verb-icon">
        {#if runningVerb === verb}
          <Spinner size={14} />
        {:else}
          <Icon icon={verbIcon(verb)} size={14} />
        {/if}
      </span>
    </button>
  {/each}

  <button
    class="sync-menu-btn"
    type="button"
    aria-label="More sync actions"
    title="Fetch / Pull / Push"
    aria-haspopup="menu"
    disabled={busy}
    onclick={openSyncMenu}
  >
    <Icon icon={iconChevronDown} size={14} />
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

  .sync-main,
  .sync-pin-btn,
  .sync-menu-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    min-height: 2rem;
    padding: 0 0.5rem;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.75rem;
    cursor: pointer;
    transition:
      background-color 0.12s ease,
      color 0.12s ease;
  }

  .sync-main {
    min-width: 2rem;
  }

  .sync-pin-btn {
    flex: none;
    min-width: 1.75rem;
    padding: 0 0.3rem;
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .sync-menu-btn {
    flex: none;
    min-width: 1.75rem;
    padding: 0 0.2rem;
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .sync-actions:not(.busy) .sync-main:hover:not(:disabled),
  .sync-actions:not(.busy) .sync-pin-btn:hover:not(:disabled),
  .sync-actions:not(.busy) .sync-menu-btn:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .sync-main:focus-visible,
  .sync-pin-btn:focus-visible,
  .sync-menu-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .sync-main:disabled,
  .sync-pin-btn:disabled,
  .sync-menu-btn:disabled {
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

  .sync-word {
    flex: none;
    font-weight: 500;
    line-height: 1;
  }

  .count {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.1rem;
    min-width: 1rem;
    padding: 0 0.25rem;
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.65rem;
    font-weight: 600;
    line-height: 1.1;
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
    .sync-word {
      display: none;
    }

    .count {
      display: none;
    }
  }
</style>
