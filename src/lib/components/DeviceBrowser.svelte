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
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

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

<section class="browser flex min-w-0 flex-col gap-2.5" aria-label="Device files">
  <nav
    class="crumbs text-muted-foreground flex flex-wrap items-center gap-1 text-[0.8125rem]"
    aria-label="Device path"
  >
    {#each crumbs as crumb, index (crumb.target)}
      {#if index > 0}
        <span class="crumb-sep opacity-60" aria-hidden="true">/</span>
      {/if}
      {#if index === crumbs.length - 1}
        <span
          class="crumb current cursor-default font-medium text-foreground"
          aria-current="location"
        >
          {crumb.label}
        </span>
      {:else}
        <button
          class="crumb focus-visible:outline-primary hover:bg-muted text-primary max-w-64 cursor-pointer truncate rounded-md border-0 bg-transparent px-1 py-0.5 text-[0.8125rem] outline-none focus-visible:outline-2"
          type="button"
          onclick={() => void browse(crumb.target)}
        >
          {crumb.label}
        </button>
      {/if}
    {/each}
  </nav>

  <div class="toolbar flex flex-wrap items-center gap-2">
    <form
      class="path-jump flex min-w-56 flex-1 gap-2"
      onsubmit={(event) => {
        event.preventDefault();
        onManualSubmit();
      }}
    >
      <Input
        class="input min-w-0 flex-1 font-mono text-[0.8125rem]"
        type="text"
        aria-label="Device path"
        placeholder="Absolute path (empty = home, ~ = home)"
        autocomplete="off"
        spellcheck="false"
        bind:value={manualPath}
      />
      <Button class="secondary" variant="outline" size="xs" type="submit" disabled={loading}>
        Go
      </Button>
    </form>
    <Button
      class="secondary"
      variant="outline"
      size="xs"
      type="button"
      disabled={scanning}
      onclick={() => void scanHome()}
    >
      {scanning ? "Scanning…" : "Scan home"}
    </Button>
  </div>

  {#if error}
    <p class="error text-destructive wrap-anywhere m-0 text-[0.8125rem]" role="alert">
      {error}
    </p>
  {:else if loading}
    <p class="hint text-muted-foreground m-0 text-[0.8125rem]" aria-busy="true">
      Loading…
    </p>
  {:else if listing}
    <ul class="rows m-0 flex list-none flex-col gap-1 p-0" aria-label="Directory contents">
      {#each entries as entry (entry.path)}
        <li class="row-wrapper hover:border-border flex items-center gap-1.5 rounded-xl border border-transparent bg-card">
          {#if entry.kind === "dir"}
            <button
              class="row text-foreground flex w-full min-w-0 flex-1 cursor-pointer items-center gap-2.5 border-0 bg-transparent px-2.5 py-1.5 text-left"
              type="button"
              onclick={() => void browse(entry.path)}
            >
              <span class="row-icon inline-flex flex-none text-primary" aria-hidden="true">
                <Icon icon={iconDir} size={18} />
              </span>
              <span class="row-name truncate min-w-0 text-sm">{entry.name}</span>
            </button>
          {:else}
            <span
              class="row static flex w-full min-w-0 flex-1 items-center gap-2.5 px-2.5 py-1.5"
            >
              <span
                class="row-icon muted text-muted-foreground inline-flex flex-none opacity-70"
                aria-hidden="true"
              >
                <Icon icon={iconDir} size={18} />
              </span>
              <span class="row-name truncate min-w-0 text-sm">{entry.name}</span>
            </span>
          {/if}
        </li>
      {:else}
        <li class="hint text-muted-foreground text-[0.8125rem]">This directory is empty.</li>
      {/each}
    </ul>
    {#if listing.truncated}
      <p class="hint text-muted-foreground m-0 text-[0.8125rem]">
        Listing truncated (too many entries).
      </p>
    {/if}
  {/if}

  {#if candidates !== null}
    <div class="scan mt-2 flex flex-col gap-1.5">
      <h3 class="scan-title m-0 text-sm font-medium text-foreground">
        Repositories found
      </h3>
      <ul class="rows m-0 flex list-none flex-col gap-1 p-0" aria-label="Found repositories">
        {#each candidates as candidate (candidate.path)}
          <li class="row-wrapper hover:border-border flex items-center gap-1.5 rounded-xl border border-transparent bg-card">
            <span class="row static flex w-full min-w-0 flex-1 items-center gap-2.5 px-2.5 py-1.5">
              <span class="row-icon repo inline-flex flex-none text-primary" aria-hidden="true">
                <Icon icon={iconRepo} size={18} />
              </span>
              <span class="row-main flex min-w-0 flex-1 flex-col">
                <span class="row-name truncate min-w-0 text-sm">{candidate.name}</span>
                <span class="row-path text-muted-foreground truncate min-w-0 font-mono text-[0.6875rem]">
                  {candidate.path}
                </span>
              </span>
            </span>
            <Button
              class="secondary open mr-2"
              variant="outline"
              size="xs"
              type="button"
              onclick={() => void openProject(candidate.path)}
            >
              Open as project
            </Button>
          </li>
        {:else}
          <li class="hint text-muted-foreground text-[0.8125rem]">
            No repositories found under the home directory.
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</section>
