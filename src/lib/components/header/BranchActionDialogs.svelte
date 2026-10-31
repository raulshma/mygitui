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
  import { Button } from "$lib/components/ui/button/index.js";

  let { controller }: { controller: BranchOpsController } = $props();

  let stashButton = $state<HTMLButtonElement | null>(null);

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
    class="scrim fixed inset-0 z-60 flex items-center justify-center bg-black/40"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        controller.cancelChoice();
      }
    }}
  >
    <div
      class="dialog w-[min(24rem,calc(100vw-2rem))] rounded-lg bg-card px-6 py-5 text-sm text-card-foreground shadow-md"
      role="dialog"
      aria-modal="true"
      aria-labelledby="switch-choice-title"
    >
      <h2 id="switch-choice-title" class="title mb-2 mt-0 text-lg font-medium">
        Uncommitted changes
      </h2>
      <p class="text mb-4 mt-0 text-card-foreground [overflow-wrap:anywhere]">
        You have uncommitted changes. What should happen to them when
        switching to <strong>{controller.switchChoice.name}</strong>?
      </p>
      <div class="actions flex flex-wrap justify-end gap-2">
        <Button
          class="primary rounded-full px-5 py-[0.45rem] text-[0.875rem] focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-1"
          bind:ref={stashButton}
          onclick={() => void controller.chooseStash()}
        >
          Stash changes
        </Button>
        <Button
          class="danger-btn rounded-full bg-destructive px-4 py-[0.45rem] text-[0.875rem] text-white hover:bg-destructive/90 focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-1 focus-visible:ring-0"
          variant="destructive"
          onclick={() => void controller.chooseForce()}
        >
          Proceed anyway
        </Button>
        <Button
          class="secondary rounded-full border-border px-4 py-[0.45rem] text-[0.875rem] text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-1 focus-visible:ring-0"
          variant="outline"
          onclick={() => controller.cancelChoice()}
        >
          Cancel
        </Button>
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
