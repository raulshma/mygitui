<!--
  ConfirmDialog (M9 F9) — accessible replacement for `window.confirm`.

  Props: `open` (bindable), `title`, `message` (markup allowed via snippet
  slot), `confirmLabel`, `cancelLabel`, an optional `alternateLabel`
  (third, non-default action such as "Overwrite"), `danger` (error-colored
  confirm), `onConfirm`, `onCancel`, `onAlternate`. Focus lands on the least
  destructive action — Cancel on `danger` dialogs, Confirm otherwise (WAI
  APG alertdialog guidance: a reflexive Enter must not trigger the harmful
  action). Enter confirms once focused, Escape cancels; focus is trapped by
  the dialog (single dialog). Focus is restored to the previously active
  element on close. Chrome comes from the shadcn ui/dialog primitives.
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
    alternateLabel = undefined,
    alternateDanger = false,
    danger = false,
    onConfirm,
    onCancel = undefined,
    onAlternate = undefined,
  }: {
    open?: boolean;
    title: string;
    /** Plain text, or a snippet for rich content. */
    message?: string | Snippet;
    confirmLabel?: string;
    cancelLabel?: string;
    /** Optional third action (e.g. a destructive "Overwrite"). */
    alternateLabel?: string;
    alternateDanger?: boolean;
    danger?: boolean;
    onConfirm: () => void;
    onCancel?: () => void;
    onAlternate?: () => void;
  } = $props();

  let confirmButton = $state<HTMLElement | null>(null);
  let cancelButton = $state<HTMLElement | null>(null);

  // Focus lands on the least destructive action whenever the dialog opens:
  // Cancel on `danger` dialogs (a reflexive Enter must not destroy work),
  // Confirm otherwise (Enter-to-confirm stays a safe shortcut).
  $effect(() => {
    if (open) {
      requestAnimationFrame(() => (danger ? cancelButton : confirmButton)?.focus());
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

  function alternate(): void {
    onAlternate?.();
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
    class="dialog w-[26rem] max-w-none text-sm text-popover-foreground sm:max-w-none"
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
    <Dialog.Title id="confirm-title" class="m-0 mb-2 font-medium">{title}</Dialog.Title>
    {#if message}
      <div id="confirm-message" class="m-0 mb-4 text-muted-foreground [overflow-wrap:anywhere]">
        {#if typeof message === "string"}{message}{:else}{@render message()}{/if}
      </div>
    {/if}
    <div class="mt-4 flex flex-nowrap justify-end gap-2">
      <Button
        bind:ref={cancelButton}
        variant="outline"
        class="text-muted-foreground"
        type="button"
        onclick={() => close(true)}
      >
        {cancelLabel}
      </Button>
      {#if alternateLabel}
        <Button
          variant={alternateDanger ? "destructive" : "outline"}
          type="button"
          onclick={alternate}
        >
          {alternateLabel}
        </Button>
      {/if}
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
