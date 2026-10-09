<!--
  MergeDialog (M9 F2) — merge `branch` into the current branch with the
  `git merge` options mygitui exposes: fast-forward policy, squash,
  --no-commit, and `-X ours/theirs` conflict favor.

  The caller provides the branch name; this component owns the options form
  and the merge run. Outcomes toast (squashed / no-commit / merged /
  fast-forward / up-to-date); conflicts land in the existing banner +
  ConflictEditor flow (RepoView reacts to the refreshed status).

  M12 (lane C) dirty guard: on open the dialog checks `repo_status` and —
  when the working copy carries any real change (see mergeModel.ts) — shows
  a warning banner and disables the Merge confirm (no override): commit or
  stash first.
-->
<script lang="ts">
  import { mergeBranch, repoStatus } from "$lib/ipc/client";
  import type { MergeOptions, MergeResult } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { workingCopyDirty } from "./mergeModel";

  let {
    open,
    repoId,
    branch,
    onClose,
    onDone = undefined,
  }: {
    open: boolean;
    repoId: string;
    /** Branch to merge INTO the current one. */
    branch: string;
    /** Close request (cancel or successful merge). */
    onClose: () => void;
    /** Called after the merge settles (any outcome) — owner refreshes. */
    onDone?: (result: MergeResult | null) => void;
  } = $props();

  let ffMode = $state<"auto" | "no_ff">("auto");
  let squash = $state(false);
  let noCommit = $state(false);
  let favor = $state<"none" | "ours" | "theirs">("none");
  let busy = $state(false);
  /** True when the working copy has uncommitted changes (blocks Merge). */
  let dirty = $state(false);

  $effect(() => {
    if (open) {
      ffMode = "auto";
      squash = false;
      noCommit = false;
      favor = "none";
      dirty = false;
      // Dirty guard: check the working copy fresh on every open. A failed
      // check does not block (the merge itself will surface real errors).
      repoStatus(repoId)
        .then((status) => {
          if (open) dirty = workingCopyDirty(status.entries);
        })
        .catch(() => {});
    }
  });

  function outcomeMessage(result: MergeResult): string {
    switch (result.outcome) {
      case "merged":
        return "Merged (merge commit created)";
      case "fast_forward":
        return "Fast-forwarded";
      case "squashed":
        return "Squash-merged — the changes are staged; commit to finish";
      case "no_commit":
        return "Merged — commit to finish (MERGE_HEAD is set)";
      case "up_to_date":
        return "Already up to date";
      case "conflicted":
        return "Conflicts — resolve them, then commit";
    }
  }

  async function run(): Promise<void> {
    if (busy) return;
    busy = true;
    const options: MergeOptions = {
      no_ff: ffMode === "no_ff",
      squash,
      no_commit: noCommit,
      favor,
    };
    try {
      const result = await mergeBranch(repoId, branch, options);
      toast(
        result.outcome === "conflicted"
          ? outcomeMessage(result)
          : `Merge of ${branch}: ${outcomeMessage(result)}`,
        { kind: result.outcome === "conflicted" ? "error" : "success" },
      );
      onClose();
      onDone?.(result);
    } catch (err) {
      toast(
        `Merge failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
      onDone?.(null);
    } finally {
      busy = false;
    }
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
        onClose();
      }
    }}
  >
    <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
    <form
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="merge-title"
      onsubmit={(e) => {
        e.preventDefault();
        void run();
      }}
    >
      <h2 id="merge-title" class="title">Merge “{branch}”</h2>
      <p class="text">Into the current branch.</p>

      {#if dirty}
        <p class="dirty" role="alert">
          Working copy has uncommitted changes — commit or stash first
        </p>
      {/if}

      <fieldset class="group">
        <legend>Fast-forward</legend>
        <label>
          <input type="radio" bind:group={ffMode} value="auto" />
          <span>Allow fast-forward</span>
        </label>
        <label>
          <input type="radio" bind:group={ffMode} value="no_ff" />
          <span>Always create a merge commit (--no-ff)</span>
        </label>
      </fieldset>

      <fieldset class="group">
        <legend>Result</legend>
        <label>
          <input type="checkbox" bind:checked={squash} />
          <span>Squash — stage the result, single-parent commit later</span>
        </label>
        <label>
          <input type="checkbox" bind:checked={noCommit} />
          <span>Stop before committing (--no-commit)</span>
        </label>
      </fieldset>

      <fieldset class="group">
        <legend>Conflicting hunks (-X)</legend>
        <label>
          <input type="radio" bind:group={favor} value="none" />
          <span>Leave conflicts for me to resolve</span>
        </label>
        <label>
          <input type="radio" bind:group={favor} value="ours" />
          <span>Auto-resolve in favor of the current branch</span>
        </label>
        <label>
          <input type="radio" bind:group={favor} value="theirs" />
          <span>Auto-resolve in favor of “{branch}”</span>
        </label>
      </fieldset>

      <div class="actions">
        <button
          class="secondary"
          type="button"
          onclick={onClose}
          disabled={busy}
        >
          Cancel
        </button>
        <button class="primary" type="submit" disabled={busy || dirty}>
          {busy ? "Merging…" : "Merge"}
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
    width: min(28rem, calc(100vw - 2rem));
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
    margin: 0 0 0.25rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .text {
    margin: 0 0 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .dirty {
    margin: 0 0 0.75rem;
    padding: 0.4rem 0.6rem;
    border-radius: var(--m3-shape-small, 8px);
    background: color-mix(in srgb, var(--m3-error, #ba1a1a) 12%, transparent);
    color: var(--m3-error, #ba1a1a);
    font-size: 0.8125rem;
  }

  .group {
    border: 1px solid var(--m3-outline-variant, transparent);
    border-radius: var(--m3-shape-medium, 12px);
    padding: 0.5rem 0.75rem;
    margin: 0 0 0.75rem;
  }

  legend {
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 0 0.3rem;
  }

  label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.15rem 0;
    cursor: pointer;
  }

  label input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.25rem;
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
