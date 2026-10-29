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
  import { initDeepLinks } from "$lib/entry/deeplink";
  import { pickFolder } from "$lib/ipc/client";
  import {
    hideRecentRepos,
    openTab,
    recentRepos,
    restoreSession,
    startTabEvents,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import Toaster from "$lib/components/Toaster.svelte";
  import TabStrip from "$lib/components/TabStrip.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { formatRelativeTime } from "$lib/git/format";
  import iconBrand from "@ktibow/iconset-material-symbols/account-tree";
  import iconAdd from "@ktibow/iconset-material-symbols/add";
  import iconClone from "@ktibow/iconset-material-symbols/download";
  import iconFolder from "@ktibow/iconset-material-symbols/folder-code";
  import iconPin from "@ktibow/iconset-material-symbols/push-pin";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconSearch from "@ktibow/iconset-material-symbols/search";
  import iconBack from "@ktibow/iconset-material-symbols/arrow-back-rounded";
  // Startup: the popout-only views below are dynamically imported inside
  // the popout branch — the main window never renders them, so their
  // module trees (history graph, file-history, popout panels) stay out of
  // the launch-critical graph.
  // M4 F1: panel popout query contract (?panel=diff|history&repo=<id>).
  import { parsePopoutQuery } from "$lib/layout/popout";
  import QuickSwitcher from "$lib/components/QuickSwitcher.svelte";
  import CloneDialog from "$lib/components/CloneDialog.svelte";
  import CommandPalette from "$lib/components/CommandPalette.svelte";
  // M9 F1: global context menu (opened by panels via contextMenu.show).
  import ContextMenu from "$lib/components/menu/ContextMenu.svelte";
  // M4 F2: global keybind engine + palette execution entry point.
  import { startKeybinds } from "$lib/palette/keybinds";
  import { executeCommandById, togglePalette } from "$lib/palette/palette.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import AiHealthChip from "$lib/components/ai/AiHealthChip.svelte";
  import AiSettings from "$lib/components/ai/AiSettings.svelte";
  import SeedColorDialog from "$lib/components/SeedColorDialog.svelte";
  import { setUserSeedColor } from "$lib/theme/dynamic-color";

  /**
   * M4 F1 popout contract: a window opened with
   * `?panel=diff|history&repo=<repoId>` renders only that panel (with its
   * own store wiring) instead of the tab shell. Parsed once at startup.
   */
  const popout = parsePopoutQuery(
    typeof location !== "undefined" ? location.search : "",
  );

  let repos = $state(recentRepos.list());

  /** Home search text (shown once the list is long enough to scan). */
  let repoFilter = $state("");

  /** Repos matching the home filter (name or path, case-insensitive). */
  const filteredRepos = $derived.by(() => {
    const needle = repoFilter.trim().toLowerCase();
    if (needle === "") return repos;
    return repos.filter(
      (repo) =>
        repoName(repo.path).toLowerCase().includes(needle) ||
        repo.path.toLowerCase().includes(needle),
    );
  });

  /** Keep the search field visible while it holds a query. */
  const showRepoFilter = $derived(repos.length > 3 || repoFilter.length > 0);

  /** Clone-dialog visibility (home view's "Clone repo…" button). */
  let cloneOpen = $state(false);

  /** AI settings dialog (opened by the AiHealthChip, M12). */
  let aiSettingsOpen = $state(false);

  /** Accent-seed picker (opened by the palette seed commands, M12). */
  let seedOpen = $state(false);

  /** The focused repository tab, or `null` when nothing is open. */
  const active = $derived(tabStore.active);

  /** Tabs exist (workspace or the recents overlay above it). */
  const tabsOpen = $derived(tabStore.tabs.length > 0);

  /**
   * Launch restore is still resolving the stored active tab: hold the launch
   * view instead of painting the recents list (or a non-active tab) first.
   */
  const launching = $derived(tabStore.restoring && tabStore.activeId === null);

  /** Recents overlay from the top bar; otherwise the tab's workspace. */
  const showWorkspace = $derived(tabsOpen && !tabStore.recentsOpen);

  /** Repository name shown while the last session reopens. */
  const restoreName = $derived(
    tabStore.restoreRoot ? repoName(tabStore.restoreRoot) : null,
  );

  function refreshRepos(): void {
    repos = recentRepos.list();
  }

  function repoName(path: string): string {
    const trimmed = path.replace(/[\\/]+$/, "");
    const segments = trimmed.split(/[\\/]/).filter(Boolean);
    return segments.length > 0 ? segments[segments.length - 1]! : path;
  }

  /**
   * "Opened 2 days ago" for plausible timestamps. Recents are stored in
   * milliseconds, but old/hand-seeded stores may carry seconds; values that
   * cannot be a real recency are omitted instead of printing "56 years ago".
   */
  function lastOpenedLabel(timestamp: number): string | null {
    if (!Number.isFinite(timestamp) || timestamp <= 0) return null;
    const millis = timestamp > 1e12 ? timestamp : timestamp * 1000;
    const age = Date.now() - millis;
    const TEN_YEARS = 10 * 365 * 24 * 60 * 60 * 1000;
    if (age < 0 || age > TEN_YEARS) return null;
    return `Opened ${formatRelativeTime(Math.floor(millis / 1000))}`;
  }

  function togglePin(path: string): void {
    recentRepos.togglePin(path);
    refreshRepos();
  }

  /** Row ✕ button: drop one entry from the recent list (repo on disk untouched). */
  function removeFromRecents(path: string): void {
    recentRepos.remove(path);
    refreshRepos();
    toast(`Removed ${repoName(path)} from recent repositories`, {
      kind: "info",
    });
  }

  /** Home "Add existing repo…" / tab-strip "+": folder picker → open as tab. */
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

    // Mark this webview session booted before hiding the splash: any later
    // reload skips the full-screen overlay (index.html reads this flag).
    try {
      sessionStorage.setItem("mygitui.booted", "1");
    } catch {
      // Best-effort; the splash still fades out below.
    }

    // Fade out the index.html splash (#app-splash) now that the app shell
    // has mounted; runs for the main window and popouts alike.
    const splash = document.getElementById("app-splash");
    if (splash) {
      splash.classList.add("app-splash--done");
      setTimeout(() => splash.remove(), 250);
    }

    void initTheme().catch((err) =>
      console.error("[mygitui] theme init failed:", err),
    );

    startTabEvents();

    // Relaunch polish: reopen the tabs (and the focused one) from the last
    // session. Runs before the CLI-arg/deep-link listeners below, so an
    // explicit `mygitui <path>` still wins the focus when it lands.
    if (!popout) void restoreSession();

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
    cleanups.push(
      onUiEvent("open-clone-dialog", () => {
        cloneOpen = true;
      }),
    );

    // The palette command routes through the typed UI bus (commands.ts must
    // not import the palette store — that would be a registry cycle; the
    // event map in palette/events.ts is pure types, so no cycle there).
    cleanups.push(
      onUiEvent("toggle-command-palette", () => togglePalette()),
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

    // M12: first-launch argv + mygitui://open deep links — same tab-open
    // path as the cli-args listener above (guarded against double-opens).
    void initDeepLinks((path) => openRepoTab(path));

    // M12: the AI health chip opens the settings dialog; the palette seed
    // command opens the accent picker.
    cleanups.push(
      onUiEvent("open-ai-settings", () => {
        aiSettingsOpen = true;
      }),
    );
    cleanups.push(
      onUiEvent("open-seed-dialog", () => {
        seedOpen = true;
      }),
    );

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
        {#await import("$lib/components/panels/HistoryView.svelte") then HistoryViewModule}
          <HistoryViewModule.default repoId={popout.repoId} />
        {/await}
      {:else if popout.panel === "filehistory"}
        {#await import("$lib/components/panels/FileHistoryView.svelte") then FileHistoryViewModule}
          <FileHistoryViewModule.default
            repoId={popout.repoId}
            path={popout.path ?? ""}
          />
        {/await}
      {:else if popout.panel === "workflowrun"}
        {#await import("$lib/components/forge/WorkflowRunView.svelte") then WorkflowRunViewModule}
          <WorkflowRunViewModule.default
            repoId={popout.repoId}
            runId={popout.runId ?? 0}
          />
        {/await}
      {:else if popout.panel === "actionrun"}
        {#await import("$lib/components/actions/ActionRunView.svelte") then ActionRunViewModule}
          <ActionRunViewModule.default
            repoId={popout.repoId}
            runId={popout.actionRunId ?? ""}
          />
        {/await}
      {:else if popout.panel === "commitdetail"}
        {#await import("$lib/components/layout/PopoutCommitDetail.svelte") then PopoutCommitDetailModule}
          <PopoutCommitDetailModule.default
            repoId={popout.repoId}
            sha={popout.sha ?? ""}
          />
        {/await}
      {:else}
        {#await import("$lib/components/layout/PopoutDiff.svelte") then PopoutDiffModule}
          <PopoutDiffModule.default repoId={popout.repoId} />
        {/await}
      {/if}
    </main>
  {:else}
    {#if !launching && tabsOpen}
      <TabStrip openFolder={openFromDisk}>
        {#snippet trailing()}
          <AiHealthChip compact />
          <ThemeToggle />
        {/snippet}
      </TabStrip>
    {/if}

    {#if launching}
      <!-- Launch restore is still opening the stored repo: hold this frame
           instead of flashing the recents list (or a non-active tab). -->
      <main class="launch" aria-busy="true">
        <span class="launch-mark" aria-hidden="true">
          <Icon icon={iconBrand} size={26} />
        </span>
        <span class="launch-title">
          Opening {restoreName ?? "your last workspace"}…
        </span>
        {#if tabStore.restoreRoot}
          <span class="launch-sub" title={tabStore.restoreRoot}>
            {tabStore.restoreRoot}
          </span>
        {/if}
        <span class="launch-spinner" aria-hidden="true"></span>
      </main>
    {:else if showWorkspace}
      <main class="repo-view">
        {#if active}
            {#await import("$lib/components/panels/RepoView.svelte")}
              <div class="repo-loading" aria-busy="true">
                <span class="launch-spinner" aria-hidden="true"></span>
                <span>Loading workspace…</span>
              </div>
            {:then RepoViewModule}
              <RepoViewModule.default
                repoId={active.id}
                name={active.name}
                root={active.root}
                status={active.status}
              />
            {:catch}
              <p class="repo-load-error" role="alert">
                The repository workspace could not be loaded. Restart the app to try again.
              </p>
            {/await}
        {/if}
      </main>
    {:else}
      <main class="home">
        {#if !tabsOpen}
          <header class="home-top">
            <div class="brand">
              <span class="brand-mark" aria-hidden="true">
                <Icon icon={iconBrand} size={20} />
              </span>
              <span class="brand-text">
                <span class="brand-name">mygitui</span>
                <span class="brand-tag">your local git workspace</span>
              </span>
            </div>
            <div class="home-utilities">
              <AiHealthChip />
              <ThemeToggle />
            </div>
          </header>
        {/if}

      <div class="home-body">
        <section class="recent" aria-labelledby="recent-heading">
          <div class="recent-head">
            <div class="recent-title">
              <h1 id="recent-heading" class="recent-heading">
                Recent repositories
              </h1>
              {#if repos.length > 0}
                <p class="recent-sub">
                  {#if repoFilter.trim()}
                    {filteredRepos.length} of {repos.length} repositories
                  {:else}
                    {repos.length}
                    {repos.length === 1 ? "repository" : "repositories"} ·
                    pinned first
                  {/if}
                </p>
              {/if}
            </div>
            {#if tabsOpen || repos.length > 0}
              <div class="head-actions">
                {#if tabsOpen}
                  <button
                    class="back-btn"
                    type="button"
                    onclick={hideRecentRepos}
                  >
                    <Icon icon={iconBack} size={14} />
                    Back to {active?.name ?? "workspace"}
                  </button>
                {/if}
                {#if repos.length > 0}
                  <button
                    class="add-btn"
                    type="button"
                    onclick={() => void openFromDisk()}
                  >
                    <Icon icon={iconAdd} size={14} />
                    Add existing repo…
                  </button>
                  <button
                    class="clone-btn"
                    type="button"
                    onclick={() => (cloneOpen = true)}
                  >
                    <Icon icon={iconClone} size={14} />
                    Clone repo…
                  </button>
                {/if}
              </div>
            {/if}
          </div>

          {#if showRepoFilter}
            <label class="repo-search">
              <span class="search-icon" aria-hidden="true">
                <Icon icon={iconSearch} size={15} />
              </span>
              <input
                type="text"
                aria-label="Filter recent repositories"
                placeholder="Filter repositories by name or path"
                bind:value={repoFilter}
              />
              {#if repoFilter}
                <button
                  class="search-clear"
                  type="button"
                  aria-label="Clear repository filter"
                  title="Clear filter"
                  onclick={() => (repoFilter = "")}
                >
                  <Icon icon={iconClose} size={13} />
                </button>
              {/if}
            </label>
          {/if}

          {#if repos.length === 0}
            <div class="empty">
              <span class="empty-icon" aria-hidden="true">
                <Icon icon={iconFolder} size={30} />
              </span>
              <h2 class="empty-title">No repositories yet</h2>
              <p class="empty-text">
                Open a repository folder from disk, clone a remote, or drag a
                folder onto this window to get started.
              </p>
              <div class="empty-actions">
                <button
                  class="add-btn"
                  type="button"
                  onclick={() => void openFromDisk()}
                >
                  <Icon icon={iconAdd} size={14} />
                  Add existing repo…
                </button>
                <button
                  class="clone-btn"
                  type="button"
                  onclick={() => (cloneOpen = true)}
                >
                  <Icon icon={iconClone} size={14} />
                  Clone repo…
                </button>
              </div>
              <div class="empty-hints">
                <span class="hint">
                  <kbd class="kbd">Ctrl</kbd><kbd class="kbd">K</kbd>
                  quick switcher
                </span>
                <span class="hint">
                  <kbd class="kbd">Ctrl</kbd><kbd class="kbd">Shift</kbd
                  ><kbd class="kbd">P</kbd>
                  command palette
                </span>
              </div>
            </div>
          {:else if filteredRepos.length === 0}
            <div class="no-results">
              <p class="empty-title">No repositories match “{repoFilter}”</p>
              <button
                class="clear-search"
                type="button"
                onclick={() => (repoFilter = "")}
              >
                Clear filter
              </button>
            </div>
          {:else}
            <ul class="repo-list">
              {#each filteredRepos as repo (repo.path)}
                {@const when = lastOpenedLabel(repo.lastOpened)}
                <li class="repo-row-wrapper">
                  <button
                    class="repo-row"
                    type="button"
                    onclick={() => void openRepoTab(repo.path)}
                  >
                    <span class="repo-icon" aria-hidden="true">
                      <Icon icon={iconFolder} size={20} />
                    </span>
                    <span class="repo-main">
                      <span class="repo-title">
                        <span class="repo-name">{repoName(repo.path)}</span>
                        {#if repo.pinned}
                          <span class="pin-mark" title="Pinned">
                            <Icon icon={iconPin} size={13} />
                          </span>
                        {/if}
                      </span>
                      <span class="repo-path">{repo.path}</span>
                    </span>
                    {#if when}
                      <span class="repo-when">{when}</span>
                    {/if}
                  </button>
                  <span class="row-actions">
                    <button
                      class="row-action pin"
                      class:pinned={repo.pinned}
                      type="button"
                      aria-pressed={repo.pinned}
                      title={repo.pinned ? "Unpin repository" : "Pin repository"}
                      aria-label={repo.pinned
                        ? "Unpin repository"
                        : "Pin repository"}
                      onclick={() => togglePin(repo.path)}
                    >
                      <Icon icon={iconPin} size={15} />
                    </button>
                    <button
                      class="row-action remove"
                      type="button"
                      title="Remove from recent list"
                      aria-label={`Remove ${repoName(repo.path)} from the recent list`}
                      onclick={() => removeFromRecents(repo.path)}
                    >
                      <Icon icon={iconClose} size={15} />
                    </button>
                  </span>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>

      <footer class="home-foot">
        <span class="hint">
          <kbd class="kbd">Ctrl</kbd><kbd class="kbd">K</kbd> quick switcher
        </span>
        <span class="hint">
          <kbd class="kbd">Ctrl</kbd><kbd class="kbd">Shift</kbd
          ><kbd class="kbd">P</kbd>
          command palette
        </span>
        <span class="foot-note">Tip: drop a folder here to open it</span>
      </footer>
      </main>
    {/if}
  {/if}
</div>

<Toaster />
<QuickSwitcher />
<CommandPalette />
<CloneDialog bind:open={cloneOpen} />
<ContextMenu />

<!-- M12: AI settings dialog (opened by the AI health chip / palette). -->
<AiSettings bind:open={aiSettingsOpen} />

{#if seedOpen}
  <SeedColorDialog
    onSubmit={(hex) => {
      setUserSeedColor(hex);
      void initTheme();
    }}
    onClear={() => {
      setUserSeedColor(null);
      void initTheme();
    }}
    onCancel={() => (seedOpen = false)}
  />
{/if}

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  /* Per-tab content area */
  .repo-view {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }

  .repo-loading {
    flex: 1;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 0.6rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.875rem;
  }

  .repo-load-error {
    margin: auto;
    color: var(--m3-error, #b3261e);
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

  /* Launch restore: same visual language as the pre-load splash. */
  .launch {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.65rem;
    background:
      radial-gradient(
        circle at 50% -12%,
        color-mix(in srgb, var(--m3-primary) 8%, transparent),
        transparent 46%
      ),
      var(--m3-surface);
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .launch-mark {
    display: grid;
    place-items: center;
    width: 3.25rem;
    height: 3.25rem;
    margin-bottom: 0.35rem;
    border-radius: 0.95rem;
    background: var(--m3-primary-container, var(--m3-surface-container));
    color: var(--m3-on-primary-container, var(--m3-primary));
    box-shadow: var(--m3-elevation-1);
  }

  .launch-title {
    color: var(--m3-on-surface);
    font-size: 0.9375rem;
    font-weight: 500;
  }

  .launch-sub {
    max-width: min(32rem, 80vw);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.75rem;
    opacity: 0.75;
  }

  .launch-spinner {
    width: 1.4rem;
    height: 1.4rem;
    margin-top: 0.35rem;
    border-radius: 50%;
    border: 2.5px solid color-mix(in srgb, var(--m3-primary) 25%, transparent);
    border-top-color: var(--m3-primary);
    animation: launch-spin 0.8s linear infinite;
  }

  @keyframes launch-spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* Home area (no tabs open) */
  .home {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    background:
      radial-gradient(
        circle at 50% -12%,
        color-mix(in srgb, var(--m3-primary) 8%, transparent),
        transparent 46%
      ),
      var(--m3-surface);
  }

  .home-top {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.75rem 1.25rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: color-mix(
      in srgb,
      var(--m3-surface-container-low, var(--m3-surface)) 82%,
      transparent
    );
    backdrop-filter: blur(8px);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    min-width: 0;
  }

  .brand-mark {
    flex: none;
    display: grid;
    place-items: center;
    width: 2.25rem;
    height: 2.25rem;
    border-radius: 0.65rem;
    background: var(--m3-primary-container, var(--m3-surface-container));
    color: var(--m3-on-primary-container, var(--m3-primary));
    box-shadow: var(--m3-elevation-1);
  }

  .brand-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    line-height: 1.15;
  }

  .brand-name {
    font-size: 1.0625rem;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .brand-tag {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .home-utilities {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .home-body {
    flex: 1;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 2.75rem 1.25rem 2rem;
  }

  .recent {
    width: 100%;
    max-width: 44rem;
    min-width: 0;
  }

  .recent-head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .recent-title {
    min-width: 0;
  }

  .recent-heading {
    margin: 0;
    font-size: 1.375rem;
    font-weight: 500;
    letter-spacing: -0.01em;
    color: var(--m3-on-surface);
  }

  .recent-sub {
    margin: 0.2rem 0 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .head-actions {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .add-btn,
  .clone-btn,
  .back-btn,
  .clear-search {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    border-radius: var(--m3-shape-full, 9999px);
    font: inherit;
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.4rem 0.9rem;
    white-space: nowrap;
    cursor: pointer;
  }

  .add-btn,
  .back-btn,
  .clear-search {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: none;
    color: var(--m3-on-surface);
  }

  .add-btn:hover,
  .back-btn:hover,
  .clear-search:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-color: var(--m3-outline, var(--m3-primary));
  }

  .back-btn {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .clear-search {
    margin-top: 0.75rem;
    color: var(--m3-primary);
  }

  .clone-btn {
    border: 1px solid transparent;
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .clone-btn:hover {
    filter: brightness(1.06);
  }

  .add-btn:focus-visible,
  .clone-btn:focus-visible,
  .back-btn:focus-visible,
  .clear-search:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 2px;
  }

  .repo-search {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    height: 2.1rem;
    margin-bottom: 0.75rem;
    padding: 0 0.6rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .repo-search:focus-within {
    border-color: var(--m3-primary);
    box-shadow: 0 0 0 1px var(--m3-primary);
  }

  .search-icon {
    display: inline-flex;
    flex: none;
    opacity: 0.8;
  }

  .repo-search input {
    flex: 1;
    min-width: 0;
    border: none;
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.8125rem;
    outline: none;
  }

  .repo-search input::placeholder {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.8;
  }

  .search-clear {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.25rem;
    height: 1.25rem;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: transparent;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .search-clear:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .search-clear:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
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
    align-items: center;
    gap: 0.125rem;
    padding-right: 0.35rem;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    transition:
      background-color 0.12s ease,
      border-color 0.12s ease;
  }

  .repo-row-wrapper:hover,
  .repo-row-wrapper:focus-within {
    border-color: var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .repo-row {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    text-align: left;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-medium, 12px);
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    padding: 0.55rem 0.7rem;
    cursor: pointer;
  }

  .repo-row:hover {
    border-color: transparent;
    background: none;
  }

  .repo-row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .repo-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 2.5rem;
    height: 2.5rem;
    border-radius: 0.625rem;
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-primary));
  }

  .repo-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .repo-title {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
  }

  .repo-name {
    font-weight: 600;
    font-size: 0.9375rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pin-mark {
    flex: none;
    display: inline-flex;
    color: var(--m3-primary);
  }

  .repo-path {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }

  .repo-when {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
    white-space: nowrap;
  }

  .row-actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.125rem;
    opacity: 0.75;
    transition: opacity 0.12s ease;
  }

  .repo-row-wrapper:hover .row-actions,
  .repo-row-wrapper:focus-within .row-actions {
    opacity: 1;
  }

  .row-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.9rem;
    height: 1.9rem;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-small, 8px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 1;
    cursor: pointer;
    transition:
      background-color 0.12s ease,
      color 0.12s ease,
      opacity 0.12s ease;
  }

  .row-action:hover,
  .row-action:focus-visible {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    opacity: 1;
  }

  .row-action:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .row-action.pinned {
    color: var(--m3-primary);
    opacity: 1;
  }

  .row-action.remove:hover {
    background: var(--m3-error-container, var(--m3-surface-container-high));
    color: var(--m3-error);
  }

  .no-results {
    padding: 2rem 1rem;
    border: 1px dashed var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-large, 16px);
    text-align: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .no-results .empty-title {
    margin-bottom: 0.5rem;
    color: var(--m3-on-surface);
  }

  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.4rem;
    padding: 2.5rem 1.5rem;
    border: 1px dashed var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-large, 28px);
    background: color-mix(
      in srgb,
      var(--m3-surface-container-low, var(--m3-surface)) 65%,
      transparent
    );
    text-align: center;
  }

  .empty-icon {
    display: grid;
    place-items: center;
    width: 3.5rem;
    height: 3.5rem;
    margin-bottom: 0.35rem;
    border-radius: 50%;
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-primary));
  }

  .empty-title {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .empty-text {
    max-width: 26rem;
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
    line-height: 1.5;
  }

  .empty-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.5rem;
    margin-top: 0.75rem;
  }

  .empty-hints {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.75rem 1rem;
    margin-top: 1rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .hint {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    white-space: nowrap;
  }

  .home-foot {
    flex: none;
    display: flex;
    align-items: center;
    gap: 1.25rem;
    padding: 0.65rem 1.25rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: color-mix(
      in srgb,
      var(--m3-surface-container-low, var(--m3-surface)) 82%,
      transparent
    );
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .foot-note {
    margin-left: auto;
    opacity: 0.85;
  }

  @media (max-width: 720px) {
    .brand-tag,
    .foot-note,
    .repo-when {
      display: none;
    }

    .home-body {
      padding-top: 1.75rem;
    }
  }
</style>
