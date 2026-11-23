<script lang="ts">
  /**
   * Worktree panel (M3) — linked worktrees plus add/remove/open.
   *
   * The backend lists *linked* worktrees only; the current repository's own
   * root arrives via the `root` prop and is shown first as the "main"
   * entry. Each linked row shows name, path, branch (or detached @ sha)
   * and locked/prunable/detached badges, with Open (opens the worktree as
   * a repository tab) and Remove (confirm; a force checkbox appears when
   * the worktree is locked or prunable). The add form takes a path (text
   * input + native folder picker), then either an existing not-checked-out
   * branch or a new branch name.
   *
   * The branch feed for the add form comes from the shared refs cache
   * (`$lib/stores/refs.svelte`) — the four other ref-listing surfaces use
   * the same cache, so a repo open scans once, not five times. External
   * ref moves (terminal, outside editors) reload this panel through the
   * debounced repo-change effect below.
   */
  import {
    pickFolder,
    remoteBranches,
    worktreeAdd,
    worktreeLock,
    worktreePrune,
    worktreeRemove,
    worktrees,
    worktreeUnlock,
  } from "$lib/ipc/client";
  import type { BranchInfo, RemoteBranchInfo, WorktreeInfo } from "$lib/ipc/types";
  import { isSignificant, onRepoChange } from "$lib/events/repoEvents";
  import { branchesFor, invalidateRefs } from "$lib/stores/refs.svelte";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import EmptyState from "../layout/EmptyState.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import {
    baseName,
    worktreeBadges,
    worktreeCandidateBranches,
    worktreeNeedsForce,
    worktreeRefLabel,
  } from "./panelModel";

  let {
    repoId,
    root,
    onMutated = undefined,
  }: {
    repoId: string;
    /** This repo's own worktree root (shown as the "main" entry). */
    root: string;
    /** Called after a mutation that changes repo state (add/remove). */
    onMutated?: () => void;
  } = $props();

  const iconAdd = $derived(appIcon("add")!);
  const iconRefresh = $derived(appIcon("refresh")!);
  const iconPrune = $derived(appIcon("delete-sweep")!);
  const iconBrowse = $derived(appIcon("folder-open")!);
  const iconOpen = $derived(appIcon("open-in-new")!);
  const iconRemove = $derived(appIcon("delete")!);
  const iconLock = $derived(appIcon("lock")!);
  const iconUnlock = $derived(appIcon("lock-open")!);
  const iconCheck = $derived(appIcon("check")!);

  let list = $state<WorktreeInfo[]>([]);
  let allBranches = $state<BranchInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  /** Any mutation in flight (buttons disabled while true). */
  let busy = $state(false);
  /** Path of the worktree whose removal is in flight (per-row progress). */
  let removingPath = $state<string | null>(null);

  // Per-row remove state (force checkbox for locked/prunable worktrees).
  let removeConfirmFor = $state<string | null>(null);
  let removeForce = $state(false);

  // Add form
  let formOpen = $state(false);
  let addPath = $state("");
  /**
   * "existing" = check out an existing branch; "new" = create one;
   * "remote" = create the local branch at a remote-tracking tip (Phase 5
   * `from_remote` `<remote>/<branch>` shorthand) and put the worktree on it.
   */
  let addMode = $state<"existing" | "new" | "remote">("existing");
  let addBranch = $state("");
  let addNewBranch = $state("");
  /** `<remote>/<name>` picked in the remote-branch mode. */
  let addRemoteBranch = $state("");

  /** Branches safe to check out in a new worktree (not checked out here). */
  const candidates = $derived(worktreeCandidateBranches(allBranches));

  /**
   * M12: the backend now lists the main worktree too (`is_main`); this
   * panel already renders the repo's own root as the synthetic "main" card
   * above, so a backend main entry for the same path is dropped (chip or
   * not, it would be a duplicate row). Main entries for OTHER roots (this
   * tab is itself a linked worktree) stay — with the main chip.
   */
  const linked = $derived(list.filter((info) => info.path !== root));

  async function onPrune(): Promise<void> {
    busy = true;
    try {
      const pruned = await worktreePrune(repoId);
      toast(`Pruned ${pruned} stale worktree${pruned === 1 ? "" : "s"}`, {
        kind: "success",
      });
      reloadAfterMutation();
      onMutated?.();
    } catch (err) {
      fail("Prune worktrees", err);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    // Reload when the repo switches.
    void repoId;
    formOpen = false;
    removeConfirmFor = null;
    lockReasonFor = null;
    void reload();
  });

  // External ref moves (the terminal panel, an outside editor, the header
  // switcher) never reached this panel before — it reloaded only on repo
  // switch and after its OWN mutations. Reload on significant repo
  // changes, debounced 500 ms like FilesPanel's tree refresh so a watcher
  // burst coalesces into one reload. The panel is keep-alive mounted
  // (TabGroup), so this also reloads while hidden — accepted: the reads
  // are cheap and shared (the refs cache was already dropped by its own
  // repoEvents subscription; every panel's debounced reload then shares
  // ONE rescan). StatusPanel's cleanup pattern: the pending debounce dies
  // with the panel, or it would fire reload() IPC against a repo that may
  // since have been switched away.
  $effect(() => {
    let reloadTimer: ReturnType<typeof setTimeout> | null = null;
    const unregister = onRepoChange({
      filter: (change) => change.repo_id === repoId && isSignificant(change),
      handle: () => {
        if (reloadTimer) clearTimeout(reloadTimer);
        reloadTimer = setTimeout(() => {
          reloadTimer = null;
          void reload();
        }, 500);
      },
    });
    return () => {
      unregister();
      if (reloadTimer) {
        clearTimeout(reloadTimer);
        reloadTimer = null;
      }
    };
  });

  async function reload(): Promise<void> {
    // Stale-response guard: the panel is keep-alive across repo switches —
    // a slow worktree list from repo A must never land after the switch.
    const id = repoId;
    loading = true;
    error = null;
    try {
      // worktrees is panel-specific (uncached); the branch feed for the
      // add form rides the shared refs cache (module header). The
      // remote-branch feed for the "remote" add mode is best-effort.
      const [wt, br] = await Promise.all([worktrees(id), branchesFor(id)]);
      if (id !== repoId) return;
      list = wt;
      allBranches = br;
      // Keep the existing-branch select on a valid choice (repo switches
      // can invalidate the previous pick).
      const names = new Set(candidates.map((b) => b.name));
      if (!names.has(addBranch)) {
        addBranch = candidates[0]?.name ?? "";
      }
    } catch (err) {
      if (id !== repoId) return;
      error = err instanceof Error ? err.message : String(err);
    } finally {
      if (id === repoId) loading = false;
    }
    loadRemoteBranches(id);
  }

  // -- remote-branch feed for the "remote" add mode ---------------------------

  let remoteBranchList = $state<RemoteBranchInfo[]>([]);

  /** `<remote>/<name>` specs offered by the remote-branch mode. */
  const remoteBranchSpecs = $derived(
    remoteBranchList.map((rb) => `${rb.remote}/${rb.name}`),
  );

  /** Best-effort feed; the select degrades to an empty list on failure. */
  async function loadRemoteBranches(id: string): Promise<void> {
    try {
      const rbs = await remoteBranches(id);
      if (id !== repoId) return;
      remoteBranchList = rbs;
      if (!remoteBranchSpecs.includes(addRemoteBranch)) {
        addRemoteBranch = remoteBranchSpecs[0] ?? "";
      }
    } catch {
      if (id === repoId) remoteBranchList = [];
    }
  }

  function fail(what: string, err: unknown): void {
    toast(`${what} failed: ${err instanceof Error ? err.message : String(err)}`, {
      kind: "error",
    });
  }

  /**
   * Post-mutation reload: `worktree add -b` creates a branch ref in the
   * common git dir, so drop the shared refs cache FIRST — a plain reload
   * could still be served the pre-mutation branch list from the cache's
   * TTL window (the watcher event that invalidates it may still be
   * batched/in flight). Remove/prune invalidate too: harmless for the
   * branch list and keeps one idiom per panel.
   */
  function reloadAfterMutation(): void {
    invalidateRefs(repoId);
    void reload();
  }

  function afterMutation(what: string, detail: string): void {
    toast(`${what}: ${detail}`, { kind: "success" });
    reloadAfterMutation();
    onMutated?.();
  }

  async function onOpen(info: { name: string; path: string }): Promise<void> {
    try {
      await openTab(info.path);
      toast(`Opened ${info.name} in a tab`, { kind: "success" });
    } catch (err) {
      fail("Open", err);
    }
  }

  function onRemoveClick(info: WorktreeInfo): void {
    if (removeConfirmFor === info.path) {
      removeConfirmFor = null;
      return;
    }
    removeConfirmFor = info.path;
    removeForce = worktreeNeedsForce(info);
  }

  /** Remove confirmation (ConfirmDialog state, M9 F9). */
  let removeDialogFor = $state<WorktreeInfo | null>(null);

  function onRemove(info: WorktreeInfo): void {
    removeDialogFor = info;
  }

  async function onRemoveConfirmed(): Promise<void> {
    const info = removeDialogFor;
    if (!info) return;
    removeDialogFor = null;
    busy = true;
    removingPath = info.path;
    try {
      await worktreeRemove(repoId, info.name, removeForce);
      removeConfirmFor = null;
      removeForce = false;
      toast(`Removed worktree ${info.name}`, { kind: "success" });
      reloadAfterMutation();
      onMutated?.();
    } catch (err) {
      fail("Remove worktree", err);
    } finally {
      busy = false;
      removingPath = null;
    }
  }

  async function browse(): Promise<void> {
    const dir = await pickFolder();
    if (dir) addPath = dir;
  }

  async function onAdd(): Promise<void> {
    const path = addPath.trim();
    if (!path) return;
    busy = true;
    try {
      if (addMode === "existing") {
        await worktreeAdd(repoId, path, addBranch || undefined);
      } else if (addMode === "new") {
        const name = addNewBranch.trim();
        if (!name) {
          busy = false;
          return;
        }
        await worktreeAdd(repoId, path, undefined, name);
      } else {
        // Remote-branch mode: `<remote>/<name>` spec — the backend creates
        // the local branch at the tracking tip and puts the worktree on it.
        const spec = addRemoteBranch.trim();
        if (!spec) {
          busy = false;
          return;
        }
        await worktreeAdd(repoId, path, undefined, undefined, spec);
      }
      addPath = "";
      addNewBranch = "";
      formOpen = false;
      afterMutation("Add worktree", path);
    } catch (err) {
      fail("Add worktree", err);
    } finally {
      busy = false;
    }
  }

  // -- lock / unlock (Phase 5) -----------------------------------------------

  /** Path of the worktree an inline lock-reason form is open for. */
  let lockReasonFor = $state<string | null>(null);
  let lockReason = $state("");

  function onLock(info: WorktreeInfo): void {
    lockReasonFor = info.path;
    lockReason = "";
  }

  async function onLockConfirmed(info: WorktreeInfo): Promise<void> {
    busy = true;
    try {
      await worktreeLock(repoId, info.path, lockReason.trim() || undefined);
      lockReasonFor = null;
      lockReason = "";
      afterMutation("Lock worktree", info.name);
    } catch (err) {
      fail("Lock worktree", err);
    } finally {
      busy = false;
    }
  }

  async function onUnlock(info: WorktreeInfo): Promise<void> {
    busy = true;
    try {
      await worktreeUnlock(repoId, info.path);
      afterMutation("Unlock worktree", info.name);
    } catch (err) {
      fail("Unlock worktree", err);
    } finally {
      busy = false;
    }
  }
</script>

<aside class="worktree-panel flex min-h-0 flex-col text-ui-base" aria-label="Worktrees">
  <div class="toolbar flex flex-none items-center gap-1.5 px-2 py-1.5">
    <Button
      class="aria-pressed:bg-accent aria-pressed:text-accent-foreground"
      variant="default"
      size="icon-sm"
      type="button"
      aria-label={formOpen ? "Close worktree form" : "Add worktree"}
      aria-pressed={formOpen}
      title={formOpen ? "Close the add-worktree form" : "Add a linked worktree"}
      disabled={busy}
      onclick={() => (formOpen = !formOpen)}
    >
      <Icon icon={iconAdd} size={16} />
    </Button>
    <!-- M12: git worktree prune — drops stale admin dirs for deleted paths. -->
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Prune stale worktrees"
      title="git worktree prune — remove administration entries for deleted worktree folders"
      disabled={busy}
      onclick={() => void onPrune()}
    >
      <Icon icon={iconPrune} size={16} />
    </Button>
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh worktrees"
      title={loading ? "Refreshing worktrees…" : "Refresh worktrees"}
      disabled={loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
  </div>

  {#if formOpen}
    <form
      class="add-form flex flex-none flex-col gap-1.5 border-b border-border px-2 py-1.5"
      aria-label="Add worktree"
      onsubmit={(e) => {
        e.preventDefault();
        void onAdd();
      }}
    >
      <div class="path-row flex flex-wrap items-center gap-1.5">
        <input
          class="path min-w-40 flex-1 rounded border border-input bg-background px-2 py-0.5 font-mono text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
          type="text"
          placeholder="Worktree folder path"
          aria-label="Worktree folder path"
          bind:value={addPath}
        />
        <Hint label="Browse for a folder">
          {#snippet child({ props })}
            <Button
              class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
              variant="ghost"
              size="icon-sm"
              type="button"
              {...props}
              aria-label="Browse for worktree folder"
              onclick={() => void browse()}
            >
              <Icon icon={iconBrowse} size={16} />
            </Button>
          {/snippet}
        </Hint>
        <Button
          variant="default"
          size="icon-sm"
          type="submit"
          aria-label="Create worktree"
          title="Create the linked worktree"
          disabled={busy || !addPath.trim() || (addMode === "new" && !addNewBranch.trim()) || (addMode === "remote" && !addRemoteBranch.trim())}
        >
          <Icon icon={iconAdd} size={16} />
        </Button>
      </div>
      <div class="mode-row flex flex-wrap items-center gap-1.5">
        <label class="mode flex items-center gap-1.5 text-ui-meta text-muted-foreground">
          <span class="mode-label">Branch</span>
          <select
            class="max-w-56 rounded border border-input bg-background px-2 py-0.5 text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
            aria-label="Worktree branch mode"
            bind:value={addMode}
          >
            <option value="existing">existing branch</option>
            <option value="new">new branch</option>
            <option value="remote">remote branch</option>
          </select>
        </label>
        {#if addMode === "existing"}
          <select
            class="max-w-56 rounded border border-input bg-background px-2 py-0.5 text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
            aria-label="Existing branch for the worktree"
            bind:value={addBranch}
          >
            {#each candidates as branch (branch.name)}
              <option value={branch.name}>{branch.name}</option>
            {/each}
          </select>
        {:else if addMode === "remote"}
          <!-- Remote-branch mode: the local branch is created at the
               tracking tip automatically (`worktree_add` from_remote
               `<remote>/<branch>`). -->
          <Hint label="Creates a local branch tracking this remote branch, then checks the worktree out on it">
            {#snippet child({ props })}
              <select
                {...props}
                class="max-w-56 rounded border border-input bg-background px-2 py-0.5 font-mono text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
                aria-label="Remote branch for the worktree"
                bind:value={addRemoteBranch}
              >
                {#if remoteBranchSpecs.length === 0}
                  <option value="">(fetch to discover remote branches)</option>
                {/if}
                {#each remoteBranchSpecs as spec (spec)}
                  <option value={spec}>{spec}</option>
                {/each}
              </select>
            {/snippet}
          </Hint>
        {:else}
          <input
            class="new-branch min-w-32 flex-1 rounded border border-input bg-background px-2 py-0.5 font-mono text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
            type="text"
            placeholder="new branch name"
            aria-label="New branch name for the worktree"
            bind:value={addNewBranch}
          />
        {/if}
      </div>
    </form>
  {/if}

  {#if error}
    <p class="state error py-4 px-3 text-ui-base text-destructive" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <EmptyState title="No linked worktrees." />
  {:else}
    <ul class="worktrees m-0 flex min-h-0 flex-1 list-none flex-col gap-2 overflow-y-auto px-2 py-1.5" role="list" aria-label="Worktree list">
      <!-- Main entry: the backend lists linked worktrees only, so this
           repository's own root comes from the `root` prop. -->
      <li class="card main flex flex-col gap-1 rounded-lg border border-dashed border-border bg-card px-2 py-1.5">
        <div class="row flex min-w-0 flex-wrap items-center gap-1.5">
          <span class="name font-semibold text-primary-ink">{baseName(root)}</span>
          <span class="badge main-badge inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-secondary px-1.5 py-0 text-ui-micro font-medium text-secondary-foreground">main</span>
          <span class="ref ml-auto truncate text-ui-meta text-muted-foreground">this repository</span>
        </div>
        <Hint label={root}>
          {#snippet child({ props })}
            <div class="path truncate font-mono text-ui-micro text-muted-foreground" {...props}>{root}</div>
          {/snippet}
        </Hint>
        <div class="actions flex items-center gap-1">
          <Hint label="Open this repository's tab">
            {#snippet child({ props })}
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                {...props}
                aria-label="Focus this repository's tab"
                onclick={() => void onOpen({ name: baseName(root), path: root })}
              >
                <Icon icon={iconOpen} size={16} />
              </Button>
            {/snippet}
          </Hint>
        </div>
      </li>
      {#each linked as info (info.path)}
        <li class="card flex flex-col gap-1 rounded-lg border border-border bg-card px-2 py-1.5" aria-busy={removingPath === info.path}>
          <div class="row flex min-w-0 flex-wrap items-center gap-1.5">
            <span class="name font-semibold text-primary-ink">{info.name}</span>
            {#if info.is_main}
              <!-- M12: marks the repository's true main worktree (this tab is
                   itself a linked one). -->
              <Hint label="The repository's main worktree">
                {#snippet child({ props })}
                  <span class="badge main-badge inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-secondary px-1.5 py-0 text-ui-micro font-medium text-secondary-foreground" {...props}>main</span>
                {/snippet}
              </Hint>
            {/if}
            {#each worktreeBadges(info) as badge (badge.label)}
              <Hint label={badge.title}>
                {#snippet child({ props })}
                  <span
                    class="badge inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-muted px-1.5 py-0 text-ui-micro font-medium {badge.danger ? "bg-destructive/10 text-destructive" : "text-muted-foreground"}"
                    class:danger={badge.danger}
                    {...props}
                  >
                    {badge.label}
                  </span>
                {/snippet}
              </Hint>
            {/each}
            <span class="ref ml-auto truncate text-ui-meta text-muted-foreground">{worktreeRefLabel(info)}</span>
          </div>
          <Hint label={info.path}>
            {#snippet child({ props })}
              <div class="path truncate font-mono text-ui-micro text-muted-foreground" {...props}>{info.path}</div>
            {/snippet}
          </Hint>
          <div class="actions flex items-center gap-1">
            {#if removingPath === info.path}
              <!-- Removal in flight: row-level progress while the backend
                   deletes the admin area + working tree. -->
              <span class="removing animate-pulse text-ui-meta text-muted-foreground" role="status">Removing…</span>
            {:else}
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Open worktree ${info.name} as a tab`}
                title="Open this worktree as a tab"
                disabled={busy}
                onclick={() => void onOpen(info)}
              >
                <Icon icon={iconOpen} size={16} />
              </Button>
              {#if info.locked}
                <!-- Locked worktrees resist prune/remove — unlock here. -->
                <Hint label="Unlock this worktree (allows prune / remove)">
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                      variant="ghost"
                      size="icon-xs"
                      type="button"
                      aria-label={`Unlock worktree ${info.name}`}
                      title="Unlock this worktree"
                      disabled={busy}
                      onclick={() => void onUnlock(info)}
                    >
                      <Icon icon={iconUnlock} size={16} />
                    </Button>
                  {/snippet}
                </Hint>
              {:else}
                <Hint label="Lock this worktree (blocks prune / remove; optional reason)">
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                      variant="ghost"
                      size="icon-xs"
                      type="button"
                      aria-label={`Lock worktree ${info.name}`}
                      title="Lock this worktree"
                      disabled={busy}
                      onclick={() => onLock(info)}
                    >
                      <Icon icon={iconLock} size={16} />
                    </Button>
                  {/snippet}
                </Hint>
              {/if}
              <Button
                class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Remove worktree ${info.name}`}
                title="Remove this worktree"
                disabled={busy}
                onclick={() => onRemoveClick(info)}
              >
                <Icon icon={iconRemove} size={16} />
              </Button>
            {/if}
          </div>
          {#if lockReasonFor === info.path && !removingPath}
            <!-- Inline lock-reason prompt (optional text, like the stash
                 branch/rename forms). -->
            <form
              class="lock-form flex items-center gap-1.5 border-t border-dashed border-border pt-1"
              aria-label={`Lock reason for ${info.name}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onLockConfirmed(info);
              }}
            >
              <input
                class="min-w-32 flex-1 rounded border border-input bg-background px-2 py-0.5 text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
                type="text"
                placeholder="reason (optional)"
                aria-label="Lock reason"
                bind:value={lockReason}
              />
              <Button
                variant="default"
                size="icon-sm"
                type="submit"
                aria-label="Lock worktree"
                title="Lock this worktree"
                disabled={busy}
              >
                <Icon icon={iconCheck} size={16} />
              </Button>
            </form>
          {/if}
          {#if removeConfirmFor === info.path && removingPath !== info.path}
            <div class="remove-confirm flex items-center gap-2 border-t border-dashed border-border pt-1">
              {#if worktreeNeedsForce(info)}
                <Hint label="Required while the worktree is locked or prunable">
                  {#snippet child({ props })}
                    <label
                      class="toggle flex cursor-pointer items-center gap-1 whitespace-nowrap text-ui-micro text-muted-foreground"
                      {...props}
                    >
                      <input type="checkbox" class="m-0 accent-primary" bind:checked={removeForce} />
                      <span>force</span>
                    </label>
                  {/snippet}
                </Hint>
              {/if}
              <Button
                class="h-auto shrink-0 rounded-full px-3 py-0.5 text-ui-meta"
                variant="destructive"
                type="button"
                disabled={busy}
                onclick={() => void onRemove(info)}
              >
                Confirm remove
              </Button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<ConfirmDialog
  bind:open={
    () => removeDialogFor !== null,
    (v) => {
      if (!v) removeDialogFor = null;
    }
  }
  title={removeDialogFor ? `Remove worktree “${removeDialogFor.name}”?` : ""}
  message={removeDialogFor
    ? `${removeDialogFor.path}${worktreeNeedsForce(removeDialogFor) ? " — locked or prunable, force required" : ""}`
    : ""}
  confirmLabel="Remove worktree"
  danger
  onConfirm={() => void onRemoveConfirmed()}
/>
