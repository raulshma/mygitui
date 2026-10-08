<script lang="ts">
  /**
   * Preview dialog (M3 safety) — generic "what will this do?" modal.
   *
   * Renders an `ops_preview` result (summary + scrollable file list) and
   * drives the confirm flow the caller performs:
   *   - `danger` rows add an error-colored confirm button and a
   *     "Create undo checkpoint" checkbox (default ON; when left checked the
   *     caller runs `guard_checkpoint` before the op, unchecked = skip);
   *   - `confirmPhrase` (type-the-name) gates the confirm button;
   *   - Escape and Cancel invoke `onCancel`.
   *
   * The dialog never performs the operation itself: `onConfirm` receives
   * whether a checkpoint was requested and the caller executes guard + op.
   * Render it inside `{#if open}` — mounting IS opening (same contract as
   * AuthDialog's conditional render).
   */
  import type { PreviewInfo } from "$lib/ipc/types";
  import {
    classifyDanger,
    previewSummaryLines,
  } from "./safetyModel";

  let {
    info,
    danger,
    confirmPhrase = undefined,
    title = undefined,
    onConfirm,
    onCancel,
  }: {
    info: PreviewInfo;
    danger: boolean;
    /** When set, the confirm button stays disabled until the user types this. */
    confirmPhrase?: string;
    /** Dialog heading; derived from the danger classification when omitted. */
    title?: string;
    onConfirm: (createCheckpoint: boolean) => void;
    onCancel: () => void;
  } = $props();

  const dangerInfo = $derived(
    danger
      ? classifyDanger(info.kind)
      : { level: "warn" as const, verb: "Apply", noun: info.kind },
  );

  const lines = $derived(previewSummaryLines(info));
  let phrase = $state("");
  let checkpoint = $state(true);
  let phraseInput: HTMLInputElement | undefined = $state();
  let confirmButton: HTMLButtonElement | undefined = $state();

  const phraseOk = $derived(
    confirmPhrase === undefined || phrase === confirmPhrase,
  );

  // Fresh dialog state on every mount, focus the first control.
  $effect(() => {
    phrase = "";
    checkpoint = true;
    requestAnimationFrame(() => {
      if (confirmPhrase !== undefined) phraseInput?.focus();
      else confirmButton?.focus();
    });
  });

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      onCancel();
    }
  }

  function confirm(): void {
    if (!phraseOk) return;
    onConfirm(danger && checkpoint);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="scrim" role="presentation" onkeydown={onKeydown}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="preview-title"
    aria-describedby="preview-summary"
  >
    <h2 id="preview-title" class="title" class:danger={dangerInfo.level === "danger"}>
      {title ?? `${dangerInfo.verb} — preview`}
    </h2>

    <p id="preview-summary" class="summary">{info.summary}</p>

    {#if lines.length > 1}
      <ul class="files" role="list" aria-label="Affected files">
        {#each lines.slice(1) as line, i (i)}
          <li>{line}</li>
        {/each}
      </ul>
    {/if}

    {#if danger}
      <label class="check">
        <input type="checkbox" bind:checked={checkpoint} />
        <span>Create undo checkpoint (snapshot first)</span>
      </label>
    {/if}

    {#if confirmPhrase !== undefined}
      <label class="phrase">
        <span class="phrase-label">
          Type <code>{confirmPhrase}</code> to confirm
        </span>
        <input
          type="text"
          spellcheck="false"
          autocomplete="off"
          aria-label={`Type ${confirmPhrase} to confirm`}
          placeholder={confirmPhrase}
          bind:value={phrase}
          bind:this={phraseInput}
        />
      </label>
    {/if}

    <div class="actions">
      <button
        class="confirm"
        class:danger-btn={dangerInfo.level === "danger"}
        type="button"
        disabled={!phraseOk}
        bind:this={confirmButton}
        onclick={confirm}
      >
        {dangerInfo.verb}
      </button>
      <button class="cancel" type="button" onclick={onCancel}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    width: min(30rem, calc(100vw - 2rem));
    max-height: calc(100vh - 4rem);
    display: flex;
    flex-direction: column;
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

  .title.danger {
    color: var(--m3-error, inherit);
  }

  .summary {
    margin: 0 0 0.5rem;
    color: var(--m3-on-surface);
    overflow-wrap: anywhere;
    white-space: pre-line;
  }

  .files {
    margin: 0 0 0.75rem;
    padding: 0.35rem 0.6rem;
    max-height: 12rem;
    overflow-y: auto;
    list-style: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .files li {
    padding: 0.1rem 0;
    overflow-wrap: anywhere;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0 0 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .check input {
    accent-color: var(--m3-primary);
  }

  .phrase {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin: 0 0 0.75rem;
  }

  .phrase-label {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .phrase code {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-error, inherit);
    font-weight: 600;
  }

  .phrase input {
    font: inherit;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.45rem 0.6rem;
  }

  .phrase input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.25rem;
  }

  .confirm {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-weight: 500;
    padding: 0.45rem 1.25rem;
    cursor: pointer;
  }

  .confirm:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .confirm.danger-btn {
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, white);
  }

  .cancel {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .confirm:focus-visible,
  .cancel:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }
</style>
