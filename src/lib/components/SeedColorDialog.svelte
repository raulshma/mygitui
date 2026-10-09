<script lang="ts">
  /**
   * SeedColorDialog (M12) — accent seed picker behind the palette commands
   * "Appearance: Set/Reset accent seed color…". Submit applies the color
   * through `onSubmit(hex)` (caller re-runs `initTheme()`); "Clear" removes
   * the override via `onClear()`. Both paths close the dialog.
   */
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { normalizeHex, userSeedColor } from "$lib/theme/dynamic-color";

  let {
    onSubmit,
    onClear,
    onCancel = undefined,
  }: {
    /** Submit with a validated `#rrggbb` seed. */
    onSubmit: (hex: string) => void;
    /** Remove the override and return to the baseline palette. */
    onClear: () => void;
    onCancel?: () => void;
  } = $props();

  let open = $state(true);
  let picked = $state(userSeedColor() ?? "#6750a4");
  let error = $state("");

  function submit(): void {
    // DialogShell renders the form and preventDefaults before calling
    // onsubmit, so no event handling is needed here.
    const hex = normalizeHex(picked);
    if (!hex) {
      error = "Enter a hex color like #6750a4";
      return;
    }
    open = false;
    onSubmit(hex);
  }

  function clear(): void {
    open = false;
    onClear();
  }
</script>

<DialogShell
  bind:open
  labelledBy="seed-title"
  width="22rem"
  {onCancel}
  onsubmit={() => submit()}
>
  {#snippet children()}
    <h2 id="seed-title" class="seed-title">Accent seed color</h2>
    <p class="seed-hint">
      Derives the whole Material 3 palette (light + dark). Stored per
      machine; leave empty of wallpapers — this is a manual override of the
      same pipeline.
    </p>
    <div class="seed-row">
      <input
        class="seed-color"
        type="color"
        bind:value={picked}
        aria-label="Pick accent seed color"
      />
      <input
        class="seed-text"
        type="text"
        bind:value={picked}
        placeholder="#6750a4"
        spellcheck="false"
        aria-label="Accent seed hex value"
      />
    </div>
    {#if error}
      <p class="seed-error" role="alert">{error}</p>
    {/if}
  {/snippet}
  {#snippet actions()}
    <button class="seed-clear" type="button" onclick={clear}>
      Clear override
    </button>
    <button class="seed-apply" type="submit">Apply</button>
  {/snippet}
</DialogShell>

<style>
  .seed-title {
    margin: 0 0 0.35rem;
    font-size: 1rem;
    font-weight: 600;
    color: var(--m3-on-surface, inherit);
  }

  .seed-hint {
    margin: 0 0 0.75rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .seed-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .seed-color {
    width: 2.75rem;
    height: 2.25rem;
    padding: 0.1rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.5rem;
    background: none;
    cursor: pointer;
  }

  .seed-text {
    flex: 1;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.8125rem;
    padding: 0.4rem 0.55rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.5rem;
    background: none;
    color: var(--m3-on-surface, inherit);
  }

  .seed-error {
    margin: 0.5rem 0 0;
    font-size: 0.75rem;
    color: var(--m3-error, #b3261e);
  }

  .seed-clear,
  .seed-apply {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 9999px;
    padding: 0.35rem 0.9rem;
    font: inherit;
    font-size: 0.8125rem;
    cursor: pointer;
    background: none;
    color: var(--m3-on-surface, inherit);
  }

  .seed-apply {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    border-color: var(--m3-primary);
  }

  .seed-apply:hover,
  .seed-clear:hover {
    filter: brightness(1.08);
  }
</style>
