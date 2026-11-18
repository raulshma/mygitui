<script lang="ts">
  /**
   * Agent mode — minimal working-copy changes list: Staged / Changed /
   * Untracked groups from `changesModel.ts`, dense StatusPanel-style rows,
   * hover stage/unstage/discard (no confirm dialogs; the backend
   * checkpoints discards). Pure selection/opening is delegated up via
   * `onOpenPath`; refreshes flow through `refreshStatus` + `onMutated`.
   *
   * The list is the StatusPanel's tree in miniature: large sections
   * virtualize (visible window + spacers), rows share one roving
   * tabindex (Up/Down/Home/End move, Enter/Space opens), and failed
   * mutations toast instead of dying in the console.
   */
  import type { ChangeKind, RepoStatus, StatusEntry } from "$lib/ipc/types";
  import { discard, stage } from "$lib/ipc/client";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import { Button } from "$lib/components/ui/button/index.js";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import { visibleRange } from "$lib/components/graph/layout";
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
    ).filter((section) => section.entries.length > 0);
  });

  // -- virtualization (the StatusPanel section-window model) ------------------

  /** Dense-list geometry: `h-6` rows and section headers are both 24px. */
  const ROW_HEIGHT = 24;
  const SECTION_HEIGHT = 24;
  const ROW_OVERSCAN = 12;
  /** Below this a section renders whole (window math costs more than rows). */
  const VIRTUALIZE_THRESHOLD = 150;

  let treeElement: HTMLDivElement | undefined = $state();
  let treeScrollTop = $state(0);
  let treeViewportHeight = $state(640);

  /** Tracks the tree scroller's viewport (rAF-free scroll/resize updates). */
  function trackTreeViewport(node: HTMLDivElement) {
    treeElement = node;
    const update = () => {
      treeScrollTop = node.scrollTop;
      treeViewportHeight = Math.max(1, node.clientHeight);
    };
    const observer = new ResizeObserver(update);
    observer.observe(node);
    node.addEventListener("scroll", update, { passive: true });
    update();
    return {
      destroy() {
        observer.disconnect();
        node.removeEventListener("scroll", update);
        if (treeElement === node) treeElement = undefined;
      },
    };
  }

  /** Per-section visible window: full list below the threshold, else the
   *  rows intersecting the viewport (± overscan) between top/bottom spacers. */
  const sectionWindows = $derived.by(() => {
    let sectionTop = 0;
    return sections.map((section) => {
      const rowsTop = sectionTop + SECTION_HEIGHT;
      let first = 0;
      let last = section.entries.length;

      if (section.entries.length > VIRTUALIZE_THRESHOLD) {
        const rowsBottom = rowsTop + section.entries.length * ROW_HEIGHT;
        const viewportBottom = treeScrollTop + treeViewportHeight;
        if (rowsBottom <= treeScrollTop + SECTION_HEIGHT) {
          first = section.entries.length;
          last = section.entries.length;
        } else if (rowsTop >= viewportBottom) {
          first = 0;
          last = 0;
        } else {
          const range = visibleRange(
            treeScrollTop - rowsTop,
            treeViewportHeight,
            section.entries.length,
            ROW_HEIGHT,
            ROW_OVERSCAN,
          );
          first = Math.min(section.entries.length, range.first);
          last = Math.max(first, Math.min(section.entries.length, range.last));
        }
      }

      sectionTop = rowsTop + section.entries.length * ROW_HEIGHT;

      return {
        section,
        first,
        last,
        rows:
          first === 0 && last === section.entries.length
            ? section.entries
            : section.entries.slice(first, last),
      };
    });
  });

  // -- roving tabindex keyboard model (StatusPanel's tree, single-select) -----

  /** Keyboard order: every entry across sections (not just mounted ones). */
  const flatRows = $derived(sections.flatMap((section) => section.entries));

  /** Roving focus: path of the focused row (null → first row). */
  let focusKey: string | null = $state(null);
  /** Row elements by path (roving-tabindex focus targets). */
  const rowEls = $state<Record<string, HTMLElement | undefined>>({});

  /** The single tab-reachable row (falls back to the first row when the
   *  focused one is filtered out or collapsed away). */
  const tabIndexKey = $derived(
    focusKey !== null && flatRows.some((row) => row.path === focusKey)
      ? focusKey
      : (flatRows[0]?.path ?? null),
  );

  /**
   * Moves roving focus to `key`, scrolling unmounted (virtualized-out) rows
   * into the viewport first by computed offset, then focusing after the
   * window re-renders (the StatusPanel focusRow fallback).
   */
  function focusRow(key: string): void {
    focusKey = key;
    const mounted = rowEls[key];
    if (mounted?.isConnected) {
      mounted.focus({ preventScroll: true });
      const element = treeElement;
      if (element) {
        const elRect = mounted.getBoundingClientRect();
        const treeRect = element.getBoundingClientRect();
        if (elRect.top < treeRect.top + SECTION_HEIGHT) {
          element.scrollTop -= treeRect.top + SECTION_HEIGHT - elRect.top;
        } else if (elRect.bottom > treeRect.bottom) {
          element.scrollTop += elRect.bottom - treeRect.bottom;
        }
      }
      return;
    }

    const element = treeElement;
    if (!element) return;

    let offset = 0;
    let rowTop: number | null = null;
    for (const section of sections) {
      offset += SECTION_HEIGHT;
      const index = section.entries.findIndex((row) => row.path === key);
      if (index !== -1) {
        rowTop = offset + index * ROW_HEIGHT;
        break;
      }
      offset += section.entries.length * ROW_HEIGHT;
    }
    if (rowTop === null) return;

    const stickyTop = element.scrollTop + SECTION_HEIGHT;
    const viewportBottom = element.scrollTop + element.clientHeight;
    if (rowTop < stickyTop) {
      element.scrollTop = Math.max(0, rowTop - SECTION_HEIGHT);
    } else if (rowTop + ROW_HEIGHT > viewportBottom) {
      element.scrollTop = rowTop + ROW_HEIGHT - element.clientHeight;
    }
    treeScrollTop = element.scrollTop;
    treeViewportHeight = Math.max(1, element.clientHeight);
    requestAnimationFrame(() => rowEls[key]?.focus({ preventScroll: true }));
  }

  /**
   * VS Code list keyboard model (single-select): Up/Down/PageUp/PageDown/
   * Home/End move roving focus (clamped, no wraparound); Enter/Space run
   * the focused row's primary action (open the diff). Movement does not
   * change the selection — Enter does.
   */
  function onTreeKeydown(event: KeyboardEvent): void {
    const rows = flatRows;
    if (rows.length === 0) return;
    const current =
      focusKey !== null ? rows.findIndex((r) => r.path === focusKey) : -1;
    const base = current === -1 ? 0 : current;

    const pageRows = Math.max(
      1,
      Math.floor(treeViewportHeight / ROW_HEIGHT) - 1,
    );
    let next: number | null = null;
    if (event.key === "ArrowDown") next = Math.min(base + 1, rows.length - 1);
    else if (event.key === "ArrowUp") next = Math.max(base - 1, 0);
    else if (event.key === "PageDown") {
      next = Math.min(base + pageRows, rows.length - 1);
    } else if (event.key === "PageUp") {
      next = Math.max(base - pageRows, 0);
    } else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = rows.length - 1;

    if (next !== null) {
      event.preventDefault();
      focusRow(rows[next]!.path);
      return;
    }

    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const row = rows[base];
    if (!row) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openEntry(row);
    }
  }

  let mutating = $state(false);

  /**
   * One stage/unstage/discard mutation: refresh + notify on success; a
   * failure toasts (the StatusPanel's "X failed: reason" shape) — the
   * console alone would leave the failed action invisible.
   */
  async function runMutation(label: string, run: () => Promise<unknown>): Promise<void> {
    if (mutating) return;
    mutating = true;
    try {
      await run();
      void refreshStatus(repoId);
      onMutated?.();
    } catch (err) {
      toast(`${label} failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      mutating = false;
    }
  }

  function stageEntry(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    void runMutation("Stage", () =>
      stage(repoId, { targets: [{ file: entry.path }], unstage: false }),
    );
  }

  function unstageEntry(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    void runMutation("Unstage", () =>
      stage(repoId, { targets: [{ file: entry.path }], unstage: true }),
    );
  }

  /** Discard mirrors StatusPanel: rename targets include the old path. */
  function discardEntry(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    void runMutation("Discard", async () => {
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

  /** Pointer clicks move roving focus too, so keyboard nav continues from
   *  the clicked row (StatusPanel's onRowClick behavior). */
  function rowClick(entry: StatusEntry): void {
    focusKey = entry.path;
    openEntry(entry);
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
    tabindex={entry.path === tabIndexKey ? 0 : -1}
    bind:this={rowEls[entry.path]}
    onclick={() => rowClick(entry)}
    onkeydown={onTreeKeydown}
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
      bind:this={treeElement}
      use:trackTreeViewport
    >
      {#each sectionWindows as window (window.section.id)}
        <div class="section" role="group" aria-label={window.section.label}>
          <div class="flex h-6 w-full flex-none items-center gap-ui-3 box-border border-b border-border bg-card px-2 select-none">
            <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-ui-micro leading-[1.2] font-bold tracking-[0.045em] uppercase text-muted-foreground">{window.section.label}</span>
            <span class="ml-auto min-w-5 shrink-0 rounded-full bg-muted px-1.5 text-center text-ui-micro font-semibold tabular-nums text-muted-foreground">{window.section.entries.length}</span>
          </div>
          {#if window.first > 0}
            <div class="row-spacer" aria-hidden="true" style:height={`${window.first * ROW_HEIGHT}px`}></div>
          {/if}
          {#each window.rows as entry (entry.path)}
            {@render row(entry, window.section.id)}
          {/each}
          {#if window.last < window.section.entries.length}
            <div class="row-spacer" aria-hidden="true" style:height={`${(window.section.entries.length - window.last) * ROW_HEIGHT}px`}></div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
