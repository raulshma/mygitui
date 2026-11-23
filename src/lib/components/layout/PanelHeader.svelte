<script lang="ts">
  /**
   * PanelHeader — the ONE header-row pattern for panels (plan §4.3: "one
   * row = title + count badge + filter toggle + overflow; replaces bespoke
   * header+toolbar+filter stacks"). Presentational only: no store reads, no
   * adoption baked in — the workstreams (A/B/D) swap their bespoke
   * header+toolbar+filter stacks for this.
   *
 * Anatomy: optional `leading` actions · `title` (`text-ui-meta`, the 12px
 * secondary ramp; §3.5 readable floor) · optional count badge
 * (`text-ui-micro`, the 11px badges-only ramp) · optional filter input
 * (`filterInput` snippet, rendered when the caller shows it — the input
 * expands inline before the toggle, Workstream A/B/D) · spacer · optional
 * filter icon toggle (a ≥28px hit target, `size-ui-hit`; `filterOn` set =
 * the toggle renders, boolean pressed state + `onFilterToggle`) · optional
 * `trailing` actions (an OverflowMenu for secondary commands, per §3.3).
 *
 * Height is the 36px chrome row (`h-9`); callers can override the row's
 * classes (`class`) for surface-specific backgrounds, and everything the
 * row can't express yet (segmented controls) stays in the caller's hands —
 * this component only standardizes the one-row grammar.
 */
  import type { Snippet } from "svelte";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { cn } from "$lib/utils.js";

  let {
    title,
    count = undefined,
    filterOn = undefined,
    onFilterToggle = undefined,
    filterInput = undefined,
    class: className = "",
    leading = undefined,
    trailing = undefined,
  }: {
    /** The panel's name (sentence case — not micro-uppercase). */
    title: string;
    /** Count badge next to the title; omit when there is nothing to count. */
    count?: number | string;
    /**
     * Filter toggle state; leave undefined to render no toggle at all.
     * The input itself is the caller's `filterInput` snippet (expand inline
     * next to this header, Workstream A/B/D).
     */
    filterOn?: boolean;
    onFilterToggle?: (next: boolean) => void;
    /** Inline filter input, rendered just before the filter toggle. */
    filterInput?: Snippet;
    /** Extra classes for the row (backgrounds, per-surface chrome). */
    class?: string;
    /** Actions before the title (rare: back buttons, panes switches). */
    leading?: Snippet;
    /** Actions at the end — typically an OverflowMenu ("⋯"). */
    trailing?: Snippet;
  } = $props();

  const iconFilter = $derived(appIcon("filter-list")!);
</script>

<div
  class={cn(
    "flex h-9 min-h-9 flex-none items-center gap-ui-3 border-b border-border px-ui-panel",
    className,
  )}
>
  {#if leading}{@render leading()}{/if}
  <span class="panel-title min-w-0 truncate text-ui-meta font-semibold text-foreground">
    {title}
  </span>
  {#if count !== undefined}
    <Badge
      variant="secondary"
      class="flex-none rounded-full px-ui-375 text-ui-micro font-medium tabular-nums"
    >
      {count}
    </Badge>
  {/if}
  <div class="flex-1"></div>
  {#if filterInput}{@render filterInput()}{/if}
  {#if filterOn !== undefined}
    <Hint label={filterOn ? "Hide filter" : "Show filter"}>
      {#snippet child({ props })}
        <button
          {...props}
          type="button"
          aria-pressed={filterOn}
          aria-label={filterOn ? "Hide filter" : "Show filter"}
          class={cn(
            "inline-flex size-ui-hit flex-none cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
            filterOn ? "bg-primary/10 text-primary-ink" : "text-muted-foreground hover:text-foreground",
          )}
          onclick={() => onFilterToggle?.(!filterOn)}
        >
          <Icon icon={iconFilter} size={16} />
        </button>
      {/snippet}
    </Hint>
  {/if}
  {#if trailing}{@render trailing()}{/if}
</div>
