<script lang="ts">
  /**
   * Branch panel (M2, extended in M3) — local branches +
   * create/switch/rename/delete + reset.
   *
   * Rows show name, short sha, upstream with ↑ahead/↓behind, a "gone" badge
   * when the upstream disappeared, and a HEAD marker on the current branch.
   *
   * Switching with a dirty worktree offers a real choice: "Stash changes"
   * (stash_push → switch) or "Proceed anyway" (forced checkout); clean
   * switches stay one click. Deleting previews the operation first
   * (`ops_preview`): merged branches get a plain confirm; unmerged ones open
   * the PreviewDialog — type the branch name, optionally keep the undo
   * checkpoint (guard_checkpoint "pre-branch-delete"), then force-delete.
   * "Reset to…" per row opens the ResetDialog (reset the current branch to
   * that branch — gitk semantics). Every mutation reloads the list and
   * notifies the owner (`onMutated` → RepoView refreshStatus). Remote
   * branches land later; the panel is local-only for now.
   */
  import {
    branchCreate,
    branchDelete,
    branchRename,
    branchSwitch,
    branches,
    guardCheckpoint,
    opsPreview,
    stashPush,
  } from "$lib/ipc/client";
  import type { BranchInfo, PreviewInfo } from "$lib/ipc/types";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { busy } from "$lib/stores/ops.svelte";
  import { toast } from "$lib/toast";
  import PreviewDialog from "$lib/components/safety/PreviewDialog.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import { isMergedPreview } from "$lib/components/safety/safetyModel";

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Called after a successful mutation (RepoView refreshes). */
    onMutated?: () => void;
  } = $props();

  let list = $state<BranchInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Create form
  let newName = $state("");
  let newFrom = $state("");
  let newCheckout = $state(true);

  // Inline row editors
  let renaming = $state<{ old: string; value: string } | null>(null);

  // M3 flows
  let switchChoice = $state<{ name: string } | null>(null);
  let previewDelete = $state<{ name: string; info: PreviewInfo } | null>(null);
  let resetTo = $state<string | null>(null);
  let stashButton = $state<HTMLButtonElement | undefined>();

  const branchBusy = $derived(busy(repoId, "branch"));
  const headBranch = $derived(list.find((b) => b.is_head) ?? null);

  // Focus the primary choice when the switch dialog appears.
  $effect(() => {
    if (switchChoice) {
      requestAnimationFrame(() => stashButton?.focus());
    }
  });

  $effect(() => {
    // Reload when the repo switches.
    void repoId;
    renaming = null;
    switchChoice = null;
    previewDelete = null;
    resetTo = null;
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      list = await branches(repoId);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  /** True when the repo's working copy has entries (dirty). */
  function dirtyWorktree(): boolean {
    const tab = tabStore.tabs.find((t) => t.id === repoId);
    return Boolean(tab?.status && tab.status.entries.length > 0);
  }

  function afterMutation(): void {
    void reload();
    onMutated?.();
  }

  async function run(label: string, run: () => Promise<void>): Promise<void> {
    try {
      await run();
      afterMutation();
    } catch (err) {
      toast(`${label} failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }

  async function onCreate(): Promise<void> {
    const name = newName.trim();
    if (!name) return;
    await run(`Create branch ${name}`, () =>
      branchCreate(repoId, name, newCheckout, newFrom || undefined),
    );
    newName = "";
    newFrom = "";
    newCheckout = true;
  }

  async function onSwitch(branch: BranchInfo): Promise<void> {
    if (dirtyWorktree()) {
      // Real choice replaces the old "M3 adds auto-stash" hint.
      switchChoice = { name: branch.name };
      return;
    }
    await doSwitch(branch.name, false);
  }

  async function doSwitch(name: string, force: boolean): Promise<void> {
    switchChoice = null;
    await run(`Switch to ${name}`, () => branchSwitch(repoId, name, force));
  }

  /** "Stash changes": stash_push → clean switch. */
  async function onSwitchStash(): Promise<void> {
    if (!switchChoice) return;
    const name = switchChoice.name;
    switchChoice = null;
    await run(`Switch to ${name}`, async () => {
      await stashPush(repoId, `pre-switch stash (switching to ${name})`);
      await branchSwitch(repoId, name, false);
    });
  }

  /** "Proceed anyway": forced checkout (discards local modifications). */
  async function onSwitchForce(): Promise<void> {
    if (!switchChoice) return;
    const name = switchChoice.name;
    await doSwitch(name, true);
  }

  async function onRename(): Promise<void> {
    if (!renaming) return;
    const value = renaming.value.trim();
    if (!value || value === renaming.old) {
      renaming = null;
      return;
    }
    const old = renaming.old;
    renaming = null;
    await run(`Rename ${old} → ${value}`, () => branchRename(repoId, old, value));
  }

  /**
   * Starts a delete: preview first. Merged → plain confirm (safe delete);
   * unmerged → PreviewDialog with type-the-name + undo checkpoint.
   */
  async function onDeleteStart(branch: BranchInfo): Promise<void> {
    const into = headBranch?.name ?? "HEAD";
    let info: PreviewInfo;
    try {
      info = await opsPreview(repoId, "branch_delete", {
        name: branch.name,
        into,
      });
    } catch (err) {
      toast(
        `Cannot preview delete of ${branch.name}: ${
          err instanceof Error ? err.message : String(err)
        }`,
        { kind: "error" },
      );
      return;
    }
    if (isMergedPreview(info)) {
      if (window.confirm(`Delete branch ${branch.name}? (${info.summary})`)) {
        await run(`Delete ${branch.name}`, () =>
          branchDelete(repoId, branch.name, false),
        );
      }
      return;
    }
    previewDelete = { name: branch.name, info };
  }

  /** PreviewDialog confirmed: optional guard checkpoint, then force delete. */
  async function onDeleteConfirm(createCheckpoint: boolean): Promise<void> {
    const job = previewDelete;
    if (!job) return;
    previewDelete = null;
    await run(`Force delete ${job.name}`, async () => {
      if (createCheckpoint) {
        const cp = await guardCheckpoint(repoId, "pre-branch-delete");
        await branchDelete(repoId, job.name, true);
        toast(
          `Force deleted ${job.name} — undo available (checkpoint ${cp.id})`,
          { kind: "success" },
        );
      } else {
        await branchDelete(repoId, job.name, true);
      }
    });
  }

  function shortSha(sha: string): string {
    return sha.slice(0, 7);
  }
</script>

<aside class="branch-panel" aria-label="Branches">
  <div class="toolbar">
    <button class="tb" type="button" onclick={() => void reload()} disabled={loading}>
      {loading ? "Loading…" : "Refresh"}
    </button>
    {#if branchBusy}<span class="busy" role="status">Branch op running…</span>{/if}
  </div>

  <form
    class="create"
    aria-label="Create branch"
    onsubmit={(e) => {
      e.preventDefault();
      void onCreate();
    }}
  >
    <input
      class="name"
      type="text"
      placeholder="New branch name"
      aria-label="New branch name"
      bind:value={newName}
    />
    <label class="from">
      <span class="fl">from</span>
      <select bind:value={newFrom} aria-label="Create branch from">
        <option value="">HEAD</option>
        {#each list as b (b.name)}
          <option value={b.name}>{b.name}</option>
        {/each}
      </select>
    </label>
    <label class="toggle" title="Check out the new branch after creating it">
      <input type="checkbox" bind:checked={newCheckout} />
      <span>checkout</span>
    </label>
    <button class="go" type="submit" disabled={!newName.trim()}>
      Create
    </button>
  </form>

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state">No branches.</p>
  {:else}
    <ul class="branches" role="list" aria-label="Local branches">
      {#each list as branch (branch.name)}
        <li class="row" class:head={branch.is_head}>
          {#if renaming?.old === branch.name}
            <form
              class="rename"
              aria-label={`Rename ${branch.name}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onRename();
              }}
            >
              <input
                class="name"
                type="text"
                aria-label={`New name for ${branch.name}`}
                bind:value={renaming.value}
                onkeydown={(e) => {
                  if (e.key === "Escape") renaming = null;
                }}
              />
              <button class="go" type="submit">Save</button>
              <button class="tb" type="button" onclick={() => (renaming = null)}>
                Cancel
              </button>
            </form>
          {:else}
            <span class="marker" aria-label="Current branch" title="Current branch">
              {branch.is_head ? "●" : ""}
            </span>
            <span class="bname" title={branch.name}>{branch.name}</span>
            <span class="sha">{shortSha(branch.sha)}</span>
            {#if branch.upstream}
              <span class="upstream" title={branch.upstream}>
                {branch.upstream}
                {#if branch.ahead > 0 || branch.behind > 0}
                  <span class="ab">↑{branch.ahead} ↓{branch.behind}</span>
                {/if}
                {#if branch.gone}<span class="gone">gone</span>{/if}
              </span>
            {/if}
            <span class="actions">
              {#if !branch.is_head}
                <button class="tb" type="button" onclick={() => void onSwitch(branch)}>
                  Switch
                </button>
              {:else}
                <span class="head-chip">HEAD</span>
              {/if}
              <button
                class="tb"
                type="button"
                aria-label={`Rename ${branch.name}`}
                onclick={() => (renaming = { old: branch.name, value: branch.name })}
              >
                Rename
              </button>
              <button
                class="tb"
                type="button"
                aria-label={`Reset current branch to ${branch.name}`}
                title="Reset current branch to here ({branch.name})"
                onclick={() => (resetTo = branch.name)}
              >
                Reset to…
              </button>
              <button
                class="tb danger"
                type="button"
                aria-label={`Delete ${branch.name}`}
                disabled={branch.is_head}
                title={branch.is_head ? "Cannot delete the current branch" : "Delete"}
                onclick={() => void onDeleteStart(branch)}
              >
                Delete
              </button>
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if switchChoice}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="scrim"
      role="presentation"
      onkeydown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          switchChoice = null;
        }
      }}
    >
      <div
        class="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="switch-choice-title"
      >
        <h2 id="switch-choice-title" class="title">Uncommitted changes</h2>
        <p class="text">
          You have uncommitted changes. What should happen to them when
          switching to <strong>{switchChoice.name}</strong>?
        </p>
        <div class="actions">
          <button
            class="primary"
            type="button"
            bind:this={stashButton}
            onclick={() => void onSwitchStash()}
          >
            Stash changes
          </button>
          <button class="danger-btn" type="button" onclick={() => void onSwitchForce()}>
            Proceed anyway
          </button>
          <button class="secondary" type="button" onclick={() => (switchChoice = null)}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  {/if}

  {#if previewDelete}
    <PreviewDialog
      info={previewDelete.info}
      danger
      confirmPhrase={previewDelete.name}
      title={`Force delete ${previewDelete.name}`}
      onConfirm={(createCheckpoint) => void onDeleteConfirm(createCheckpoint)}
      onCancel={() => (previewDelete = null)}
    />
  {/if}

  <ResetDialog
    {repoId}
    open={resetTo !== null}
    defaultTarget={resetTo ?? undefined}
    onClose={() => (resetTo = null)}
    onDone={() => {
      resetTo = null;
      afterMutation();
    }}
  />
</aside>

<style>
  .branch-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .busy {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  .create {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0 0.5rem 0.375rem;
    flex: none;
    flex-wrap: wrap;
  }

  .create .name,
  .rename .name {
    flex: 1;
    min-width: 6rem;
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  .create .name:focus-visible,
  .rename .name:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .from {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .fl {
    font-size: 0.6875rem;
  }

  select {
    font: inherit;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.3rem;
    max-width: 9rem;
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
    cursor: pointer;
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
  .go:focus-visible,
  .danger:focus-visible,
  .primary:focus-visible,
  .secondary:focus-visible,
  .danger-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .tb:disabled {
    cursor: not-allowed;
    opacity: 0.55;
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

  .go:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .danger {
    color: var(--m3-error, inherit);
  }

  .branches {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    padding: 0 0 0.75rem;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.375rem;
    padding: 0.2rem 0.5rem 0.2rem 0.25rem;
    min-width: 0;
  }

  .row:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .row.head {
    background: var(--m3-secondary-container, transparent);
  }

  .marker {
    flex: none;
    width: 0.875rem;
    text-align: center;
    color: var(--m3-primary);
  }

  .bname {
    flex: 0 1 auto;
    min-width: 3rem;
    max-width: 11rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .sha {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .upstream {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
    font-family: ui-monospace, Consolas, monospace;
  }

  .ab {
    color: var(--m3-primary);
    padding-left: 0.375rem;
  }

  .gone {
    margin-left: 0.375rem;
    padding: 0 0.35rem;
    border: 1px solid var(--m3-error, currentColor);
    border-radius: var(--m3-shape-full, 9999px);
    color: var(--m3-error, inherit);
    font-size: 0.625rem;
  }

  .head-chip {
    flex: none;
    padding: 0.05rem 0.45rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-inverse-surface, var(--m3-surface-container-highest));
    color: var(--m3-inverse-on-surface, var(--m3-on-surface));
    font-size: 0.625rem;
    font-weight: 600;
    letter-spacing: 0.04em;
  }

  .actions {
    margin-left: auto;
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .rename {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex: 1;
    min-width: 0;
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

  /* Switch dirty-choice modal (same pattern as AuthDialog). */
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(24rem, calc(100vw - 2rem));
    padding: 1.25rem 1.5rem;
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  .title {
    margin: 0 0 0.5rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .text {
    margin: 0 0 1rem;
    color: var(--m3-on-surface);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .primary {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-weight: 500;
    padding: 0.45rem 1.25rem;
    cursor: pointer;
  }

  .danger-btn {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, white);
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .secondary {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }
</style>
