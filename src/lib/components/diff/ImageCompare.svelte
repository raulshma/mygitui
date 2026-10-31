<script lang="ts">
  /**
   * Old/new image comparison for `is_image` files.
   *
   * Two view modes:
   *   - "side": old and new <img> side by side (default)
   *   - "overlay": new image stacked over old with an opacity slider and a
   *     blink toggle (CSS animation steps between 0/1 — animation origin
   *     outranks the inline opacity, so blink wins while active)
   *
   * URLs come from the host via `onLoadImage`; when it is absent, rejects, or
   * resolves null, the corresponding placeholder panel is shown.
   */
  import type { FileDiff } from "$lib/ipc/types";
  import type { LoadImageFn } from "$lib/components/diff/rowModel";
  import { Toggle } from "$lib/components/ui/toggle/index.js";

  let {
    file,
    onLoadImage,
  }: {
    file: FileDiff;
    onLoadImage?: LoadImageFn;
  } = $props();

  let oldUrl = $state<string | null>(null);
  let newUrl = $state<string | null>(null);
  let loading = $state(true);
  let overlay = $state(false);
  let opacityPct = $state(100);
  let blink = $state(false);

  $effect(() => {
    const resolve = onLoadImage;
    const target = file;
    oldUrl = null;
    newUrl = null;
    loading = !!resolve;
    blink = false;
    if (!resolve) return;

    let alive = true;
    const load = async (path: string, old: boolean): Promise<string | null> => {
      try {
        return await resolve(path, old);
      } catch {
        return null;
      }
    };
    void Promise.all([
      load(target.old_path ?? target.path, true),
      load(target.path, false),
    ]).then(([o, n]) => {
      if (!alive) return;
      oldUrl = o;
      newUrl = n;
      loading = false;
    });
    return () => {
      alive = false;
    };
  });

  function switchToOverlay(next: boolean): void {
    overlay = next;
    if (!overlay) blink = false;
  }
</script>

<div class="imgcmp flex h-full min-h-0 flex-col gap-2 px-3 py-2 box-border">
  <div class="controls flex shrink-0 items-center gap-3">
    <div class="seg flex overflow-hidden rounded-full border border-border" role="group" aria-label="Image compare mode">
      <Toggle
        class="h-auto rounded-none border-0 px-[0.6rem] py-[0.3rem] font-normal text-[0.6875rem] leading-none text-muted-foreground aria-pressed:text-foreground focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary focus-visible:ring-0"
        pressed={!overlay}
        onPressedChange={(next) => next && switchToOverlay(false)}
      >Side by side</Toggle>
      <Toggle
        class="h-auto rounded-none border-0 px-[0.6rem] py-[0.3rem] font-normal text-[0.6875rem] leading-none text-muted-foreground aria-pressed:text-foreground focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary focus-visible:ring-0"
        pressed={overlay}
        onPressedChange={(next) => next && switchToOverlay(true)}
      >Overlay</Toggle>
    </div>
    <label class="slider flex min-w-0 flex-1 items-center gap-2">
      <span class="lbl shrink-0 text-[0.6875rem] text-muted-foreground">Old</span>
      <input
        type="range"
        min="0"
        max="100"
        step="1"
        class="min-w-16 flex-1 accent-primary"
        bind:value={opacityPct}
        disabled={!overlay || blink}
        aria-label="New image opacity (0 = old image, 100 = new image)"
      />
      <span class="lbl shrink-0 text-[0.6875rem] text-muted-foreground">New</span>
    </label>
    <Toggle
      class="h-auto shrink-0 rounded-full border border-border px-3 py-[0.3rem] font-normal text-[0.6875rem] text-muted-foreground aria-pressed:border-transparent aria-pressed:text-[var(--app-renamed)] aria-pressed:bg-[var(--app-renamed-bg)] disabled:cursor-default disabled:opacity-45 focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary focus-visible:ring-0"
      pressed={blink}
      disabled={!overlay}
      onPressedChange={(next) => (blink = next)}
    >Blink</Toggle>
  </div>

  {#if overlay}
    <div class="stage stack relative grid min-h-0 flex-1 place-items-center">
      {#if oldUrl}
        <img class="base max-h-full max-w-full object-contain rounded border border-[var(--diff-divider)] bg-background [grid-area:1_/_1]" src={oldUrl} alt={`Old version of ${file.path}`} />
      {:else}
        <div class="placeholder grid h-full w-full min-h-12 place-items-center rounded border border-dashed border-border bg-muted text-xs text-muted-foreground [grid-area:1_/_1]">{loading ? "Loading old image…" : "Old image unavailable"}</div>
      {/if}
      {#if newUrl}
        <img class="overlay max-h-full max-w-full object-contain rounded border border-[var(--diff-divider)] bg-background [grid-area:1_/_1] {blink ? 'animate-[diff-blink_1s_step-end_infinite] motion-reduce:animate-none' : ''}" style:opacity={opacityPct / 100} src={newUrl} alt={`New version of ${file.path}`} />
      {:else}
        <div class="placeholder overlay grid h-full w-full min-h-12 place-items-center rounded border border-dashed border-border bg-muted text-xs text-muted-foreground [grid-area:1_/_1] {blink ? 'animate-[diff-blink_1s_step-end_infinite] motion-reduce:animate-none' : ''}" style:opacity={opacityPct / 100}>
          {loading ? "Loading new image…" : "New image unavailable"}
        </div>
      {/if}
    </div>
  {:else}
    <div class="stage side flex min-h-0 flex-1 gap-3">
      <figure class="panel m-0 flex min-w-0 flex-1 flex-col gap-1">
        {#if oldUrl}
          <img class="min-h-0 w-full flex-1 object-contain rounded border border-[var(--diff-divider)] bg-background" src={oldUrl} alt={`Old version of ${file.path}`} />
        {:else}
          <div class="placeholder grid h-full w-full min-h-12 place-items-center rounded border border-dashed border-border bg-muted text-xs text-muted-foreground">{loading ? "Loading old image…" : "Old image unavailable"}</div>
        {/if}
        <figcaption class="shrink-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.6875rem] text-muted-foreground">Old — {file.old_path ?? file.path}</figcaption>
      </figure>
      <figure class="panel m-0 flex min-w-0 flex-1 flex-col gap-1">
        {#if newUrl}
          <img class="min-h-0 w-full flex-1 object-contain rounded border border-[var(--diff-divider)] bg-background" src={newUrl} alt={`New version of ${file.path}`} />
        {:else}
          <div class="placeholder grid h-full w-full min-h-12 place-items-center rounded border border-dashed border-border bg-muted text-xs text-muted-foreground">{loading ? "Loading new image…" : "New image unavailable"}</div>
        {/if}
        <figcaption class="shrink-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.6875rem] text-muted-foreground">New — {file.path}</figcaption>
      </figure>
    </div>
  {/if}
</div>
