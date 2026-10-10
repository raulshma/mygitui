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

<aside class="worktree-panel" aria-label="Worktrees">
  <div class="toolbar">
    <button class="act" type="button" onclick={() => (formOpen = !formOpen)} disabled={busy}>
      {formOpen ? "Close form" : "Add worktree"}
    </button>
    <!-- M12: git worktree prune — drops stale admin dirs for deleted paths. -->
    <button
      class="act"
      type="button"
      disabled={busy}
      title="git worktree prune — remove administration entries for deleted worktree folders"
      onclick={() => void onPrune()}
    >
      Prune stale…
    </button>
    <button class="tb" type="button" onclick={() => void reload()} disabled={loading}>
      {loading ? "Loading…" : "Refresh"}
    </button>
  </div>

  {#if formOpen}
    <form
      class="add-form"
      aria-label="Add worktree"
      onsubmit={(e) => {
        e.preventDefault();
        void onAdd();
      }}
    >
      <div class="path-row">
        <input
          class="path"
          type="text"
          placeholder="Worktree folder path"
          aria-label="Worktree folder path"
          bind:value={addPath}
        />
        <button class="tb" type="button" aria-label="Browse for worktree folder" onclick={() => void browse()}>
          Browse…
        </button>
      </div>
      <div class="mode-row">
        <label class="mode">
          <span class="mode-label">Branch</span>
          <select
            aria-label="Worktree branch mode"
            bind:value={addMode}
          >
            <option value="existing">existing branch</option>
            <option value="new">new branch</option>
          </select>
        </label>
        {#if addMode === "existing"}
          <select aria-label="Existing branch for the worktree" bind:value={addBranch}>
            {#each candidates as branch (branch.name)}
              <option value={branch.name}>{branch.name}</option>
            {/each}
          </select>
        {:else}
          <input
            class="new-branch"
            type="text"
            placeholder="new branch name"
            aria-label="New branch name for the worktree"
            bind:value={addNewBranch}
          />
        {/if}
      </div>
      <button
        class="go"
        type="submit"
        disabled={busy || !addPath.trim() || (addMode === "new" && !addNewBranch.trim())}
      >
        Add worktree
      </button>
    </form>
  {/if}

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state">No linked worktrees.</p>
  {:else}
    <ul class="worktrees" role="list" aria-label="Worktree list">
      <!-- Main entry: the backend lists linked worktrees only, so this
           repository's own root comes from the `root` prop. -->
      <li class="card main">
        <div class="row">
          <span class="name">{baseName(root)}</span>
          <span class="badge main-badge">main</span>
          <span class="ref">this repository</span>
        </div>
        <div class="path" title={root}>{root}</div>
        <div class="actions">
          <button
            class="tb"
            type="button"
            aria-label="Focus this repository's tab"
            onclick={() => void onOpen({ name: baseName(root), path: root })}
          >
            Open
          </button>
        </div>
      </li>
      {#each linked as info (info.path)}
        <li class="card" aria-busy={removingPath === info.path}>
          <div class="row">
            <span class="name">{info.name}</span>
            {#if info.is_main}
              <!-- M12: marks the repository's true main worktree (this tab is
                   itself a linked one). -->
              <span class="badge main-badge" title="The repository's main worktree">main</span>
            {/if}
            {#each worktreeBadges(info) as badge (badge.label)}
              <span
                class="badge"
                class:danger={badge.danger}
                title={badge.title}
              >
                {badge.label}
              </span>
            {/each}
            <span class="ref">{worktreeRefLabel(info)}</span>
          </div>
          <div class="path" title={info.path}>{info.path}</div>
          <div class="actions">
            {#if removingPath === info.path}
              <!-- Removal in flight: row-level progress while the backend
                   deletes the admin area + working tree. -->
              <span class="removing" role="status">Removing…</span>
            {:else}
              <button
                class="tb"
                type="button"
                disabled={busy}
                aria-label={`Open worktree ${info.name} as a tab`}
                onclick={() => void onOpen(info)}
              >
                Open
              </button>
              <button
                class="tb danger"
                type="button"
                disabled={busy}
                aria-label={`Remove worktree ${info.name}`}
                onclick={() => onRemoveClick(info)}
              >
                Remove
              </button>
            {/if}
          </div>
          {#if removeConfirmFor === info.path && removingPath !== info.path}
            <div class="remove-confirm">
              {#if worktreeNeedsForce(info)}
                <label class="toggle" title="Required while the worktree is locked or prunable">
                  <input type="checkbox" bind:checked={removeForce} />
                  <span>force</span>
                </label>
              {/if}
              <button
                class="go danger-go"
                type="button"
                disabled={busy}
                onclick={() => void onRemove(info)}
              >
                Confirm remove
              </button>
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

<style>
  .worktree-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .add-form {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex: none;
  }

  .path-row,
  .mode-row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
  }

  .path,
  .new-branch {
    flex: 1;
    min-width: 10rem;
    font: inherit;
    font-size: 0.72rem;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  .new-branch {
    min-width: 8rem;
  }

  .path:focus-visible,
  .new-branch:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .mode {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  select {
    font: inherit;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.3rem;
    max-width: 14rem;
  }

  select:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .worktrees {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    padding: 0.375rem 0.5rem;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .card {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    padding: 0.375rem 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .card.main {
    border-style: dashed;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
    flex-wrap: wrap;
  }

  .name {
    font-weight: 600;
    color: var(--m3-primary);
  }

  .badge {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.625rem;
    padding: 0.05rem 0.45rem;
  }

  .badge.main-badge {
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
  }

  .badge.danger {
    background: var(--m3-error-container, var(--m3-surface-container-highest));
    color: var(--m3-on-error-container, var(--m3-on-surface));
  }

  .ref {
    margin-left: auto;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .path {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .remove-confirm {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding-top: 0.25rem;
    border-top: 1px dashed var(--m3-outline-variant, var(--m3-primary));
  }

  .removing {
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    animation: removing-pulse 1.2s ease-in-out infinite;
  }

  @keyframes removing-pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.45;
    }
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .toggle input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .tb {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb:focus-visible,
  .act:focus-visible,
  .go:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .danger {
    color: var(--m3-error, inherit);
  }

  .act {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .go {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .go.danger-go {
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, var(--m3-on-primary));
  }

  .tb:disabled,
  .act:disabled,
  .go:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .state {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 1rem 0.75rem;
    margin: 0;
    font-size: 0.8125rem;
  }

  .error {
    color: var(--m3-error, inherit);
  }
</style>
