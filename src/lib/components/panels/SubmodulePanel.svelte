<script lang="ts">
  /**
   * Submodule panel — inspect and manage git submodules.
   *
   * Rows use the same compact source-control vocabulary as StatusPanel:
   * folder icon + dimmed dir + bold basename, a short status tag ("new
   * commits", "modified", "not initialized"), and hover-revealed actions.
   * The second line keeps the checked-out vs recorded shas and the URL
   * available without opening a detail view.
   *
   * Actions: Update checks the module out at the recorded gitlink (and
   * initializes it when needed), Sync rewrites the URL from `.gitmodules`
   * into the repo config, Open (initialized modules only) opens the module
   * worktree as a repository tab.
   */
  import { submoduleSync, submoduleUpdate, submodules } from "$lib/ipc/client";
  import type { SubmoduleInfo } from "$lib/ipc/types";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { IconButton } from "$lib/components/m3";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { splitPath, submoduleDisplay } from "./statusModel";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconUpdate from "@ktibow/iconset-material-symbols/download";
  import iconSync from "@ktibow/iconset-material-symbols/sync";
  import iconOpen from "@ktibow/iconset-material-symbols/open-in-new";
  import iconModule from "@ktibow/iconset-material-symbols/folder-special";
  import iconCheck from "@ktibow/iconset-material-symbols/check-circle";

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
      error = describeError(err);
    } finally {
      loading = false;
    }
  }

  function describeError(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function fail(what: string, err: unknown): void {
    toast(`${what} failed: ${describeError(err)}`, { kind: "error" });
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

  function title(info: SubmoduleInfo): string {
    const display = submoduleDisplay(info);
    const lines = [info.path, display.detail];
    if (info.url) lines.push(info.url);
    lines.push(
      `HEAD ${shortSha(info.head_sha)} · recorded ${shortSha(info.recorded_sha)}`,
    );
    return lines.join("\n");
  }

  async function onUpdate(info: SubmoduleInfo): Promise<void> {
    if (busy) return;
    busy = true;
    const initialize = init || !info.initialized;
    try {
      await submoduleUpdate(repoId, info.path, initialize, recursive);
      toast(
        initialize
          ? `Initialized submodule ${info.path}`
          : `Updated submodule ${info.path}`,
        { kind: "success" },
      );
      void reload();
      onMutated?.();
    } catch (err) {
      fail("Submodule update", err);
    } finally {
      busy = false;
    }
  }

  async function onSync(info: SubmoduleInfo): Promise<void> {
    if (busy) return;
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
    if (!info.initialized) {
      toast("Submodule is not initialized — update it first", {
        kind: "error",
      });
      return;
    }
    try {
      await openTab(modulePath(info));
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
    <span class="count" title="Submodules in this repository">{list.length}</span>
    <IconButton
      icon={iconRefresh}
      label="Refresh submodules"
      title={loading ? "Refreshing submodules…" : "Refresh submodules"}
      dense
      class={loading ? "spin" : ""}
      disabled={loading}
      onclick={() => void reload()}
    />
  </div>

  {#if error}
    <div class="state error" role="alert">
      <p>{error}</p>
      <button type="button" onclick={() => void reload()}>Retry</button>
    </div>
  {:else if loading && list.length === 0}
    <p class="state" role="status">Loading submodules…</p>
  {:else if list.length === 0}
    <div class="state empty" role="status">
      <span class="empty-icon"><Icon icon={iconCheck} size={22} /></span>
      <p class="empty-title">No submodules</p>
      <p class="empty-hint">Add one with <code>git submodule add</code>.</p>
    </div>
  {:else}
    <ul class="modules" role="list" aria-label="Submodule list" aria-busy={loading}>
      {#each list as info (info.path)}
        {@const display = submoduleDisplay(info)}
        {@const parts = splitPath(info.path)}
        <li class="module" class:danger={display.tone === "danger"}>
          <div class="line">
            <Icon icon={iconModule} size={14} />
            <button
              class="open"
              type="button"
              title={title(info)}
              onclick={() => void onOpen(info)}
            >
              <span class="path">
                {#if parts.dir}<span class="dir">{parts.dir}</span>{/if}
                <span class="base">{parts.base}</span>
              </span>
              {#if display.tag}
                <span class={`tag ${display.tone}`} title={display.detail}>
                  {display.tag}
                </span>
              {/if}
            </button>
            <span class="actions">
              <IconButton
                icon={iconUpdate}
                label={`${info.initialized ? "Update" : "Initialize"} submodule ${info.path}`}
                title={info.initialized
                  ? "Check out the commit the superproject records"
                  : "Initialize and update this submodule"}
                dense
                tabindex={-1}
                disabled={busy}
                onclick={() => void onUpdate(info)}
              />
              <IconButton
                icon={iconSync}
                label={`Sync submodule ${info.path}`}
                title="Rewrite the URL from .gitmodules into the repo config"
                dense
                tabindex={-1}
                disabled={busy}
                onclick={() => void onSync(info)}
              />
              {#if info.initialized}
                <IconButton
                  icon={iconOpen}
                  label={`Open submodule ${info.path} as a tab`}
                  title="Open this submodule as a tab"
                  dense
                  tabindex={-1}
                  onclick={() => void onOpen(info)}
                />
              {/if}
            </span>
          </div>
          <div class="meta" title="Checked-out HEAD vs recorded gitlink">
            {#if updateAvailable(info)}
              <span class="update">↓ update available</span>
            {/if}
            <span class="head">HEAD {shortSha(info.head_sha)}</span>
            <span class="sep">·</span>
            <span>recorded {shortSha(info.recorded_sha)}</span>
            {#if info.url}
              <span class="url" title={info.url}>{info.url}</span>
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
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.4rem;
    flex: none;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
  }

  .spacer {
    flex: 1;
  }

  .count {
    font-size: 0.625rem;
    font-variant-numeric: tabular-nums;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.4rem;
  }

  :global(.spin svg) {
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
    cursor: pointer;
  }

  .toggle input {
    accent-color: var(--m3-primary);
    margin: 0;
    width: 0.8rem;
    height: 0.8rem;
  }

  .toggle:focus-within {
    color: var(--m3-on-surface);
  }

  .state {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 1rem 0.75rem;
    margin: 0;
    font-size: 0.75rem;
  }

  .state.error {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.4rem;
    color: var(--app-deleted, var(--m3-error));
  }

  .state.error p {
    margin: 0;
  }

  .state.error button {
    border: 1px solid var(--app-deleted, var(--m3-error));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--app-deleted-bg, transparent);
    color: var(--app-deleted, var(--m3-error));
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.1rem 0.5rem;
    cursor: pointer;
  }

  .state.empty {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.25rem;
    padding: 2rem 1rem;
    text-align: center;
    user-select: none;
  }

  .empty-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2.5rem;
    height: 2.5rem;
    margin-bottom: 0.2rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    color: var(--app-added, var(--m3-primary));
  }

  .empty-title {
    margin: 0;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .empty-hint {
    margin: 0;
    font-size: 0.72rem;
  }

  .empty-hint code {
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
  }

  .modules {
    list-style: none;
    margin: 0;
    padding: 0 0 0.5rem;
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }

  .module {
    padding: 0.25rem 0.4rem;
    border-bottom: 1px solid color-mix(in srgb, var(--app-border, var(--m3-outline-variant)) 55%, transparent);
  }

  .module:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
  }

  .line {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
  }

  .line > :global(svg) {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    padding: 0;
    text-align: left;
    cursor: pointer;
  }

  .open:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
    border-radius: 2px;
  }

  .path {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.75rem;
  }

  .dir {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.78;
  }

  .base {
    color: var(--m3-on-surface);
    font-weight: 600;
  }

  .tag {
    flex: none;
    max-width: 9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.625rem;
    line-height: 1.35;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .tag.dirty {
    color: var(--app-modified, var(--m3-tertiary));
  }

  .tag.danger,
  .module.danger .base {
    color: var(--app-deleted, var(--m3-error));
  }

  .actions {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.0625rem;
    opacity: 0;
    transition: opacity 0.1s ease;
  }

  .module:hover .actions,
  .module:focus-within .actions {
    opacity: 1;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
    padding-left: 1.25rem;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.625rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.85;
    white-space: nowrap;
  }

  .meta .head {
    color: var(--m3-primary);
  }

  .meta .sep {
    opacity: 0.5;
  }

  .meta .update {
    flex: none;
    color: var(--app-modified, var(--m3-tertiary));
    font-weight: 600;
  }

  .meta .url {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: right;
    opacity: 0.75;
  }

  @media (pointer: coarse) {
    .actions {
      opacity: 1;
    }
  }
</style>
