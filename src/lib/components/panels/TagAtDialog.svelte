<script lang="ts">
  /**
   * TagAtDialog (M12, Lane D) — small modal to create a tag at a specific
   * commit (HistoryView "Tag here…" context-menu action).
   *
   * Fields: tag name, "annotated" checkbox (enables the message input —
   * the backend makes a tag annotated by passing a message), message.
   * Enter submits, Escape cancels; the dialog never calls IPC itself —
   * the owner performs `tagCreate` in `onSubmit(name, annotated, message)`
   * and closes by clearing its state.
   */
  import DialogShell from "$lib/components/safety/DialogShell.svelte";

  let {
    sha,
    onSubmit,
    onCancel = undefined,
  }: {
    /** Commit the tag points at (title context only). */
    sha: string;
    /** Submit with the trimmed name, annotated flag and message. */
    onSubmit: (name: string, annotated: boolean, message: string) => void;
    onCancel?: () => void;
  } = $props();

  let name = $state("");
  let annotated = $state(false);
  let message = $state("");

  function submit(): void {
    const trimmed = name.trim();
    if (!trimmed || (annotated && !message.trim())) return;
    onSubmit(trimmed, annotated, message.trim());
  }
</script>

<DialogShell
  open={true}
  labelledBy="tag-at-title"
  width="24rem"
  {onCancel}
  onsubmit={submit}
>
  {#snippet children()}
    <h2 id="tag-at-title" class="tagat-title">New tag at {sha.slice(0, 7)}</h2>
    <input
      class="tagat-input"
      placeholder="Tag name"
      aria-label="Tag name"
      bind:value={name}
      data-autofocus
      onfocus={(e) => e.currentTarget.select()}
    />
    <label class="tagat-toggle" title="Annotated tags carry a message, tagger and date">
      <input type="checkbox" bind:checked={annotated} />
      <span>annotated</span>
    </label>
    {#if annotated}
      <input
        class="tagat-input"
        placeholder="Message (makes the tag annotated)"
        aria-label="Tag message"
        bind:value={message}
      />
    {/if}
  {/snippet}
  {#snippet actions()}
    <button class="tagat-secondary" type="button" onclick={() => onCancel?.()}>
      Cancel
    </button>
    <button
      class="tagat-primary"
      type="submit"
      disabled={!name.trim() || (annotated && !message.trim())}
    >
      Create tag
    </button>
  {/snippet}
</DialogShell>

<style>
  .tagat-title {
    margin: 0 0 0.75rem;
    font-size: 1.125rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .tagat-input {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.45rem 0.6rem;
    margin-bottom: 0.6rem;
  }

  .tagat-input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .tagat-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .tagat-toggle input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .tagat-primary {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-weight: 500;
    padding: 0.45rem 1.25rem;
    cursor: pointer;
  }

  .tagat-primary:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .tagat-secondary {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }
</style>
