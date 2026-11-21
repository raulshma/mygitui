<script lang="ts">
  /**
   * AiModelPicker (item 55) — the compact per-run model selector the AI
   * launchers render: "Default" (the AiSettings-configured model) plus the
   * last models that actually served a successful run for the current
   * backend (from the job history ring, `recentModels`) plus a free-text
   * "Custom…" entry.
   *
   * The choice persists per repo (`modelChoice.svelte.ts`) and rides the
   * existing job-start call: launchers pass it as `AiRunOptions.model`
   * (or `ctx.model`), which flows through `runFeature` into the provider
   * request — the provider resolves the override over its configured
   * default, so picking "Default" always follows AI settings again.
   *
   * Two skins:
   *  - inline (default): the original one-row `<select>` — used where a
   *    controls row has room to spare (ExplainHunkPanel's header).
   *  - compact (`compact`): a tiny caret button with an attached menu —
   *    the commit-flow skin (plan §6.3): the composer shows ONE sparkle
   *    for generation plus this caret for overrides; no full-width
   *    dropdown squats in the bar. The menu is the shared context-menu
   *    (keyboard, Esc, outside-close) and the free-text entry opens a
   *    small anchored card (the AiFeatureButton overlay grammar — the
   *    trigger sits in a clipping ancestor, so the card is portaled).
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { modelChoice, recentModels } from "$lib/ai/modelChoice.svelte";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { floating } from "$lib/components/overlay/floating";

  let {
    repoId,
    disabled = false,
    compact = false,
    popUp = false,
  }: {
    repoId: string;
    disabled?: boolean;
    /** Menu skin: caret trigger + attached override menu (commit composer). */
    compact?: boolean;
    /** Compact menu opens upward (bottom-docked composers). */
    popUp?: boolean;
  } = $props();

  /** Select sentinel for the free-text entry (inline skin). */
  const CUSTOM = "__custom__";

  const backendLabel = $derived(ai.config.backend === "openrouter" ? "OpenRouter" : "OpenCode");
  const recent = $derived(recentModels(ai.jobs.history, ai.config.backend));
  const override = $derived(modelChoice.overrideFor(repoId));

  const iconCaret = $derived(appIcon("chevron-down")!);

  /** Free-text entry visible? */
  let customOpen = $state(false);
  let customText = $state("");
  /** The select's value ("" = Default). */
  let selected = $state("");
  /** Compact-skin state: menu trigger element + anchored custom card. */
  let trigger = $state<HTMLElement | undefined>(undefined);

  // Follow the persisted choice while the free-text entry is closed (it
  // owns the select display while open; applying/cancelling closes it and
  // this effect resnaps the select to the store).
  $effect(() => {
    if (customOpen) return;
    selected = override ?? "";
  });

  function onPick(value: string): void {
    if (value === CUSTOM) {
      customText = override ?? "";
      customOpen = true;
      return;
    }
    customOpen = false;
    modelChoice.setOverride(repoId, value.length > 0 ? value : null);
  }

  function applyCustom(): void {
    const text = customText.trim();
    modelChoice.setOverride(repoId, text.length > 0 ? text : null);
    customOpen = false;
  }

  function cancelCustom(): void {
    customOpen = false;
  }

  // -- compact skin -------------------------------------------------------------

  const compactHint = $derived(
    override === null
      ? `Model for AI runs in this repository: default (${backendLabel})`
      : `Model for AI runs in this repository: ${override} (override)`,
  );

  /** The attached override menu: Default, recent successes, Custom… */
  function openMenu(event: MouseEvent): void {
    const entries = [
      {
        id: "model-default",
        label: `${override === null ? "✓ " : ""}Default (${backendLabel})`,
        run: () => modelChoice.setOverride(repoId, null),
      },
      ...recent.map((model) => ({
        id: `model-${model}`,
        label: `${override === model ? "✓ " : ""}${model}`,
        run: () => modelChoice.setOverride(repoId, model),
      })),
      { separator: true as const },
      {
        id: "model-custom",
        label: "Custom model id…",
        run: () => {
          customText = override ?? "";
          customOpen = true;
        },
      },
    ];
    showMenuAtElement(event.currentTarget as HTMLElement, entries, {
      align: "end",
      above: popUp,
    });
  }
</script>

{#if compact}
  <span class="ai-model-picker relative inline-flex flex-none items-center" bind:this={trigger}>
    <Hint label={compactHint}>
      {#snippet child({ props })}
        <button
          {...props}
          class="inline-flex size-5 cursor-pointer flex-none items-center justify-center rounded-sm border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50"
          type="button"
          disabled={disabled}
          aria-label="Model for AI runs in this repository"
          aria-haspopup="menu"
          onclick={openMenu}
        >
          <Icon icon={iconCaret} size={14} />
        </button>
      {/snippet}
    </Hint>
    {#if customOpen}
      <!-- Free-text entry: portaled + clamped (the composer lives in
           clipping ancestors); Enter applies, Esc cancels. -->
      <div
        class="invisible fixed left-0 top-0 z-40 flex max-w-[min(20rem,calc(100vw_-_1rem))] items-center gap-1 rounded-lg border border-border bg-popover p-1.5 text-popover-foreground shadow-overlay"
        use:floating={{ anchor: trigger, above: popUp, alignEnd: true }}
      >
        <input
          class="h-6 w-44 rounded-sm border border-border bg-background px-1.5 font-mono text-ui-meta text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
          placeholder="provider/model"
          spellcheck="false"
          aria-label="Custom model id"
          bind:value={customText}
          onclick={(e) => e.stopPropagation()}
          onkeydown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              applyCustom();
            } else if (event.key === "Escape") {
              event.preventDefault();
              cancelCustom();
            }
          }}
        />
        <button
          class="shrink-0 rounded-sm px-1.5 py-px text-ui-meta text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          type="button"
          onclick={applyCustom}
        >
          Apply
        </button>
        <button
          class="shrink-0 rounded-sm px-1.5 py-px text-ui-meta text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          type="button"
          onclick={cancelCustom}
        >
          Cancel
        </button>
      </div>
    {/if}
  </span>
{:else}
  <span class="ai-model-picker inline-flex flex-none items-center gap-1">
    <select
      class="h-6 max-w-[13rem] rounded-full border border-border bg-background px-2 text-ui-meta text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-50"
      aria-label="Model for AI runs in this repository"
      disabled={disabled}
      bind:value={selected}
      onchange={() => onPick(selected)}
    >
      <option value="">Default ({backendLabel})</option>
      {#if override !== null && !recent.includes(override)}
        <option value={override}>Custom · {override}</option>
      {/if}
      {#each recent as model (model)}
        <option value={model}>{model}</option>
      {/each}
      <option value={CUSTOM}>Custom…</option>
    </select>
    {#if customOpen}
      <input
        class="h-6 w-40 rounded-sm border border-border bg-background px-2 font-mono text-ui-meta text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
        placeholder="provider/model"
        spellcheck="false"
        aria-label="Custom model id"
        bind:value={customText}
        onkeydown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            applyCustom();
          } else if (event.key === "Escape") {
            event.preventDefault();
            cancelCustom();
          }
        }}
      />
      <button
        class="rounded-sm px-1.5 py-px text-ui-meta text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        onclick={applyCustom}
      >
        Apply
      </button>
      <button
        class="rounded-sm px-1.5 py-px text-ui-meta text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        onclick={cancelCustom}
      >
        Cancel
      </button>
    {/if}
  </span>
{/if}
