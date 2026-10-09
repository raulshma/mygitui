<!--
  PromptDialog (M9 F9) — accessible replacement for `window.prompt`.

  Props: `open` (bindable), `title`, `message`, `placeholder`, `initial`,
  `confirmLabel`, `cancelLabel`, `onSubmit(value)` (trimmed value;
  cancel = no call), `onCancel`. Input autofocuses with the initial value
  selected; Enter submits, Escape cancels. Chrome lives in DialogShell.
-->
<script lang="ts">
  import DialogShell from "./DialogShell.svelte";

  let {
    open = $bindable(false),
    title,
    message = "",
    placeholder = "",
    initial = "",
    confirmLabel = "OK",
    cancelLabel = "Cancel",
    onSubmit,
    onCancel = undefined,
  }: {
    open?: boolean;
    title: string;
    message?: string;
    placeholder?: string;
    initial?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    onSubmit: (value: string) => void;
    onCancel?: () => void;
  } = $props();

  let value = $state("");

  $effect(() => {
    if (open) {
      value = initial;
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

<DialogShell
  bind:open
  labelledBy="prompt-title"
  width="24rem"
  {onCancel}
  onsubmit={submit}
>
  {#snippet children()}
    <h2 id="prompt-title" class="sdlg-title">{title}</h2>
    {#if message}<p class="sdlg-message">{message}</p>{/if}
    <input
      class="sdlg-input"
      bind:value
      {placeholder}
      aria-label={title}
      data-autofocus
      onfocus={(e) => e.currentTarget.select()}
    />
  {/snippet}
  {#snippet actions()}
    <button class="sdlg-secondary" type="button" onclick={() => close(true)}>
      {cancelLabel}
    </button>
    <button class="sdlg-primary" type="submit" disabled={!value.trim()}>
      {confirmLabel}
    </button>
  {/snippet}
</DialogShell>

<style>
  .sdlg-input {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.45rem 0.6rem;
  }

  .sdlg-input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }
</style>
