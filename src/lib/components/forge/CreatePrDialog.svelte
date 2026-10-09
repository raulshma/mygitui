<script lang="ts">
  /**
   * Create-PR dialog (M6 forge) — AI-prefillable PR creation.
   *
   * On open it resets, loads the base-branch options (`branches()`,
   * preferring remote-tracked branches) and up to 10 recent commits (first
   * log page) as the AI hint. "Generate title/body" dispatches the
   * documented `ai-generate-pr` CustomEvent on `window`
   * (`detail = { repoId, commits: [{ sha, summary }] }`) and the dialog
   * listens for `ai-pr-result` (`detail = { subject, body }`) to fill the
   * fields — user typing clears the pending wait (docs/contracts.md "M6 FE
   * contracts"; reconciled with H1's commit-message contract by the
   * orchestrator).
   *
   * Create runs `prCreate` (120 s backend timeout): success toasts, calls
   * `onCreated` (the owner refreshes context + PR list; the dialog stays
   * open showing the URL + "Open on GitHub" until Done), `AlreadyExists`
   * offers the existing PR's URL instead. Cancel / Escape / Done →
   * `onClose()`; the owner flips `open`.
   */
  import {
    branches,
    ForgeIpcError,
    prCreate,
    streamLog,
    type PrCreated,
  } from "$lib/ipc/client";
  import type { BranchInfo } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { openExternal } from "$lib/stores/forge.svelte";
  import { prNumberFromUrl } from "./forgeModel";

  let {
    repoId,
    open,
    onClose,
    onCreated,
  }: {
    repoId: string;
    /** Rendered only when true — mounting is opening. */
    open: boolean;
    onClose: () => void;
    /** Called after a successful create (owner refreshes; dialog stays open). */
    onCreated: (created: PrCreated) => void;
  } = $props();

  let branchList = $state<BranchInfo[]>([]);
  let base = $state("");
  let title = $state("");
  let body = $state("");
  let draft = $state(false);
  let loading = $state(false);
  let creating = $state(false);
  let error = $state<string | null>(null);
  /** Set on success (form switches to the created view). */
  let created = $state<PrCreated | null>(null);
  /** Set when gh reports an existing PR for the branch. */
  let existingUrl = $state<string | null>(null);
  /** Recent commits handed to the AI layer as context. */
  let commits = $state<{ sha: string; summary: string }[]>([]);
  let aiPending = $state(false);
  /** Inline failure from the last AI generation (cleared on retry/success). */
  let aiError = $state<string | null>(null);

  let seq = 0;

  $effect(() => {
    if (!open) return;
    const current = ++seq;
    branchList = [];
    base = "";
    title = "";
    body = "";
    draft = false;
    creating = false;
    error = null;
    created = null;
    existingUrl = null;
    commits = [];
    aiPending = false;
    aiError = null;
    loading = true;

    branches(repoId)
      .then((list) => {
        if (current !== seq) return;
        branchList = list;
        base = defaultBase(list);
        loading = false;
      })
      .catch((err: unknown) => {
        if (current !== seq) return;
        error = err instanceof Error ? err.message : String(err);
        loading = false;
      });

    // First log page → up to 10 recent commit summaries as the AI hint.
    const buffer: { sha: string; summary: string }[] = [];
    streamLog(
      repoId,
      { regex: false, refs: [], follow: false },
      (page) => {
        for (const commit of page.commits) {
          if (buffer.length >= 10) break;
          buffer.push({ sha: commit.sha, summary: commit.summary });
        }
      },
    )
      .then(() => {
        if (current === seq) commits = buffer;
      })
      .catch(() => {
        // AI hint is optional; creation works without it.
      });
  });

  // AI result listener (only while open). Failure events clear the wait and
  // show the reason inline — without this the button sticks on "Waiting…".
  $effect(() => {
    if (!open) return;
    const handler = (event: Event): void => {
      const detail =
        (event as CustomEvent<{ subject?: unknown; body?: unknown }>).detail ?? {};
      if (typeof detail.subject === "string" && detail.subject.trim()) {
        title = detail.subject.trim();
      }
      if (typeof detail.body === "string") {
        body = detail.body;
      }
      aiPending = false;
      aiError = null;
    };
    const errorHandler = (event: Event): void => {
      const detail = (event as CustomEvent<{ message?: unknown }>).detail ?? {};
      aiPending = false;
      aiError =
        typeof detail.message === "string" && detail.message.length > 0
          ? detail.message
          : "AI generation failed";
    };
    window.addEventListener("ai-pr-result", handler);
    window.addEventListener("ai-pr-error", errorHandler);
    return () => {
      window.removeEventListener("ai-pr-result", handler);
      window.removeEventListener("ai-pr-error", errorHandler);
    };
  });

  /** Base options: remote-tracked branches first (fallback: all). */
  const baseOptions = $derived.by(() => {
    const tracked = branchList.filter((b) => b.upstream);
    return (tracked.length > 0 ? tracked : branchList).map((b) => b.name);
  });

  /** Existing-PR number for the already-exists view (URL-only survivor). */
  const existingNumber = $derived(prNumberFromUrl(existingUrl));

  /** "main"/"master" when present, else the first option. */
  function defaultBase(list: BranchInfo[]): string {
    const tracked = list.filter((b) => b.upstream);
    const names = (tracked.length > 0 ? tracked : list).map((b) => b.name);
    return names.includes("main") ? "main" : names.includes("master") ? "master" : (names[0] ?? "");
  }

  function requestAi(): void {
    aiPending = true;
    aiError = null;
    window.dispatchEvent(
      new CustomEvent("ai-generate-pr", { detail: { repoId, commits } }),
    );
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !creating) {
      event.preventDefault();
      onClose();
    }
  }

  const canCreate = $derived(!!base && !!title.trim() && !creating && !loading && !created);

  async function create(): Promise<void> {
    if (!canCreate) return;
    creating = true;
    error = null;
    try {
      const pr = await prCreate(repoId, base, title.trim(), body, draft);
      toast(`PR #${pr.number} created`, { kind: "success" });
      created = pr;
      onCreated(pr);
    } catch (err) {
      if (err instanceof ForgeIpcError) {
        if (err.kind === "AlreadyExists") {
          existingUrl = err.url;
        } else if (err.kind === "NoGh") {
          error = "gh CLI not found — install it from https://cli.github.com/.";
        } else if (err.kind === "NotAuthed") {
          error = "gh is not authenticated — run `gh auth login` in a terminal.";
        } else {
          error = err.message;
        }
      } else {
        error = err instanceof Error ? err.message : String(err);
      }
    } finally {
      creating = false;
    }
  }

  async function openCreated(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="scrim" role="presentation" onkeydown={onKeydown}>
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="create-pr-title">
      <h2 id="create-pr-title" class="dialog-title">Create pull request</h2>

      {#if created}
        {@const done = created}
        <p class="created-line" aria-live="polite">
          PR #{done.number} created successfully.
        </p>
        <p class="created-url" title={done.url}>{done.url}</p>
        <div class="actions">
          <button class="confirm" type="button" onclick={() => void openCreated(done.url)}>
            Open PR
          </button>
          <button class="cancel" type="button" onclick={onClose}>Done</button>
        </div>
      {:else if existingUrl !== null}
        {@const url = existingUrl}
        <p class="created-line" role="alert">
          A pull request already exists for this branch
          {existingNumber !== null ? `(#{existingNumber})` : ""}.
        </p>
        <p class="created-url" title={url}>{url}</p>
        <div class="actions">
          <button class="confirm" type="button" onclick={() => void openCreated(url)}>
            Open PR
          </button>
          <button class="cancel" type="button" onclick={onClose}>Close</button>
        </div>
      {:else}
        <label class="field">
          <span class="label">Base branch</span>
          <select bind:value={base} disabled={loading || baseOptions.length === 0}>
            {#each baseOptions as name (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>
        </label>

        <div class="ai-row">
          <button class="tb" type="button" onclick={requestAi} disabled={loading || aiPending}>
            {aiPending ? "Waiting for AI…" : "Generate title/body"}
          </button>
          {#if aiPending}
            <span class="ai-hint">Waiting for the AI layer — type to fill manually.</span>
          {:else if aiError}
            <span class="ai-hint ai-fail" role="alert">{aiError}</span>
          {/if}
        </div>

        <label class="field">
          <span class="label">Title</span>
          <input
            type="text"
            bind:value={title}
            placeholder="PR title"
            oninput={() => (aiPending = false)}
          />
        </label>

        <label class="field">
          <span class="label">Description</span>
          <textarea
            rows="7"
            bind:value={body}
            placeholder="What does this PR change?"
            oninput={() => (aiPending = false)}
          ></textarea>
        </label>

        <label class="draft" title="Open the PR as a draft (no reviewers requested)">
          <input type="checkbox" bind:checked={draft} />
          <span>Draft</span>
        </label>

        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}

        <div class="actions">
          <span class="hint">Runs `gh pr create` with your gh credentials.</span>
          <button
            class="confirm"
            type="button"
            disabled={!canCreate}
            onclick={() => void create()}
          >
            {creating ? "Creating…" : "Create PR"}
          </button>
          <button class="cancel" type="button" disabled={creating} onclick={onClose}>
            Cancel
          </button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 45%);
    display: grid;
    place-items: center;
    z-index: 60;
  }

  .dialog {
    width: min(34rem, calc(100vw - 3rem));
    max-height: calc(100vh - 4rem);
    overflow-y: auto;
    background: var(--m3-surface-container, var(--m3-surface));
    color: var(--m3-on-surface);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-large, 16px);
    padding: 1rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
  }

  .dialog-title {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .label {
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  select,
  input[type="text"],
  textarea {
    font: inherit;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.3rem 0.45rem;
    resize: vertical;
  }

  select:focus-visible,
  input:focus-visible,
  textarea:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .ai-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .ai-hint {
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    overflow-wrap: anywhere;
  }

  .ai-hint.ai-fail {
    color: var(--m3-error, inherit);
  }

  .tb {
    align-self: flex-start;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .draft {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .draft input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .error {
    margin: 0;
    color: var(--m3-error, inherit);
    font-size: 0.75rem;
  }

  .created-line {
    margin: 0;
    font-weight: 600;
  }

  .created-url {
    margin: 0;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.25rem;
    flex-wrap: wrap;
  }

  .hint {
    flex: 1;
    min-width: 10rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .confirm {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.7813rem;
    padding: 0.3rem 1rem;
    cursor: pointer;
  }

  .confirm:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .cancel {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.7813rem;
    padding: 0.3rem 1rem;
    cursor: pointer;
  }

  .confirm:focus-visible,
  .cancel:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
