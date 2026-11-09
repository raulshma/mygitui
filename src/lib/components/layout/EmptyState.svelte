<script lang="ts">
  import type { IconifyIcon } from "@iconify/types";
  import type { Snippet } from "svelte";
  import Icon from "../icons/Icon.svelte";

  /**
   * Shared empty-state block for panels and views: centered icon, title and
   * optional hint (plus an optional actions slot) on the app's dense-UI text
   * ramp. Replaces the per-panel hand-rolled variants so every surface uses
   * the same sizes and spacing.
   */
  let {
    icon,
    title,
    hint,
    class: className = "",
    children,
  }: {
    icon?: IconifyIcon;
    title: string;
    hint?: string;
    class?: string;
    children?: Snippet;
  } = $props();
</script>

<!-- No role="status": static placeholder content, and a live-region role here
     collides with in-row progress indicators that legitimately use it. -->
<div
  class="empty-state flex min-h-0 flex-1 select-none flex-col items-center justify-center gap-1 px-4 py-8 text-center text-muted-foreground {className}"
>
  {#if icon}
    <span
      class="empty-icon mb-0.5 grid size-8 place-items-center rounded-full bg-muted text-muted-foreground"
      aria-hidden="true"
    >
      <Icon {icon} size={16} />
    </span>
  {/if}
  <p class="empty-title m-0 text-ui-base font-medium text-foreground">{title}</p>
  {#if hint}
    <p class="empty-hint m-0 max-w-[15rem] text-ui-meta leading-[1.4]">{hint}</p>
  {/if}
  {#if children}
    <div class="empty-actions mt-3 flex flex-wrap items-center justify-center gap-2">
      {@render children()}
    </div>
  {/if}
</div>
