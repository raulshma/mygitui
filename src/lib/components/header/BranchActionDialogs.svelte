<!--
  BranchActionDialogs — renders the three dialogs a BranchOpsController
  opens, shared by the branch panel and the header branch switcher so both
  surfaces present identical safety UX.

  Mount it once per controller (conditional rendering is driven by the
  controller's state, so it is safe to keep it always mounted):
    - `switchChoice` → the dirty-switch choice (stash / proceed / cancel);
      the stash button takes focus when the dialog appears;
    - `confirmDelete` → ConfirmDialog for a merged (safe) delete;
    - `previewDelete` → PreviewDialog (type-the-name + undo checkpoint) for
      a force delete. The controller performs the operations itself; this
      component only renders and closes.
-->
<script lang="ts">
  import type { BranchOpsController } from "$lib/git/branchOps.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PreviewDialog from "$lib/components/safety/PreviewDialog.svelte";

  let { controller }: { controller: BranchOpsController } = $props();

  let stashButton = $state<HTMLButtonElement | undefined>();

  // Focus the primary choice when the switch dialog appears.
  $effect(() => {
    if (controller.switchChoice) {
      requestAnimationFrame(() => stashButton?.focus());
    }
  });
</script>

<ConfirmDialog
  bind:open={
    () => controller.confirmDelete !== null,
    (v) => {
      if (!v) controller.cancelDelete();
    }
  }
  title={controller.confirmDelete
    ? `Delete branch ${controller.confirmDelete.name}?`
    : ""}
  message="The branch is fully merged into the current branch — deleting it loses no commits."
  confirmLabel="Delete branch"
  danger
  onConfirm={() => void controller.confirmMergedDelete()}
/>

{#if controller.switchChoice}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="scrim"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        controller.cancelChoice();
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
        switching to <strong>{controller.switchChoice.name}</strong>?
      </p>
      <div class="actions">
        <button
          class="primary"
          type="button"
          bind:this={stashButton}
          onclick={() => void controller.chooseStash()}
        >
          Stash changes
        </button>
        <button
          class="danger-btn"
          type="button"
          onclick={() => void controller.chooseForce()}
        >
          Proceed anyway
        </button>
        <button
          class="secondary"
          type="button"
          onclick={() => controller.cancelChoice()}
        >
          Cancel
        </button>
      </div>
    </div>
  </div>
{/if}

{#if controller.previewDelete}
  <PreviewDialog
    info={controller.previewDelete.info}
    danger
    confirmPhrase={controller.previewDelete.name}
    title={`Force delete ${controller.previewDelete.name}`}
    onConfirm={(createCheckpoint) =>
      void controller.confirmForceDelete(createCheckpoint)}
    onCancel={() => controller.cancelDelete()}
  />
{/if}

<style>
  /* Dirty-switch modal (same pattern as AuthDialog). */
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

  .primary:focus-visible,
  .secondary:focus-visible,
  .danger-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
