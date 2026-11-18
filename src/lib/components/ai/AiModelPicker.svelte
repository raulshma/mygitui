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
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { modelChoice, recentModels } from "$lib/ai/modelChoice.svelte";

  let {
    repoId,
    disabled = false,
  }: {
    repoId: string;
    disabled?: boolean;
  } = $props();

  /** Select sentinel for the free-text entry. */
  const CUSTOM = "__custom__";

  const backendLabel = $derived(ai.config.backend === "openrouter" ? "OpenRouter" : "OpenCode");
  const recent = $derived(recentModels(ai.jobs.history, ai.config.backend));
  const override = $derived(modelChoice.overrideFor(repoId));

  /** Free-text entry visible? */
  let customOpen = $state(false);
  let customText = $state("");
  /** The select's value ("" = Default). */
  let selected = $state("");

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
</script>

<span class="ai-model-picker inline-flex flex-none items-center gap-1">
  <select
    class="h-6 max-w-[13rem] rounded-full border border-border bg-background px-2 text-ui-tiny text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-50"
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
      class="h-6 w-40 rounded-sm border border-border bg-background px-2 font-mono text-ui-tiny text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
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
      class="rounded-sm px-1.5 py-px text-ui-tiny text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      type="button"
      onclick={applyCustom}
    >
      Apply
    </button>
    <button
      class="rounded-sm px-1.5 py-px text-ui-tiny text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      type="button"
      onclick={cancelCustom}
    >
      Cancel
    </button>
  {/if}
</span>
