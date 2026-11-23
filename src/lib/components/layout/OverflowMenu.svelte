<script lang="ts">
  /**
   * OverflowMenu — the ONE "⋯" pattern for secondary commands (plan §4.3:
   * "OverflowMenu (shadcn dropdown-menu wrapper): standard '⋯' for secondary
   * commands; keyboard navigable"). Progressive disclosure (§3.3): secondary
   * commands live here instead of as always-visible buttons.
   *
   * A thin wrapper over the vendored shadcn dropdown-menu: the trigger is a
   * ghost `⋯` button at the 28px hit-target floor (`size-ui-hit`, plan §4.1 —
   * the 16px glyph sits inside a 28×28 control), keyboard navigation and
   * focus management are the bits-ui menu's own, and the items are the
   * caller's snippet (DropdownMenu.Item / .Separator / .CheckboxItem…).
   *
   * The Hint around the trigger runs `eager` — the lazy trigger swap would
   * detach the element bits-ui's menu anchored to and the menu would never
   * show (see the Hint docs).
   */
  import type { Snippet } from "svelte";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { cn } from "$lib/utils.js";

  let {
    label,
    disabled = false,
    align = "end",
    side = "bottom",
    class: className = "",
    contentClass = "",
    content,
    onOpenChange = undefined,
  }: {
    /** Accessible name + tooltip for the "⋯" trigger. Required: icon-only. */
    label: string;
    disabled?: boolean;
    /** Which side of the trigger the menu opens on. */
    side?: "top" | "bottom" | "left" | "right";
    /** Menu alignment against the trigger. */
    align?: "start" | "center" | "end";
    /** Extra classes for the trigger (base: 28px ghost hit-target). */
    class?: string;
    /** Extra classes for the menu content (default auto-width: 11–20rem). */
    contentClass?: string;
    /** The menu body: DropdownMenu.Item / .Separator / .CheckboxItem… */
    content: Snippet;
    /** Open/close notifications (e.g. per-open data refreshes). */
    onOpenChange?: (open: boolean) => void;
  } = $props();

  const iconMore = $derived(appIcon("more-horiz")!);
</script>

<DropdownMenu.Root {onOpenChange}>
  <Hint {label} {side} eager>
    {#snippet child({ props })}
      <DropdownMenu.Trigger
        {...props}
        class={cn(
          "inline-flex size-ui-hit flex-none cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50",
          className,
        )}
        {disabled}
        aria-label={label}
      >
        <Icon icon={iconMore} size={16} />
      </DropdownMenu.Trigger>
    {/snippet}
  </Hint>
  <!-- Auto-width (ContextMenu idiom): the vendored content pins its width to
       the 28px anchor, flooring at min-width — long labels wrapped inside
       160px even with the whole screen free. `w-auto` wins the tailwind-merge
       conflict against that var; `max-w-80` still caps pathological rows. -->
  <DropdownMenu.Content {align} {side} class={cn("w-auto min-w-44 max-w-80", contentClass)}>
    {@render content()}
  </DropdownMenu.Content>
</DropdownMenu.Root>
