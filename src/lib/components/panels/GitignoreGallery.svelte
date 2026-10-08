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
    class="scrim"
    role="presentation"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        open = false;
      }
    }}
  >
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="gi-title">
      <h2 id="gi-title" class="title">.gitignore templates</h2>
      <input
        class="search"
        type="search"
        bind:value={query}
        placeholder="Search templates"
        aria-label="Search gitignore templates"
      />
      {#if loading}
        <p class="state" role="status">Loading templates…</p>
      {:else if error}
        <p class="state error" role="alert">{error}</p>
      {:else if filtered.length === 0}
        <p class="state">No template matches “{query}”.</p>
      {:else}
        <ul class="list">
          {#each filtered as template (template.name)}
            <li>
              <button
                class="row"
                type="button"
                disabled={applying !== null}
                title={template.description}
                onclick={() => void apply(template)}
              >
                <span class="name">{template.name}</span>
                <span class="desc">{template.description}</span>
                {#if applying === template.name}<span class="desc">applying…</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="actions">
        <button class="secondary" type="button" onclick={() => (open = false)}>
          Close
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--m3-scrim, black) 40%, transparent);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    width: min(26rem, calc(100vw - 2rem));
    max-height: min(30rem, calc(100vh - 4rem));
    padding: 1.25rem 1.5rem;
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.875rem;
  }

  .title {
    margin: 0 0 0.75rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .search {
    font: inherit;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.4rem 0.6rem;
    margin-bottom: 0.5rem;
  }

  .search:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .state {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 0.5rem 0;
    margin: 0;
  }

  .error {
    color: var(--m3-error, inherit);
  }

  .list {
    margin: 0;
    padding: 0;
    list-style: none;
    overflow-y: auto;
    min-height: 0;
  }

  .row {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    font: inherit;
    color: var(--m3-on-surface);
    padding: 0.4rem 0.5rem;
    border-radius: var(--m3-shape-small, 8px);
    cursor: pointer;
  }

  .row:hover:not(:disabled),
  .row:focus-visible {
    background: var(--m3-surface-container-highest, var(--m3-surface));
  }

  .row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .name {
    font-weight: 600;
    flex: none;
  }

  .desc {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    padding-top: 0.75rem;
  }

  .secondary {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.45rem 1rem;
    cursor: pointer;
  }
</style>
