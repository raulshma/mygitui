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
  import { fly } from "svelte/transition";
  import { dismissToast, getToasts, pauseToast, resumeToast, type ToastKind } from "$lib/toast";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import iconSuccess from "@ktibow/iconset-material-symbols/check-circle";
  import iconError from "@ktibow/iconset-material-symbols/error";
  import iconInfo from "@ktibow/iconset-material-symbols/info";
  import iconClose from "@ktibow/iconset-material-symbols/close";

  const GLYPHS = { success: iconSuccess, error: iconError, info: iconInfo } as const;

  function glyph(kind: ToastKind) {
    return GLYPHS[kind];
  }

  const reduceMotion =
    typeof window !== "undefined" &&
    (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false);
  const motion = reduceMotion ? 0 : 160;
</script>

<div class="toaster fixed right-4 bottom-4 z-1000 flex max-w-[min(24rem,calc(100vw-2rem))] flex-col gap-2">
  {#each getToasts() as item (item.id)}
    <div
      class="toast group pointer-events-auto flex items-center gap-2 rounded-lg border border-border border-l-[3px] border-l-primary bg-popover px-3 py-2.5 text-popover-foreground shadow-lg outline-none data-[kind=success]:border-l-[var(--app-added)] data-[kind=error]:border-l-destructive"
      role={item.kind === "error" ? "alert" : "status"}
      aria-live={item.kind === "error" ? "assertive" : "polite"}
      data-kind={item.kind}
      onpointerenter={() => pauseToast(item.id)}
      onpointerleave={() => resumeToast(item.id)}
      in:fly={{ y: 8, duration: motion }}
      out:fly={{ y: 4, duration: motion }}
    >
      <!-- Semantic accents: shape + glyph carry the meaning, color reinforces it. -->
      <span
        class="badge grid size-5 flex-none place-items-center rounded-full bg-primary text-primary-foreground group-data-[kind=success]:bg-[var(--app-added)] group-data-[kind=error]:bg-destructive"
        aria-hidden="true"
      >
        <Icon icon={glyph(item.kind)} size={13} />
      </span>
      <span class="message min-w-0 flex-1 text-ui-base [overflow-wrap:anywhere]">{item.message}</span>
      {#if item.count > 1}
        <span
          class="repeat flex-none rounded-full bg-muted px-1.5 py-0.5 text-ui-tiny font-semibold leading-none text-muted-foreground tabular-nums"
          aria-label={`Repeated ${item.count} times`}
        >×{item.count}</span>
      {/if}
      <Button
        class="close flex-none text-muted-foreground/70 hover:text-foreground focus-visible:text-foreground"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Dismiss notification"
        onclick={() => dismissToast(item.id)}
      >
        <Icon icon={iconClose} size={13} />
      </Button>
    </div>
  {/each}
</div>