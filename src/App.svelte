<script lang="ts">
  /**
   * mygitui application shell (M1): tab strip over open repositories + a
   * per-tab status summary stub (full panels — working copy, diff, history —
   * land later in M1). With no tabs open, the home area lists recent
   * repositories; clicking one (or dropping a folder / passing CLI paths)
   * opens it as a tab via the tab store.
   */
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { initTheme } from "$lib/theme";
  import { isTauri, listenDragDrop } from "$lib/entry/dragdrop";
  import { pickFolder } from "$lib/ipc/client";
  import {
    openTab,
    recentRepos,
    startTabEvents,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import Toaster from "$lib/components/Toaster.svelte";
  import TabStrip from "$lib/components/TabStrip.svelte";
  import RepoView from "$lib/components/panels/RepoView.svelte";
  import HistoryView from "$lib/components/panels/HistoryView.svelte";
  import PopoutDiff from "$lib/components/layout/PopoutDiff.svelte";
  // M4 F1: panel popout query contract (?panel=diff|history&repo=<id>).
  import { parsePopoutQuery } from "$lib/layout/popout";
  import QuickSwitcher from "$lib/components/QuickSwitcher.svelte";
  import CloneDialog from "$lib/components/CloneDialog.svelte";
  import CommandPalette from "$lib/components/CommandPalette.svelte";
  // M4 F2: global keybind engine + palette execution entry point.
  import { startKeybinds } from "$lib/palette/keybinds";
  import { executeCommandById, togglePalette } from "$lib/palette/palette.svelte";

  /**
   * M4 F1 popout contract: a window opened with
   * `?panel=diff|history&repo=<repoId>` renders only that panel (with its
   * own store wiring) instead of the tab shell. Parsed once at startup.
   */
  const popout = parsePopoutQuery(
    typeof location !== "undefined" ? location.search : "",
  );

  let repos = $state(recentRepos.list());

  /** Clone-dialog visibility (home view's "Clone repo…" button). */
  let cloneOpen = $state(false);

  /** The focused repository tab, or `null` when the home view is showing. */
  const active = $derived(tabStore.active);

  function refreshRepos(): void {
    repos = recentRepos.list();
  }

  function repoName(path: string): string {
    const trimmed = path.replace(/[\\/]+$/, "");
    const segments = trimmed.split(/[\\/]/).filter(Boolean);
    return segments.length > 0 ? segments[segments.length - 1]! : path;
  }

  function togglePin(path: string): void {
    recentRepos.togglePin(path);
    refreshRepos();
  }

  /** "+" button: real folder picker (@tauri-apps/plugin-dialog). */
  async function openFromDisk(): Promise<void> {
    const path = await pickFolder();
    if (path) await openRepoTab(path);
  }

  async function openRepoTab(path: string): Promise<void> {
    try {
      await openTab(path);
      refreshRepos();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast(`Failed to open ${path}: ${message}`, { kind: "error" });
    }
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];

    void initTheme().catch((err) =>
      console.error("[mygitui] theme init failed:", err),
    );

    startTabEvents();

    // Popout windows render one panel only — no shell-level listeners.
    if (popout) {
      return () => {
        for (const unlisten of cleanups) unlisten();
      };
    }

    // M4 F2: global keyboard shortcuts (Ctrl/Cmd+Shift+P palette et al).
    cleanups.push(startKeybinds((commandId) => {
      void executeCommandById(commandId);
    }));

    // Palette "Clone repository…" opens the dialog App owns (real today).
    const onOpenCloneDialog = (): void => {
      cloneOpen = true;
    };
    document.addEventListener("open-clone-dialog", onOpenCloneDialog);
    cleanups.push(() =>
      document.removeEventListener("open-clone-dialog", onOpenCloneDialog),
    );

    // The palette command routes through an event (commands.ts must not
    // import the palette store — that would be a registry cycle).
    const onTogglePalette = (): void => togglePalette();
    window.addEventListener("toggle-command-palette", onTogglePalette);
    cleanups.push(() =>
      window.removeEventListener("toggle-command-palette", onTogglePalette),
    );

    void listenDragDrop((paths) => {
      for (const path of paths) void openRepoTab(path);
    }).then((unlisten) => cleanups.push(unlisten));

    if (isTauri()) {
      void listen<string[]>("cli-args", (event) => {
        const args = Array.isArray(event.payload) ? event.payload : [];
        for (const arg of args) void openRepoTab(arg);
      })
        .then((unlisten) => cleanups.push(unlisten))
        .catch((err) =>
          console.error("[mygitui] cli-args listener failed:", err),
        );
    }

    return () => {
      for (const unlisten of cleanups) unlisten();
    };
  });
</script>

<div class="app-shell">
  {#if popout}
    <!-- M4 F1 popout window: exactly one panel, own store wiring. -->
    <main class="popout-view">
      {#if popout.panel === "history"}
        <HistoryView repoId={popout.repoId} />
      {:else}
        <PopoutDiff repoId={popout.repoId} />
      {/if}
    </main>
  {:else if tabStore.tabs.length > 0}
    <TabStrip openFolder={openFromDisk} />

    <main class="repo-view">
      {#if active}
        <RepoView
          repoId={active.id}
          name={active.name}
          root={active.root}
          status={active.status}
        />
      {/if}
    </main>
  {:else}
    <main class="home">
      <section class="recent" aria-labelledby="recent-heading">
        <div class="recent-head">
          <h1 id="recent-heading" class="recent-heading">Recent repositories</h1>
          <button
            class="clone-btn"
            type="button"
            onclick={() => (cloneOpen = true)}
          >
            Clone repo…
          </button>
        </div>

        {#if repos.length === 0}
          <p class="empty">
            No recent repositories yet. Drag a repository folder onto this
            window, launch <code>mygitui .</code> from a terminal, or clone a
            repository with the button above.
          </p>
        {:else}
          <ul class="repo-list">
            {#each repos as repo (repo.path)}
              <li class="repo-row-wrapper">
                <button
                  class="repo-row"
                  type="button"
                  onclick={() => void openRepoTab(repo.path)}
                >
                  <span class="repo-name">{repoName(repo.path)}</span>
                  <span class="repo-path">{repo.path}</span>
                </button>
                <button
                  class="pin"
                  type="button"
                  aria-pressed={repo.pinned}
                  title={repo.pinned ? "Unpin repository" : "Pin repository"}
                  aria-label={repo.pinned ? "Unpin repository" : "Pin repository"}
                  onclick={() => togglePin(repo.path)}
                >
                  {repo.pinned ? "★" : "☆"}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    </main>
  {/if}
</div>

<Toaster />
<QuickSwitcher />
<CommandPalette />
<CloneDialog bind:open={cloneOpen} />

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  /* Per-tab content area */
  .repo-view {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
  }

  /* M4 F1 popout window: one panel filling the window. */
  .popout-view {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .popout-view > :global(*) {
    flex: 1;
    min-height: 0;
  }

  /* Home area (no tabs open) */
  .home {
    flex: 1;
    overflow-y: auto;
    display: flex;
    justify-content: center;
    padding: 3rem 1rem;
  }

  .recent {
    width: 100%;
    max-width: 40rem;
  }

  .recent-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .recent-head .recent-heading {
    margin: 0;
  }

  .clone-btn {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.8125rem;
    padding: 0.4rem 1rem;
    cursor: pointer;
  }

  .clone-btn:hover,
  .clone-btn:focus-visible {
    filter: brightness(1.1);
  }

  .clone-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }

  .recent-heading {
    font-size: 1.25rem;
    font-weight: 500;
    margin: 0 0 1rem;
    color: var(--m3-on-surface);
  }

  .empty {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    border: 1px dashed var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.75rem;
    padding: 2.5rem 1.5rem;
    text-align: center;
  }

  .empty code {
    font-family: ui-monospace, Consolas, monospace;
  }

  .repo-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .repo-row-wrapper {
    display: flex;
    align-items: stretch;
    gap: 0.25rem;
  }

  .repo-row {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.125rem;
    text-align: left;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.75rem;
    background: var(--m3-surface-container, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    padding: 0.625rem 0.875rem;
    cursor: pointer;
  }

  .repo-row:hover,
  .repo-row:focus-visible {
    border-color: var(--m3-primary);
  }

  .repo-name {
    font-weight: 500;
    font-size: 0.9375rem;
  }

  .repo-path {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    font-family: ui-monospace, Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }

  .pin {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.75rem;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    width: 2.75rem;
    cursor: pointer;
    font-size: 0.875rem;
  }

  .pin[aria-pressed="true"] {
    color: var(--m3-on-primary);
    background: var(--m3-primary);
  }
</style>
