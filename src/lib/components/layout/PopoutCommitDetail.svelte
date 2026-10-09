<script lang="ts">
  /**
   * PopoutCommitDetail — the `?panel=commitdetail&repo=<id>&sha=<sha>`
   * popout window's whole content.
   *
   * A popped-out detail has no HistoryView selection state, so this
   * component wires its own: it fetches the commit's `CommitInfo` via a
   * one-root log walk (`streamLog` with `refs: [sha]`), re-fetches when
   * the backend announces repo changes (refs/describe/signature move),
   * and renders `CommitDetail` filling the window. Clicking a parent
   * retargets the window to that commit — the popout's replacement for
   * the inline pane's "reveal in list" navigation.
   *
   * Cherry-pick / revert / rebase-from-here act on the shown commit alone
   * (no multi-selection without the list). The bookmark star stays hidden:
   * bookmarks persist per worktree root, which only the main window knows.
   */
  import { cherryPick, revertCommits, streamLog, onRepoChanged } from "$lib/ipc/client";
  import type { CommitInfo, LogFilter, MergeResult, RepoId } from "$lib/ipc/types";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import RebasePlanner from "$lib/components/rebase/RebasePlanner.svelte";
  import { toast } from "$lib/toast";

  let { repoId, sha }: { repoId: RepoId; sha: string } = $props();

  /**
   * The commit being shown (parent clicks retarget this). `sha` comes from
   * the window's URL query and never changes — capturing its initial value
   * is intended.
   */
  // svelte-ignore state_referenced_locally
  let currentSha = $state(sha);
  let info = $state<CommitInfo | null>(null);
  let error = $state<string | null>(null);
  /** Generation counter; bumping re-triggers the fetch effect. */
  let reload = $state(0);

  /** True while a cherry-pick/revert IPC is in flight. */
  let opBusy = $state(false);
  let plannerOpen = $state(false);

  $effect(() => {
    const target = currentSha;
    const generation = reload;
    void repoId;
    info = null;
    error = null;
    // One-root walk: the first page leads with the target commit itself.
    const filter: LogFilter = {
      text: null,
      regex: false,
      author: null,
      path: null,
      after_unix: null,
      before_unix: null,
      refs: [target],
      follow: false,
      pickaxe: null,
    };
    streamLog(repoId, filter, (page) => {
      if (generation !== reload || info !== null) return;
      const exact = page.commits.find((c) => c.sha === target);
      // Abbreviated input resolves to the full sha — accept a prefix match.
      const match = exact ?? page.commits.find((c) => c.sha.startsWith(target)) ?? null;
      if (match) info = match;
    })
      .then(() => {
        if (generation !== reload) return;
        if (info === null) {
          error = "Commit not found (it may have been pruned).";
        }
      })
      .catch((err: unknown) => {
        if (generation !== reload) return;
        error = err instanceof Error ? err.message : String(err);
      });
  });

  // The popout has no tab store; watch the backend directly.
  $effect(() => {
    const id = repoId;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    onRepoChanged((event) => {
      if (event.repo_id === id) reload += 1;
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Popouts still work without the watcher; the data is static then.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  // -- single-commit versions of the HistoryView actions -----------------------

  function reportMergeResult(op: string, result: MergeResult): void {
    if (result.outcome === "conflicted" || result.conflicts.length > 0) {
      toast(`${op} conflicts — resolve in the status panel`, { kind: "error" });
    } else {
      toast(`${op} commit — done`, { kind: "success" });
    }
  }

  async function doCherryPick(): Promise<void> {
    if (opBusy || !info) return;
    opBusy = true;
    try {
      reportMergeResult("Cherry-picked", await cherryPick(repoId, [info.sha]));
    } catch (err) {
      toast(`Cherry-pick failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    } finally {
      opBusy = false;
    }
  }

  async function doRevert(): Promise<void> {
    if (opBusy || !info) return;
    opBusy = true;
    try {
      reportMergeResult("Reverted", await revertCommits(repoId, [info.sha]));
    } catch (err) {
      toast(`Revert failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    } finally {
      opBusy = false;
    }
  }

  function rebaseFromHere(): void {
    if (!info || opBusy) return;
    plannerOpen = true;
  }
</script>

<section class="popout-commit" aria-label="Commit detail">
  {#if error && !info}
    <p class="state err" role="alert">{error}</p>
  {:else if !info}
    <p class="state" role="status">Loading commit…</p>
  {:else}
    <CommitDetail
      {repoId}
      {info}
      actionCount={1}
      actionBusy={opBusy}
      onSelectParent={(parent) => (currentSha = parent)}
      onCherryPick={() => void doCherryPick()}
      onRevert={() => void doRevert()}
      onRebaseFromHere={rebaseFromHere}
    />
  {/if}

  {#if plannerOpen}
    <RebasePlanner
      {repoId}
      baseSha={info?.sha ?? null}
      onClose={() => (plannerOpen = false)}
      onFinished={() => (plannerOpen = false)}
    />
  {/if}
</section>

<style>
  .popout-commit {
    display: flex;
    flex-direction: column;
    height: 100vh;
    min-height: 0;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  .popout-commit > :global(.detail) {
    flex: 1;
    min-height: 0;
  }

  .state {
    margin: auto;
    padding: 2rem 1rem;
    text-align: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .state.err {
    color: var(--m3-error, red);
  }
</style>
