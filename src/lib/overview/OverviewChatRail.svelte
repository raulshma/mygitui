<script lang="ts">
  /**
   * The Overview's chat rail: the relocated AgentChat scoped to the
   * expanded/selected repo. With no repo selected it shows a "Select a repo
   * to chat about" empty state. Auto-hidden below 1280px inner width (the
   * same breakpoint the Agent view used); collapsible via the strip's rail
   * toggle.
   */
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import AgentChat from "./AgentChat.svelte";
  import type { OverviewRowData } from "./overviewModel";

  let {
    repo,
    onCollapse,
  }: {
    /** The chat target (the expanded/selected row); null = empty state. */
    repo: OverviewRowData | null;
    onCollapse: () => void;
  } = $props();

  const iconFolder = $derived(appIcon("folder-code")!);
  const iconClose = $derived(appIcon("close")!);
</script>

<aside
  class="overview-chat-rail flex w-[21rem] flex-none flex-col border-l border-border bg-card"
  aria-label="Overview chat"
>
  <div class="flex h-7 flex-none items-center gap-2 border-b border-border px-2 text-ui-meta text-muted-foreground">
    <span class="min-w-0 flex-1 truncate font-medium text-foreground">
      {repo ? repo.name : "Chat"}
    </span>
    <button
      class="inline-flex size-5 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      type="button"
      aria-label="Hide the chat rail"
      onclick={onCollapse}
    >
      <Icon icon={iconClose} size={12} />
    </button>
  </div>
  {#if repo === null || repo.repoId === null}
    <EmptyState
      icon={iconFolder}
      title="Select a repo to chat about"
      hint="Expand a repository row to ask the assistant about its staged diff and recent commits."
    />
  {:else}
    <AgentChat repoId={repo.repoId} root={repo.root} repoName={repo.name} />
  {/if}
</aside>
