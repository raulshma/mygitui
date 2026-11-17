<script lang="ts">
  /**
   * Agent mode — minimal working-copy changes list: Staged / Changed /
   * Untracked groups from `changesModel.ts`, dense StatusPanel-style rows,
   * hover stage/unstage/discard (no confirm dialogs; the backend
   * checkpoints discards). Pure selection/opening is delegated up via
   * `onOpenPath`; refreshes flow through `refreshStatus` + `onMutated`.
   */
  import type { ChangeKind, RepoStatus, StatusEntry } from "$lib/ipc/types";
  import { discard, stage } from "$lib/ipc/client";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import {
    groupStatusEntries,
    isDiscardable,
    isStageable,
    kindLetter,
    openSide,
    splitPath,
  } from "./changesModel";

  let {
    repoId,
    status,
    selectedPath = null,
    onOpenPath,
    onMutated,
  }: {
    repoId: string;
    status: RepoStatus | null;
    selectedPath?: string | null;
    onOpenPath?: (path: string, side: "worktree" | "index") => void;
    onMutated?: () => void;
  } = $props();

  const MAX_ROWS_PER_GROUP = 200;

  const iconStage = $derived(appIcon("add")!);
  const iconUnstage = $derived(appIcon("remove")!);
  const iconDiscard = $derived(appIcon("undo")!);
  const iconCheck = $derived(appIcon("check")!);

  /** Kind letter color — the app status tokens (StatusPanel's mapping). */
  const KIND_COLOR: Record<string, string> = {
    added: "text-[color:var(--app-added)]",
    deleted: "text-[color:var(--app-deleted)]",
    modified: "text-[color:var(--app-modified)]",
    renamed: "text-[color:var(--app-renamed)]",
    copied: "text-[color:var(--app-renamed)]",
    untracked: "text-[color:var(--app-untracked)]",
    conflicted: "text-[color:var(--app-conflicted)]",
  };

  function kindColorClass(kind: ChangeKind): string {
    return KIND_COLOR[kind] ?? "text-[color:var(--app-modified)]";
  }

  type SectionId = "staged" | "changed" | "untracked";

  interface SectionView {
    id: SectionId;
    label: string;
    entries: StatusEntry[];
    visible: StatusEntry[];
    overflow: number;
  }

  const sections = $derived.by<SectionView[]>(() => {
    if (!status) return [];
    const groups = groupStatusEntries(status.entries);
    return (
      [
        { id: "staged", label: "Staged", entries: groups.staged },
        { id: "changed", label: "Changed", entries: groups.changed },
        { id: "untracked", label: "Untracked", entries: groups.untracked },
      ] as SectionView[]
    )
      .filter((section) => section.entries.length > 0)
      .map((section) => ({
        ...section,
        visible: section.entries.slice(0, MAX_ROWS_PER_GROUP),
        overflow: Math.max(0, section.entries.length - MAX_ROWS_PER_GROUP),
      }));
  });

  let mutating = $state(false);

  /**
   * One stage/unstage/discard mutation: refresh + notify on success,
   * console.error only on failure (minimal UI; backend checkpoints).
   */
  async function runMutation(run: () => Promise<unknown>): Promise<void> {
    if (mutating) return;
    mutating = true;
    try {
      await run();
      void refreshStatus(repoId);
      onMutated?.();
    } catch (err) {
      console.error("[AgentChanges] mutation failed:", err);
    } finally {
      mutating = false;
    }
  }

  function stageEntry(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    void runMutation(() =>
      stage(repoId, { targets: [{ file: entry.path }], unstage: false }),
    );
  }

  function unstageEntry(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    void runMutation(() =>
      stage(repoId, { targets: [{ file: entry.path }], unstage: true }),
    );
  }

  /** Discard mirrors StatusPanel: rename targets include the old path. */
  function discardEntry(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    void runMutation(async () => {
      const targets = [{ file: entry.path }];
      const old = entry.old_path;
      if (old && old !== entry.path) targets.push({ file: old });
      await discard(repoId, targets, {
        checkpoint: appSettings.checkpointsEnabled,
      });
    });
  }

  function openEntry(entry: StatusEntry): void {
    onOpenPath?.(entry.path, openSide(entry));
  }

  function rowKeydown(event: KeyboardEvent, entry: StatusEntry): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openEntry(entry);
    }
  }

  function rowTitle(entry: StatusEntry, kind: ChangeKind): string {
    const lines = [entry.path, kindLetter(kind)];
    if (entry.old_path) lines.push(`from ${entry.old_path}`);
    return lines.join("\n");
  }

  const kindOf = (entry: StatusEntry, section: SectionId): ChangeKind =>
    section === "staged" ? entry.index : entry.worktree;
</script>

{#snippet row(entry: StatusEntry, section: SectionId)}
  {@const kind = kindOf(entry, section)}
  {@const parts = splitPath(entry.path)}
  <div
    class="group/row relative flex h-6 min-h-6 min-w-0 items-center gap-ui-4 box-border cursor-pointer py-0 pr-2 pl-2 hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2 [&.selected]:bg-[color:var(--app-active)] [&.selected]:shadow-[inset_2px_0_0_var(--primary)]"
    class:selected={selectedPath === entry.path}
    role="treeitem"
    aria-selected={selectedPath === entry.path}
    aria-label={rowTitle(entry, kind)}
    tabindex={-1}
    onclick={() => openEntry(entry)}
    onkeydown={(event) => rowKeydown(event, entry)}
  >
    <span
      class="kind relative z-1 inline-flex w-4 shrink-0 cursor-help items-center justify-center font-mono text-ui-micro leading-none font-bold {kindColorClass(kind)}"
      aria-hidden="true"
    >
      {kindLetter(kind)}
    </span>
    <span class="path flex min-w-0 flex-1 items-baseline gap-ui-35 overflow-hidden text-xs leading-[1.25]">
      <span class="min-w-8 overflow-hidden text-ellipsis whitespace-nowrap text-foreground font-normal">{parts.base}</span>
      {#if parts.dir}
        <span class="dir ml-auto grow min-w-0 max-w-[48%] overflow-hidden text-ellipsis whitespace-nowrap text-right text-ui-micro text-muted-foreground opacity-70">{parts.dir}</span>
      {/if}
    </span>
    <span class="row-actions absolute top-1/2 right-5 flex -translate-y-1/2 items-center gap-px opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-visible/row:opacity-100 group-has-[:focus-visible]/row:opacity-100">
      <span
        aria-hidden="true"
        class="pointer-events-none absolute -left-4 -top-ui-1 -bottom-ui-1 w-4 bg-[linear-gradient(to_right,transparent,var(--background))] group-hover/row:bg-[linear-gradient(to_right,transparent,color-mix(in_srgb,var(--foreground)_5%,var(--background)))]"
      ></span>
      {#if section === "staged"}
        <Button
          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label={`Unstage ${entry.path}`}
          title="Unstage this file"
          tabindex={-1}
          disabled={mutating}
          onclick={(event) => unstageEntry(entry, event)}
        >
          <Icon icon={iconUnstage} size={16} />
        </Button>
      {:else}
        {#if isStageable(entry)}
          <Button
            class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label={`Stage ${entry.path}`}
            title="Stage this file"
            tabindex={-1}
            disabled={mutating}
            onclick={(event) => stageEntry(entry, event)}
          >
            <Icon icon={iconStage} size={16} />
          </Button>
        {/if}
        {#if isDiscardable(entry)}
          <Button
            class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label={`Discard changes to ${entry.path}`}
            title="Discard changes to this file"
            tabindex={-1}
            disabled={mutating}
            onclick={(event) => discardEntry(entry, event)}
          >
            <Icon icon={iconDiscard} size={16} />
          </Button>
        {/if}
      {/if}
    </span>
  </div>
{/snippet}

<div
  class="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background text-foreground text-ui-base select-none"
>
  {#if !status}
    <EmptyState title="Loading changes…" />
  {:else if sections.length === 0}
    <EmptyState icon={iconCheck} title="Working copy clean" hint="No staged, changed, or untracked files." />
  {:else}
    <div
      class="min-h-0 flex-1 overflow-y-auto outline-none [scrollbar-gutter:stable]"
      role="tree"
      aria-label="Changed files"
      tabindex="-1"
    >
      {#each sections as section (section.id)}
        <div class="section" role="group" aria-label={section.label}>
          <div class="flex h-6 w-full flex-none items-center gap-ui-3 box-border border-b border-border bg-card px-2 select-none">
            <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-ui-micro leading-[1.2] font-bold tracking-[0.045em] uppercase text-muted-foreground">{section.label}</span>
            <span class="ml-auto min-w-5 shrink-0 rounded-full bg-muted px-1.5 text-center text-ui-micro font-semibold tabular-nums text-muted-foreground">{section.entries.length}</span>
          </div>
          {#each section.visible as entry (entry.path)}
            {@render row(entry, section.id)}
          {/each}
          {#if section.overflow > 0}
            <div class="flex h-6 items-center px-2 text-ui-micro text-muted-foreground">
              +{section.overflow} more
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
