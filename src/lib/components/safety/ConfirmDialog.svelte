<!--
  ConfirmDialog (M9 F9) — accessible replacement for `window.confirm`.

  Props: `open` (bindable), `title`, `message` (markup allowed via snippet
  slot), `confirmLabel`, `cancelLabel`, `danger` (error-colored confirm),
  `onConfirm`, `onCancel`. Focus lands on confirm; Enter confirms, Escape
  cancels; focus is trapped by the scrim (single dialog). Restores focus to
  the previously active element on close. Chrome lives in DialogShell.
-->
<script lang="ts">
  import DialogShell from "./DialogShell.svelte";
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

<DialogShell
  bind:open
  labelledBy="confirm-title"
  describedBy={message ? "confirm-message" : undefined}
  role="alertdialog"
  {onCancel}
>
  {#snippet children()}
    <h2 id="confirm-title" class="sdlg-title">{title}</h2>
    {#if message}
      <div id="confirm-message" class="sdlg-message">
        {#if typeof message === "string"}{message}{:else}{@render message()}{/if}
      </div>
    {/if}
  {/snippet}
  {#snippet actions()}
    <button class="sdlg-secondary" type="button" onclick={() => close(true)}>
      {cancelLabel}
    </button>
    <button
      class={danger ? "sdlg-danger" : "sdlg-primary"}
      type="button"
      data-autofocus
      onclick={confirm}
    >
      {confirmLabel}
    </button>
  {/snippet}
</DialogShell>
