<script lang="ts">
  /**
   * Agent view — the super-minimal alternate workspace (opt-in via the
   * header toggle, persisted by `agentMode`). Three resizable columns
   * (`SplitPane`, ratios persisted per-root by `agentSession`): changes +
   * git info + commit box on the left, the selected file's diff in the
   * center, open PRs + the advisory AI chat on the right (hidden below the
   * xl viewport, resizable when shown). Selection state lives here and is
   * persisted per root, so returning to a repo restores it; the child
   * components are presentation + their own IPC reads.
   */
  import type { RepoStatus } from "$lib/ipc/types";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { clampRatio } from "$lib/layout/layoutModel";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import { diffHunkActions } from "$lib/components/panels/statusModel";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { agentMode } from "./agentMode.svelte";
  import { getAgentSession, patchAgentSession } from "./agentSession.svelte";
  import AgentChanges from "./AgentChanges.svelte";
  import AgentDiff from "./AgentDiff.svelte";
  import AgentGitInfo from "./AgentGitInfo.svelte";
  import AgentPRs from "./AgentPRs.svelte";
  import AgentChat from "./AgentChat.svelte";

  let {
    repoId,
    name,
    root,
    status,
  }: {
    repoId: string;
    name: string;
    root: string;
    status: RepoStatus | null;
  } = $props();

  type DiffSideArg = "worktree" | "index";

  let selectedPath = $state<string | null>(null);
  let selectedSide = $state<DiffSideArg>("worktree");

  // Split ratios mirror the persisted session (write-through: the store is
  // not reactive, the local state drives the panes).
  let leftRatio = $state(0.24);
  let rightRatio = $state(0.7);

  // The right column was `hidden xl:flex`; 1280px is Tailwind's xl screen.
  let innerWidth = $state(1280);
  const showRight = $derived(innerWidth >= 1280);

  const iconAgent = $derived(appIcon("agent") ?? appIcon("bolt")!);
  const iconClose = $derived(appIcon("close")!);

  /** First staged entry diff reads index↔worktree; everything else worktree↔index. */
  function defaultSelection(next: RepoStatus): { path: string; side: DiffSideArg } | null {
    for (const entry of next.entries) {
      if (entry.index !== "unmodified" && entry.worktree === "unmodified") {
        return { path: entry.path, side: "index" };
      }
    }
    const changed = next.entries.find((e) => e.worktree !== "unmodified");
    return changed ? { path: changed.path, side: "worktree" } : null;
  }

  function persistSelection(): void {
    if (root === "") return;
    patchAgentSession(root, {
      selectedPath,
      selectedSide: selectedSide,
    });
  }

  // Repo re-entry restores the per-root session (selection + split ratios)
  // instead of resetting — the workspaceSession pattern.
  $effect(() => {
    void repoId;
    const session = getAgentSession(root);
    selectedPath = session.selectedPath;
    selectedSide = session.selectedSide;
    leftRatio = session.leftRatio;
    rightRatio = session.rightRatio;
  });
  $effect(() => {
    if (status === null) return;
    const stillThere =
      selectedPath !== null &&
      status.entries.some((e) => e.path === selectedPath);
    if (stillThere) return;
    const picked = defaultSelection(status);
    selectedPath = picked?.path ?? null;
    selectedSide = picked?.side ?? "worktree";
    persistSelection();
  });

  function onOpenPath(path: string, side: DiffSideArg): void {
    selectedPath = path;
    selectedSide = side;
    persistSelection();
  }

  function setLeftRatio(ratio: number): void {
    leftRatio = clampRatio(ratio);
    if (root !== "") patchAgentSession(root, { leftRatio });
  }

  function setRightRatio(ratio: number): void {
    rightRatio = clampRatio(ratio);
    if (root !== "") patchAgentSession(root, { rightRatio });
  }

  function onMutated(): void {
    void refreshStatus(repoId);
  }

  /** Hunk actions follow the selected entry like the workspace's diff
   *  (diffHunkActions): conflicted paths get none, the staged side
   *  unstages, discard is worktree-only. */
  const hunkActions = $derived.by(() => {
    const entry =
      selectedPath !== null
        ? (status?.entries.find((e) => e.path === selectedPath) ?? null)
        : null;
    if (!entry) return { staging: undefined, discard: false };
    return diffHunkActions(entry, selectedSide === "index" ? "staged" : "unstaged");
  });
</script>

<svelte:window bind:innerWidth />

<div class="agent-view flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background text-foreground">
  <header
    class="agent-top flex h-9 flex-none items-center gap-2 border-b border-border px-2"
  >
    <span
      class="grid size-5 flex-none place-items-center rounded bg-primary/10 text-primary-ink"
      aria-hidden="true"
    >
      <Icon icon={iconAgent} size={13} />
    </span>
    <span class="flex-none text-xs font-semibold tracking-[-0.01em]">Agent</span>
    <span
      class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-xs text-muted-foreground"
      title={root}>{name}</span
    >
    <Hint label="Exit Agent mode (back to the full workspace)">
      {#snippet child({ props })}
        <Button
          {...props}
          variant="ghost"
          size="sm"
          type="button"
          onclick={() => agentMode.set(false)}
        >
          <Icon icon={iconClose} size={14} />
          <span class="hidden sm:inline">Exit</span>
        </Button>
      {/snippet}
    </Hint>
  </header>

  <div class="agent-body flex min-h-0 min-w-0 flex-1">
    <SplitPane
      axis="x"
      ratio={leftRatio}
      onRatio={setLeftRatio}
      label="Resize changes and diff"
    >
      {#snippet a()}
        <aside class="agent-left flex min-h-0 min-w-0 flex-col border-r border-border">
          <div class="min-h-0 min-w-0 flex-1">
            <AgentChanges
              {repoId}
              {status}
              selectedPath={selectedPath}
              onOpenPath={onOpenPath}
              onMutated={onMutated}
            />
          </div>
          <div class="flex-none">
            <CommitBar {repoId} {status} onCommitted={onMutated} />
          </div>
          <div class="max-h-[45%] flex-none overflow-y-auto border-t border-border">
            <AgentGitInfo {repoId} {status} />
          </div>
        </aside>
      {/snippet}
      {#snippet b()}
        {#if showRight}
          <SplitPane
            axis="x"
            ratio={rightRatio}
            onRatio={setRightRatio}
            label="Resize diff and PRs/chat"
          >
            {#snippet a()}
              <section class="agent-center flex min-h-0 min-w-0 flex-1 flex-col">
                <AgentDiff
                  {repoId}
                  path={selectedPath}
                  side={selectedSide}
                  hunkStaging={hunkActions.staging}
                  hunkDiscard={hunkActions.discard}
                  onMutated={onMutated}
                />
              </section>
            {/snippet}
            {#snippet b()}
              <aside class="agent-right flex min-h-0 min-w-0 flex-col border-l border-border">
                <div class="max-h-[42%] min-h-[8rem] flex-none overflow-y-auto border-b border-border">
                  <AgentPRs {repoId} />
                </div>
                <div class="min-h-0 flex-1">
                  <AgentChat {repoId} {root} repoName={name} />
                </div>
              </aside>
            {/snippet}
          </SplitPane>
        {:else}
          <section class="agent-center flex min-h-0 min-w-0 flex-1 flex-col">
            <AgentDiff
              {repoId}
              path={selectedPath}
              side={selectedSide}
              hunkStaging={hunkActions.staging}
              hunkDiscard={hunkActions.discard}
              onMutated={onMutated}
            />
          </section>
        {/if}
      {/snippet}
    </SplitPane>
  </div>
</div>
