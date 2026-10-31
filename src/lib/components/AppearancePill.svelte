<script lang="ts">
  /**
   * One appearance-dialog option pill: a <label> wrapping the bare
   * RadioGroup.Item circle (it renders only the checked indicator) plus the
   * option content — the whole pill toggles, and `has-data-[state=checked]`
   * styles the selected one.
   */
  import type { Snippet } from "svelte";
  import { Item as RadioGroupItem } from "$lib/components/ui/radio-group/index.js";

  let {
    value,
    class: extraClass = "",
    warm,
    children,
  }: {
    value: string;
    /** Extra label classes (two-line style pills, tighter px-3 pills). */
    class?: string;
    /** Hover preload for code-split choices (style sheets, fonts, icons). */
    warm?: () => void;
    children: Snippet;
  } = $props();

  const PILL =
    "flex cursor-pointer items-center gap-2 rounded-lg border border-transparent p-2 has-data-[state=checked]:border-border has-data-[state=checked]:bg-muted/50";
  const PILL_TEXT = "flex items-center gap-2 text-sm";
</script>

<label class="{PILL} {extraClass}" onpointerenter={warm}>
  <RadioGroupItem {value} />
  <span class={PILL_TEXT}>{@render children()}</span>
</label>
