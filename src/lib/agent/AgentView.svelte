<script lang="ts">
  /**
   * Agent view — the super-minimal alternate workspace (opt-in via the
   * header toggle, persisted by `agentMode`). One fixed three-column
   * layout, no tabs/splitters/layout tree: changes + git info on the left,
   * the selected file's diff in the center, open PRs + the advisory AI
   * chat on the right (hidden on narrow windows). Selection state lives
   * here; the child components are presentation + their own IPC reads.
   */
  import type { RepoStatus } from "$lib/ipc/types";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { agentMode } from "./agentMode.svelte";
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

  // Follow the working copy: reset on repo switch, and re-pick when the
  // selected path disappears (staged away, discarded, renamed).
  $effect(() => {
    void repoId;
    selectedPath = null;
    selectedSide = "worktree";
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
  });

  function onOpenPath(path: string, side: DiffSideArg): void {
    selectedPath = path;
    selectedSide = side;
  }

  function onMutated(): void {
    void refreshStatus(repoId);
  }
</script>

<div class="agent-view flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background text-foreground">
  <header
    class="agent-top flex h-9 flex-none items-center gap-2 border-b border-border px-2"
  >
    <span
      class="grid size-5 flex-none place-items-center rounded bg-primary/10 text-primary"
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
    <aside
      class="agent-left flex w-[19rem] min-w-[15rem] flex-none flex-col border-r border-border"
    >
      <div class="min-h-0 flex-1 overflow-y-auto">
        <AgentChanges
          {repoId}
          {status}
          selectedPath={selectedPath}
          onOpenPath={onOpenPath}
          onMutated={onMutated}
        />
      </div>
      <div class="max-h-[45%] flex-none overflow-y-auto border-t border-border">
        <AgentGitInfo {repoId} {status} />
      </div>
    </aside>

    <section class="agent-center flex min-w-0 flex-1 flex-col">
      <AgentDiff {repoId} path={selectedPath} side={selectedSide} />
    </section>

    <aside
      class="agent-right hidden w-[22rem] flex-none flex-col border-l border-border xl:flex"
    >
      <div class="max-h-[42%] min-h-[8rem] flex-none overflow-y-auto border-b border-border">
        <AgentPRs {repoId} />
      </div>
      <div class="min-h-0 flex-1">
        <AgentChat {repoId} {root} repoName={name} />
      </div>
    </aside>
  </div>
</div>
