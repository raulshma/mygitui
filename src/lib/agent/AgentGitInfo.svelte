<script lang="ts">
  /**
   * Agent mode — compact "git info" card: branch + ahead/behind, in-progress
   * op badges, working-copy counts, the first page of recent commits and a
   * fetch/pull/push row. Every sub-block degrades silently (console.error
   * only) when status is null or a stream fails — never a heavy panel.
   */
  import { streamLog } from "$lib/ipc/client";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import {
    fetchDefaultRemote,
    pullCurrentBranch,
    pushCurrentBranch,
    type SyncOutcome,
    type SyncVerb,
  } from "$lib/git/sync";
  import { describeError } from "$lib/errors";
  import { formatRelativeTime } from "$lib/git/format";
  import type { CommitInfo, LogFilter, RepoStatus } from "$lib/ipc/types";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { groupStatusEntries } from "./changesModel";

  let { repoId, status }: { repoId: string; status: RepoStatus | null } = $props();

  const MAX_COMMITS = 5;

  const iconFetch = $derived(appIcon("download")!);
  const iconPull = $derived(appIcon("sync")!);
  const iconPush = $derived(appIcon("upload")!);

  /** Same default shape `toLogFilter(EMPTY_FILTER)` serializes elsewhere. */
  const DEFAULT_LOG_FILTER: LogFilter = {
    text: null,
    regex: false,
    author: null,
    path: null,
    after_unix: null,
    before_unix: null,
    refs: [],
    follow: false,
    pickaxe: null,
  };

  const counts = $derived(status ? groupStatusEntries(status.entries) : null);

  let commits = $state<CommitInfo[]>([]);

  /** Stale-guard for the log stream (bumped per effect run). */
  let logGen = 0;

  $effect(() => {
    const id = repoId;
    const current = ++logGen;
    commits = [];
    void streamLog(id, DEFAULT_LOG_FILTER, (page) => {
      if (current !== logGen) return;
      commits = page.commits.slice(0, MAX_COMMITS);
    }).catch((err: unknown) => {
      if (current !== logGen) return;
      commits = [];
      console.error("[AgentGitInfo] log stream failed:", err);
    });
  });

  let syncing = $state<SyncVerb | null>(null);
  let syncError = $state<string | null>(null);
  /** Precondition skips ("no remote") — ordinary states, not failures. */
  let syncNote = $state<string | null>(null);

  /**
   * Mirrors the header quick actions' shared sync verbs (`$lib/git/sync`):
   * fetch = primary remote (prune), pull = ff-only on the current branch,
   * push = plain single-branch push. Completion refreshes the status.
   */
  async function runSync(verb: SyncVerb): Promise<void> {
    if (repoId === "" || syncing !== null) return;
    syncing = verb;
    syncError = null;
    syncNote = null;
    try {
      let outcome: SyncOutcome;
      if (verb === "fetch") {
        outcome = await fetchDefaultRemote(repoId);
      } else if (verb === "pull") {
        outcome = await pullCurrentBranch(repoId, status?.branch ?? null);
      } else {
        outcome = await pushCurrentBranch(repoId, status?.branch ?? null);
      }
      if (outcome.kind === "skipped") {
        syncNote = outcome.reason === "no-remote" ? "no remote" : "no branch";
      } else {
        void refreshStatus(repoId);
      }
    } catch (err) {
      syncError = describeError(err);
    } finally {
      syncing = null;
    }
  }
</script>

<div class="flex min-h-0 min-w-0 flex-col rounded-md border border-border bg-card text-ui-base select-none">
  {#if status}
    <div class="flex h-6 w-full items-center gap-2 px-2">
      <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-ui-meta font-medium" title={status.branch ?? "detached"}>
        {status.branch ?? "detached"}
      </span>
      {#if status.head}
        <span class="shrink-0 font-mono text-ui-meta text-muted-foreground">{status.head.slice(0, 7)}</span>
      {/if}
      {#if status.ahead > 0 || status.behind > 0}
        <span class="shrink-0 text-ui-meta tabular-nums text-muted-foreground">
          {#if status.ahead > 0}↑{status.ahead}{/if}
          {#if status.ahead > 0 && status.behind > 0}{" "}{/if}
          {#if status.behind > 0}↓{status.behind}{/if}
        </span>
      {/if}
    </div>
  {/if}

  {#if status && (status.merging || status.rebasing || status.sequencer || status.bisecting)}
    <div class="flex w-full flex-wrap items-center gap-1 px-2 pb-1">
      {#if status.merging}<Badge variant="secondary" class="rounded-full px-2 py-px h-auto text-ui-micro bg-muted text-foreground">merge</Badge>{/if}
      {#if status.rebasing}<Badge variant="secondary" class="rounded-full px-2 py-px h-auto text-ui-micro bg-muted text-foreground">rebase</Badge>{/if}
      {#if status.sequencer}<Badge variant="secondary" class="rounded-full px-2 py-px h-auto text-ui-micro bg-muted text-foreground">cherry-pick</Badge>{/if}
      {#if status.bisecting}<Badge variant="secondary" class="rounded-full px-2 py-px h-auto text-ui-micro bg-muted text-foreground">bisect</Badge>{/if}
    </div>
  {/if}

  {#if counts}
    <div class="flex h-6 w-full items-center px-2 text-ui-meta text-muted-foreground">
      {counts.staged.length} staged · {counts.changed.length} changed · {counts.untracked.length} untracked
    </div>
  {/if}

  {#if commits.length > 0}
    <div class="flex h-6 w-full items-center border-t border-border px-2 text-ui-micro font-bold tracking-[0.045em] uppercase text-muted-foreground select-none">
      Recent
    </div>
    {#each commits as commit (commit.sha)}
      <div class="flex h-6 w-full items-center gap-2 px-2 min-w-0">
        <span class="shrink-0 font-mono text-ui-meta text-muted-foreground">{commit.sha.slice(0, 7)}</span>
        <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-ui-meta text-foreground" title={commit.summary}>{commit.summary}</span>
        <span class="shrink-0 text-ui-meta text-muted-foreground">{formatRelativeTime(commit.committer.time)}</span>
      </div>
    {/each}
  {/if}

  <div class="flex h-6 w-full items-center gap-0.5 border-t border-border px-1.5">
    <Hint label="Fetch">
      {#snippet child({ props })}
        <Button
          {...props}
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Fetch"
          disabled={syncing !== null}
          onclick={() => void runSync("fetch")}
        >
          <Icon icon={iconFetch} size={14} />
        </Button>
      {/snippet}
    </Hint>
    <Hint label="Pull (fast-forward)">
      {#snippet child({ props })}
        <Button
          {...props}
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Pull"
          disabled={syncing !== null}
          onclick={() => void runSync("pull")}
        >
          <Icon icon={iconPull} size={14} />
        </Button>
      {/snippet}
    </Hint>
    <Hint label="Push">
      {#snippet child({ props })}
        <Button
          {...props}
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Push"
          disabled={syncing !== null}
          onclick={() => void runSync("push")}
        >
          <Icon icon={iconPush} size={14} />
        </Button>
      {/snippet}
    </Hint>
    {#if syncing}
      <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap pl-1 text-ui-micro text-muted-foreground">
        {syncing}ing…
      </span>
    {:else if syncError}
      <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap pl-1 text-ui-micro text-destructive" role="alert">{syncError}</span>
    {:else if syncNote}
      <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap pl-1 text-ui-micro text-muted-foreground">{syncNote}</span>
    {/if}
  </div>
</div>
