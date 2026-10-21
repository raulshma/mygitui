<script lang="ts" module>
  /**
   * Compact icon-only action button.
   *
   * Dense toolbars (side-panel headers, row actions, workspace chrome) use
   * this instead of text buttons so a narrow column keeps its width for
   * content. `label` is the accessible name and doubles as the tooltip;
   * `pressed` renders a toggle (`aria-pressed`). Visual variants stay
   * intentionally small: ghost (default), outlined, filled.
   */
</script>

<script lang="ts">
  import type { IconifyIcon } from "@iconify/types";
  import Icon from "$lib/components/icons/Icon.svelte";

  interface Props {
    icon: IconifyIcon;
    /** Accessible name; also the tooltip unless `title` overrides it. */
    label: string;
    title?: string;
    /** Icon glyph size in pixels. */
    size?: number;
    variant?: "ghost" | "outlined" | "filled";
    tone?: "default" | "primary" | "danger" | "success";
    disabled?: boolean;
    /** Renders `aria-pressed` for toggle buttons. */
    pressed?: boolean;
    /** Tighter 1.35rem square for dense rows. */
    dense?: boolean;
    /** Native tab order; `-1` for hover-revealed row actions. */
    tabindex?: number;
    type?: "button" | "submit";
    onclick?: (event: MouseEvent) => void;
    "aria-expanded"?: boolean;
    "aria-haspopup"?: "menu" | "dialog" | "listbox" | boolean;
    "aria-controls"?: string;
    class?: string;
  }

  let {
    icon,
    label,
    title = label,
    size = 16,
    variant = "ghost",
    tone = "default",
    disabled = false,
    pressed = undefined,
    dense = false,
    tabindex = undefined,
    type = "button",
    onclick = undefined,
    "aria-expanded": ariaExpanded = undefined,
    "aria-haspopup": ariaHasPopup = undefined,
    "aria-controls": ariaControls = undefined,
    class: className = "",
  }: Props = $props();
</script>

<button
  class="icon-btn {variant} {tone} {className}"
  class:dense
  {type}
  aria-label={label}
  {title}
  aria-pressed={pressed}
  aria-expanded={ariaExpanded}
  aria-haspopup={ariaHasPopup}
  aria-controls={ariaControls}
  {disabled}
  {tabindex}
  {onclick}
>
  <Icon {icon} {size} />
</button>

<style>
  .icon-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.65rem;
    height: 1.65rem;
    padding: 0;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
    transition:
      background-color 0.12s ease,
      color 0.12s ease;
  }

  .icon-btn:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .icon-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .icon-btn[aria-expanded="true"],
  .icon-btn[aria-pressed="true"] {
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
  }

  .icon-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .icon-btn.dense {
    width: 1.35rem;
    height: 1.35rem;
  }

  .icon-btn.outlined {
    border-color: var(--m3-outline-variant, var(--m3-primary));
  }

  .icon-btn.outlined:hover:not(:disabled) {
    border-color: var(--m3-primary);
  }

  .icon-btn.filled {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .icon-btn.filled:hover:not(:disabled) {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    filter: brightness(1.08);
  }

  .icon-btn.tone-primary {
    color: var(--m3-primary);
  }

  .icon-btn.tone-danger {
    color: var(--m3-error, var(--m3-on-surface-variant));
  }

  .icon-btn.tone-danger:hover:not(:disabled) {
    background: var(--m3-error-container, var(--m3-surface-container-high));
    color: var(--m3-on-error-container, var(--m3-error));
  }

  .icon-btn.tone-success {
    color: var(--m3-tertiary, var(--m3-on-surface-variant));
  }
</style>
