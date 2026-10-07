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

<div class="imgcmp">
  <div class="controls">
    <div class="seg" role="group" aria-label="Image compare mode">
      <button
        type="button"
        class:active={!overlay}
        aria-pressed={!overlay}
        onclick={() => switchToOverlay(false)}
      >Side by side</button>
      <button
        type="button"
        class:active={overlay}
        aria-pressed={overlay}
        onclick={() => switchToOverlay(true)}
      >Overlay</button>
    </div>
    <label class="slider">
      <span class="lbl">Old</span>
      <input
        type="range"
        min="0"
        max="100"
        step="1"
        bind:value={opacityPct}
        disabled={!overlay || blink}
        aria-label="New image opacity (0 = old image, 100 = new image)"
      />
      <span class="lbl">New</span>
    </label>
    <button
      type="button"
      class="blink-btn"
      class:active={blink}
      aria-pressed={blink}
      disabled={!overlay}
      onclick={() => (blink = !blink)}
    >Blink</button>
  </div>

  {#if overlay}
    <div class="stage stack" class:blink>
      {#if oldUrl}
        <img class="base" src={oldUrl} alt={`Old version of ${file.path}`} />
      {:else}
        <div class="placeholder">{loading ? "Loading old image…" : "Old image unavailable"}</div>
      {/if}
      {#if newUrl}
        <img class="overlay" style:opacity={opacityPct / 100} src={newUrl} alt={`New version of ${file.path}`} />
      {:else}
        <div class="placeholder overlay" style:opacity={opacityPct / 100}>
          {loading ? "Loading new image…" : "New image unavailable"}
        </div>
      {/if}
    </div>
  {:else}
    <div class="stage side">
      <figure class="panel">
        {#if oldUrl}
          <img src={oldUrl} alt={`Old version of ${file.path}`} />
        {:else}
          <div class="placeholder">{loading ? "Loading old image…" : "Old image unavailable"}</div>
        {/if}
        <figcaption>Old — {file.old_path ?? file.path}</figcaption>
      </figure>
      <figure class="panel">
        {#if newUrl}
          <img src={newUrl} alt={`New version of ${file.path}`} />
        {:else}
          <div class="placeholder">{loading ? "Loading new image…" : "New image unavailable"}</div>
        {/if}
        <figcaption>New — {file.path}</figcaption>
      </figure>
    </div>
  {/if}
</div>

<style>
  .imgcmp {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    padding: 0.5rem 0.75rem;
    box-sizing: border-box;
    gap: 0.5rem;
  }

  .controls {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .seg {
    display: flex;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 999px;
    overflow: hidden;
  }
  .seg button {
    border: none;
    background: transparent;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
    line-height: 1;
    padding: 0.3rem 0.6rem;
    cursor: pointer;
  }
  .seg button.active {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }
  .seg button:focus-visible,
  .blink-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .slider {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex: 1;
    min-width: 0;
  }
  .lbl {
    flex: none;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant);
  }
  .slider input[type="range"] {
    flex: 1;
    min-width: 4rem;
    accent-color: var(--m3-primary);
  }

  .blink-btn {
    flex: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 999px;
    background: transparent;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
    padding: 0.3rem 0.75rem;
    cursor: pointer;
  }
  .blink-btn.active {
    background: var(--m3-tertiary-container);
    color: var(--m3-on-tertiary-container);
    border-color: transparent;
  }
  .blink-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  /* ---- side-by-side ---- */
  .stage.side {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 0.75rem;
  }
  .panel {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    margin: 0;
    gap: 0.25rem;
  }
  .panel img {
    flex: 1;
    min-height: 0;
    width: 100%;
    object-fit: contain;
    border: 1px solid var(--diff-divider);
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
  }
  .panel figcaption {
    flex: none;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant);
    font-family: ui-monospace, Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ---- overlay compare ---- */
  .stage.stack {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
  }
  .stage.stack .base,
  .stage.stack .overlay,
  .stage.stack .placeholder {
    grid-area: 1 / 1;
  }
  .stage.stack img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border: 1px solid var(--diff-divider);
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
  }

  .placeholder {
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    min-height: 3rem;
    border: 1px dashed var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    color: var(--m3-on-surface-variant);
    font-size: 0.75rem;
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .stage.blink .overlay {
    animation: diff-blink 1s step-end infinite;
  }
  @keyframes diff-blink {
    0% { opacity: 0; }
    50% { opacity: 1; }
  }
  @media (prefers-reduced-motion: reduce) {
    .stage.blink .overlay {
      animation: none;
    }
  }
</style>
