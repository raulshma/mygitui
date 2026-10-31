<script lang="ts">
  /**
   * Reset dialog (M3 safety) — move the current branch with a live preview.
   *
   * Mode radio (soft / mixed / keep / merge / hard, plain-language), a
   * target ref input with a datalist of branches + tags, and a live
   * `ops_preview` of the reset impact (what would change on disk), refreshed
   * on open and on target change (debounced 300ms, stale responses dropped).
   *
   * Confirm flows:
   *   - hard/keep/merge (destructive): `guard_checkpoint("pre-<mode>-reset")`
   *     → `reset` → success toast quoting the checkpoint id ("undo
   *     available") → `onDone()`;
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
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import {
    buildResetParams,
    buildResetPreviewParams,
    RESET_MODES,
    RESET_PREVIEW_KIND,
    previewSummaryLines,
    resetIsDestructive,
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
  let targetField = $state<HTMLInputElement | null>(null);

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

  async function confirm(): Promise<void> {
    if (running || !target.trim()) return;
    running = true;
    try {
      if (resetIsDestructive(mode)) {
        const cp = await guardCheckpoint(repoId, `pre-${mode}-reset`);
        await resetRepo(repoId, ...resetArgs());
        toast(
          `${label(mode)} reset to ${target} — undo available (checkpoint ${cp.id})`,
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
  <Dialog.Root {open}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class="dialog max-h-[calc(100vh-4rem)] w-[min(30rem,calc(100vw-2rem))] max-w-none gap-0 overflow-y-auto rounded-lg bg-card px-6 py-5 text-sm text-popover-foreground shadow-xl sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="reset-title"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={(e) => {
        if (e.key === "Escape" && !running) {
          e.preventDefault();
          onClose();
        }
      }}
    >
      <Dialog.Title id="reset-title" class="m-0 mb-3 text-lg font-medium">Reset current branch</Dialog.Title>

      <form
        onsubmit={(event) => {
          event.preventDefault();
          void confirm();
        }}
      >
        <fieldset class="modes mb-[0.875rem] mt-0 flex flex-col gap-2 border-0 p-0">
          <legend class="sr-only">Reset mode</legend>
          {#each RESET_MODES as m (m.kind)}
            <label class="mode flex cursor-pointer items-start gap-2">
              <input class="mt-[0.15rem] accent-primary" type="radio" name="reset-mode" value={m.kind} bind:group={mode} />
              <span class="mode-text flex flex-col gap-[0.1rem]">
                <span class="mode-label font-semibold">{m.label}</span>
                <span class="mode-desc text-xs text-muted-foreground">{m.description}</span>
              </span>
            </label>
          {/each}
        </fieldset>

        <label class="target mb-3 flex flex-col gap-1">
          <span class="field-label text-xs text-muted-foreground">Reset to (ref, branch, tag or sha)</span>
          <Input
            type="text"
            spellcheck="false"
            autocomplete="off"
            list="reset-target-suggestions"
            class="h-auto rounded-sm px-[0.6rem] py-[0.45rem] font-mono text-sm focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-ring"
            bind:value={target}
            bind:ref={targetField}
          />
          <datalist id="reset-target-suggestions">
            {#each suggestions as name (name)}
              <option value={name}></option>
            {/each}
          </datalist>
        </label>

        <div class="preview mb-[0.875rem] min-h-[3.25rem] rounded-sm border border-border bg-background px-[0.6rem] py-2" aria-live="polite">
          {#if previewError}
            <p class="preview-error m-0 text-xs text-destructive [overflow-wrap:anywhere]" role="alert">Preview failed: {previewError}</p>
          {:else if previewing || !previewInfo}
            <p class="preview-loading m-0 text-xs text-muted-foreground">Previewing…</p>
          {:else}
            <p class="preview-summary mb-[0.35rem] mt-0 text-[0.78rem] text-foreground">{lines[0]}</p>
            {#if lines.length > 1}
              <ul class="preview-files m-0 max-h-36 list-none overflow-y-auto p-0 font-mono text-[0.7rem] text-muted-foreground" role="list" aria-label="Files affected by a hard reset">
                {#each lines.slice(1) as line, i (i)}
                  <li class="py-[0.08rem] [overflow-wrap:anywhere]">{line}</li>
                {/each}
              </ul>
            {:else}
              <p class="preview-empty m-0 text-xs text-muted-foreground">No file changes.</p>
            {/if}
            {#if mode === "keep" || mode === "merge"}
              <p class="preview-note mb-0 mt-[0.4rem] text-[0.7rem] text-muted-foreground">
                Preview shows the file impact; a {label(mode).toLowerCase()} reset
                preserves uncommitted changes where it safely can.
              </p>
            {:else if mode !== "hard"}
              <p class="preview-note mb-0 mt-[0.4rem] text-[0.7rem] text-muted-foreground">
                Preview shows the hard-reset impact; {label(mode).toLowerCase()} reset keeps your changes.
              </p>
            {/if}
          {/if}
        </div>

        <div class="actions flex items-center justify-end gap-2">
          {#if resetIsDestructive(mode)}
            <span class="guard-note mr-auto text-[0.7rem] text-muted-foreground">A checkpoint is created first (undo available).</span>
          {/if}
          <Button
            variant={resetIsDestructive(mode) ? "destructive" : "default"}
            type="submit"
            disabled={running || !target.trim()}
          >
            {running ? "Resetting…" : `Reset (${label(mode)})`}
          </Button>
          <Button
            variant="outline"
            class="text-muted-foreground"
            type="button"
            disabled={running}
            onclick={onClose}
          >
            Cancel
          </Button>
        </div>
      </form>
    </Dialog.Content>
  </Dialog.Root>
{/if}
