<script lang="ts" module>
  /**
   * Indeterminate progress glyph (M3 circular progress).
   *
   * Purely visual: callers own the live-region text next to it. `size` is
   * the square diameter in pixels; the ring inherits `currentColor`, so it
   * works on filled buttons (on-primary) and plain surfaces alike.
   */
</script>

<script lang="ts">
  let { size = 14 }: { size?: number } = $props();
</script>

<span
  class="spinner"
  style:width={`${size}px`}
  style:height={`${size}px`}
  aria-hidden="true"
></span>

<style>
  .spinner {
    display: inline-block;
    flex: none;
    border: 2px solid color-mix(in srgb, currentColor 28%, transparent);
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
