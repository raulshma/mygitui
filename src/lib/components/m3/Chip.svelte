<script lang="ts" module>
  /**
   * Fixed local API over m3-svelte's Chip.
   * variant semantics (from the M3 spec):
   *   input     — information item (e.g. a person in a to field)
   *   assist    — smart action, starts with a verb
   *   general   — filter/suggestion, a category or query
   */
  export type ChipVariant = "input" | "assist" | "general";
</script>

<script lang="ts">
  import { Chip as M3Chip } from "m3-svelte";
  import type { Snippet } from "svelte";
  import type { IconifyIcon } from "@iconify/types";

  interface Props {
    variant: ChipVariant;
    icon?: IconifyIcon;
    trailingIcon?: IconifyIcon;
    elevated?: boolean;
    selected?: boolean;
    disabled?: boolean;
    onclick?: (event: MouseEvent) => void;
    "aria-label"?: string;
    children?: Snippet;
  }

  let {
    variant,
    icon = undefined,
    trailingIcon = undefined,
    elevated = false,
    selected = false,
    disabled = undefined,
    onclick = undefined,
    "aria-label": ariaLabel = undefined,
    children,
  }: Props = $props();

  // m3-svelte's action-props union requires a non-optional onclick; our API
  // keeps it optional, so hand off a no-op when the consumer has none. (Chip
  // hardcodes type="button" on its element, so the handler has no semantic
  // side effects there.)
  const noop = () => {};
</script>

<!-- Same class/children caveats as Button.svelte. -->
<M3Chip
  {variant}
  {icon}
  {trailingIcon}
  {elevated}
  {selected}
  {disabled}
  onclick={onclick ?? noop}
  aria-label={ariaLabel}
>
  {#if children}{@render children()}{/if}
</M3Chip>
