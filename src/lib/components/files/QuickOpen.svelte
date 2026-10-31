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
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

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

  const iconSearch = $derived(appIcon("search")!);

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

<div
  class="scrim fixed inset-0 z-[90] flex items-start justify-center bg-black/32 px-4 pt-[10vh] pb-4"
  role="presentation"
  onclick={onScrimClick}
>
  <div
    class="panel flex max-h-[60vh] w-[min(34rem,100%)] flex-col overflow-hidden rounded-md border border-border bg-popover shadow-[0_12px_40px_rgb(0_0_0/0.35)]"
    role="dialog"
    aria-modal="true"
    aria-label="Go to file"
  >
    <div
      class="inputrow flex flex-none items-center gap-[0.45rem] border-b border-border px-3 py-[0.55rem] text-muted-foreground"
      role="combobox"
      aria-expanded={hits.length > 0}
      aria-controls="quick-open-list"
      aria-haspopup="listbox"
    >
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
        class="min-w-0 flex-1 border-0 bg-transparent font-sans text-sm text-foreground outline-none"
      />
      {#if loading}
        <Spinner class="size-[13px]" role="presentation" aria-hidden="true" />
      {/if}
    </div>

    {#if errorText !== ""}
      <p class="state state-error p-[0.9rem] text-center text-[0.8125rem] text-destructive" role="alert">{errorText}</p>
    {:else if !loading && hits.length === 0}
      <p class="state p-[0.9rem] text-center text-[0.8125rem] text-muted-foreground">No matching files</p>
    {:else if visible.length > 0}
      <ul
        class="list m-0 min-h-0 flex-1 list-none overflow-y-auto p-1"
        id="quick-open-list"
        role="listbox"
        aria-label="Matching files"
      >
        {#each visible as hit, i (hit.path)}
          <!-- svelte-ignore a11y_click_events_have_key_events -- the
               input owns all keyboard navigation (combobox pattern) -->
          <li
            class="row flex cursor-pointer items-center gap-[0.45rem] rounded-sm px-2 py-[0.28rem] text-[0.8125rem] text-foreground {i === safeSelected
              ? 'bg-primary/15'
              : ''}"
            class:sel={i === safeSelected}
            role="option"
            aria-selected={i === safeSelected}
            onclick={() => pick(hit.path)}
            onpointerenter={() => (selectedIndex = i)}
          >
            <span
              class="ficon inline-flex flex-none"
              style={`color:${fileIconColorForPath(hit.path, fileIconPreferences.theme)}`}
            >
              <Icon
                icon={fileIconForPath(hit.path, fileIconPreferences.theme)}
                size={14}
              />
            </span>
            <span class="path min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs">
              {#each pathTokens(hit.path, query) as token, ti (ti)}{#if token.separator}<span class="sep text-muted-foreground">{token.text}</span>{:else if token.marked}<mark class="bg-transparent font-semibold text-primary">{token.text}</mark>{:else}{token.text}{/if}{/each}
            </span>
          </li>
        {/each}
      </ul>
      {#if hits.length > visible.length}
        <p class="more m-0 flex-none border-t border-border px-3 py-[0.3rem] text-center text-[0.6875rem] text-muted-foreground">{hits.length} matches — keep typing to narrow</p>
      {/if}
    {/if}
  </div>
</div>
