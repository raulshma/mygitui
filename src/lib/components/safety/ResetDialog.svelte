<script lang="ts">
  /**
   * Reset dialog (M3 safety) — move the current branch with a live preview.
   *
   * Mode radio (soft / mixed / hard, plain-language), a target ref input
   * with a datalist of branches + tags, and a live `ops_preview` of the
   * hard reset (what would be discarded), refreshed on open and on target
   * change (debounced 300ms, stale responses dropped).
   *
   * Confirm flows:
   *   - hard: `guard_checkpoint("pre-hard-reset")` → `reset` → success toast
   *     quoting the checkpoint id ("undo available") → `onDone()`;
   *   - soft/mixed: single confirm, no checkpoint (recoverable via reflog)
   *     → `onDone()`.
   * Cancel / Escape → `onClose()`. The dialog never closes itself: the
   * owner flips `open` in its `onDone`/`onClose` handlers (and refreshes).
   */
  import {
    branches,
    guardCheckpoint,
    opsPreview,
    repoRefs,
    resetRepo,
  } from "$lib/ipc/client";
  import type { PreviewInfo, ResetKind } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import {
    buildResetParams,
    buildResetPreviewParams,
    RESET_MODES,
    RESET_PREVIEW_KIND,
    previewSummaryLines,
  } from "./safetyModel";

  let {
    repoId,
    open,
    defaultTarget = undefined,
    onClose,
    onDone,
  }: {
    repoId: string;
    /** Rendered only when true — mounting is opening. */
    open: boolean;
    /** Initial target ref (branch name, tag, sha…); defaults to HEAD. */
    defaultTarget?: string;
    onClose: () => void;
    /** Called after a successful reset (owner refreshes status + closes). */
    onDone: () => void;
  } = $props();

  let mode = $state<ResetKind>("hard");
  let target = $state("HEAD");
  let suggestions = $state<string[]>([]);
  let previewInfo = $state<PreviewInfo | null>(null);
  let previewError = $state<string | null>(null);
  let previewing = $state(false);
  let running = $state(false);
  let targetField: HTMLInputElement | undefined = $state();

  // Fresh state + focus + suggestion load on every open.
  $effect(() => {
    if (!open) return;
    mode = "hard";
    target = defaultTarget && defaultTarget !== "" ? defaultTarget : "HEAD";
    previewInfo = null;
    previewError = null;
    running = false;
    void loadSuggestions();
    requestAnimationFrame(() => targetField?.focus());
  });

  // Live preview: on open + target change, debounced 300ms. The request
  // counter drops responses that race (typing fast).
  let previewSeq = 0;
  $effect(() => {
    if (!open) return;
    const to = target;
    const seq = ++previewSeq;
    previewing = true;
    const timer = window.setTimeout(() => {
      opsPreview(repoId, RESET_PREVIEW_KIND, buildResetPreviewParams(to))
        .then((info) => {
          if (seq === previewSeq) {
            previewInfo = info;
            previewError = null;
          }
        })
        .catch((err: unknown) => {
          if (seq === previewSeq) {
            previewInfo = null;
            previewError = err instanceof Error ? err.message : String(err);
          }
        })
        .finally(() => {
          if (seq === previewSeq) previewing = false;
        });
    }, 300);
    return () => window.clearTimeout(timer);
  });

  async function loadSuggestions(): Promise<void> {
    const names = new Set<string>();
    try {
      const list = await branches(repoId);
      for (const b of list) names.add(b.name);
    } catch {
      // Datalist is best-effort.
    }
    try {
      const refs = await repoRefs(repoId);
      for (const [ref] of refs) {
        if (ref.startsWith("refs/tags/")) names.add(ref.slice("refs/tags/".length));
      }
    } catch {
      // Datalist is best-effort.
    }
    suggestions = [...names].sort((a, b) => a.localeCompare(b));
  }

  const lines = $derived(previewInfo ? previewSummaryLines(previewInfo) : []);

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !running) {
      event.preventDefault();
      onClose();
    }
  }

  async function confirm(): Promise<void> {
    if (running || !target.trim()) return;
    running = true;
    try {
      if (mode === "hard") {
        const cp = await guardCheckpoint(repoId, "pre-hard-reset");
        await resetRepo(repoId, ...resetArgs());
        toast(
          `Hard reset to ${target} — undo available (checkpoint ${cp.id})`,
          { kind: "success" },
        );
      } else {
        await resetRepo(repoId, ...resetArgs());
        toast(`${label(mode)} reset to ${target}`, { kind: "success" });
      }
      running = false;
      onDone();
    } catch (err) {
      running = false;
      toast(`Reset failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }

  function resetArgs(): [ResetKind, string] {
    const params = buildResetParams(mode, target.trim());
    return [params.kind, params.to];
  }

  function label(kind: ResetKind): string {
    return RESET_MODES.find((m) => m.kind === kind)?.label ?? kind;
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="scrim" role="presentation" onkeydown={onKeydown}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="reset-title"
    >
      <h2 id="reset-title" class="title">Reset current branch</h2>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void confirm();
        }}
      >
        <fieldset class="modes">
          <legend class="sr-only">Reset mode</legend>
          {#each RESET_MODES as m (m.kind)}
            <label class="mode">
              <input type="radio" name="reset-mode" value={m.kind} bind:group={mode} />
              <span class="mode-text">
                <span class="mode-label">{m.label}</span>
                <span class="mode-desc">{m.description}</span>
              </span>
            </label>
          {/each}
        </fieldset>

        <label class="target">
          <span class="field-label">Reset to (ref, branch, tag or sha)</span>
          <input
            type="text"
            spellcheck="false"
            autocomplete="off"
            list="reset-target-suggestions"
            bind:value={target}
            bind:this={targetField}
          />
          <datalist id="reset-target-suggestions">
            {#each suggestions as name (name)}
              <option value={name}></option>
            {/each}
          </datalist>
        </label>

        <div class="preview" aria-live="polite">
          {#if previewError}
            <p class="preview-error" role="alert">Preview failed: {previewError}</p>
          {:else if previewing || !previewInfo}
            <p class="preview-loading">Previewing…</p>
          {:else}
            <p class="preview-summary">{lines[0]}</p>
            {#if lines.length > 1}
              <ul class="preview-files" role="list" aria-label="Files affected by a hard reset">
                {#each lines.slice(1) as line, i (i)}
                  <li>{line}</li>
                {/each}
              </ul>
            {:else}
              <p class="preview-empty">No file changes.</p>
            {/if}
            {#if mode !== "hard"}
              <p class="preview-note">
                Preview shows the hard-reset impact; {label(mode).toLowerCase()} reset keeps your changes.
              </p>
            {/if}
          {/if}
        </div>

        <div class="actions">
          {#if mode === "hard"}
            <span class="guard-note">A checkpoint is created first (undo available).</span>
          {/if}
          <button class="confirm" type="submit" disabled={running || !target.trim()}>
            {running ? "Resetting…" : `Reset (${label(mode)})`}
          </button>
          <button class="cancel" type="button" disabled={running} onclick={onClose}>
            Cancel
          </button>
        </div>
      </form>
    </div>
  </div>
{/if}

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
    overflow-y: auto;
    padding: 1.25rem 1.5rem;
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  .title {
    margin: 0 0 0.75rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
    border: 0;
  }

  .modes {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin: 0 0 0.875rem;
    padding: 0;
    border: none;
  }

  .mode {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    cursor: pointer;
  }

  .mode input {
    accent-color: var(--m3-primary);
    margin: 0.15rem 0 0;
  }

  .mode-text {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .mode-label {
    font-weight: 600;
  }

  .mode-desc {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .target {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin: 0 0 0.75rem;
  }

  .field-label {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .target input {
    font: inherit;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.45rem 0.6rem;
  }

  .target input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .preview {
    margin: 0 0 0.875rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    padding: 0.5rem 0.6rem;
    min-height: 3.25rem;
  }

  .preview-loading,
  .preview-empty {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .preview-summary {
    margin: 0 0 0.35rem;
    font-size: 0.78rem;
    color: var(--m3-on-surface);
  }

  .preview-files {
    margin: 0;
    padding: 0;
    max-height: 9rem;
    overflow-y: auto;
    list-style: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.7rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .preview-files li {
    padding: 0.08rem 0;
    overflow-wrap: anywhere;
  }

  .preview-error {
    margin: 0;
    color: var(--m3-error, inherit);
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .preview-note {
    margin: 0.4rem 0 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.7rem;
  }

  .actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .guard-note {
    margin-right: auto;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.7rem;
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

  .cancel {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }

  .cancel:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .confirm:focus-visible,
  .cancel:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }
</style>
