<script lang="ts">
  /**
   * EditorEmptyOverview — the enhanced "Nothing open" view for EditorPane.
   *
   * Displays:
   * - Git details: active branch & upstream status, working tree changes
   *   (staged/unstaged/untracked/conflicts), latest commit info, in-progress operations.
   * - GitHub details: active PR for the current branch, CI/check status,
   *   recent workflow runs, or degraded status (offline, not signed in, etc.)
   *   reusing existing stores (`forgeStore`, `forgeChips`) without extra IPC calls.
   * - Quick actions and shortcuts (open first change, browse history, switch branch, open forge).
   */
  import type { BranchInfo, CommitInfo, RepoStatus } from "$lib/ipc/types";
  import { countsFromEntries } from "$lib/overview/overviewModel";
  import {
    conflictSource,
    conflictSourceLabel,
    shortSha,
  } from "$lib/components/panels/panelModel";
  import { formatRelativeTime, shortRefName } from "$lib/git/format";
  import { branchesFor } from "$lib/stores/refs.svelte";
  import { streamLog } from "$lib/ipc/client";
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { forgeChips } from "$lib/forge/chips.svelte";
  import {
    countChecks,
    degradedGuidance,
    runDurationLabel,
    summarize,
  } from "$lib/forge/indication.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import { copyText } from "$lib/clipboard";
  import AuthorAvatar from "$lib/components/panels/AuthorAvatar.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Kbd } from "$lib/components/ui/kbd/index.js";

  let {
    repoId,
    root = "",
    repoName = "",
    status = null,
    onOpenFirstChange = undefined,
    onBrowseHistory = undefined,
  }: {
    repoId: string;
    root?: string;
    repoName?: string;
    status: RepoStatus | null;
    onOpenFirstChange?: () => void;
    onBrowseHistory?: () => void;
  } = $props();

  const iconTree = $derived(appIcon("account-tree")!);
  const iconBranch = $derived(appIcon("call-split") ?? appIcon("account-tree")!);
  const iconForge = $derived(appIcon("pull-request") ?? appIcon("merge-type")!);
  const iconCheck = $derived(appIcon("check")!);
  const iconAlert = $derived(appIcon("triangle-alert")!);
  const iconError = $derived(appIcon("error")!);
  const iconPlay = $derived(appIcon("play-arrow")!);
  const iconExternal = $derived(appIcon("open-in-new")!);
  const iconHistory = $derived(appIcon("history")!);
  const iconDiff = $derived(appIcon("difference") ?? appIcon("description")!);

  // -- Git details -----------------------------------------------------------
  const branchLabel = $derived(
    status
      ? (status.branch ??
          (status.detached
            ? status.head
              ? `detached @ ${shortSha(status.head)}`
              : "detached HEAD"
            : "unknown"))
      : "",
  );

  const changeCount = $derived(
    status ? new Set(status.entries.map((e) => e.path)).size : 0,
  );

  const counts = $derived(
    status
      ? countsFromEntries(status.entries)
      : { staged: 0, unstaged: 0, untracked: 0, conflicts: 0 },
  );

  const conflictOp = $derived(status ? conflictSource(status) : null);

  // Upstream branch info query
  let currentBranchInfo = $state<BranchInfo | null>(null);
  $effect(() => {
    const bName = status?.branch;
    if (!repoId || !bName) {
      currentBranchInfo = null;
      return;
    }
    let cancelled = false;
    branchesFor(repoId)
      .then((list) => {
        if (!cancelled) {
          currentBranchInfo = list.find((b) => b.name === bName) ?? null;
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  });

  // Latest commit info (shallow log fetch of HEAD)
  let headCommit = $state<CommitInfo | null>(null);
  let headLoading = $state(false);
  $effect(() => {
    const sha = status?.head;
    if (!repoId || !sha) {
      headCommit = null;
      return;
    }
    if (headCommit?.sha === sha) return;

    let cancelled = false;
    headLoading = true;
    streamLog(
      repoId,
      {
        regex: false,
        refs: [sha],
        follow: false,
      },
      (page) => {
        if (cancelled) return;
        const match =
          page.commits.find((c) => c.sha === sha) ??
          page.commits.find((c) => c.sha.startsWith(sha)) ??
          page.commits[0] ??
          null;
        if (match) {
          headCommit = match;
        }
      },
      "editor-overview-head",
    )
      .catch(() => {})
      .finally(() => {
        if (!cancelled) headLoading = false;
      });

    return () => {
      cancelled = true;
    };
  });

  // -- GitHub details --------------------------------------------------------
  let now = $state(Date.now());
  const snapshot = $derived(repoId ? forgeStore.snapshotStaleFor(repoId) : null);
  const chips = $derived(repoId ? forgeChips.chipsFor(repoId, now) : null);

  // Keep `now` updated if there is a running workflow run
  const hasRunning = $derived(
    snapshot?.run?.status === "in_progress" ||
      snapshot?.run?.status === "queued" ||
      snapshot?.run?.status === "waiting",
  );
  $effect(() => {
    if (!hasRunning) return;
    now = Date.now();
    const interval = setInterval(() => {
      now = Date.now();
    }, 1_000);
    return () => clearInterval(interval);
  });

  const degradedInfo = $derived.by(() => {
    if (chips?.worst?.state.kind === "degraded") {
      return degradedGuidance(
        chips.worst.state.degraded,
        chips.worst.state.message,
      );
    }
    return null;
  });

  const checksCount = $derived(
    snapshot?.head_checks ? countChecks(snapshot.head_checks) : null,
  );
  const checksSummary = $derived(
    snapshot?.head_checks ? summarize(snapshot.head_checks) : "",
  );

  const runDuration = $derived(
    snapshot?.run ? runDurationLabel(snapshot.run, now) : null,
  );

  // -- Navigation actions ----------------------------------------------------
  function handleFocusStatus(): void {
    if (repoId) {
      emitUiEvent("focus-panel", { panel: "status", repoId });
    }
  }

  function handleFocusForge(): void {
    if (repoId) {
      emitUiEvent("focus-panel", { panel: "forge", repoId });
    }
  }

  function handleFocusChecks(): void {
    if (repoId) {
      emitUiEvent("focus-panel", { panel: "forge", repoId, tab: "checks" });
    }
  }

  function handleSwitchBranch(): void {
    if (repoId) {
      emitUiEvent("branch-switcher-open", { repoId });
    }
  }

  function prStateClass(state: string, isDraft: boolean): string {
    if (isDraft) return "bg-muted text-muted-foreground border-border";
    switch (state.toUpperCase()) {
      case "OPEN":
        return "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 border-emerald-500/30";
      case "MERGED":
        return "bg-purple-500/15 text-purple-600 dark:text-purple-400 border-purple-500/30";
      case "CLOSED":
        return "bg-destructive/15 text-destructive border-destructive/30";
      default:
        return "bg-muted text-muted-foreground border-border";
    }
  }

  function prStateLabel(state: string, isDraft: boolean): string {
    if (isDraft) return "Draft";
    switch (state.toUpperCase()) {
      case "OPEN":
        return "Open";
      case "MERGED":
        return "Merged";
      case "CLOSED":
        return "Closed";
      default:
        return state;
    }
  }

  function runToneClass(conclusion: string, statusText: string): string {
    if (statusText === "in_progress" || statusText === "queued" || statusText === "waiting") {
      return "text-amber-500";
    }
    if (conclusion === "success") return "text-emerald-500";
    if (conclusion === "failure") return "text-destructive";
    return "text-muted-foreground";
  }
</script>

{#if !status}
  <div
    class="welcome flex min-h-0 flex-1 select-none flex-col items-center justify-center gap-ui-4 p-8 text-center text-muted-foreground"
    aria-label="Repository overview loading"
  >
    <span
      class="wicon inline-flex size-11 items-center justify-center rounded-full border border-border bg-card text-primary-ink"
      aria-hidden="true"
    >
      <Icon icon={iconTree} size={22} />
    </span>
    <p class="wtitle m-0 text-sm font-medium text-foreground">Nothing open</p>
    <p
      class="wloading mt-ui-15 inline-flex items-center gap-ui-4 text-ui-micro"
      role="status"
    >
      <Spinner class="size-3" role="presentation" aria-hidden="true" />
      Reading repository…
    </p>
    <p class="wtext m-0 max-w-[22rem] text-xs leading-[1.45]">
      Select a change in the source-control sidebar to see its diff, or pick
      a commit in history to inspect it.
    </p>
    <ul
      class="whints mt-[0.9rem] flex list-none flex-wrap justify-center gap-x-[1.1rem] gap-y-[0.4rem] border-t border-border pt-[0.7rem] text-ui-micro text-muted-foreground"
      aria-label="Keyboard shortcuts"
    >
      <li class="inline-flex items-center gap-ui-15"><Kbd>Ctrl</Kbd>+<Kbd>Shift</Kbd>+<Kbd>P</Kbd><span class="ml-ui-4">Command palette</span></li>
      <li class="inline-flex items-center gap-ui-15"><Kbd>F5</Kbd><span class="ml-ui-4">Refresh repository</span></li>
      <li class="inline-flex items-center gap-ui-15"><Kbd>Ctrl</Kbd>+<Kbd>1</Kbd><span class="ml-ui-4">Focus source control</span></li>
    </ul>
  </div>
{:else}
  <div
    class="welcome flex h-full min-h-0 flex-1 flex-col overflow-y-auto select-none items-center p-6 text-muted-foreground"
    aria-label="Repository overview"
  >
    <div class="flex w-full max-w-2xl flex-col items-center gap-5 my-auto">
      <!-- Identity header -->
      <div class="flex flex-col items-center text-center gap-2">
        <span
          class="wicon inline-flex size-11 items-center justify-center rounded-full border border-border bg-card text-primary-ink shadow-xs"
          aria-hidden="true"
        >
          <Icon icon={iconTree} size={22} />
        </span>
        <div>
          <h2 class="wtitle text-sm font-semibold text-foreground m-0">
            {repoName || "Nothing open"}
          </h2>
          <p class="wtext text-ui-micro text-muted-foreground m-0 mt-0.5 max-w-[24rem]">
            Select a change in source control to see its diff, or choose a quick action.
          </p>
        </div>
      </div>

      <!-- Overview Cards Grid -->
      <div class="wsummary grid grid-cols-1 md:grid-cols-2 gap-3.5 w-full">
        <!-- Git Status Card -->
        <div
          class="flex flex-col gap-3 rounded-lg border border-border bg-card/60 p-3.5 text-left shadow-2xs"
          aria-label="Git repository status"
        >
          <div class="flex items-center justify-between pb-2 border-b border-border/60">
            <div class="flex items-center gap-2 text-ui-xs font-semibold text-foreground">
              <span class="text-primary-ink flex-none inline-flex items-center">
                <Icon icon={iconBranch} size={15} />
              </span>
              <span>Git Status</span>
            </div>
            <Hint label="Switch branch">
              {#snippet child({ props })}
                <button
                  {...props}
                  type="button"
                  class="wchip wbranch inline-flex items-center gap-1.5 rounded-full border border-primary/35 bg-primary/10 px-2.5 py-0.5 font-mono text-ui-micro font-medium text-primary-ink hover:bg-primary/20 transition-colors cursor-pointer"
                  onclick={handleSwitchBranch}
                  aria-label={`Current branch: ${branchLabel}. Click to switch branch`}
                >
                  <span class="truncate max-w-36">{branchLabel}</span>
                </button>
              {/snippet}
            </Hint>
          </div>

          <!-- In-progress operations alert -->
          {#if conflictOp}
            <div
              class="flex items-center gap-2 rounded-md bg-destructive/15 border border-destructive/30 px-2.5 py-1.5 text-ui-micro font-medium text-destructive"
              role="alert"
            >
              <span class="flex-none inline-flex items-center">
                <Icon icon={iconAlert} size={14} />
              </span>
              <span>{conflictSourceLabel(conflictOp)} in progress</span>
            </div>
          {:else if status.bisecting}
            <div
              class="flex items-center gap-2 rounded-md bg-amber-500/15 border border-amber-500/30 px-2.5 py-1.5 text-ui-micro font-medium text-amber-600 dark:text-amber-400"
              role="alert"
            >
              <span class="flex-none inline-flex items-center">
                <Icon icon={iconAlert} size={14} />
              </span>
              <span>Bisect in progress</span>
            </div>
          {/if}

          <!-- Upstream status -->
          <div class="flex flex-wrap items-center gap-1.5 text-ui-micro">
            {#if status.ahead > 0}
              <Hint label={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}>
                {#snippet child({ props })}
                  <span
                    class="wchip wahead inline-flex items-center gap-1 rounded-full bg-accent px-2 py-0.5 tabular-nums text-accent-foreground font-medium"
                    {...props}
                  >
                    ↑ {status.ahead} to push
                  </span>
                {/snippet}
              </Hint>
            {/if}
            {#if status.behind > 0}
              <Hint label={`${status.behind} ${status.behind === 1 ? "commit" : "commits"} to pull`}>
                {#snippet child({ props })}
                  <span
                    class="wchip wbehind inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 tabular-nums text-muted-foreground font-medium"
                    {...props}
                  >
                    ↓ {status.behind} to pull
                  </span>
                {/snippet}
              </Hint>
            {/if}
            {#if currentBranchInfo?.gone}
              <span class="inline-flex items-center gap-1 rounded-full bg-destructive/15 text-destructive border border-destructive/30 px-2 py-0.5 font-medium">
                Upstream gone
              </span>
            {:else if currentBranchInfo?.upstream && status.ahead === 0 && status.behind === 0}
              <span class="inline-flex items-center gap-1 text-muted-foreground">
                <span class="text-emerald-500 flex-none inline-flex items-center">
                  <Icon icon={iconCheck} size={13} />
                </span>
                <span>Up to date with {shortRefName(currentBranchInfo.upstream)}</span>
              </span>
            {:else if !currentBranchInfo?.upstream && status.ahead === 0 && status.behind === 0 && !status.detached}
              <span class="text-muted-foreground">Local branch (no upstream tracking)</span>
            {/if}
          </div>

          <!-- Working Tree Changes -->
          <div class="flex flex-col gap-1.5 rounded-md border border-border/50 bg-background/50 p-2.5">
            <div class="flex items-center justify-between text-ui-micro text-muted-foreground">
              <span class="font-medium text-foreground">Working Tree</span>
              <button
                type="button"
                class="text-primary-ink hover:underline text-ui-micro cursor-pointer font-medium"
                onclick={handleFocusStatus}
              >
                View changes
              </button>
            </div>
            {#if changeCount === 0}
              <div class="flex items-center gap-1.5 text-ui-micro text-muted-foreground">
                <span class="text-emerald-500 flex-none inline-flex items-center">
                  <Icon icon={iconCheck} size={13} />
                </span>
                <span>Clean working tree</span>
              </div>
            {:else}
              <div class="flex flex-wrap items-center gap-1 text-ui-micro">
                <span class="text-foreground font-medium mr-1">
                  {changeCount} {changeCount === 1 ? "file" : "files"}:
                </span>
                {#if counts.staged > 0}
                  <span class="rounded bg-primary/15 text-primary-ink px-1.5 py-0.2 font-mono text-ui-micro font-medium">
                    +{counts.staged} staged
                  </span>
                {/if}
                {#if counts.unstaged > 0}
                  <span class="rounded bg-accent text-accent-foreground px-1.5 py-0.2 font-mono text-ui-micro font-medium">
                    ~{counts.unstaged} modified
                  </span>
                {/if}
                {#if counts.untracked > 0}
                  <span class="rounded bg-muted text-muted-foreground px-1.5 py-0.2 font-mono text-ui-micro">
                    ?{counts.untracked} untracked
                  </span>
                {/if}
                {#if counts.conflicts > 0}
                  <span class="rounded bg-destructive/15 text-destructive px-1.5 py-0.2 font-mono text-ui-micro font-medium">
                    !{counts.conflicts} conflicted
                  </span>
                {/if}
              </div>
            {/if}
          </div>

          <!-- Latest Commit (HEAD) -->
          <div class="flex flex-col gap-1.5 rounded-md border border-border/50 bg-background/50 p-2.5">
            <div class="flex items-center justify-between text-ui-micro text-muted-foreground">
              <span class="font-medium text-foreground">Latest Commit</span>
              {#if headCommit}
                <span class="font-mono text-ui-micro text-muted-foreground">{shortSha(headCommit.sha)}</span>
              {/if}
            </div>
            {#if headLoading && !headCommit}
              <div class="flex items-center gap-2 text-ui-micro text-muted-foreground">
                <Spinner class="size-3 flex-none" />
                <span>Reading HEAD commit…</span>
              </div>
            {:else if headCommit}
              <div class="flex flex-col gap-1">
                <p class="m-0 text-ui-xs font-medium text-foreground truncate" title={headCommit.summary}>
                  {headCommit.summary}
                </p>
                <div class="flex items-center gap-1.5 text-ui-micro text-muted-foreground">
                  <AuthorAvatar name={headCommit.author.name} email={headCommit.author.email} size={15} />
                  <span class="truncate max-w-28">{headCommit.author.name}</span>
                  <span>•</span>
                  <span>{formatRelativeTime(headCommit.author.time)}</span>
                </div>
              </div>
            {:else if status.head}
              <div class="flex items-center gap-1.5 font-mono text-ui-micro text-muted-foreground">
                <span>{shortSha(status.head)}</span>
              </div>
            {:else}
              <span class="text-ui-micro text-muted-foreground">No commits yet (unborn repo)</span>
            {/if}
          </div>
        </div>

        <!-- GitHub Integration Card -->
        <div
          class="flex flex-col gap-3 rounded-lg border border-border bg-card/60 p-3.5 text-left shadow-2xs"
          aria-label="GitHub details"
        >
          <div class="flex items-center justify-between pb-2 border-b border-border/60">
            <div class="flex items-center gap-2 text-ui-xs font-semibold text-foreground">
              <span class="text-primary-ink flex-none inline-flex items-center">
                <Icon icon={iconForge} size={15} />
              </span>
              <span>GitHub</span>
            </div>
            <Button
              variant="ghost"
              size="sm"
              class="h-6 px-2 text-ui-micro text-primary-ink hover:text-primary-ink font-medium"
              onclick={handleFocusForge}
            >
              Open Forge
            </Button>
          </div>

          {#if degradedInfo}
            <!-- Degraded status (no-gh / not-authed / offline / non-github) -->
            <div
              class="flex flex-col gap-2 rounded-md bg-muted/60 border border-border p-3 text-left"
              role="alert"
            >
              <div class="flex items-center gap-1.5 text-ui-xs font-medium text-foreground">
                <span class="text-amber-500 flex-none inline-flex items-center">
                  <Icon icon={iconAlert} size={14} />
                </span>
                <span>{degradedInfo.title}</span>
              </div>
              <p class="m-0 text-ui-micro text-muted-foreground leading-relaxed">
                {degradedInfo.message}
              </p>
              {#if degradedInfo.command}
                <div class="flex items-center justify-between gap-2 rounded bg-background px-2 py-1 font-mono text-ui-micro border border-border/60">
                  <span class="select-all text-foreground truncate">{degradedInfo.command}</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    class="h-5 px-1.5 text-ui-micro"
                    onclick={() => copyText(degradedInfo!.command!, "command")}
                  >
                    Copy
                  </Button>
                </div>
              {/if}
              {#if degradedInfo.docsUrl}
                <Button
                  variant="outline"
                  size="sm"
                  class="h-6 text-ui-micro self-start gap-1 mt-0.5"
                  onclick={() => openExternal(degradedInfo!.docsUrl!)}
                >
                  <Icon icon={iconExternal} size={12} />
                  Documentation
                </Button>
              {/if}
            </div>
          {:else}
            <!-- Pull Request -->
            <div class="flex flex-col gap-1.5 rounded-md border border-border/50 bg-background/50 p-2.5">
              <div class="flex items-center justify-between text-ui-micro text-muted-foreground">
                <span class="font-medium text-foreground">Pull Request</span>
                {#if snapshot?.pr}
                  <span
                    class="inline-flex items-center gap-1 rounded-full border px-2 py-0.2 text-ui-micro font-medium {prStateClass(snapshot.pr.state, snapshot.pr.is_draft)}"
                  >
                    {prStateLabel(snapshot.pr.state, snapshot.pr.is_draft)}
                  </span>
                {/if}
              </div>
              {#if snapshot?.pr}
                <div class="flex items-center justify-between gap-2">
                  <div class="min-w-0 flex-1">
                    <button
                      type="button"
                      class="text-left text-ui-xs font-medium text-foreground hover:text-primary-ink truncate block w-full cursor-pointer"
                      title={`#${snapshot.pr.number} ${snapshot.pr.title}`}
                      onclick={() => openExternal(snapshot.pr!.url)}
                    >
                      #{snapshot.pr.number} {snapshot.pr.title}
                    </button>
                    <div class="flex items-center gap-2 text-ui-micro text-muted-foreground mt-0.5">
                      <span>into {snapshot.pr.base_ref_name}</span>
                      <span>•</span>
                      <span class="text-emerald-500 font-mono">+{snapshot.pr.additions}</span>
                      <span class="text-destructive font-mono">-{snapshot.pr.deletions}</span>
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    class="size-6 flex-none text-muted-foreground hover:text-foreground"
                    title="Open pull request in browser"
                    onclick={() => openExternal(snapshot.pr!.url)}
                  >
                    <Icon icon={iconExternal} size={13} />
                  </Button>
                </div>
              {:else}
                <span class="text-ui-micro text-muted-foreground">No open pull request for this branch</span>
              {/if}
            </div>

            <!-- CI Checks -->
            <div class="flex flex-col gap-1.5 rounded-md border border-border/50 bg-background/50 p-2.5">
              <div class="flex items-center justify-between text-ui-micro text-muted-foreground">
                <span class="font-medium text-foreground">CI Checks</span>
                <button
                  type="button"
                  class="text-primary-ink hover:underline text-ui-micro cursor-pointer font-medium"
                  onclick={handleFocusChecks}
                >
                  Details
                </button>
              </div>
              {#if checksCount && checksCount.total > 0}
                <div class="flex items-center justify-between gap-2">
                  <div class="flex items-center gap-1.5 text-ui-micro font-medium">
                    {#if checksCount.fail > 0}
                      <span class="rounded bg-destructive/15 text-destructive px-1.5 py-0.2 font-mono">
                        {checksCount.fail} failing
                      </span>
                    {/if}
                    {#if checksCount.pending > 0}
                      <span class="rounded bg-amber-500/15 text-amber-600 dark:text-amber-400 px-1.5 py-0.2 font-mono inline-flex items-center gap-1">
                        <Spinner class="size-2.5" />
                        {checksCount.pending} running
                      </span>
                    {/if}
                    {#if checksCount.pass > 0}
                      <span class="rounded bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 px-1.5 py-0.2 font-mono">
                        {checksCount.pass} passed
                      </span>
                    {/if}
                  </div>
                  <span class="text-ui-micro text-muted-foreground truncate max-w-36">{checksSummary}</span>
                </div>
              {:else if snapshot?.head_checks}
                <span class="text-ui-micro text-muted-foreground">No checks reported for HEAD</span>
              {:else}
                <span class="text-ui-micro text-muted-foreground">Checks pending or unavailable</span>
              {/if}
            </div>

            <!-- Recent Actions Workflow Run -->
            <div class="flex flex-col gap-1.5 rounded-md border border-border/50 bg-background/50 p-2.5">
              <div class="flex items-center justify-between text-ui-micro text-muted-foreground">
                <span class="font-medium text-foreground">Latest Workflow Run</span>
                {#if snapshot?.run}
                  <span class="inline-flex items-center gap-1 text-ui-micro font-medium {runToneClass(snapshot.run.conclusion, snapshot.run.status)}">
                    {#if snapshot.run.status === "in_progress" || snapshot.run.status === "queued" || snapshot.run.status === "waiting"}
                      <Spinner class="size-2.5" />
                      <span>{snapshot.run.status.replace("_", " ")}</span>
                    {:else if snapshot.run.conclusion === "success"}
                      <span class="text-emerald-500 inline-flex items-center"><Icon icon={iconCheck} size={12} /></span>
                      <span>success</span>
                    {:else if snapshot.run.conclusion === "failure"}
                      <span class="text-destructive inline-flex items-center"><Icon icon={iconError} size={12} /></span>
                      <span>failure</span>
                    {:else}
                      <span>{snapshot.run.conclusion || snapshot.run.status}</span>
                    {/if}
                  </span>
                {/if}
              </div>
              {#if snapshot?.run}
                <div class="flex items-center justify-between gap-2">
                  <button
                    type="button"
                    class="min-w-0 flex-1 text-left cursor-pointer group"
                    onclick={() => emitUiEvent("workflow-run-open", { repoId, runId: snapshot.run!.database_id })}
                  >
                    <p class="m-0 text-ui-xs font-medium text-foreground group-hover:text-primary-ink truncate">
                      #{snapshot.run.number} {snapshot.run.name}
                    </p>
                    <div class="flex items-center gap-2 text-ui-micro text-muted-foreground mt-0.5">
                      <span>{snapshot.run.event}</span>
                      {#if runDuration}
                        <span>•</span>
                        <span>{runDuration}</span>
                      {/if}
                    </div>
                  </button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    class="size-6 flex-none text-muted-foreground hover:text-foreground"
                    title="Open run in editor"
                    onclick={() => emitUiEvent("workflow-run-open", { repoId, runId: snapshot.run!.database_id })}
                  >
                    <Icon icon={iconPlay} size={13} />
                  </Button>
                </div>
              {:else}
                <span class="text-ui-micro text-muted-foreground">No recent workflow runs</span>
              {/if}
            </div>
          {/if}
        </div>
      </div>

      <!-- Quick Actions -->
      <div class="wactions flex flex-wrap justify-center gap-2 mt-1">
        {#if changeCount > 0 && onOpenFirstChange}
          <Button
            class="h-auto rounded-full px-3.5 py-1 text-ui-base"
            type="button"
            onclick={onOpenFirstChange}
          >
            <span class="mr-1 inline-flex items-center"><Icon icon={iconDiff} size={14} /></span>
            Open first change
          </Button>
        {/if}
        {#if onBrowseHistory}
          <Button
            class="h-auto rounded-full px-3.5 py-1 text-ui-base"
            variant="outline"
            type="button"
            onclick={onBrowseHistory}
          >
            <span class="mr-1 inline-flex items-center"><Icon icon={iconHistory} size={14} /></span>
            Browse history
          </Button>
        {/if}
        <Button
          class="h-auto rounded-full px-3.5 py-1 text-ui-base"
          variant="outline"
          type="button"
          onclick={handleSwitchBranch}
        >
          <span class="mr-1 inline-flex items-center"><Icon icon={iconBranch} size={14} /></span>
          Switch branch
        </Button>
        <Button
          class="h-auto rounded-full px-3.5 py-1 text-ui-base"
          variant="outline"
          type="button"
          onclick={handleFocusForge}
        >
          <span class="mr-1 inline-flex items-center"><Icon icon={iconForge} size={14} /></span>
          Open Forge
        </Button>
      </div>

      <!-- Keyboard shortcuts -->
      <ul
        class="whints flex list-none flex-wrap justify-center gap-x-[1.1rem] gap-y-[0.4rem] border-t border-border/60 pt-3 text-ui-micro text-muted-foreground w-full"
        aria-label="Keyboard shortcuts"
      >
        <li class="inline-flex items-center gap-ui-15"><Kbd>Ctrl</Kbd>+<Kbd>Shift</Kbd>+<Kbd>P</Kbd><span class="ml-ui-4">Command palette</span></li>
        <li class="inline-flex items-center gap-ui-15"><Kbd>F5</Kbd><span class="ml-ui-4">Refresh repository</span></li>
        <li class="inline-flex items-center gap-ui-15"><Kbd>Ctrl</Kbd>+<Kbd>1</Kbd><span class="ml-ui-4">Focus source control</span></li>
      </ul>
    </div>
  </div>
{/if}
