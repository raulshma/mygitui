<!--
  DialogShell — shared chrome for the safety dialogs: scrim, focus
  restore on close, Escape-to-cancel, panel box, and the action-button
  styles. Body and actions are snippets; the shell marks the element
  carrying `data-autofocus` for initial focus. Classes are prefixed
  `sdlg-` because they style snippet content rendered in the caller.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    open = $bindable(false),
    labelledBy,
    describedBy = undefined,
    role = "dialog",
    width = "26rem",
    onCancel = undefined,
    onsubmit = undefined,
    children,
    actions,
  }: {
    open?: boolean;
    /** id of the element labelling the dialog (aria-labelledby). */
    labelledBy: string;
    /** id of the element describing the dialog (aria-describedby). */
    describedBy?: string | undefined;
    role?: "dialog" | "alertdialog";
    width?: string;
    onCancel?: () => void;
    /** When set, the panel is a `<form>`; Enter submits (preventDefault'd). */
    onsubmit?: (() => void) | undefined;
    children: Snippet;
    actions: Snippet;
  } = $props();

  let panelEl = $state<HTMLElement | undefined>();
  let restoreTo: Element | null = null;

  $effect(() => {
    if (open) {
      restoreTo = document.activeElement;
      requestAnimationFrame(() => {
        panelEl
          ?.querySelector<HTMLElement>("[data-autofocus]")
          ?.focus();
      });
    } else if (restoreTo instanceof HTMLElement) {
      restoreTo.focus();
      restoreTo = null;
    }
  });

  function cancel(): void {
    open = false;
    onCancel?.();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="sdlg-scrim"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        cancel();
      }
    }}
  >
    {#if onsubmit}
      <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
      <form
        bind:this={panelEl}
        class="sdlg-dialog"
        style:width
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        onsubmit={(e) => {
          e.preventDefault();
          onsubmit();
        }}
      >
        {@render children()}
        <div class="sdlg-actions">{@render actions()}</div>
      </form>
    {:else}
      <div
        bind:this={panelEl}
        class="sdlg-dialog"
        style:width
        role={role}
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
      >
        {@render children()}
        <div class="sdlg-actions">{@render actions()}</div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .sdlg-scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .sdlg-dialog {
    padding: 1.25rem 1.5rem;
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  .sdlg-dialog :global(.sdlg-title) {
    margin: 0 0 0.5rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .sdlg-dialog :global(.sdlg-message) {
    margin: 0 0 1rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    overflow-wrap: anywhere;
  }

  .sdlg-actions {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  form.sdlg-dialog .sdlg-actions {
    margin-top: 1rem;
    flex-wrap: nowrap;
  }

  .sdlg-dialog :global(.sdlg-primary) {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-weight: 500;
    padding: 0.45rem 1.25rem;
    cursor: pointer;
  }

  .sdlg-dialog :global(.sdlg-primary:disabled) {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .sdlg-dialog :global(.sdlg-danger) {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, white);
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .sdlg-dialog :global(.sdlg-secondary) {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .sdlg-dialog :global(.sdlg-primary:focus-visible),
  .sdlg-dialog :global(.sdlg-danger:focus-visible),
  .sdlg-dialog :global(.sdlg-secondary:focus-visible) {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
