<script lang="ts">
  /**
   * Clean dialog (M4 housekeeping) — delete untracked paths with a preview.
   *
   * On open it runs `opsPreview(repoId, "clean", { dirs: true })` and lists
   * the untracked files as checkboxes (all checked by default), grouped by
   * their immediate directory. The confirm button carries the danger
   * styling; when more than {@link CLEAN_CONFIRM_THRESHOLD} paths are
   * selected it requires typing "clean" first.
   *
   * Confirm flow: `repoClean(repoId, selectedPaths, "pre-clean")` — the
   * backend creates the undo checkpoint itself before deleting — then a
   * success toast quoting the removed count and `onDone()` (the owner
   * refreshes status and closes). Cancel / Escape → `onClose()`. The dialog
   * never closes itself: the owner flips `open` in its handlers.
   */
  import { opsPreview, repoClean } from "$lib/ipc/client";
  import type { PreviewFile } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import {
    CLEAN_CONFIRM_WORD,
    cleanSelectionSummary,
    groupByDir,
    groupLabel,
    needsTypeConfirm,
  } from "./actionsModel";

  let {
    repoId,
    open,
    onClose,
    onDone,
  }: {
    repoId: string;
    /** Rendered only when true — mounting is opening. */
    open: boolean;
    onClose: () => void;
    /** Called after a successful clean (owner refreshes status + closes). */
    onDone: () => void;
  } = $props();

  let files = $state<PreviewFile[]>([]);
  let checked = $state<Record<string, boolean>>({});
  let loading = $state(false);
  let error = $state<string | null>(null);
  let running = $state(false);
  let confirmText = $state("");

  // Fresh state + preview on every open (stale responses dropped by seq).
  let previewSeq = 0;
  $effect(() => {
    if (!open) return;
    files = [];
    checked = {};
    error = null;
    running = false;
    confirmText = "";
    loading = true;
    const seq = ++previewSeq;
    opsPreview(repoId, "clean", { dirs: true })
      .then((info) => {
        if (seq !== previewSeq) return;
        files = info.files ?? [];
        const all: Record<string, boolean> = {};
        for (const file of files) all[file.path] = true;
        checked = all;
        loading = false;
      })
      .catch((err: unknown) => {
        if (seq !== previewSeq) return;
        error = err instanceof Error ? err.message : String(err);
        loading = false;
      });
  });

  const groups = $derived(groupByDir(files));
  const selectedPaths = $derived(files.filter((f) => checked[f.path]).map((f) => f.path));
  const typeConfirmNeeded = $derived(needsTypeConfirm(selectedPaths.length));
  const confirmBlocked = $derived(
    running ||
      selectedPaths.length === 0 ||
      (typeConfirmNeeded && confirmText !== CLEAN_CONFIRM_WORD),
  );

  function setAll(value: boolean): void {
    const all: Record<string, boolean> = {};
    for (const file of files) all[file.path] = value;
    checked = all;
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !running) {
      event.preventDefault();
      onClose();
    }
  }

  async function confirm(): Promise<void> {
    if (confirmBlocked) return;
    running = true;
    try {
      // The backend checkpoints the full workdir state ("pre-clean") before
      // deleting, so this is undoable via checkpoints.
      const removed = await repoClean(repoId, selectedPaths, "pre-clean");
      toast(
        `Cleaned ${removed} path${removed === 1 ? "" : "s"} — undo available (checkpoint "pre-clean")`,
        { kind: "success" },
      );
      running = false;
      onDone();
    } catch (err) {
      running = false;
      toast(`Clean failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="scrim" role="presentation" onkeydown={onKeydown}>
    <div
      class="dialog"
      class:danger={selectedPaths.length > 0}
      role="dialog"
      aria-modal="true"
      aria-labelledby="clean-title"
    >
      <h2 id="clean-title" class="title">Clean untracked paths</h2>

      {#if loading}
        <p class="status" aria-live="polite">Previewing untracked paths…</p>
      {:else if error}
        <p class="status error" role="alert">Preview failed: {error}</p>
      {:else if files.length === 0}
        <p class="status" aria-live="polite">Nothing to clean — no untracked paths.</p>
      {:else}
        <p class="summary" aria-live="polite">
          {cleanSelectionSummary(selectedPaths.length, files.length)}
        </p>

        <div class="bulk">
          <button class="link" type="button" onclick={() => setAll(true)}>Select all</button>
          <button class="link" type="button" onclick={() => setAll(false)}>Select none</button>
        </div>

        <div class="file-list" role="list" aria-label="Untracked paths to delete">
          {#each groups as group (group.dir)}
            <div class="group">
              <div class="group-label">{groupLabel(group)}</div>
              {#each group.files as file (file.path)}
                <label class="file">
                  <input type="checkbox" bind:checked={checked[file.path]} />
                  <span class="path">{file.path}</span>
                </label>
              {/each}
            </div>
          {/each}
        </div>

        {#if typeConfirmNeeded}
          <label class="confirm-field">
            <span class="confirm-hint">
              This deletes {selectedPaths.length} paths — type “{CLEAN_CONFIRM_WORD}” to confirm
            </span>
            <input
              type="text"
              spellcheck="false"
              autocomplete="off"
              placeholder={CLEAN_CONFIRM_WORD}
              bind:value={confirmText}
            />
          </label>
        {/if}
      {/if}

      <div class="actions">
        <span class="guard-note">A checkpoint is created first (undo available).</span>
        <button
          class="confirm"
          type="button"
          disabled={confirmBlocked || loading || files.length === 0}
          onclick={() => void confirm()}
        >
          {running
            ? "Cleaning…"
            : `Delete ${selectedPaths.length} path${selectedPaths.length === 1 ? "" : "s"}`}
        </button>
        <button class="cancel" type="button" disabled={running} onclick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
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
    width: min(32rem, calc(100vw - 2rem));
    max-height: calc(100vh - 4rem);
    overflow-y: auto;
    padding: 1.25rem 1.5rem;
    border-radius: 16px;
    background: var(--m3-surface-container-high, var(--m3-background));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  .title {
    margin: 0 0 0.75rem;
    font-size: 1.125rem;
    font-weight: 500;
    color: var(--m3-error);
  }

  .status {
    margin: 0 0 0.75rem;
    color: var(--m3-on-surface-variant);
  }

  .status.error {
    color: var(--m3-error);
    overflow-wrap: anywhere;
  }

  .summary {
    margin: 0 0 0.4rem;
    font-weight: 500;
  }

  .bulk {
    display: flex;
    gap: 0.75rem;
    margin: 0 0 0.4rem;
  }

  .link {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-primary);
    cursor: pointer;
    text-decoration: underline;
  }

  .link:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }

  .file-list {
    margin: 0 0 0.75rem;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 4px;
    background: var(--m3-surface-container-lowest, transparent);
    padding: 0.5rem 0.6rem;
    max-height: 16rem;
    overflow-y: auto;
  }

  .group {
    margin: 0 0 0.4rem;
  }

  .group:last-child {
    margin-bottom: 0;
  }

  .group-label {
    color: var(--m3-on-surface-variant);
    font-size: 0.7rem;
    font-weight: 600;
    font-family: ui-monospace, Consolas, monospace;
    margin-bottom: 0.1rem;
  }

  .file {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    cursor: pointer;
    padding: 0.06rem 0;
  }

  .file input {
    accent-color: var(--m3-error);
    margin: 0;
  }

  .path {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    overflow-wrap: anywhere;
  }

  .confirm-field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin: 0 0 0.75rem;
  }

  .confirm-hint {
    color: var(--m3-error);
    font-size: 0.75rem;
  }

  .confirm-field input {
    font: inherit;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, transparent);
    border: 1px solid var(--m3-error);
    border-radius: 4px;
    padding: 0.4rem 0.6rem;
  }

  .confirm-field input:focus-visible {
    outline: 2px solid var(--m3-error);
    outline-offset: -1px;
  }

  .actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .guard-note {
    margin-right: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.7rem;
  }

  .confirm {
    border: none;
    border-radius: 9999px;
    background: var(--m3-error);
    color: var(--m3-on-error);
    font: inherit;
    font-weight: 500;
    padding: 0.45rem 1.25rem;
    cursor: pointer;
  }

  .confirm:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .cancel {
    border: 1px solid var(--m3-outline-variant);
    border-radius: 9999px;
    background: none;
    color: var(--m3-on-surface-variant);
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .cancel:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .confirm:focus-visible,
  .cancel:focus-visible {
    outline: 2px solid var(--m3-error);
    outline-offset: 2px;
  }
</style>
