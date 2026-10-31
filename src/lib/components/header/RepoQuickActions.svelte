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
   * segments in the same group, so a favorite verb skips the menu entirely;
   * a pinned pull/push carries its incoming/outgoing count badge too, so
   * work waiting on either side of the remote stays visible at a glance.
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
  import { Button } from "$lib/components/ui/button/index.js";
  import {
    showMenuAt,
    showMenuAtElement,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

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

  const iconFetch = $derived(appIcon("cloud-download")!);
  const iconPull = $derived(appIcon("arrow-downward")!);
  const iconPush = $derived(appIcon("arrow-upward")!);
  const iconSync = $derived(appIcon("sync")!);
  const iconChevronDown = $derived(appIcon("chevron-down")!);
  const iconPin = $derived(appIcon("push-pin")!);

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

  /**
   * The count a pinned segment carries: pull shows incoming (behind), push
   * outgoing (ahead), fetch none. `null` when there is nothing to show.
   */
  function pinCount(
    verb: SyncVerb,
  ): { count: number; dir: "behind" | "ahead" } | null {
    if (verb === "pull" && behind > 0) return { count: behind, dir: "behind" };
    if (verb === "push" && ahead > 0) return { count: ahead, dir: "ahead" };
    return null;
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
  class="sync-actions inline-flex flex-none items-stretch overflow-hidden rounded-sm border border-border bg-popover"
  class:busy
  role="group"
  aria-label="Git sync actions"
  aria-busy={busy}
>
  <Button
    class="sync-main min-w-8 gap-[0.3rem] rounded-none border-0 px-2 text-xs transition-colors duration-[120ms] hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
    variant="ghost"
    aria-label={primaryAriaLabel}
    aria-busy={runningVerb !== null}
    title={primaryTitle}
    disabled={busy || (primaryVerb !== "fetch" && !canUseBranch)}
    onclick={() => void run(primaryVerb)}
  >
    <span class="verb-icon inline-flex h-3.5 w-3.5 flex-none items-center justify-center">
      {#if runningVerb !== null}
        <Spinner class="size-3.5" role="presentation" aria-hidden="true" />
      {:else}
        <Icon icon={iconSync} size={15} />
      {/if}
    </span>
    <span class="sync-word flex-none font-medium leading-none max-[760px]:hidden">Sync</span>
    {#if behind > 0 && runningVerb === null}
      <span
        class="count behind inline-flex h-auto min-w-4 flex-none items-center gap-[0.1rem] rounded-full bg-muted px-1 text-[0.65rem] font-semibold leading-[1.1] text-center tabular-nums text-muted-foreground max-[760px]:hidden"
        title={`${behind} behind`}
      >
        <Icon icon={iconPull} size={10} />
        {badgeCount(behind)}
      </span>
    {/if}
    {#if ahead > 0 && runningVerb === null}
      <span
        class="count ahead inline-flex h-auto min-w-4 flex-none items-center gap-[0.1rem] rounded-full bg-accent px-1 text-[0.65rem] font-semibold leading-[1.1] text-center tabular-nums text-accent-foreground max-[760px]:hidden"
        title={`${ahead} ahead`}
      >
        <Icon icon={iconPush} size={10} />
        {badgeCount(ahead)}
      </span>
    {/if}
  </Button>

  {#each pinnedVerbs as verb (verb)}
    <Button
      class="sync-pin-btn min-w-7 flex-none gap-[0.3rem] rounded-none border-0 border-l border-border px-[0.3rem] text-xs transition-colors duration-[120ms] hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
      variant="ghost"
      data-sync-pin={verb}
      aria-label={pinTitle(verb)}
      aria-busy={runningVerb === verb}
      title={pinTitle(verb)}
      disabled={busy || (verb !== "fetch" && !canUseBranch)}
      onclick={() => void run(verb)}
      oncontextmenu={(event) => openPinContext(event, verb)}
    >
      <span class="verb-icon inline-flex h-3.5 w-3.5 flex-none items-center justify-center">
        {#if runningVerb === verb}
          <Spinner class="size-3.5" role="presentation" aria-hidden="true" />
        {:else}
          <Icon icon={verbIcon(verb)} size={14} />
        {/if}
      </span>
      {#if pinCount(verb) !== null && runningVerb === null}
        {@const badge = pinCount(verb)!}
        <span
          class={`count ${badge.dir} inline-flex h-auto min-w-4 flex-none items-center gap-[0.1rem] rounded-full px-1 text-[0.65rem] font-semibold leading-[1.1] text-center tabular-nums max-[760px]:hidden ${badge.dir === "ahead" ? "bg-accent text-accent-foreground" : "bg-muted text-muted-foreground"}`}
          title={`${badge.count} ${badge.dir}`}
        >
          {badgeCount(badge.count)}
        </span>
      {/if}
    </Button>
  {/each}

  <Button
    class="sync-menu-btn min-w-7 flex-none gap-[0.3rem] rounded-none border-0 border-l border-border px-[0.2rem] text-xs transition-colors duration-[120ms] hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
    variant="ghost"
    aria-label="More sync actions"
    title="Fetch / Pull / Push"
    aria-haspopup="menu"
    disabled={busy}
    onclick={openSyncMenu}
  >
    <Icon icon={iconChevronDown} size={14} />
  </Button>
</div>
