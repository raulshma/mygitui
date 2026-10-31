<!--
  GitignoreGallery (M9 F10) — the builtin `.gitignore` template gallery.

  Renders as a small inline popover-style dialog: search box over the
  curated template list (name + description); clicking a template applies
  it to the repo root `.gitignore` (idempotent per line) and closes.
-->
<script lang="ts">
  import { gitignoreApplyTemplate, gitignoreTemplates } from "$lib/ipc/client";
  import type { GitignoreTemplate } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import { Button } from "$lib/components/ui/button/index.js";

  let {
    open = $bindable(false),
    repoId,
    onApplied = undefined,
  }: {
    open?: boolean;
    repoId: string;
    /** Called after a template is applied (owner refreshes status). */
    onApplied?: () => void;
  } = $props();

  let templates = $state<GitignoreTemplate[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let query = $state("");
  let applying = $state<string | null>(null);

  $effect(() => {
    if (!open || templates.length > 0) return;
    loading = true;
    error = null;
    gitignoreTemplates()
      .then((list) => (templates = list))
      .catch((err: unknown) => {
        error = err instanceof Error ? err.message : String(err);
      })
      .finally(() => (loading = false));
  });

  const filtered = $derived(
    query.trim() === ""
      ? templates
      : templates.filter((t) =>
          `${t.name} ${t.description}`.toLowerCase().includes(query.trim().toLowerCase()),
        ),
  );

  async function apply(template: GitignoreTemplate): Promise<void> {
    if (applying) return;
    applying = template.name;
    try {
      await gitignoreApplyTemplate(repoId, template.name);
      toast(`Applied ${template.name} gitignore template`, { kind: "success" });
      open = false;
      onApplied?.();
    } catch (err) {
      toast(
        `Apply failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      applying = null;
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="scrim fixed inset-0 z-[70] flex items-center justify-center bg-black/40"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        open = false;
      }
    }}
  >
    <div class="dialog flex max-h-[min(30rem,calc(100vh-4rem))] w-[min(26rem,calc(100vw-2rem))] flex-col rounded-lg border border-border bg-card px-6 py-5 text-sm text-foreground shadow-xl" role="dialog" aria-modal="true" aria-labelledby="gi-title">
      <h2 id="gi-title" class="title m-0 mb-3 text-lg font-medium">.gitignore templates</h2>
      <input
        class="search mb-2 rounded-md border border-border bg-background px-2.5 py-1.5 text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
        type="search"
        bind:value={query}
        placeholder="Search templates"
        aria-label="Search gitignore templates"
      />
      {#if loading}
        <p class="state m-0 py-2 text-muted-foreground" role="status">Loading templates…</p>
      {:else if error}
        <p class="state error m-0 py-2 text-destructive" role="alert">{error}</p>
      {:else if filtered.length === 0}
        <p class="state m-0 py-2 text-muted-foreground">No template matches “{query}”.</p>
      {:else}
        <ul class="list m-0 min-h-0 list-none overflow-y-auto p-0">
          {#each filtered as template (template.name)}
            <li>
              <Button
                class="w-full cursor-pointer items-baseline justify-start gap-2 rounded-md px-2 py-1.5 text-left font-normal enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-100"
                variant="ghost"
                type="button"
                disabled={applying !== null}
                title={template.description}
                onclick={() => void apply(template)}
              >
                <span class="name flex-none font-semibold">{template.name}</span>
                <span class="desc overflow-hidden text-ellipsis whitespace-nowrap text-xs text-muted-foreground">{template.description}</span>
                {#if applying === template.name}<span class="desc overflow-hidden text-ellipsis whitespace-nowrap text-xs text-muted-foreground">applying…</span>{/if}
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="actions flex justify-end pt-3">
        <Button class="rounded-full px-4 py-[7px]" variant="outline" type="button" onclick={() => (open = false)}>
          Close
        </Button>
      </div>
    </div>
  </div>
{/if}
