<script lang="ts">
  /**
   * mygitui application shell (M0): tab strip placeholder + home area with
   * recent repositories. Opening repos (tabs, panels, graph) lands in M1+.
   */
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { initTheme } from "$lib/theme";
  import { isTauri, listenDragDrop } from "$lib/entry/dragdrop";
  import { RecentRepoStore } from "$lib/entry/recentRepos";
  import { toast } from "$lib/toast";
  import Toaster from "$lib/components/Toaster.svelte";

  const recent = new RecentRepoStore();
  let repos = $state(recent.list());

  function refreshRepos(): void {
    repos = recent.list();
  }

  function repoName(path: string): string {
    const trimmed = path.replace(/[\\/]+$/, "");
    const segments = trimmed.split(/[\\/]/).filter(Boolean);
    return segments.length > 0 ? segments[segments.length - 1]! : path;
  }

  function openRepo(path: string): void {
    // Placeholder until the repo-open flow lands in M1.
    toast(`Open repository: ${path} (arrives in M1)`);
  }

  function togglePin(path: string): void {
    recent.togglePin(path);
    refreshRepos();
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];

    void initTheme().catch((err) =>
      console.error("[mygitui] theme init failed:", err),
    );

    void listenDragDrop((paths) => {
      for (const path of paths) {
        recent.add(path);
        toast(`Dropped: ${path}`);
      }
      refreshRepos();
    }).then((unlisten) => cleanups.push(unlisten));

    if (isTauri()) {
      void listen<string[]>("cli-args", (event) => {
        const args = Array.isArray(event.payload) ? event.payload : [];
        for (const arg of args) toast(`Arg: ${arg}`);
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
  <header class="tabstrip" aria-label="Open repositories">
    <button class="tab active" type="button" role="tab" aria-selected="true">
      Home
    </button>
    <button
      class="tab-new"
      type="button"
      disabled
      title="Open a repository (arrives in M1)"
      aria-label="Open a repository (arrives in M1)"
    >
      +
    </button>
  </header>

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
                onclick={() => openRepo(repo.path)}
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

  /* Tab strip */
  .tabstrip {
    display: flex;
    align-items: stretch;
    gap: 0.125rem;
    padding: 0.25rem 0.5rem 0;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex: none;
  }

  .tab {
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.875rem;
    padding: 0.5rem 1rem;
    cursor: pointer;
  }

  .tab.active {
    color: var(--m3-primary);
    border-bottom-color: var(--m3-primary);
  }

  .tab-new {
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 1rem;
    line-height: 1;
    padding: 0.5rem 0.75rem;
    cursor: not-allowed;
  }

  /* Home area */
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
