<script lang="ts" module>
  /**
   * Fixed local API over m3-svelte's Button. These types (and the props
   * below) are mygitui's stable surface; m3-svelte churn stops here.
   */
  export type ButtonVariant = "elevated" | "filled" | "tonal" | "outlined" | "text";
  export type ButtonSize = "xs" | "s" | "m" | "l" | "xl";
  export type ButtonIconType = "none" | "left" | "full";
</script>

<script lang="ts">
  import { Button as M3Button } from "m3-svelte";
  import type { Snippet } from "svelte";

  interface Props {
    variant?: ButtonVariant;
    size?: ButtonSize;
    iconType?: ButtonIconType;
    square?: boolean;
    /** Default "button" (never accidentally submits forms). */
    type?: "button" | "submit" | "reset";
    disabled?: boolean;
    onclick?: (event: MouseEvent) => void;
    "aria-label"?: string;
    children?: Snippet;
  }

  let {
    variant = "filled",
    size = "s",
    iconType = "none",
    square = false,
    type = "button",
    disabled = false,
    onclick = undefined,
    "aria-label": ariaLabel = undefined,
    children,
  }: Props = $props();
</script>

<!--
  No `class` forwarding: m3-svelte spreads props onto its element after its
  own class attribute, so a forwarded class would clobber component styles.
  The children guard keeps the always-rendered {@render} in m3-svelte safe.
-->
<M3Button
  {variant}
  {size}
  {iconType}
  {square}
  {type}
  {disabled}
  {onclick}
  aria-label={ariaLabel}
>
  {#if children}{@render children()}{/if}
</M3Button>
