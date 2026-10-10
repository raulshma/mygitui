<script lang="ts" module>
  /**
   * Fixed local API over the M3 switch. m3-svelte's Switch is pinned at
   * 3.25rem × 2rem (and resizing it would mean re-specifying its internal
   * layout through :global overrides), so this wrapper implements the M3
   * switch spec directly at the compact size the app's dense rows need,
   * using the same --m3c-* tokens. M3-svelte churn still stops here.
   */
</script>

<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    checked: boolean;
    disabled?: boolean;
    "aria-label"?: string;
    /** Tooltip on the whole control. */
    title?: string;
    /** Label text rendered after the switch (inside the wrapping label). */
    children?: Snippet;
  }

  let {
    checked = $bindable(false),
    disabled = false,
    "aria-label": ariaLabel = undefined,
    title = undefined,
    children,
  }: Props = $props();
</script>

<label class="switch" class:disabled {title}>
  <input
    type="checkbox"
    role="switch"
    bind:checked
    {disabled}
    aria-label={ariaLabel}
  />
  <span class="track" aria-hidden="true">
    <span class="handle"></span>
  </span>
  {#if children}<span class="text">{@render children()}</span>{/if}
</label>

<style>
  .switch {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
  }
  .switch.disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  /* Visually hidden but focusable/labelled control. */
  input {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: 0;
    opacity: 0;
  }

  .track {
    flex: none;
    position: relative;
    width: 2.25rem;
    height: 1.25rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3c-surface-container-highest, var(--m3-surface-container-highest, var(--m3-surface-variant)));
    border: 2px solid var(--m3c-outline, var(--m3-outline));
    transition:
      background var(--m3-easing, 0.2s ease),
      border-color var(--m3-easing, 0.2s ease);
  }

  .handle {
    position: absolute;
    left: 0.25rem;
    top: 50%;
    translate: 0 -50%;
    width: 0.75rem;
    height: 0.75rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3c-outline, var(--m3-outline));
    transition:
      left var(--m3-easing-fast-spatial, 0.2s ease),
      background var(--m3-easing, 0.2s ease),
      scale var(--m3-easing-fast-spatial, 0.2s ease);
  }

  input:checked + .track {
    background: var(--m3c-primary, var(--m3-primary));
    border-color: var(--m3c-primary, var(--m3-primary));
  }
  input:checked + .track .handle {
    left: 0.875rem;
    scale: 1.5;
    background: var(--m3c-on-primary, var(--m3-on-primary));
  }

  .switch:not(.disabled):hover .handle {
    background: var(--m3c-on-surface-variant, var(--m3-on-surface-variant));
  }
  .switch:not(.disabled):hover input:checked + .track .handle {
    background: var(--m3c-on-primary, var(--m3-on-primary));
  }

  input:focus-visible + .track {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }

  .text {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }
</style>
