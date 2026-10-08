<script lang="ts">
  /**
   * Clone-repository dialog (B4 lane).
   *
   * App.svelte owns the `open` flag (bindable) and mounts this component
   * for the whole session, so an in-flight clone keeps reporting even if
   * the dialog is closed — progress events are matched by URL and the
   * success path (toast + `openTab`) still runs.
   *
   * Backend contract: `cloneRepo(url, destination, depth?)` → Rust
   * `repo_clone`, which emits `clone-progress` events
   * `{url, received, total, objects}` (received = bytes; total/objects =
   * object counts — libgit2 has no total-bytes estimate) and resolves with
   * the cloned repository path.
   */
  import { onMount } from "svelte";
  import {
    cloneRepo,
    cloneBlobless,
    onCloneProgress,
    pickFolder,
    type CloneProgressEvent,
  } from "$lib/ipc/client";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";

  let { open = $bindable(false) }: { open?: boolean } = $props();

  let url = $state("");
  let destination = $state("");
  let shallow = $state(false);
  let blobless = $state(false);
  let cloning = $state(false);
  let error = $state("");
  let progress = $state<CloneProgressEvent | null>(null);
  /** URL of the clone currently in flight (progress events are matched on it). */
  let activeUrl: string | null = null;
  let urlInput: HTMLInputElement | undefined = $state();

  const canClone = $derived(url.trim() !== "" && destination.trim() !== "" && !cloning);
  const percent = $derived(
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.objects / progress.total) * 100))
      : null,
  );

  // Progress events live for the component's lifetime (not just while the
  // dialog is open) so background clones still complete visibly.
  onMount(() => {
    let unlisten: (() => void) | undefined;
    void onCloneProgress((event) => {
      if (activeUrl !== null && event.url === activeUrl) progress = event;
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  });

  $effect(() => {
    if (open) {
      error = "";
      urlInput?.focus();
    }
  });

  function close(): void {
    if (cloning) {
      // Keep the dialog up while a clone runs (no cancel in M1).
      toast("Clone is still running — it continues in the background.", {
        kind: "info",
      });
      open = false;
      return;
    }
    open = false;
  }

  async function browse(): Promise<void> {
    const picked = await pickFolder();
    if (picked) destination = picked;
  }

  async function startClone(): Promise<void> {
    if (!canClone) return;
    const targetUrl = url.trim();
    const destPath = destination.trim();
    cloning = true;
    error = "";
    progress = null;
    activeUrl = targetUrl;
    try {
      // Blobless clones route through the git CLI (libgit2 has no --filter).
      const path = blobless
        ? await cloneBlobless(targetUrl, destPath, shallow ? 1 : undefined)
        : await cloneRepo(targetUrl, destPath, shallow ? 1 : undefined);
      toast(`Cloned ${targetUrl} → ${path}`, { kind: "success" });
      open = false;
      try {
        await openTab(path);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        toast(`Failed to open ${path}: ${message}`, { kind: "error" });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      toast(`Clone failed: ${message}`, { kind: "error" });
    } finally {
      cloning = false;
      activeUrl = null;
    }
  }

  function fmtBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    const units = ["KB", "MB", "GB", "TB"];
    let value = n;
    let unit = -1;
    do {
      value /= 1024;
      unit += 1;
    } while (value >= 1024 && unit < units.length - 1);
    return `${value.toFixed(1)} ${units[unit]}`;
  }
</script>

{#if open}
  <div class="scrim">
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="clone-title"
    >
      <h2 id="clone-title" class="title">Clone repository</h2>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void startClone();
        }}
      >
        <label class="field">
          <span class="field-label">Repository URL</span>
          <input
            class="input"
            type="text"
            placeholder="https://github.com/user/repo.git"
            autocomplete="off"
            spellcheck="false"
            bind:value={url}
            bind:this={urlInput}
            disabled={cloning}
          />
        </label>

        <label class="field">
          <span class="field-label">Destination folder</span>
          <span class="dest-row">
            <input
              class="input"
              type="text"
              placeholder="C:\\path\\to\\folder"
              autocomplete="off"
              spellcheck="false"
              bind:value={destination}
              disabled={cloning}
            />
            <button
              class="secondary"
              type="button"
              disabled={cloning}
              onclick={() => void browse()}
            >
              Browse…
            </button>
          </span>
        </label>

        <label class="check">
          <input type="checkbox" bind:checked={shallow} disabled={cloning} />
          <span>Shallow clone (<code>--depth 1</code>)</span>
        </label>

        <label class="check">
          <input type="checkbox" bind:checked={blobless} disabled={cloning} />
          <span>Blobless clone (<code>--filter=blob:none</code>, fetches file contents on demand)</span>
        </label>

        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}

        {#if cloning && progress}
          <div
            class="progress"
            role="progressbar"
            aria-label="Clone progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent ?? undefined}
          >
            <div class="bar" style:width={percent !== null ? `${percent}%` : "100%"} class:indeterminate={percent === null}></div>
          </div>
          <p class="stats">
            {percent !== null ? `${percent}% · ` : ""}{progress.objects}/{progress.total} objects · {fmtBytes(progress.received)}
          </p>
        {:else if cloning}
          <p class="stats">Contacting remote…</p>
        {/if}

        <div class="actions">
          <button class="secondary" type="button" onclick={close}>
            {cloning ? "Hide" : "Cancel"}
          </button>
          <button class="primary" type="submit" disabled={!canClone}>
            {cloning ? "Cloning…" : "Clone"}
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
    z-index: 50;
    background: color-mix(in srgb, var(--m3-scrim, #000) 32%, transparent);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
  }

  .dialog {
    width: min(32rem, calc(100vw - 2rem));
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-extra-large, 28px);
    box-shadow: var(--m3-elevation-3, 0 4px 8px rgba(0, 0, 0, 0.3));
    padding: 1.5rem;
  }

  .title {
    margin: 0 0.25rem 1rem;
    font-size: 1.375rem;
    font-weight: 400;
    color: var(--m3-on-surface);
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field-label {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .dest-row {
    display: flex;
    gap: 0.5rem;
  }

  .input {
    flex: 1;
    min-width: 0;
    border: none;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-highest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.8125rem;
    padding: 0.625rem 0.75rem;
  }

  .input:focus-visible {
    outline: 2px solid var(--m3-primary);
  }

  .input:disabled {
    opacity: 0.6;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
  }

  .check input {
    accent-color: var(--m3-primary);
  }

  .check code {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .error {
    margin: 0;
    color: var(--m3-error);
    font-size: 0.8125rem;
  }

  .progress {
    height: 0.5rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-highest, var(--m3-surface));
    overflow: hidden;
  }

  .bar {
    height: 100%;
    background: var(--m3-primary);
    border-radius: inherit;
    transition: width 0.15s ease;
  }

  .bar.indeterminate {
    animation: slide 1.1s ease-in-out infinite;
  }

  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(100%);
    }
  }

  .stats {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .primary,
  .secondary {
    font: inherit;
    font-size: 0.875rem;
    padding: 0.5rem 1.25rem;
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
  }

  .primary {
    border: none;
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .primary:disabled {
    background: color-mix(in srgb, var(--m3-on-surface) 12%, transparent);
    color: color-mix(in srgb, var(--m3-on-surface) 38%, transparent);
    cursor: not-allowed;
  }

  .secondary {
    border: 1px solid var(--m3-outline, var(--m3-primary));
    background: none;
    color: var(--m3-primary);
  }

  .secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .primary:focus-visible,
  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }
</style>
