<!--
  PromptDialog (M9 F9) — accessible replacement for `window.prompt`.

  Props: `open` (bindable), `title`, `message`, `placeholder`, `initial`,
  `confirmLabel`, `onSubmit(value)` (trimmed value; cancel = no call),
  `onCancel`. Input autofocuses with the initial value selected; Enter
  submits, Escape cancels.
-->
<script lang="ts">
  let {
    open = $bindable(false),
    title,
    message = "",
    placeholder = "",
    initial = "",
    confirmLabel = "OK",
    onSubmit,
    onCancel = undefined,
  }: {
    open?: boolean;
    title: string;
    message?: string;
    placeholder?: string;
    initial?: string;
    confirmLabel?: string;
    onSubmit: (value: string) => void;
    onCancel?: () => void;
  } = $props();

  let inputEl = $state<HTMLInputElement | undefined>();
  let value = $state("");
  let restoreTo: Element | null = null;

  $effect(() => {
    if (open) {
      value = initial;
      restoreTo = document.activeElement;
      requestAnimationFrame(() => {
        inputEl?.focus();
        inputEl?.select();
      });
    } else if (restoreTo instanceof HTMLElement) {
      restoreTo.focus();
      restoreTo = null;
    }
  });

  function close(cancelOnly = false): void {
    open = false;
    if (cancelOnly) onCancel?.();
  }

  function submit(): void {
    const trimmed = value.trim();
    if (!trimmed) return;
    close();
    onSubmit(trimmed);
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
    <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
    <form
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="prompt-title"
      onsubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <h2 id="prompt-title" class="title">{title}</h2>
      {#if message}<p class="message">{message}</p>{/if}
      <input
        bind:this={inputEl}
        bind:value
        {placeholder}
        aria-label={title}
      />
      <div class="actions">
        <button class="secondary" type="button" onclick={() => close(true)}>
          Cancel
        </button>
        <button class="primary" type="submit" disabled={!value.trim()}>
          {confirmLabel}
        </button>
      </div>
    </form>
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

  .message {
    margin: 0 0 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    overflow-wrap: anywhere;
  }

  input {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.45rem 0.6rem;
  }

  input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 1rem;
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

  .primary:disabled {
    opacity: 0.55;
    cursor: not-allowed;
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
  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
