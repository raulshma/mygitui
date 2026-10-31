<!--
  ConfirmDialog (M9 F9) — accessible replacement for `window.confirm`.

  Props: `open` (bindable), `title`, `message` (markup allowed via snippet
  slot), `confirmLabel`, `cancelLabel`, `danger` (error-colored confirm),
  `onConfirm`, `onCancel`. Focus lands on confirm; Enter confirms, Escape
  cancels; focus is trapped by the dialog (single dialog). Focus is restored
  to the previously active element on close. Chrome comes from the shadcn
  ui/dialog primitives.
-->
<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import type { Snippet } from "svelte";

  let {
    open = $bindable(false),
    title,
    message = "",
    confirmLabel = "Confirm",
    cancelLabel = "Cancel",
    danger = false,
    onConfirm,
    onCancel = undefined,
  }: {
    open?: boolean;
    title: string;
    /** Plain text, or a snippet for rich content. */
    message?: string | Snippet;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel?: () => void;
  } = $props();

  let confirmButton = $state<HTMLElement | null>(null);

  // Focus lands on confirm (the old data-autofocus convention) whenever
  // the dialog opens.
  $effect(() => {
    if (open) {
      requestAnimationFrame(() => confirmButton?.focus());
    }
  });

  function close(cancelOnly = false): void {
    open = false;
    if (cancelOnly) onCancel?.();
  }

  function confirm(): void {
    // onConfirm runs BEFORE close(): close() writes the parent's bound
    // variable synchronously, and confirm handlers read that variable to
    // know what to act on (e.g. the pending worktree). Nulling first made
    // every handler see `null` and silently no-op. Handlers null the state
    // themselves; close() here is only the fallback for ones that don't.
    onConfirm();
    close();
  }
</script>

{#if open}
  <!-- Gated so closed dialogs fully unmount (legacy DialogShell semantics);
       bits-ui otherwise keeps Content mounted until the exit transition
       ends, which never completes under jsdom. -->
  <Dialog.Root bind:open>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
  <Dialog.Content
    class="dialog w-[26rem] max-w-none gap-0 rounded-lg bg-card px-6 py-5 text-sm text-popover-foreground shadow-xl sm:max-w-none"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
    aria-describedby={message ? "confirm-message" : undefined}
    showCloseButton={false}
    escapeKeydownBehavior="ignore"
    interactOutsideBehavior="ignore"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close(true);
      }
    }}
  >
    <Dialog.Title id="confirm-title" class="m-0 mb-2 text-lg font-medium">{title}</Dialog.Title>
    {#if message}
      <div id="confirm-message" class="m-0 mb-4 text-muted-foreground [overflow-wrap:anywhere]">
        {#if typeof message === "string"}{message}{:else}{@render message()}{/if}
      </div>
    {/if}
    <div class="mt-4 flex flex-nowrap justify-end gap-2">
      <Button
        variant="outline"
        class="text-muted-foreground"
        type="button"
        onclick={() => close(true)}
      >
        {cancelLabel}
      </Button>
      <Button
        bind:ref={confirmButton}
        variant={danger ? "destructive" : "default"}
        type="button"
        onclick={confirm}
      >
        {confirmLabel}
      </Button>
    </div>
  </Dialog.Content>
  </Dialog.Root>
{/if}
