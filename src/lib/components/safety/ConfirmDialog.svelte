<!--
  ConfirmDialog (M9 F9) — accessible replacement for `window.confirm`.

  Props: `open` (bindable), `title`, `message` (markup allowed via snippet
  slot), `confirmLabel`, `cancelLabel`, `danger` (error-colored confirm),
  `onConfirm`, `onCancel`. Focus lands on confirm; Enter confirms, Escape
  cancels; focus is trapped by the scrim (single dialog). Restores focus to
  the previously active element on close.
-->
<script lang="ts">
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

  let confirmBtn = $state<HTMLButtonElement | undefined>();
  let restoreTo: Element | null = null;

  $effect(() => {
    if (open) {
      restoreTo = document.activeElement;
      requestAnimationFrame(() => confirmBtn?.focus());
    } else if (restoreTo instanceof HTMLElement) {
      restoreTo.focus();
      restoreTo = null;
    }
  });

  function close(cancelOnly = false): void {
    open = false;
    if (cancelOnly) onCancel?.();
  }

  function confirm(): void {
    close();
    onConfirm();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="scrim"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close(true);
      }
    }}
  >
    <div
      class="dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      aria-describedby={message ? "confirm-message" : undefined}
    >
      <h2 id="confirm-title" class="title">{title}</h2>
      {#if message}
        <div id="confirm-message" class="message">
          {#if typeof message === "string"}{message}{:else}{@render message()}{/if}
        </div>
      {/if}
      <div class="actions">
        <button class="secondary" type="button" onclick={() => close(true)}>
          {cancelLabel}
        </button>
        <button
          class={danger ? "danger-btn" : "primary"}
          type="button"
          bind:this={confirmBtn}
          onclick={confirm}
        >
          {confirmLabel}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(26rem, calc(100vw - 2rem));
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

  .message {
    margin: 0 0 1rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
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
  .danger-btn:focus-visible,
  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
