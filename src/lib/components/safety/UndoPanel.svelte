<script lang="ts">
  /**
   * Undo panel (M3 safety) — browse and restore checkpoints.
   *
   * Lists the repo's checkpoints newest-first (reason · age · branch, plus
   * a "worktree" chip when the snapshot includes working-copy state).
   * Restore asks for confirmation first (exact-restore semantics: the repo
   * returns to the snapshot, later untracked files are removed, and the
   * current state is auto-checkpointed as "pre-restore"), then calls
   * `checkpoint_restore`, toasts and refreshes. GC removes checkpoints
   * older than 30 days (`checkpoint_gc`) and toasts the count.
   *
   * No polling: loads on mount, via the Refresh button, and reloads after
   * each action. The checkpoint cache itself lives in the safety store
   * (`$lib/stores/safety.svelte`).
   */
  import {
    branchTrashList,
    branchTrashRestore,
    checkpointGc,
    checkpointRestore,
  } from "$lib/ipc/client";
  import type { BranchTrashEntry, CheckpointInfo } from "$lib/ipc/types";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { loadUndo, safetyStore } from "$lib/stores/safety.svelte";
  import { toast } from "$lib/toast";
  import { formatDateTime } from "$lib/stores/history-logic";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import { checkpointTitle } from "./safetyModel";

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Called after restore/GC (owner refreshes; RepoView refreshStatus). */
    onMutated?: () => void;
  } = $props();

  // `state` re-reads the rune store on every access inside the template.
  const undo = $derived(safetyStore.stateFor(repoId));
  let confirmingRestore = $state<string | null>(null);
  let busyId = $state<string | null>(null);
  let gcBusy = $state(false);

  $effect(() => {
    // Reload when the repo switches (panel open = both sections refresh).
    void repoId;
    confirmingRestore = null;
    restoringTrash = null;
    void loadUndo(repoId);
    void loadTrash();
  });

  async function reload(): Promise<void> {
    await loadUndo(repoId);
  }

  function afterMutation(): void {
    void refreshStatus(repoId);
    onMutated?.();
  }

  function askRestore(cp: CheckpointInfo): void {
    confirmingRestore = cp.id;
  }

  async function doRestore(cp: CheckpointInfo): Promise<void> {
    confirmingRestore = null;
    busyId = cp.id;
    try {
      await checkpointRestore(repoId, cp.id);
      toast(`Restored checkpoint ${cp.id} — previous state saved as pre-restore`, {
        kind: "success",
      });
      afterMutation();
    } catch (err) {
      toast(
        `Restore failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      busyId = null;
      await reload();
    }
  }

  /** GC confirmation (ConfirmDialog state, M9 F9). */
  let gcOpen = $state(false);

  function doGc(): void {
    gcOpen = true;
  }

  async function doGcConfirmed(): Promise<void> {
    gcBusy = true;
    try {
      const removed = await checkpointGc(repoId, 30);
      toast(
        removed === 0
          ? "No checkpoints older than 30 days"
          : `Removed ${removed} old checkpoint${removed === 1 ? "" : "s"}`,
        { kind: "success" },
      );
    } catch (err) {
      toast(
        `Clean failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      gcBusy = false;
      await reload();
    }
  }

  // -- M12: deleted branches (branch trash) ------------------------------------
  // Force-deleted branches live under refs/mygitui/trash/* until checkpoint
  // GC cleans them; Restore recreates the branch (optionally renamed).

  let trash = $state<BranchTrashEntry[]>([]);
  let trashLoading = $state(false);
  let trashError = $state<string | null>(null);
  /** Trash entry whose restore PromptDialog is open. */
  let restoringTrash = $state<BranchTrashEntry | null>(null);
  let restoreBusy = $state(false);

  async function loadTrash(): Promise<void> {
    trashLoading = true;
    trashError = null;
    try {
      trash = await branchTrashList(repoId);
    } catch (err) {
      trashError = err instanceof Error ? err.message : String(err);
    } finally {
      trashLoading = false;
    }
  }

  async function doTrashRestore(entry: BranchTrashEntry, newName: string): Promise<void> {
    restoringTrash = null;
    restoreBusy = true;
    try {
      // Same name → no `new_name` (backend errors on an existing name);
      // a renamed restore passes the new name through.
      const name = await branchTrashRestore(
        repoId,
        entry.id,
        newName === entry.name ? undefined : newName,
      );
      toast(`Restored branch ${name}`, { kind: "success" });
      await loadTrash();
      // No branch-list event exists — refresh status (branch name chips) too.
      void refreshStatus(repoId);
      onMutated?.();
    } catch (err) {
      toast(
        `Restore failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      restoreBusy = false;
    }
  }
</script>

<section class="undo-panel" aria-label="Checkpoints and undo">
  <div class="toolbar">
    <button class="tb" type="button" onclick={() => void reload()} disabled={undo.loading}>
      {undo.loading ? "Loading…" : "Refresh"}
    </button>
    <button class="tb" type="button" onclick={() => void doGc()} disabled={gcBusy}>
      {gcBusy ? "Cleaning…" : "Clean >30d"}
    </button>
    <span class="hint" title="Checkpoints are automatic snapshots created before dangerous operations">
      {undo.checkpoints.length} checkpoint{undo.checkpoints.length === 1 ? "" : "s"}
    </span>
  </div>

  {#if undo.error}
    <p class="state error" role="alert">{undo.error}</p>
  {:else if !undo.loading && undo.checkpoints.length === 0}
    <div class="empty">
      <p class="empty-title">No checkpoints yet.</p>
      <p class="empty-text">
        Checkpoints are automatic snapshots mygitui creates before dangerous
        operations (hard resets, force branch deletes, restores). Restoring a
        checkpoint returns the repository to that exact snapshot; the state at
        restore time is itself saved as a new checkpoint, so undo is always
        available.
      </p>
    </div>
  {:else}
    <ul class="checkpoints" role="list" aria-label="Checkpoints, newest first">
      {#each undo.checkpoints as cp (cp.id)}
        <li class="row">
          {#if confirmingRestore === cp.id}
            <div
              class="confirm-restore"
              role="alertdialog"
              aria-label={`Restore checkpoint ${cp.id}`}
            >
              <p class="cr-text">
                Restore returns the repo to this snapshot: changes made since
                are discarded and untracked files created after it are removed.
                The current state is checkpointed first (pre-restore), so this
                is undoable.
              </p>
              <div class="cr-actions">
                <button
                  class="danger-btn"
                  type="button"
                  disabled={busyId === cp.id}
                  onclick={() => void doRestore(cp)}
                >
                  {busyId === cp.id ? "Restoring…" : "Restore"}
                </button>
                <button
                  class="tb"
                  type="button"
                  onclick={() => (confirmingRestore = null)}
                >
                  Cancel
                </button>
              </div>
            </div>
          {:else}
            <div class="info">
              <span class="title" title={cp.ref_name}>{checkpointTitle(cp, Math.floor(Date.now() / 1000))}</span>
              <span class="chips">
                {#if cp.branch}<span class="chip">{cp.branch}</span>{/if}
                {#if cp.has_worktree_state}
                  <span class="chip worktree" title="Snapshot includes working-copy + index state">worktree</span>
                {/if}
              </span>
            </div>
            <span class="actions">
              <button
                class="tb"
                type="button"
                aria-label={`Restore checkpoint ${cp.id}`}
                disabled={busyId !== null}
                onclick={() => askRestore(cp)}
              >
                Restore
              </button>
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <!-- M12: force-deleted branches preserved under refs/mygitui/trash/* -->
  <section class="trash" aria-label="Deleted branches">
    <header class="trash-head">
      <h2 class="trash-title">Deleted branches</h2>
      <button class="tb" type="button" onclick={() => void loadTrash()} disabled={trashLoading}>
        {trashLoading ? "Loading…" : "Refresh"}
      </button>
    </header>

    {#if trashError}
      <p class="state error" role="alert">{trashError}</p>
    {:else if !trashLoading && trash.length === 0}
      <p class="trash-empty">
        No deleted branches. Force-deleted branches are kept here (under
        <code>refs/mygitui/trash/*</code>) so their commits stay recoverable
        until checkpoint GC cleans them up.
      </p>
    {:else}
      <ul class="trash-list" role="list" aria-label="Deleted branches, newest first">
        {#each trash as entry (entry.id)}
          <li class="row">
            <div class="info">
              <span class="title" title={entry.name}>{entry.name}</span>
              <span class="chips">
                <span class="chip sha-chip" title={entry.sha}>{entry.sha.slice(0, 7)}</span>
                <span class="chip" title="Deleted {formatDateTime(entry.deleted_at)}">
                  deleted {formatDateTime(entry.deleted_at)}
                </span>
              </span>
            </div>
            <span class="actions">
              <button
                class="tb"
                type="button"
                aria-label={`Restore branch ${entry.name}`}
                disabled={restoreBusy}
                onclick={() => (restoringTrash = entry)}
              >
                Restore
              </button>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</section>

<ConfirmDialog
  bind:open={gcOpen}
  title="Clean old checkpoints?"
  message="Checkpoints older than 30 days are removed; the newest is always kept."
  confirmLabel="Clean"
  onConfirm={() => void doGcConfirmed()}
/>

{#if restoringTrash}
  <PromptDialog
    open={true}
    title={`Restore branch ${restoringTrash.name}`}
    message="Restores the branch at its deleted tip. Keep the name (or edit it to restore under a new name — useful when the original was recreated)."
    initial={restoringTrash.name}
    confirmLabel="Restore branch"
    onSubmit={(value) => void doTrashRestore(restoringTrash!, value)}
    onCancel={() => (restoringTrash = null)}
  />
{/if}

<style>
  .undo-panel {
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

  .tb:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .tb:focus-visible,
  .danger-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .hint {
    margin-left: auto;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
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

  .empty {
    padding: 1rem 0.75rem;
  }

  .empty-title {
    margin: 0 0 0.35rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .empty-text {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    line-height: 1.45;
  }

  .checkpoints {
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
    gap: 0.375rem;
    padding: 0.25rem 0.5rem;
    min-width: 0;
  }

  .row:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .info {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    flex: 1;
    min-width: 0;
  }

  .title {
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chips {
    display: flex;
    gap: 0.25rem;
    flex-wrap: wrap;
  }

  .chip {
    padding: 0 0.4rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.625rem;
    max-width: 10rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip.worktree {
    color: var(--m3-primary);
    border-color: var(--m3-primary);
  }

  .actions {
    margin-left: auto;
    flex: none;
  }

  .confirm-restore {
    flex: 1;
    min-width: 0;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--m3-error, currentColor);
    border-radius: var(--m3-shape-small, 8px);
  }

  .cr-text {
    margin: 0 0 0.4rem;
    color: var(--m3-on-surface);
    font-size: 0.72rem;
    line-height: 1.4;
  }

  .cr-actions {
    display: flex;
    gap: 0.375rem;
  }

  .danger-btn {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, white);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .danger-btn:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  /* -- M12: deleted branches (trash) section ---------------------------------- */

  .trash {
    flex: none;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    margin-top: 0.25rem;
  }

  .trash-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.375rem 0.5rem 0.15rem;
  }

  .trash-title {
    margin: 0;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .trash-empty {
    margin: 0;
    padding: 0.15rem 0.5rem 0.5rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    line-height: 1.45;
  }

  .trash-empty code {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
  }

  .trash-list {
    margin: 0;
    padding: 0 0 0.5rem;
    list-style: none;
  }

  .sha-chip {
    font-family: ui-monospace, Consolas, monospace;
  }
</style>
