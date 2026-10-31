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
  import { formatDateTime } from "$lib/git/format";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { checkpointTitle } from "./safetyModel";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconGc from "@ktibow/iconset-material-symbols/delete-sweep";
  import iconRestore from "@ktibow/iconset-material-symbols/restore-page";
  import iconTrashRestore from "@ktibow/iconset-material-symbols/undo";

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

<section class="undo-panel flex min-h-0 flex-col text-[0.8125rem]" aria-label="Checkpoints and undo">
  <div class="toolbar flex shrink-0 items-center gap-2 px-2 py-[0.375rem]">
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh checkpoints"
      title={undo.loading ? "Refreshing checkpoints…" : "Refresh checkpoints"}
      disabled={undo.loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Clean old checkpoints"
      title="Delete checkpoints older than 30 days"
      disabled={gcBusy}
      onclick={() => void doGc()}
    >
      <Icon icon={iconGc} size={16} />
    </Button>
    <span class="hint ml-auto text-[0.6875rem] text-muted-foreground" title="Checkpoints are automatic snapshots created before dangerous operations">
      {undo.checkpoints.length} checkpoint{undo.checkpoints.length === 1 ? "" : "s"}
    </span>
  </div>

  {#if undo.error}
    <p class="state error m-0 px-3 py-4 text-[0.8125rem] text-destructive" role="alert">{undo.error}</p>
  {:else if !undo.loading && undo.checkpoints.length === 0}
    <div class="empty px-3 py-4">
      <p class="empty-title mb-[0.35rem] mt-0 font-semibold text-foreground">No checkpoints yet.</p>
      <p class="empty-text m-0 text-xs leading-[1.45] text-muted-foreground">
        Checkpoints are automatic snapshots mygitui creates before dangerous
        operations (hard resets, force branch deletes, restores). Restoring a
        checkpoint returns the repository to that exact snapshot; the state at
        restore time is itself saved as a new checkpoint, so undo is always
        available.
      </p>
    </div>
  {:else}
    <ul class="checkpoints m-0 min-h-0 grow list-none overflow-y-auto p-0 pb-3" role="list" aria-label="Checkpoints, newest first">
      {#each undo.checkpoints as cp (cp.id)}
        <li class="row flex min-w-0 items-center gap-[0.375rem] px-2 py-1 hover:bg-accent">
          {#if confirmingRestore === cp.id}
            <div
              class="confirm-restore min-w-0 grow rounded-md border border-destructive px-2 py-[0.35rem]"
              role="alertdialog"
              aria-label={`Restore checkpoint ${cp.id}`}
            >
              <p class="cr-text mb-[0.4rem] mt-0 text-[0.72rem] leading-[1.4] text-foreground">
                Restore returns the repo to this snapshot: changes made since
                are discarded and untracked files created after it are removed.
                The current state is checkpointed first (pre-restore), so this
                is undoable.
              </p>
              <div class="cr-actions flex gap-[0.375rem]">
                <Button
                  variant="destructive"
                  size="xs"
                  class="rounded-full px-3"
                  type="button"
                  disabled={busyId === cp.id}
                  onclick={() => void doRestore(cp)}
                >
                  {busyId === cp.id ? "Restoring…" : "Restore"}
                </Button>
                <Button
                  variant="outline"
                  size="xs"
                  class="rounded-sm text-muted-foreground"
                  type="button"
                  onclick={() => (confirmingRestore = null)}
                >
                  Cancel
                </Button>
              </div>
            </div>
          {:else}
            <div class="info flex min-w-0 grow flex-col gap-[0.15rem]">
              <span class="title truncate text-xs text-foreground" title={cp.ref_name}>{checkpointTitle(cp, Math.floor(Date.now() / 1000))}</span>
              <span class="chips flex flex-wrap gap-1">
                {#if cp.branch}<Badge variant="outline" class="chip h-auto max-w-40 overflow-hidden rounded-full px-[0.4rem] py-0 text-[0.625rem] text-muted-foreground text-ellipsis whitespace-nowrap">{cp.branch}</Badge>{/if}
                {#if cp.has_worktree_state}
                  <Badge variant="outline" class="chip worktree h-auto max-w-40 overflow-hidden rounded-full border-primary px-[0.4rem] py-0 text-[0.625rem] text-primary text-ellipsis whitespace-nowrap" title="Snapshot includes working-copy + index state">worktree</Badge>
                {/if}
              </span>
            </div>
            <span class="actions ml-auto shrink-0">
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Restore checkpoint ${cp.id}`}
                title="Restore this checkpoint"
                disabled={busyId !== null}
                onclick={() => askRestore(cp)}
              >
                <Icon icon={iconRestore} size={16} />
              </Button>
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <!-- M12: force-deleted branches preserved under refs/mygitui/trash/* -->
  <section class="trash mt-1 shrink-0 border-t border-border" aria-label="Deleted branches">
    <header class="trash-head flex items-center justify-between gap-2 px-2 pb-[0.15rem] pt-[0.375rem]">
      <h2 class="trash-title m-0 text-xs font-semibold text-foreground">Deleted branches</h2>
      <Button
        class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label="Refresh deleted branches"
        title={trashLoading ? "Refreshing deleted branches…" : "Refresh deleted branches"}
        disabled={trashLoading}
        onclick={() => void loadTrash()}
      >
        <Icon icon={iconRefresh} size={16} />
      </Button>
    </header>

    {#if trashError}
      <p class="state error m-0 px-3 py-4 text-[0.8125rem] text-destructive" role="alert">{trashError}</p>
    {:else if !trashLoading && trash.length === 0}
      <p class="trash-empty m-0 px-2 pb-2 pt-[0.15rem] text-xs leading-[1.45] text-muted-foreground">
        No deleted branches. Force-deleted branches are kept here (under
        <code class="font-mono text-[0.6875rem]">refs/mygitui/trash/*</code>) so their commits stay recoverable
        until checkpoint GC cleans them up.
      </p>
    {:else}
      <ul class="trash-list m-0 list-none p-0 pb-2" role="list" aria-label="Deleted branches, newest first">
        {#each trash as entry (entry.id)}
          <li class="row flex min-w-0 items-center gap-[0.375rem] px-2 py-1 hover:bg-accent">
            <div class="info flex min-w-0 grow flex-col gap-[0.15rem]">
              <span class="title truncate text-xs text-foreground" title={entry.name}>{entry.name}</span>
              <span class="chips flex flex-wrap gap-1">
                <Badge variant="outline" class="chip sha-chip h-auto max-w-40 overflow-hidden rounded-full px-[0.4rem] py-0 font-mono text-[0.625rem] text-muted-foreground text-ellipsis whitespace-nowrap" title={entry.sha}>{entry.sha.slice(0, 7)}</Badge>
                <Badge variant="outline" class="chip h-auto max-w-40 overflow-hidden rounded-full px-[0.4rem] py-0 text-[0.625rem] text-muted-foreground text-ellipsis whitespace-nowrap" title="Deleted {formatDateTime(entry.deleted_at)}">
                  deleted {formatDateTime(entry.deleted_at)}
                </Badge>
              </span>
            </div>
            <span class="actions ml-auto shrink-0">
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Restore branch ${entry.name}`}
                title="Restore this deleted branch"
                disabled={restoreBusy}
                onclick={() => (restoringTrash = entry)}
              >
                <Icon icon={iconTrashRestore} size={16} />
              </Button>
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
