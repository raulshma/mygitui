<script lang="ts">
  /**
   * Overview inline diff — one file's diff for an expanded row. Same
   * invocation path the old AgentDiff / the workspace diff use:
   * `streamDiff` (worktree side: index→worktree, index side: HEAD→index)
   * with a generation-counter stale guard, then the shared `DiffViewer`.
   * Read-only: no hunk actions — the Overview monitors, the workspace
   * mutates. Mounted lazily: the component only exists for the expanded
   * row, so streams stop when the row collapses.
   */
  import { streamDiff } from "$lib/ipc/client";
  import type { DiffSide, FileDiff, RepoId } from "$lib/ipc/types";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import { createImageLoader } from "$lib/components/diff/imageLoad";
  import { describeError } from "$lib/errors";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    repoId,
    path,
    side = "worktree",
  }: {
    repoId: RepoId;
    path: string;
    side?: "worktree" | "index";
  } = $props();

  const iconDifference = $derived(appIcon("difference")!);
  const iconRetry = $derived(appIcon("restart-alt")!);

  /** `new` is a reserved word, so it only appears as a property key. */
  const sides = $derived.by<{ old: DiffSide; new: DiffSide }>(() => {
    return side === "worktree"
      ? { old: "index", new: "worktree" }
      : { old: "head", new: "index" };
  });

  /** Image-diff URLs per side; `$derived` keeps the identity stable. */
  const imageLoader = $derived(createImageLoader(repoId, sides));

  let files = $state<FileDiff[]>([]);
  /** The current stream delivered at least its first page / settled. */
  let landed = $state(false);
  /** Failure message from the last stream (null = none): a failed load
   *  must render as an error, not as the "No differences" empty state. */
  let diffError = $state<string | null>(null);
  /** Bumped by the error state's Retry to re-run the streaming effect. */
  let reload = $state(0);

  /** Stale-guard: late pages of a superseded stream are dropped. */
  let gen = 0;

  $effect(() => {
    const id: RepoId = repoId;
    const p = path;
    const s = sides;
    void reload; // the error state's Retry re-runs the stream
    const current = ++gen;
    files = [];
    landed = false;
    diffError = null;
    const run = streamDiff(
      id,
      s.old,
      s.new,
      (page) => {
        if (current !== gen) return;
        files = [...files, ...page];
      },
      [p],
      // Own stream slot PER REPO: two expanded rows must not supersede each
      // other's stream (same-slot streams cancel backend-side), and the
      // overview never supersedes the workspace diff streams.
      `overview-diff-${id}`,
    )
      .then(() => {
        if (current === gen) landed = true;
      })
      .catch((err: unknown) => {
        if (current !== gen) return;
        // A dropped backend handle must render as an error, not as
        // "No changes" (the RepoView diffError pattern).
        files = [];
        landed = false;
        diffError = describeError(err);
        console.error("[OverviewFileDiff] diff stream failed:", err);
      });
    return () => {
      void run;
    };
  });
</script>

<div class="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card">
  <div class="flex h-6 w-full flex-none items-center gap-2 box-border border-b border-border bg-card px-2 select-none">
    <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-ui-meta" title={path}>{path}</span>
    <Badge
      variant="secondary"
      class="shrink-0 gap-ui-2 rounded-full px-2 py-px h-auto text-ui-micro bg-muted text-foreground"
    >
      {side === "worktree" ? "unstaged" : "staged"}
    </Badge>
  </div>
  <div class="min-h-0 flex-1 overflow-hidden">
    {#if diffError}
      <EmptyState icon={iconRetry} title="Diff failed" hint={diffError}>
        <Button
          variant="outline"
          size="sm"
          type="button"
          aria-label="Retry loading the diff"
          onclick={() => (reload += 1)}
        >Retry</Button>
      </EmptyState>
    {:else if landed && files.length === 0}
      <EmptyState icon={iconDifference} title="No differences" />
    {:else if files.length > 0}
      <DiffViewer {files} {repoId} onLoadImage={imageLoader} />
    {:else}
      <div class="flex items-center gap-2 p-2 text-ui-meta text-muted-foreground" aria-busy="true">
        <span class="inline-block size-1.5 animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
        Loading diff…
      </div>
    {/if}
  </div>
</div>
