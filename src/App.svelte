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
  import {
    openTab,
    recentRepos,
    startTabEvents,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import type { RepoStatus } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import Toaster from "$lib/components/Toaster.svelte";
  import TabStrip from "$lib/components/TabStrip.svelte";

  let repos = $state(recentRepos.list());

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

  /** "+" button: real folder picker (@tauri-apps/plugin-dialog) lands M1.1. */
  function openFromDisk(): void {
    toast(
      "Folder picker lands in M1.1 — drop a folder or pick a recent repo for now.",
    );
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

  function branchLabel(status: RepoStatus): string {
    if (status.branch) return status.branch;
    if (status.detached && status.head) return `detached @ ${status.head.slice(0, 7)}`;
    return status.detached ? "detached" : "unknown";
  }

  function changedFiles(status: RepoStatus): number {
    return status.entries.filter(
      (entry) => entry.index !== "unmodified" || entry.worktree !== "unmodified",
    ).length;
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];

    void initTheme().catch((err) =>
      console.error("[mygitui] theme init failed:", err),
    );

    startTabEvents();

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
  {#if tabStore.tabs.length > 0}
    <TabStrip openFolder={openFromDisk} />

    <main class="repo-view">
      {#if active}
        <section class="status-summary" aria-label="Repository status summary">
          <header class="repo-header">
            <h1 class="repo-title">{active.name}</h1>
            <p class="repo-root">{active.root}</p>
          </header>

          {#if active.status}
            <dl class="facts">
              <div class="fact">
                <dt>Branch</dt>
                <dd class="fact-value">{branchLabel(active.status)}</dd>
              </div>
              <div class="fact">
                <dt>Ahead</dt>
                <dd class="fact-value">↑ {active.status.ahead}</dd>
              </div>
              <div class="fact">
                <dt>Behind</dt>
                <dd class="fact-value">↓ {active.status.behind}</dd>
              </div>
              <div class="fact">
                <dt>Changed files</dt>
                <dd class="fact-value">{changedFiles(active.status)}</dd>
              </div>
            </dl>
            <p class="stub">
              Working copy, diff and history views land in M1.
            </p>
          {:else}
            <p class="loading">Loading repository status…</p>
          {/if}
        </section>
      {/if}
    </main>
  {:else}
    <main class="home">
      <section class="recent" aria-labelledby="recent-heading">
        <h1 id="recent-heading" class="recent-heading">Recent repositories</h1>

        {#if repos.length === 0}
          <p class="empty">
            No recent repositories yet. Drag a repository folder onto this
            window, or launch <code>mygitui .</code> from a terminal.
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

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  /* Per-tab content area (M1 status summary stub) */
  .repo-view {
    flex: 1;
    overflow-y: auto;
    display: flex;
    justify-content: center;
    padding: 2.5rem 1rem;
  }

  .status-summary {
    width: 100%;
    max-width: 40rem;
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }

  .repo-header {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .repo-title {
    font-size: 1.375rem;
    font-weight: 500;
    margin: 0;
    color: var(--m3-on-surface);
  }

  .repo-root {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    font-family: ui-monospace, Consolas, monospace;
    overflow-wrap: anywhere;
  }

  .facts {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(9rem, 1fr));
    gap: 0.75rem;
  }

  .fact {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.75rem;
    background: var(--m3-surface-container, var(--m3-surface));
    padding: 0.75rem 1rem;
  }

  .fact dt {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .fact-value {
    margin: 0;
    font-size: 1rem;
    font-weight: 500;
    color: var(--m3-primary);
  }

  .stub,
  .loading {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    border: 1px dashed var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.75rem;
    padding: 1.25rem 1.5rem;
    text-align: center;
    margin: 0;
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
