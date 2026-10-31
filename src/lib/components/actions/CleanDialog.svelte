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
  import * as Dialog from "$lib/components/ui/dialog/index.js";
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
  <Dialog.Root {open}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class={"dialog w-[min(32rem,calc(100vw_-_2rem))] max-w-none gap-0 overflow-y-auto rounded-2xl px-6 py-5 text-sm text-popover-foreground shadow-[0_8px_24px_rgba(0,0,0,0.3)] sm:max-w-none " + (selectedPaths.length > 0 ? "danger" : "")}
      role="dialog"
      aria-modal="true"
      aria-labelledby="clean-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <Dialog.Title id="clean-title" class="title m-0 mb-3 text-[1.125rem] font-medium text-destructive">Clean untracked paths</Dialog.Title>

      {#if loading}
        <p class="status m-0 mb-3 text-muted-foreground" aria-live="polite">Previewing untracked paths…</p>
      {:else if error}
        <p class="status error m-0 mb-3 text-destructive [overflow-wrap:anywhere]" role="alert">Preview failed: {error}</p>
      {:else if files.length === 0}
        <p class="status m-0 mb-3 text-muted-foreground" aria-live="polite">Nothing to clean — no untracked paths.</p>
      {:else}
        <p class="summary m-0 mb-[0.4rem] font-medium" aria-live="polite">
          {cleanSelectionSummary(selectedPaths.length, files.length)}
        </p>

        <div class="bulk mb-[0.4rem] flex gap-3">
          <button class="link cursor-pointer border-0 bg-transparent p-0 text-xs font-[inherit] text-primary underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" type="button" onclick={() => setAll(true)}>Select all</button>
          <button class="link cursor-pointer border-0 bg-transparent p-0 text-xs font-[inherit] text-primary underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" type="button" onclick={() => setAll(false)}>Select none</button>
        </div>

        <div class="file-list mb-3 max-h-64 overflow-y-auto rounded border border-border bg-background p-[0.5rem_0.6rem]" role="list" aria-label="Untracked paths to delete">
          {#each groups as group (group.dir)}
            <div class="group mb-[0.4rem] last:mb-0">
              <div class="group-label mb-[0.1rem] font-mono text-[0.7rem] font-semibold text-muted-foreground">{groupLabel(group)}</div>
              {#each group.files as file (file.path)}
                <label class="file flex cursor-pointer items-baseline gap-[0.4rem] py-[0.06rem]">
                  <input type="checkbox" class="m-0 accent-destructive" bind:checked={checked[file.path]} />
                  <span class="path font-mono text-[0.72rem] [overflow-wrap:anywhere]">{file.path}</span>
                </label>
              {/each}
            </div>
          {/each}
        </div>

        {#if typeConfirmNeeded}
          <label class="confirm-field mb-3 flex flex-col gap-1">
            <span class="confirm-hint text-xs text-destructive">
              This deletes {selectedPaths.length} paths — type “{CLEAN_CONFIRM_WORD}” to confirm
            </span>
            <input
              type="text"
              spellcheck="false"
              autocomplete="off"
              placeholder={CLEAN_CONFIRM_WORD}
              class="rounded border border-destructive bg-background px-[0.6rem] py-[0.4rem] font-[inherit] text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-destructive"
              bind:value={confirmText}
            />
          </label>
        {/if}
      {/if}

      <div class="actions flex items-center justify-end gap-2">
        <span class="guard-note mr-auto text-[0.7rem] text-muted-foreground">A checkpoint is created first (undo available).</span>
        <button
          class="confirm rounded-full border-0 bg-destructive px-5 py-[0.45rem] font-[inherit] font-medium text-destructive-foreground cursor-pointer focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-destructive disabled:cursor-not-allowed disabled:opacity-55"
          type="button"
          disabled={confirmBlocked || loading || files.length === 0}
          onclick={() => void confirm()}
        >
          {running
            ? "Cleaning…"
            : `Delete ${selectedPaths.length} path${selectedPaths.length === 1 ? "" : "s"}`}
        </button>
        <button class="cancel rounded-full border border-border bg-transparent px-4 py-[0.45rem] font-[inherit] text-muted-foreground cursor-pointer focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-destructive disabled:cursor-not-allowed disabled:opacity-55" type="button" disabled={running} onclick={onClose}>
          Cancel
        </button>
      </div>
    </Dialog.Content>
  </Dialog.Root>
{/if}
