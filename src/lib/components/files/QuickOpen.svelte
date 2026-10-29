<!--
  QuickOpen — fuzzy file finder (Ctrl+P / palette `files.quick-open`).

  Mounted per open (RepoView owns visibility): fetches the backend file
  index once per repo (module-level cache with a staleness window, so
  repeated opens are instant) and scores it per keystroke through the
  pure `quickOpenModel`. Combobox/listbox a11y pattern like the command
  palette: arrows move, Enter picks, Escape closes.
-->
<script lang="ts">
  import type { FileIndexEntry } from "$lib/ipc/types";
  import { fileIndex } from "$lib/ipc/client";
  import {
    buildIndex,
    pathTokens,
    searchPaths,
    type PathIndex,
  } from "./quickOpenModel";
  import { fileIconForPath, fileIconColorForPath } from "$lib/components/icons/fileIcons";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Spinner } from "$lib/components/m3";
  import iconSearch from "@ktibow/iconset-material-symbols/search";

  let {
    repoId,
    onClose,
    onPick,
  }: {
    repoId: string;
    onClose: () => void;
    /** Picks a repository-relative path (opens it in the workspace). */
    onPick: (path: string) => void;
  } = $props();

  /** Module-level per-repo index cache (survives dialog remounts). */
  const cache = new Map<
    string,
    { index: PathIndex; fetchedAt: number }
  >();
  const STALE_MS = 60_000;
  const VISIBLE = 12;

  let query = $state("");
  let loading = $state(true);
  let errorText = $state("");
  let selectedIndex = $state(0);
  let inputEl = $state<HTMLInputElement | undefined>();

  let index = $state<PathIndex | null>(null);

  const hits = $derived.by(() => (index ? searchPaths(index, query, 200) : []));
  /** Keep the selection anchored to a real hit. */
  const safeSelected = $derived(
    hits.length === 0 ? -1 : Math.min(selectedIndex, hits.length - 1),
  );
  const visible = $derived(hits.slice(0, VISIBLE));

  $effect(() => {
    // Autofocus + Escape scoping.
    inputEl?.focus();
  });

  $effect(() => {
    void (async () => {
      const cached = cache.get(repoId);
      if (cached && Date.now() - cached.fetchedAt < STALE_MS) {
        index = cached.index;
        loading = false;
        return;
      }
      loading = true;
      errorText = "";
      try {
        const result: { entries: FileIndexEntry[] } = await fileIndex(repoId);
        const fresh = buildIndex(result.entries);
        cache.set(repoId, { index: fresh, fetchedAt: Date.now() });
        // Only apply when still on the same repo dialog instance.
        index = fresh;
      } catch (err) {
        errorText = String(err);
      } finally {
        loading = false;
      }
    })();
  });

  $effect(() => {
    selectedIndex = 0;
  });

  function pick(path: string): void {
    onPick(path);
    onClose();
  }

  function onKeydown(event: KeyboardEvent): void {
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        onClose();
        break;
      case "ArrowDown":
        event.preventDefault();
        if (hits.length > 0) {
          selectedIndex = Math.min(safeSelected + 1, hits.length - 1);
        }
        break;
      case "ArrowUp":
        event.preventDefault();
        selectedIndex = Math.max(safeSelected - 1, 0);
        break;
      case "Home":
        event.preventDefault();
        selectedIndex = 0;
        break;
      case "End":
        event.preventDefault();
        if (hits.length > 0) selectedIndex = hits.length - 1;
        break;
      case "Enter": {
        event.preventDefault();
        const hit = hits[safeSelected];
        if (hit) pick(hit.path);
        break;
      }
      default:
        break;
    }
  }

  function onScrimClick(event: MouseEvent): void {
    if (event.target === event.currentTarget) onClose();
  }
</script>

<div class="scrim" role="presentation" onclick={onScrimClick}>
  <div class="panel" role="dialog" aria-modal="true" aria-label="Go to file">
    <div class="inputrow" role="combobox" aria-expanded={hits.length > 0} aria-controls="quick-open-list" aria-haspopup="listbox">
      <Icon icon={iconSearch} size={15} />
      <input
        bind:this={inputEl}
        bind:value={query}
        onkeydown={onKeydown}
        type="text"
        placeholder="Go to file…"
        aria-label="Go to file"
        autocomplete="off"
        spellcheck="false"
      />
      {#if loading}
        <Spinner size={13} />
      {/if}
    </div>

    {#if errorText !== ""}
      <p class="state state-error" role="alert">{errorText}</p>
    {:else if !loading && hits.length === 0}
      <p class="state">No matching files</p>
    {:else if visible.length > 0}
      <ul
        class="list"
        id="quick-open-list"
        role="listbox"
        aria-label="Matching files"
      >
        {#each visible as hit, i (hit.path)}
          <!-- svelte-ignore a11y_click_events_have_key_events -- the
               input owns all keyboard navigation (combobox pattern) -->
          <li
            class="row"
            class:sel={i === safeSelected}
            role="option"
            aria-selected={i === safeSelected}
            onclick={() => pick(hit.path)}
            onpointerenter={() => (selectedIndex = i)}
          >
            <span
              class="ficon"
              style={`color:${fileIconColorForPath(hit.path, fileIconPreferences.theme)}`}
            >
              <Icon
                icon={fileIconForPath(hit.path, fileIconPreferences.theme)}
                size={14}
              />
            </span>
            <span class="path">
              {#each pathTokens(hit.path, query) as token, ti (ti)}{#if token.separator}<span class="sep">{token.text}</span>{:else if token.marked}<mark>{token.text}</mark>{:else}{token.text}{/if}{/each}
            </span>
          </li>
        {/each}
      </ul>
      {#if hits.length > visible.length}
        <p class="more">{hits.length} matches — keep typing to narrow</p>
      {/if}
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
    background: color-mix(in srgb, black 32%, transparent);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 10vh 1rem 1rem;
  }

  .panel {
    width: min(34rem, 100%);
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: var(--m3-surface-container, var(--m3-surface));
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-medium, 12px);
    box-shadow: 0 12px 40px color-mix(in srgb, black 35%, transparent);
    overflow: hidden;
  }

  .inputrow {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.55rem 0.75rem;
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
    color: var(--m3-on-surface-variant);
  }

  .inputrow input {
    flex: 1;
    min-width: 0;
    border: none;
    background: none;
    outline: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.875rem;
  }

  .list {
    margin: 0;
    padding: 0.25rem;
    list-style: none;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.28rem 0.5rem;
    border-radius: var(--m3-shape-extra-small, 6px);
    cursor: pointer;
    font-size: 0.8125rem;
    color: var(--m3-on-surface);
  }

  .row.sel {
    background: var(--app-active, var(--m3-secondary-container, var(--m3-surface-container-highest)));
  }

  .ficon {
    flex: none;
    display: inline-flex;
  }

  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.75rem;
  }

  .sep {
    color: var(--m3-outline, var(--m3-on-surface-variant));
  }

  mark {
    background: none;
    color: var(--m3-primary);
    font-weight: 600;
  }

  .state {
    padding: 0.9rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
    text-align: center;
  }

  .state-error {
    color: var(--m3-error);
  }

  .more {
    flex: none;
    margin: 0;
    padding: 0.3rem 0.75rem;
    border-top: 1px solid var(--app-border, var(--m3-outline-variant));
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
    text-align: center;
  }
</style>
