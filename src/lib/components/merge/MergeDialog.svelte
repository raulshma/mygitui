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
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
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
  <Dialog.Root {open}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class="dialog max-h-[calc(100vh-4rem)] w-[min(28rem,calc(100vw-2rem))] max-w-none gap-0 overflow-y-auto rounded-2xl bg-card px-6 py-5 text-sm text-popover-foreground shadow-lg sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="merge-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        }
      }}
    >
      <form
        onsubmit={(e) => {
          e.preventDefault();
          void run();
        }}
      >
        <Dialog.Title id="merge-title" class="mb-1 text-lg font-medium text-foreground">Merge “{branch}”</Dialog.Title>
        <p class="text mb-3 text-muted-foreground">Into the current branch.</p>

        {#if dirty}
          <p class="dirty mb-3 rounded-lg bg-destructive/10 px-2.5 py-1.5 text-[0.8125rem] text-destructive" role="alert">
            Working copy has uncommitted changes — commit or stash first
          </p>
        {/if}

        <fieldset class="group mb-3 rounded-xl border border-border px-3 py-2">
          <legend class="px-1 text-[0.72rem] text-muted-foreground">Fast-forward</legend>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <input class="m-0 accent-primary" type="radio" bind:group={ffMode} value="auto" />
            <span>Allow fast-forward</span>
          </label>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <input class="m-0 accent-primary" type="radio" bind:group={ffMode} value="no_ff" />
            <span>Always create a merge commit (--no-ff)</span>
          </label>
        </fieldset>

        <fieldset class="group mb-3 rounded-xl border border-border px-3 py-2">
          <legend class="px-1 text-[0.72rem] text-muted-foreground">Result</legend>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <Checkbox bind:checked={squash} />
            <span>Squash — stage the result, single-parent commit later</span>
          </label>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <Checkbox bind:checked={noCommit} />
            <span>Stop before committing (--no-commit)</span>
          </label>
        </fieldset>

        <fieldset class="group mb-3 rounded-xl border border-border px-3 py-2">
          <legend class="px-1 text-[0.72rem] text-muted-foreground">Conflicting hunks (-X)</legend>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <input class="m-0 accent-primary" type="radio" bind:group={favor} value="none" />
            <span>Leave conflicts for me to resolve</span>
          </label>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <input class="m-0 accent-primary" type="radio" bind:group={favor} value="ours" />
            <span>Auto-resolve in favor of the current branch</span>
          </label>
          <label class="flex cursor-pointer items-center gap-1.5 py-0.5">
            <input class="m-0 accent-primary" type="radio" bind:group={favor} value="theirs" />
            <span>Auto-resolve in favor of “{branch}”</span>
          </label>
        </fieldset>

        <div class="actions mt-1 flex justify-end gap-2">
          <Button
            variant="outline"
            class="text-muted-foreground"
            type="button"
            onclick={onClose}
            disabled={busy}
          >
            Cancel
          </Button>
          <Button type="submit" disabled={busy || dirty}>
            {busy ? "Merging…" : "Merge"}
          </Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}
