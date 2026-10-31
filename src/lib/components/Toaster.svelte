<script lang="ts">
  /**
   * Renders the toast stack (bottom-right). State and auto-dismiss live in
   * `$lib/toast`; this component is purely presentational.
   *
   * Accessibility (M7 I2): each toast is its own live region — polite
   * `role="status"` for info/success, assertive `role="alert"` for errors —
   * so new toasts are announced without nesting live regions (a live-region
   * ancestor here would double-announce or suppress the children). Hovering
   * a toast pauses its auto-dismiss timer; leaving resumes the remainder.
   */
  import { dismissToast, getToasts, pauseToast, resumeToast, type ToastKind } from "$lib/toast";
  import { Button } from "$lib/components/ui/button/index.js";

  function glyph(kind: ToastKind): string {
    switch (kind) {
      case "success":
        return "✓";
      case "error":
        return "!";
      default:
        return "i";
    }
  }
</script>

<div class="toaster fixed right-4 bottom-4 z-[1000] flex max-w-[min(24rem,calc(100vw-2rem))] flex-col gap-2">
  {#each getToasts() as item (item.id)}
    <div
      class="toast group pointer-events-auto flex items-center gap-2 rounded-lg border border-border border-l-[3px] border-l-primary bg-popover px-3 py-2.5 text-popover-foreground shadow-lg outline-none data-[kind=success]:border-l-green-600 data-[kind=error]:border-l-destructive dark:data-[kind=success]:border-l-green-500 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2"
      role={item.kind === "error" ? "alert" : "status"}
      aria-live={item.kind === "error" ? "assertive" : "polite"}
      data-kind={item.kind}
      onpointerenter={() => pauseToast(item.id)}
      onpointerleave={() => resumeToast(item.id)}
    >
      <!-- Semantic accents: shape + glyph carry the meaning, color reinforces it. -->
      <span
        class="badge grid size-5 flex-none place-items-center rounded-full bg-primary text-[0.75rem] font-bold leading-none text-primary-foreground group-data-[kind=success]:bg-green-600 group-data-[kind=success]:text-white group-data-[kind=error]:bg-destructive group-data-[kind=error]:text-white dark:group-data-[kind=success]:bg-green-500"
        aria-hidden="true"
      >{glyph(item.kind)}</span>
      <span class="message min-w-0 flex-1 text-[0.8125rem] [overflow-wrap:anywhere]">{item.message}</span>
      <Button
        class="close flex-none p-0 pr-1 text-base leading-none focus-visible:text-foreground hover:text-foreground"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Dismiss notification"
        onclick={() => dismissToast(item.id)}
      >
        ×
      </Button>
    </div>
  {/each}
</div>
