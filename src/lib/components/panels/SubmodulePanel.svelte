<script lang="ts">
  /**
   * Submodule panel (M4 lane F3) — inspect and manage git submodules.
   *
   * Each row shows the submodule path, its URL, the checked-out vs recorded
   * shas (short form) with an "update available" arrow when the checked-out
   * HEAD differs from the gitlink the superproject records, an initialized
   * badge, and a status chip using git's wording ("new commits", "modified
   * content", "untracked content", "broken" for a module that cannot be
   * opened).
   *
   * Actions: Update brings the module to the recorded gitlink (`git
   * submodule update`, honoring the toolbar init / recursive checkboxes),
   * Sync rewrites the URL from `.gitmodules` into the repo config, and Open
   * (initialized modules only) opens the submodule worktree as a repository
   * tab. Without any submodules the panel shows an empty state.
   */
  import { submoduleSync, submoduleUpdate, submodules } from "$lib/ipc/client";
  import type { SubmoduleInfo } from "$lib/ipc/types";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";

  let {
    repoId,
    root,
    onMutated = undefined,
  }: {
    repoId: string;
    /** This repository's worktree root (join base for "Open as tab"). */
    root: string;
    /** Called after a mutation that changes repo state (update/sync). */
    onMutated?: () => void;
  } = $props();

  let list = $state<SubmoduleInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  /** Any mutation in flight (buttons disabled while true). */
  let busy = $state(false);
  /** Toolbar flags applied by the per-row Update action. */
  let init = $state(false);
  let recursive = $state(false);

  $effect(() => {
    // Reload when the repo switches.
    void repoId;
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      list = await submodules(repoId);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function fail(what: string, err: unknown): void {
    toast(`${what} failed: ${err instanceof Error ? err.message : String(err)}`, {
      kind: "error",
    });
  }

  function shortSha(sha: string | null): string {
    return sha === null || sha === "" ? "—" : sha.slice(0, 7);
  }

  /** head ≠ recorded: the module has commits not captured by the parent. */
  function updateAvailable(info: SubmoduleInfo): boolean {
    return (
      info.initialized &&
      info.head_sha !== null &&
      info.head_sha !== info.recorded_sha
    );
  }

  /** Absolute worktree path of a submodule (portable separator). */
  function modulePath(info: SubmoduleInfo): string {
    return `${root.replace(/[\\/]+$/, "")}/${info.path}`;
  }

  async function onUpdate(info: SubmoduleInfo): Promise<void> {
    busy = true;
    try {
      await submoduleUpdate(repoId, info.path, init, recursive);
      toast(`Updated submodule ${info.path}`, { kind: "success" });
      void reload();
      onMutated?.();
    } catch (err) {
      fail("Submodule update", err);
    } finally {
      busy = false;
    }
  }

  async function onSync(info: SubmoduleInfo): Promise<void> {
    busy = true;
    try {
      await submoduleSync(repoId, info.path);
      toast(`Synced submodule ${info.path}`, { kind: "success" });
      void reload();
    } catch (err) {
      fail("Submodule sync", err);
    } finally {
      busy = false;
    }
  }

  async function onOpen(info: SubmoduleInfo): Promise<void> {
    try {
      await openTab(modulePath(info));
      toast(`Opened ${info.path} in a tab`, { kind: "success" });
    } catch (err) {
      fail("Open", err);
    }
  }
</script>

<aside class="submodule-panel" aria-label="Submodules">
  <div class="toolbar">
    <label class="toggle" title="Also initialize uninitialized submodules">
      <input type="checkbox" bind:checked={init} />
      <span>init</span>
    </label>
    <label class="toggle" title="Update nested submodules too">
      <input type="checkbox" bind:checked={recursive} />
      <span>recursive</span>
    </label>
    <span class="spacer"></span>
    <button class="tb" type="button" onclick={() => void reload()} disabled={loading}>
      {loading ? "Loading…" : "Refresh"}
    </button>
  </div>

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state">No submodules.</p>
  {:else}
    <ul class="modules" role="list" aria-label="Submodule list">
      {#each list as info (info.path)}
        <li class="card">
          <div class="row">
            <span class="name">{info.path}</span>
            <span class="badge" class:off={!info.initialized}>
              {info.initialized ? "initialized" : "not initialized"}
            </span>
            {#if info.status}
              <span class="badge" class:danger={info.status === "broken"}>
                {info.status}
              </span>
            {/if}
          </div>
          {#if info.url}
            <div class="url" title={info.url}>{info.url}</div>
          {/if}
          <div class="shas" title="Checked-out HEAD vs recorded gitlink">
            {#if updateAvailable(info)}
              <span class="arrow" title="Update available">↓</span>
            {/if}
            <span class="head">HEAD {shortSha(info.head_sha)}</span>
            <span class="sep">·</span>
            <span class="recorded">recorded {shortSha(info.recorded_sha)}</span>
          </div>
          <div class="actions">
            <button
              class="tb"
              type="button"
              disabled={busy}
              aria-label={`Update submodule ${info.path}`}
              title="Check out the commit the superproject records"
              onclick={() => void onUpdate(info)}
            >
              Update
            </button>
            <button
              class="tb"
              type="button"
              disabled={busy}
              aria-label={`Sync submodule ${info.path}`}
              title="Rewrite the URL from .gitmodules into the repo config"
              onclick={() => void onSync(info)}
            >
              Sync
            </button>
            {#if info.initialized}
              <button
                class="tb"
                type="button"
                aria-label={`Open submodule ${info.path} as a tab`}
                onclick={() => void onOpen(info)}
              >
                Open
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<style>
  .submodule-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .spacer {
    flex: 1;
  }

  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    cursor: pointer;
  }

  .toggle input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .tb {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .tb:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .state {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 1rem 0.75rem;
    margin: 0;
  }

  .state.error {
    color: var(--m3-error);
  }

  .modules {
    list-style: none;
    margin: 0;
    padding: 0 0.5rem 0.75rem;
    overflow-y: auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .card {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
  }

  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.25rem;
    min-width: 0;
  }

  .name {
    color: var(--m3-on-surface);
    font-weight: 600;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.4rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    white-space: nowrap;
  }

  .badge.off {
    opacity: 0.7;
    border-style: dashed;
  }

  .badge.danger {
    background: var(--m3-error-container, transparent);
    color: var(--m3-on-error-container, var(--m3-error));
    border-color: transparent;
  }

  .url {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.85;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .shas {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .shas .head {
    color: var(--m3-primary);
  }

  .shas .sep {
    opacity: 0.5;
  }

  .shas .arrow {
    color: var(--m3-tertiary);
    font-weight: 700;
  }

  .actions {
    display: flex;
    gap: 0.25rem;
    flex-wrap: wrap;
  }
</style>
