<script lang="ts">
  /**
   * DeviceBrowser (remote devices) — per-device repo finder + file browser,
   * hosted by the home view's device detail.
   *
   * Two tabs over the same device filesystem:
   * - "Repositories" (default): `device_scan_repos` under the device home;
   *   auto-scans on first activation so the found repos are immediately
   *   visible, with a manual re-scan. Each candidate opens as a normal
   *   project tab via the tab store's remote open — the same code path the
   *   recents list uses (`openRemoteTab`).
   * - "Browse files": `device_browse_dir` one level at a time (`path` `""`
   *   = the device home); entries carry ABSOLUTE device paths, so
   *   navigation and the breadcrumb trail are plain string prefixes. A
   *   manual path field jumps anywhere (including `~`). Clicking a file
   *   opens it in the containing repository's file editor: the workspace
   *   reads/saves (`repo_read_file` / `repo_write_file`) are routed through
   *   the device's remote workspace lane backend-side, so the editor writes
   *   back to the device over SFTP.
   */
  import { deviceBrowseDir, deviceScanRepos } from "$lib/ipc/client";
  import type { DeviceRepoCandidate, DirListing } from "$lib/ipc/types";
  import { openRemoteTab, setActive, sshUri, tabStore } from "$lib/stores/tabs.svelte";
  import { workspaceSession } from "$lib/stores/workspaceSession.svelte";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
  } from "$lib/components/ui/tabs/index.js";
  import { describeError } from "$lib/errors";

  let { deviceId }: { deviceId: string } = $props();

  const iconDir = $derived(appIcon("folder")!);
  const iconRepo = $derived(appIcon("folder-code")!);

  /** Which tab is shown; `"repos"` | `"browse"` (plain string for binding). */
  let activeTab = $state<string>("repos");

  /** Current directory (absolute device path; `""` = home). */
  let path = $state("");
  let listing = $state<DirListing | null>(null);
  let loading = $state(false);
  let browseError = $state("");
  /** Guards against stale listings when navigation changes mid-flight. */
  let browseToken = 0;

  let manualPath = $state("");
  let scanning = $state(false);
  /** Scan results; `null` = never scanned (for the current device). */
  let candidates = $state<DeviceRepoCandidate[] | null>(null);
  let scanError = $state("");

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
    browseError = "";
    try {
      const result = await deviceBrowseDir(deviceId, target);
      if (token !== browseToken) return;
      listing = result;
    } catch (err) {
      if (token !== browseToken) return;
      listing = null;
      browseError = describeError(err);
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
    scanError = "";
    try {
      candidates = await deviceScanRepos(deviceId, null);
    } catch (err) {
      candidates = null;
      scanError = describeError(err);
    } finally {
      scanning = false;
    }
  }

  // Auto-scan the first time the Repositories tab is shown (the device
  // reset below clears `candidates`, so this fires once per device; a
  // failed scan leaves `scanError` set, which keeps the effect from
  // re-triggering in a loop — the user retries manually).
  $effect(() => {
    if (activeTab === "repos" && candidates === null && !scanning && !scanError) {
      void scanHome();
    }
  });

  /** Opens a scanned (or browsed) directory as a remote project tab. */
  async function openProject(repoPath: string): Promise<void> {
    try {
      await openRemoteTab(deviceId, repoPath);
    } catch (err) {
      // openRemoteTab keeps offline tabs instead of throwing; this is the
      // belt-and-braces path (e.g. an unknown device id).
      const message = describeError(err);
      toast(`Failed to open ${repoPath}: ${message}`, { kind: "error" });
    }
  }

  /**
   * Opens a browsed device file in its repository's file editor (item 60).
   *
   * Remote repos ARE writable through the workspace lane: the backend runs
   * `repo_read_file` / `repo_write_file` against the device's
   * `RemoteWorkspace` (SFTP) for repos with a remote handle — so the normal
   * FilesPanel editor flow works unchanged on remote repositories.
   *
   * Routing: the file must live inside an OPEN remote repo tab (longest
   * root wins) or a scanned repo candidate; the workspace session is seeded
   * with the repo-relative path so the (re)mounted / switched RepoView
   * restores it through the standard editor flow — no events to race a
   * freshly-mounted workspace. Without a containing repo the click stays
   * read-only guidance (workspace commands need a repo context).
   */
  async function openRemoteFile(entryPath: string): Promise<void> {
    // Entries carry absolute device paths; tab roots are the `ssh://` URIs.
    const uri = sshUri(deviceId, entryPath);
    const open = tabStore.tabs
      .filter(
        (tab) =>
          tab.deviceId === deviceId &&
          tab.openError === null &&
          uri.startsWith(`${tab.root}/`),
      )
      .sort((a, b) => b.root.length - a.root.length)[0] ?? null;
    const scanned = (candidates ?? [])
      .filter((candidate) => candidate.path !== "" && entryPath.startsWith(`${candidate.path}/`))
      .sort((a, b) => b.path.length - a.path.length)[0] ?? null;
    const root = open?.root ?? (scanned !== null ? sshUri(deviceId, scanned.path) : null);
    const relative = open !== null
      ? uri.slice(open.root.length + 1)
      : scanned !== null
        ? entryPath.slice(scanned.path.length + 1)
        : null;
    if (root === null || relative === null || relative === "") {
      toast("Open the repository containing this file to edit its files.", {
        kind: "info",
      });
      return;
    }
    // Seed the workspace session (merged, never clobbering the repo's other
    // open files): the switch/mount restore opens the file and makes it the
    // active editor tab.
    const session = workspaceSession.get(root);
    const openFiles = session?.openFiles ?? [];
    workspaceSession.patch(root, {
      openFiles: openFiles.includes(relative) ? openFiles : [...openFiles, relative],
      activeFile: relative,
      centerKind: "files",
    });
    if (open !== null) {
      setActive(open.id);
    } else {
      await openProject(scanned!.path);
    }
  }

  // Reset + load the home listing when a different device is shown.
  $effect(() => {
    void deviceId;
    activeTab = "repos";
    candidates = null;
    scanError = "";
    void browse("");
  });
</script>

<section class="browser flex min-w-0 flex-col gap-2.5" aria-label="Device repositories">
  <Tabs bind:value={activeTab} class="flex min-w-0 flex-col gap-2.5">
    <TabsList class="flex-none">
      <TabsTrigger value="repos">Repositories</TabsTrigger>
      <TabsTrigger value="browse">Browse files</TabsTrigger>
    </TabsList>

    <!-- Repositories: scan results with open actions. -->
    <TabsContent value="repos" class="flex min-w-0 flex-col gap-1.5">
      <div class="scan-head flex flex-wrap items-center gap-2">
        {#if scanning}
          <p class="hint text-muted-foreground m-0 text-ui-base" aria-busy="true">
            Scanning the home directory…
          </p>
        {:else if candidates !== null}
          <p class="hint text-muted-foreground m-0 text-ui-base">
            {candidates.length}
            {candidates.length === 1 ? "repository" : "repositories"} under
            the home directory
          </p>
        {/if}
        <Button
          class="secondary rescan ml-auto"
          variant="outline"
          size="xs"
          type="button"
          disabled={scanning}
          onclick={() => void scanHome()}
        >
          {candidates === null && !scanning ? "Scan home" : "Rescan"}
        </Button>
      </div>

      {#if scanError}
        <p class="error text-destructive wrap-anywhere m-0 text-ui-base" role="alert">
          {scanError}
        </p>
      {:else if scanning}
        <ul class="rows m-0 flex list-none flex-col gap-1 p-0" aria-hidden="true">
          <li class="row-wrapper flex items-center gap-1.5 rounded-xl border border-transparent bg-card opacity-60">
            <span class="row static flex w-full min-w-0 flex-1 items-center gap-2.5 px-2.5 py-1.5">
              <span class="row-icon inline-flex flex-none text-primary-ink" aria-hidden="true">
                <Icon icon={iconRepo} size={18} />
              </span>
              <span class="row-name text-muted-foreground truncate min-w-0 text-sm">
                Looking for repositories…
              </span>
            </span>
          </li>
        </ul>
      {:else if candidates !== null}
        <ul class="rows m-0 flex list-none flex-col gap-1 p-0" aria-label="Found repositories">
          {#each candidates as candidate (candidate.path)}
            <li class="row-wrapper hover:border-border flex items-center gap-1.5 rounded-xl border border-transparent bg-card">
              <span class="row static flex w-full min-w-0 flex-1 items-center gap-2.5 px-2.5 py-1.5">
                <span class="row-icon repo inline-flex flex-none text-primary-ink" aria-hidden="true">
                  <Icon icon={iconRepo} size={18} />
                </span>
                <span class="row-main flex min-w-0 flex-1 flex-col">
                  <span class="row-name truncate min-w-0 text-sm">{candidate.name}</span>
                  <span class="row-path text-muted-foreground truncate min-w-0 font-mono text-ui-micro">
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
            <li class="hint text-muted-foreground text-ui-base">
              No repositories found under the home directory.
            </li>
          {/each}
        </ul>
      {/if}
    </TabsContent>

    <!-- Browse files: breadcrumb + path jump + one-level listing. -->
    <TabsContent value="browse" class="flex min-w-0 flex-col gap-2.5">
      <nav
        class="crumbs text-muted-foreground flex flex-wrap items-center gap-1 text-ui-base"
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
              class="crumb focus-visible:outline-primary hover:bg-muted text-primary-ink max-w-64 cursor-pointer truncate rounded-md border-0 bg-transparent px-1 py-0.5 text-ui-base outline-none focus-visible:outline-2"
              type="button"
              onclick={() => void browse(crumb.target)}
            >
              {crumb.label}
            </button>
          {/if}
        {/each}
      </nav>

      <form
        class="path-jump flex flex-wrap items-center gap-2"
        onsubmit={(event) => {
          event.preventDefault();
          onManualSubmit();
        }}
      >
        <Input
          class="input min-w-56 flex-1 font-mono text-ui-base"
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

      {#if browseError}
        <p class="error text-destructive wrap-anywhere m-0 text-ui-base" role="alert">
          {browseError}
        </p>
      {:else if loading}
        <p class="hint text-muted-foreground m-0 text-ui-base" aria-busy="true">
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
                  <span class="row-icon inline-flex flex-none text-primary-ink" aria-hidden="true">
                    <Icon icon={iconDir} size={18} />
                  </span>
                  <span class="row-name truncate min-w-0 text-sm">{entry.name}</span>
                </button>
              {:else}
                <button
                  class="row text-foreground flex w-full min-w-0 flex-1 cursor-pointer items-center gap-2.5 border-0 bg-transparent px-2.5 py-1.5 text-left"
                  type="button"
                  title="Open in the repository's file editor"
                  onclick={() => void openRemoteFile(entry.path)}
                >
                  <span
                    class="row-icon muted text-muted-foreground inline-flex flex-none opacity-70"
                    aria-hidden="true"
                  >
                    <Icon icon={iconDir} size={18} />
                  </span>
                  <span class="row-name truncate min-w-0 text-sm">{entry.name}</span>
                </button>
              {/if}
            </li>
          {:else}
            <li class="hint text-muted-foreground text-ui-base">This directory is empty.</li>
          {/each}
        </ul>
        {#if listing.truncated}
          <p class="hint text-muted-foreground m-0 text-ui-base">
            Listing truncated (too many entries).
          </p>
        {/if}
      {/if}
    </TabsContent>
  </Tabs>
</section>
