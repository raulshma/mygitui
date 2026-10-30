<script lang="ts">
  /**
   * DeviceBrowser (remote devices) — per-device file browser + repo scan,
   * hosted by the home view's device detail.
   *
   * Browsing: `device_browse_dir` one level at a time (`path` `""` = the
   * device home); entries carry ABSOLUTE device paths, so navigation and
   * the breadcrumb trail are plain string prefixes. A manual path field
   * jumps anywhere (including `~`).
   *
   * "Scan home" runs `device_scan_repos(deviceId, null)`; each candidate
   * opens as a normal project tab via the tab store's remote open — the
   * same code path the recents list uses (`openRemoteTab`).
   */
  import { deviceBrowseDir, deviceScanRepos } from "$lib/ipc/client";
  import type { DeviceRepoCandidate, DirListing } from "$lib/ipc/types";
  import { openRemoteTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import iconDir from "@ktibow/iconset-material-symbols/folder";
  import iconRepo from "@ktibow/iconset-material-symbols/folder-code";

  let { deviceId }: { deviceId: string } = $props();

  /** Current directory (absolute device path; `""` = home). */
  let path = $state("");
  let listing = $state<DirListing | null>(null);
  let loading = $state(false);
  let error = $state("");
  /** Guards against stale listings when navigation changes mid-flight. */
  let browseToken = 0;

  let manualPath = $state("");
  let scanning = $state(false);
  let candidates = $state<DeviceRepoCandidate[] | null>(null);

  /** Breadcrumb trail: home plus every absolute-path segment so far. */
  const crumbs = $derived.by(() => {
    const trail = [{ label: "Home", target: "" }];
    let prefix = "";
    for (const segment of path.split("/").filter(Boolean)) {
      prefix += `/${segment}`;
      trail.push({ label: segment, target: prefix });
    }
    return trail;
  });

  /** Dirs first, then a case-insensitive name sort (stable, testable). */
  const entries = $derived.by(() => {
    const list = [...(listing?.entries ?? [])];
    list.sort((a, b) => {
      const aDir = a.kind === "dir" ? 0 : 1;
      const bDir = b.kind === "dir" ? 0 : 1;
      if (aDir !== bDir) return aDir - bDir;
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    });
    return list;
  });

  /** Browses to `target` (absolute device path; `""` = home). */
  async function browse(target: string): Promise<void> {
    const token = ++browseToken;
    path = target;
    manualPath = target;
    loading = true;
    error = "";
    try {
      const result = await deviceBrowseDir(deviceId, target);
      if (token !== browseToken) return;
      listing = result;
    } catch (err) {
      if (token !== browseToken) return;
      listing = null;
      error = err instanceof Error ? err.message : String(err);
    } finally {
      if (token === browseToken) loading = false;
    }
  }

  function onManualSubmit(): void {
    // `""` (or `~`) means the device home — normalize before navigating.
    const target = manualPath.trim() === "~" ? "" : manualPath.trim();
    void browse(target);
  }

  async function scanHome(): Promise<void> {
    scanning = true;
    error = "";
    try {
      candidates = await deviceScanRepos(deviceId, null);
    } catch (err) {
      candidates = null;
      error = err instanceof Error ? err.message : String(err);
    } finally {
      scanning = false;
    }
  }

  /** Opens a scanned (or browsed) directory as a remote project tab. */
  async function openProject(repoPath: string): Promise<void> {
    try {
      await openRemoteTab(deviceId, repoPath);
    } catch (err) {
      // openRemoteTab keeps offline tabs instead of throwing; this is the
      // belt-and-braces path (e.g. an unknown device id).
      const message = err instanceof Error ? err.message : String(err);
      toast(`Failed to open ${repoPath}: ${message}`, { kind: "error" });
    }
  }

  // Reset + load the home listing when a different device is shown.
  $effect(() => {
    void deviceId;
    candidates = null;
    void browse("");
  });
</script>

<section class="browser" aria-label="Device files">
  <nav class="crumbs" aria-label="Device path">
    {#each crumbs as crumb, index (crumb.target)}
      {#if index > 0}
        <span class="crumb-sep" aria-hidden="true">/</span>
      {/if}
      {#if index === crumbs.length - 1}
        <span class="crumb current" aria-current="location">{crumb.label}</span>
      {:else}
        <button
          class="crumb"
          type="button"
          onclick={() => void browse(crumb.target)}
        >
          {crumb.label}
        </button>
      {/if}
    {/each}
  </nav>

  <div class="toolbar">
    <form
      class="path-jump"
      onsubmit={(event) => {
        event.preventDefault();
        onManualSubmit();
      }}
    >
      <input
        class="input"
        type="text"
        aria-label="Device path"
        placeholder="Absolute path (empty = home, ~ = home)"
        autocomplete="off"
        spellcheck="false"
        bind:value={manualPath}
      />
      <button class="secondary" type="submit" disabled={loading}>
        Go
      </button>
    </form>
    <button
      class="secondary"
      type="button"
      disabled={scanning}
      onclick={() => void scanHome()}
    >
      {scanning ? "Scanning…" : "Scan home"}
    </button>
  </div>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {:else if loading}
    <p class="hint" aria-busy="true">Loading…</p>
  {:else if listing}
    <ul class="rows" aria-label="Directory contents">
      {#each entries as entry (entry.path)}
        <li class="row-wrapper">
          {#if entry.kind === "dir"}
            <button
              class="row"
              type="button"
              onclick={() => void browse(entry.path)}
            >
              <span class="row-icon" aria-hidden="true">
                <Icon icon={iconDir} size={18} />
              </span>
              <span class="row-name">{entry.name}</span>
            </button>
          {:else}
            <span class="row static">
              <span class="row-icon muted" aria-hidden="true">
                <Icon icon={iconDir} size={18} />
              </span>
              <span class="row-name">{entry.name}</span>
            </span>
          {/if}
        </li>
      {:else}
        <li class="hint">This directory is empty.</li>
      {/each}
    </ul>
    {#if listing.truncated}
      <p class="hint">Listing truncated (too many entries).</p>
    {/if}
  {/if}

  {#if candidates !== null}
    <div class="scan">
      <h3 class="scan-title">Repositories found</h3>
      <ul class="rows" aria-label="Found repositories">
        {#each candidates as candidate (candidate.path)}
          <li class="row-wrapper">
            <span class="row static">
              <span class="row-icon repo" aria-hidden="true">
                <Icon icon={iconRepo} size={18} />
              </span>
              <span class="row-main">
                <span class="row-name">{candidate.name}</span>
                <span class="row-path">{candidate.path}</span>
              </span>
            </span>
            <button
              class="secondary open"
              type="button"
              onclick={() => void openProject(candidate.path)}
            >
              Open as project
            </button>
          </li>
        {:else}
          <li class="hint">No repositories found under the home directory.</li>
        {/each}
      </ul>
    </div>
  {/if}
</section>

<style>
  .browser {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    min-width: 0;
  }

  .crumbs {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.2rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .crumb {
    border: none;
    background: none;
    color: var(--m3-primary);
    font: inherit;
    font-size: 0.8125rem;
    padding: 0.1rem 0.25rem;
    border-radius: var(--m3-shape-small, 8px);
    cursor: pointer;
    max-width: 16rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .crumb.current {
    color: var(--m3-on-surface);
    font-weight: 500;
    cursor: default;
  }

  button.crumb:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .crumb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .crumb-sep {
    opacity: 0.6;
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }

  .path-jump {
    flex: 1;
    min-width: 14rem;
    display: flex;
    gap: 0.5rem;
  }

  .input {
    flex: 1;
    min-width: 0;
    border: none;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-highest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.8125rem;
    padding: 0.45rem 0.6rem;
  }

  .input:focus-visible {
    outline: 2px solid var(--m3-primary);
  }

  .secondary {
    flex: none;
    border: 1px solid var(--m3-outline, var(--m3-primary));
    background: none;
    color: var(--m3-primary);
    font: inherit;
    font-size: 0.75rem;
    padding: 0.35rem 0.8rem;
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
    white-space: nowrap;
  }

  .secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }

  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .row-wrapper {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .row-wrapper:hover {
    border-color: var(--m3-outline-variant, var(--m3-primary));
  }

  .row {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    border: none;
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    text-align: left;
    padding: 0.4rem 0.6rem;
    cursor: pointer;
  }

  .row.static {
    cursor: default;
  }

  .row-icon {
    flex: none;
    display: inline-flex;
    color: var(--m3-primary);
  }

  .row-icon.muted {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.7;
  }

  .row-icon.repo {
    color: var(--m3-tertiary, var(--m3-primary));
  }

  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .row-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.875rem;
  }

  .row-path {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.6875rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .open {
    margin-right: 0.5rem;
  }

  .scan {
    margin-top: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .scan-title {
    margin: 0;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .error {
    margin: 0;
    color: var(--m3-error);
    font-size: 0.8125rem;
    overflow-wrap: anywhere;
  }

  .hint {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
  }
</style>
