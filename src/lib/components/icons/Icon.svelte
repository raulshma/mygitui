<script lang="ts" module>
  /**
   * Renders Iconify icon data (path data + viewBox, colors from the body's
   * own fill) at a square pixel size. The one place that turns
   * `IconifyIcon` into SVG markup — menus, buttons, and chips all draw
   * brand/symbol icons through this.
   */
</script>

<script lang="ts">
  import type { IconifyIcon } from "@iconify/types";

  let { icon, size = 16 }: { icon: IconifyIcon; size?: number } = $props();
</script>

<svg
  viewBox="{icon.left ?? 0} {icon.top ?? 0} {icon.width} {icon.height}"
  width={size}
  height={size}
  aria-hidden="true"
>{@html icon.body}</svg>

<style>
  svg {
    display: inline-block;
    vertical-align: middle;
  }
</style>
