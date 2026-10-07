<script lang="ts">
  /**
   * Renders the toast stack (bottom-right). State and auto-dismiss live in
   * `$lib/toast`; this component is purely presentational.
   */
  import { dismissToast, getToasts, type ToastKind } from "$lib/toast";

  function glyph(kind: ToastKind): string {
    switch (kind) {
      case "success":
        return "✓";
      case "error":
        return "!";
      default:
        return "i";
    }
  }
</script>

<div class="toaster" aria-live="polite">
  {#each getToasts() as item (item.id)}
    <div class="toast" role="status" data-kind={item.kind}>
      <span class="badge" aria-hidden="true">{glyph(item.kind)}</span>
      <span class="message">{item.message}</span>
      <button
        class="close"
        type="button"
        aria-label="Dismiss notification"
        onclick={() => dismissToast(item.id)}
      >
        ×
      </button>
    </div>
  {/each}
</div>

<style>
  .toaster {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    z-index: 1000;
    max-width: min(24rem, calc(100vw - 2rem));
  }

  .toast {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.625rem 0.75rem;
    border-radius: 0.5rem;
    background: var(--m3-surface-container, var(--m3-surface));
    color: var(--m3-on-surface);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    box-shadow: 0 2px 8px rgb(0 0 0 / 0.25);
    animation: toast-in 150ms ease-out;
  }

  .badge {
    flex: none;
    display: grid;
    place-items: center;
    width: 1.25rem;
    height: 1.25rem;
    border-radius: 50%;
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font-size: 0.75rem;
    font-weight: 700;
    line-height: 1;
  }

  .toast[data-kind="error"] .badge {
    outline: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .message {
    flex: 1;
    min-width: 0;
    font-size: 0.8125rem;
    overflow-wrap: anywhere;
  }

  .close {
    flex: none;
    border: none;
    background: none;
    padding: 0 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 1rem;
    line-height: 1;
    cursor: pointer;
  }

  .close:hover,
  .close:focus-visible {
    color: var(--m3-on-surface);
  }

  @keyframes toast-in {
    from {
      transform: translateY(0.5rem);
      opacity: 0;
    }
    to {
      transform: translateY(0);
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .toast {
      animation: none;
    }
  }
</style>
