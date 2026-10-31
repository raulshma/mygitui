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
   */
  import {
    branches,
    pickFolder,
    worktreeAdd,
    worktreePrune,
    worktreeRemove,
    worktrees,
  } from "$lib/ipc/client";
  import type { BranchInfo, WorktreeInfo } from "$lib/ipc/types";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
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
  /** "existing" = check out an existing branch; "new" = create one. */
  let addMode = $state<"existing" | "new">("existing");
  let addBranch = $state("");
  let addNewBranch = $state("");

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
      await reload();
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
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      const [wt, br] = await Promise.all([worktrees(repoId), branches(repoId)]);
      list = wt;
      allBranches = br;
      // Keep the existing-branch select on a valid choice (repo switches
      // can invalidate the previous pick).
      const names = new Set(candidates.map((b) => b.name));
      if (!names.has(addBranch)) {
        addBranch = candidates[0]?.name ?? "";
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function fail(what: string, err: unknown): void {
    toast(`${what} failed: ${err instanceof Error ? err.message : String(err)}`, {
      kind: "error",
    });
  }

  function afterMutation(what: string, detail: string): void {
    toast(`${what}: ${detail}`, { kind: "success" });
    void reload();
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
      void reload();
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
      } else {
        const name = addNewBranch.trim();
        if (!name) {
          busy = false;
          return;
        }
        await worktreeAdd(repoId, path, undefined, name);
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
</script>

<aside class="worktree-panel flex min-h-0 flex-col text-[13px]" aria-label="Worktrees">
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
          class="path min-w-40 flex-1 rounded border border-input bg-background px-2 py-0.5 font-mono text-[0.72rem] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
          type="text"
          placeholder="Worktree folder path"
          aria-label="Worktree folder path"
          bind:value={addPath}
        />
        <Button
          class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Browse for worktree folder"
          title="Browse for a folder"
          onclick={() => void browse()}
        >
          <Icon icon={iconBrowse} size={16} />
        </Button>
        <Button
          variant="default"
          size="icon-sm"
          type="submit"
          aria-label="Create worktree"
          title="Create the linked worktree"
          disabled={busy || !addPath.trim() || (addMode === "new" && !addNewBranch.trim())}
        >
          <Icon icon={iconAdd} size={16} />
        </Button>
      </div>
      <div class="mode-row flex flex-wrap items-center gap-1.5">
        <label class="mode flex items-center gap-1.5 text-[0.72rem] text-muted-foreground">
          <span class="mode-label">Branch</span>
          <select
            class="max-w-56 rounded border border-input bg-background px-2 py-0.5 text-[0.72rem] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
            aria-label="Worktree branch mode"
            bind:value={addMode}
          >
            <option value="existing">existing branch</option>
            <option value="new">new branch</option>
          </select>
        </label>
        {#if addMode === "existing"}
          <select
            class="max-w-56 rounded border border-input bg-background px-2 py-0.5 text-[0.72rem] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
            aria-label="Existing branch for the worktree"
            bind:value={addBranch}
          >
            {#each candidates as branch (branch.name)}
              <option value={branch.name}>{branch.name}</option>
            {/each}
          </select>
        {:else}
          <input
            class="new-branch min-w-32 flex-1 rounded border border-input bg-background px-2 py-0.5 font-mono text-[0.72rem] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
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
    <p class="state error py-4 px-3 text-[13px] text-destructive" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state py-4 px-3 text-[13px] text-muted-foreground">No linked worktrees.</p>
  {:else}
    <ul class="worktrees m-0 flex min-h-0 flex-1 list-none flex-col gap-2 overflow-y-auto px-2 py-1.5" role="list" aria-label="Worktree list">
      <!-- Main entry: the backend lists linked worktrees only, so this
           repository's own root comes from the `root` prop. -->
      <li class="card main flex flex-col gap-1 rounded-lg border border-dashed border-border bg-card px-2 py-1.5">
        <div class="row flex min-w-0 flex-wrap items-center gap-1.5">
          <span class="name font-semibold text-primary">{baseName(root)}</span>
          <span class="badge main-badge inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-secondary px-1.5 py-0 text-[0.625rem] font-medium text-secondary-foreground">main</span>
          <span class="ref ml-auto truncate text-[0.72rem] text-muted-foreground">this repository</span>
        </div>
        <div class="path truncate font-mono text-[0.6875rem] text-muted-foreground" title={root}>{root}</div>
        <div class="actions flex items-center gap-1">
          <Button
            class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label="Focus this repository's tab"
            title="Open this repository's tab"
            onclick={() => void onOpen({ name: baseName(root), path: root })}
          >
            <Icon icon={iconOpen} size={16} />
          </Button>
        </div>
      </li>
      {#each linked as info (info.path)}
        <li class="card flex flex-col gap-1 rounded-lg border border-border bg-card px-2 py-1.5" aria-busy={removingPath === info.path}>
          <div class="row flex min-w-0 flex-wrap items-center gap-1.5">
            <span class="name font-semibold text-primary">{info.name}</span>
            {#if info.is_main}
              <!-- M12: marks the repository's true main worktree (this tab is
                   itself a linked one). -->
              <span class="badge main-badge inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-secondary px-1.5 py-0 text-[0.625rem] font-medium text-secondary-foreground" title="The repository's main worktree">main</span>
            {/if}
            {#each worktreeBadges(info) as badge (badge.label)}
              <span
                class="badge inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-muted px-1.5 py-0 text-[0.625rem] font-medium {badge.danger ? "bg-destructive/10 text-destructive" : "text-muted-foreground"}"
                class:danger={badge.danger}
                title={badge.title}
              >
                {badge.label}
              </span>
            {/each}
            <span class="ref ml-auto truncate text-[0.72rem] text-muted-foreground">{worktreeRefLabel(info)}</span>
          </div>
          <div class="path truncate font-mono text-[0.6875rem] text-muted-foreground" title={info.path}>{info.path}</div>
          <div class="actions flex items-center gap-1">
            {#if removingPath === info.path}
              <!-- Removal in flight: row-level progress while the backend
                   deletes the admin area + working tree. -->
              <span class="removing animate-pulse text-[0.72rem] text-muted-foreground" role="status">Removing…</span>
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
          {#if removeConfirmFor === info.path && removingPath !== info.path}
            <div class="remove-confirm flex items-center gap-2 border-t border-dashed border-border pt-1">
              {#if worktreeNeedsForce(info)}
                <label
                  class="toggle flex cursor-pointer items-center gap-1 whitespace-nowrap text-[0.6875rem] text-muted-foreground"
                  title="Required while the worktree is locked or prunable"
                >
                  <input type="checkbox" class="m-0 accent-primary" bind:checked={removeForce} />
                  <span>force</span>
                </label>
              {/if}
              <Button
                class="h-auto shrink-0 rounded-full px-3 py-0.5 text-[0.72rem]"
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
