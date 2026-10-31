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
  import { appearance } from "$lib/theme/appearance.svelte";
  import { startMenuColor } from "$lib/theme/menu-color";
  import { startIconSets } from "$lib/components/icons/library.svelte";
  import { isTauri, listenDragDrop } from "$lib/entry/dragdrop";
  import { initDeepLinks } from "$lib/entry/deeplink";
  import { pickFolder, remotePollSetPaused } from "$lib/ipc/client";
  import {
    hideRecentRepos,
    openTab,
    recentRepos,
    restoreSession,
    retryRemoteTab,
    startTabEvents,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import {
    deviceInputFromProfile,
    remoteDevices,
    startRemoteDeviceEvents,
    PROMPT_TIMEOUT_MS,
  } from "$lib/stores/remoteDevices.svelte";
  import { toast } from "$lib/toast";
  import Toaster from "$lib/components/Toaster.svelte";
  import TabStrip from "$lib/components/TabStrip.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Input } from "$lib/components/ui/input/index.js";
  import { formatRelativeTime } from "$lib/git/format";
  import iconBrand from "@ktibow/iconset-material-symbols/account-tree";
  import iconAdd from "@ktibow/iconset-material-symbols/add";
  import iconClone from "@ktibow/iconset-material-symbols/download";
  import iconFolder from "@ktibow/iconset-material-symbols/folder-code";
  import iconPin from "@ktibow/iconset-material-symbols/push-pin";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconSearch from "@ktibow/iconset-material-symbols/search";
  import iconBack from "@ktibow/iconset-material-symbols/arrow-back-rounded";
  import iconDevices from "@ktibow/iconset-material-symbols/devices";
  import iconEdit from "@ktibow/iconset-material-symbols/edit";
  // Startup: the popout-only views below are dynamically imported inside
  // the popout branch — the main window never renders them, so their
  // module trees (history graph, file-history, popout panels) stay out of
  // the launch-critical graph.
  // M4 F1: panel popout query contract (?panel=diff|history&repo=<id>).
  import { parsePopoutQuery } from "$lib/layout/popout";
  // M4 F2: global keybind engine + palette execution entry point.
  import { startKeybinds } from "$lib/palette/keybinds";
  import { executeCommandById, isPaletteOpen, togglePalette } from "$lib/palette/palette.svelte";
  import { isSwitcherOpen } from "$lib/stores/switcher.svelte";
  import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import AiHealthChip from "$lib/components/ai/AiHealthChip.svelte";
  import DeviceBrowser from "$lib/components/DeviceBrowser.svelte";
  import type { DeviceProfile } from "$lib/ipc/types";

  /**
   * Overlay lazy-mounting: the dialogs below mount on first need instead of
   * at launch, keeping the launch-critical module graph small. Each holds
   * its own module-chunk promise; once an overlay has opened, it stays
   * mounted (bind:open just toggles), so close animations keep playing and
   * reopen is instant. Chunks warm one per idle slot after mount, so even
   * the first open skips the fetch wait.
   */

  /** Runs cb when the main thread is idle (setTimeout everywhere else). */
  function onIdle(cb: () => void, timeout = 2000): void {
    if (typeof requestIdleCallback === "function") {
      requestIdleCallback(() => cb(), { timeout });
    } else {
      setTimeout(cb, 1200);
    }
  }

  /**
   * One lazy overlay: its chunk is imported on first `chunk()` call (open
   * signal or warm-up) and cached, and `landed` stays true from then on so
   * a closed overlay stays mounted (close animations, instant reopen).
   */
  function lazyOverlay<T>(load: () => Promise<T>) {
    let promise: Promise<T> | null = null;
    let landed = $state(false);
    return {
      chunk: (): Promise<T> => {
        landed = true;
        return (promise ??= load());
      },
      get landed(): boolean {
        return landed;
      },
    };
  }

  const switcherOverlay = lazyOverlay(() => import("$lib/components/QuickSwitcher.svelte"));
  const paletteOverlay = lazyOverlay(() => import("$lib/components/CommandPalette.svelte"));
  const contextMenuOverlay = lazyOverlay(
    () => import("$lib/components/menu/ContextMenu.svelte"),
  );
  const cloneOverlay = lazyOverlay(() => import("$lib/components/CloneDialog.svelte"));
  const aiSettingsOverlay = lazyOverlay(() => import("$lib/components/ai/AiSettings.svelte"));
  const addDeviceOverlay = lazyOverlay(() => import("$lib/components/AddDeviceDialog.svelte"));
  const hostKeyOverlay = lazyOverlay(() => import("$lib/components/HostKeyDialog.svelte"));
  const deviceAuthOverlay = lazyOverlay(() => import("$lib/components/DeviceAuthDialog.svelte"));
  const confirmOverlay = lazyOverlay(() => import("$lib/components/safety/ConfirmDialog.svelte"));
  const appearanceOverlay = lazyOverlay(
    () => import("$lib/components/AppearanceDialog.svelte"),
  );

  /** Overlay chunks fetched one per idle slot, launch graph untouched. */
  const overlayWarmers: Array<() => Promise<unknown>> = [
    switcherOverlay.chunk,
    paletteOverlay.chunk,
    contextMenuOverlay.chunk,
    cloneOverlay.chunk,
    aiSettingsOverlay.chunk,
    addDeviceOverlay.chunk,
    hostKeyOverlay.chunk,
    deviceAuthOverlay.chunk,
    confirmOverlay.chunk,
    appearanceOverlay.chunk,
  ];
  function warmNextOverlay(): void {
    const next = overlayWarmers.shift();
    if (!next) return;
    void next()
      .catch(() => {})
      .then(() => onIdle(warmNextOverlay));
  }

  /**
   * M4 F1 popout contract: a window opened with
   * `?panel=diff|history&repo=<repoId>` renders only that panel (with its
   * own store wiring) instead of the tab shell. Parsed once at startup.
   */
  const popout = parsePopoutQuery(
    typeof location !== "undefined" ? location.search : "",
  );

  // Customizer (shadcn style/base/theme/fonts/...): storage → DOM during
  // component init — before any paint — so a persisted appearance never
  // flashes the defaults. Must never break startup; failures log and move
  // on. (Menu/icon wiring follows in onMount below.)
  try {
    appearance.init();
  } catch (err) {
    console.error("[mygitui] appearance init failed:", err);
  }

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

  /** Appearance customizer (opened by the palette / ThemeToggle menu). */
  let appearanceOpen = $state(false);

  // --- Remote devices (SSH) -------------------------------------------------

  /** The device whose detail view (browser + settings) the home shows. */
  let deviceDetailId = $state<string | null>(null);
  /** Add/edit-device dialog visibility + the device being edited. */
  let addDeviceOpen = $state(false);
  let editingDevice = $state<DeviceProfile | null>(null);
  /** Delete-device confirmation. */
  let removeDeviceOpen = $state(false);
  /** Draft of the detail view's poll-interval field (seconds). */
  let pollDraft = $state(5);

  /** The focused device entry, or `null` when the list is shown. */
  const deviceDetail = $derived(
    deviceDetailId !== null ? remoteDevices.byId(deviceDetailId) : null,
  );

  function openDeviceDetail(deviceId: string): void {
    deviceDetailId = deviceId;
    pollDraft = remoteDevices.byId(deviceId)?.profile.poll_secs ?? 5;
  }

  function openAddDevice(): void {
    editingDevice = null;
    addDeviceOpen = true;
  }

  function openEditDevice(device: DeviceProfile): void {
    editingDevice = device;
    addDeviceOpen = true;
  }

  /** Persists the detail view's poll-interval draft via device_update. */
  async function savePollInterval(): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile) return;
    const seconds = Math.max(1, Math.floor(pollDraft) || 1);
    try {
      await remoteDevices.update({
        ...deviceInputFromProfile(profile),
        poll_secs: seconds,
      });
      toast(`Poll interval for ${profile.name} set to ${seconds}s`, {
        kind: "success",
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast(`Failed to update ${profile.name}: ${message}`, { kind: "error" });
    }
  }

  async function forgetHostKey(): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile) return;
    const had = await remoteDevices.forgetHostKey(profile.id);
    if (had) {
      toast(`Host key for ${profile.name} forgotten — next connect re-prompts`);
    } else {
      toast(`${profile.name} has no pinned host key`);
    }
  }

  async function removeDevice(): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile) return;
    try {
      await remoteDevices.remove(profile.id);
      deviceDetailId = null;
      toast(`Removed device ${profile.name}`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast(`Failed to remove ${profile.name}: ${message}`, { kind: "error" });
    }
  }

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

  /** Home "Open repository…" / tab-strip "+": folder picker → open as tab. */
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

    // Customizer followers: menu surfaces + the icon set resolver. (The
    // config itself applied before paint; see the init above.)
    try {
      startMenuColor();
      startIconSets();
    } catch (err) {
      console.error("[mygitui] appearance init failed:", err);
    }

    startTabEvents();

    // Remote devices: registry + prompt/status event wiring (idempotent),
    // plus pruning of prompts the backend already timed out (120 s).
    startRemoteDeviceEvents();
    void remoteDevices.load();
    const pruneTimer = setInterval(
      () => remoteDevices.pruneStale(),
      Math.floor(PROMPT_TIMEOUT_MS / 4),
    );
    cleanups.push(() => clearInterval(pruneTimer));

    // Remote status polling is "paused when tab hidden": mirror the
    // document's visibility to the backend pollers (popouts render one
    // panel only and never own this wiring).
    if (!popout && isTauri()) {
      const onVisibility = () => {
        void remotePollSetPaused(document.visibilityState === "hidden");
      };
      document.addEventListener("visibilitychange", onVisibility);
      cleanups.push(() => {
        document.removeEventListener("visibilitychange", onVisibility);
        // Leaving the page must never leave the pollers paused.
        void remotePollSetPaused(false);
      });
    }

    // Relaunch polish: reopen the tabs (and the focused one) from the last
    // session. Runs before the CLI-arg/deep-link listeners below, so an
    // explicit `mygitui <path>` still wins the focus when it lands.
    if (!popout) {
      void restoreSession();
      // Overlay dialogs mount lazily (see the lazy-mounting note); warm
      // their chunks once the launch settles.
      warmNextOverlay();
    }

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

    // M12: the AI health chip opens the settings dialog.
    cleanups.push(
      onUiEvent("open-ai-settings", () => {
        aiSettingsOpen = true;
      }),
    );

    cleanups.push(
      onUiEvent("open-appearance", () => {
        appearanceOpen = true;
      }),
    );

    return () => {
      for (const unlisten of cleanups) unlisten();
    };
  });
</script>

<div class="app-shell flex h-full flex-col overflow-hidden bg-background text-foreground">
  {#if popout}
    <!-- M4 F1 popout window: exactly one panel, own store wiring. -->
    <main class="popout-view flex min-h-0 flex-1 [&>*]:min-h-0 [&>*]:flex-1">
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
      <main
        class="launch flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.65rem] bg-[radial-gradient(circle_at_50%_-12%,color-mix(in_srgb,var(--primary)_8%,transparent),transparent_46%)] bg-background text-muted-foreground"
        aria-busy="true"
      >
        <span
          class="launch-mark mb-[0.35rem] grid size-[3.25rem] place-items-center rounded-[0.95rem] bg-primary/10 text-primary shadow-sm"
          aria-hidden="true"
        >
          <Icon icon={iconBrand} size={26} />
        </span>
        <span class="launch-title text-[0.9375rem] font-medium text-foreground">
          Opening {restoreName ?? "your last workspace"}…
        </span>
        {#if tabStore.restoreRoot}
          <span
            class="launch-sub max-w-[min(32rem,80vw)] overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs opacity-75"
            title={tabStore.restoreRoot}
          >
            {tabStore.restoreRoot}
          </span>
        {/if}
        <span
          class="launch-spinner mt-[0.35rem] size-[1.4rem] animate-spin rounded-full border-[2.5px] border-primary/25 border-t-primary"
          aria-hidden="true"
        ></span>
      </main>
    {:else if showWorkspace}
      {#if active?.openError}
        <!-- Remote tab whose device is unknown/offline (or whose path
             vanished): keep the tab, offer a Retry of the open. -->
        <main
          class="repo-offline flex min-h-0 flex-1 flex-col items-center justify-center gap-2.5 p-8 text-center text-muted-foreground"
        >
          <span
            class="offline-icon mb-[0.35rem] grid size-[3.25rem] place-items-center rounded-[0.95rem] bg-muted text-muted-foreground shadow-sm"
            aria-hidden="true"
          >
            <Icon icon={iconDevices} size={26} />
          </span>
          <h2 class="offline-title m-0 text-base font-medium text-foreground">
            {active.name} is not reachable
          </h2>
          <span
            class="offline-path max-w-[min(32rem,80vw)] overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs opacity-75"
            title={active.root}>{active.root}</span
          >
          <p
            class="offline-msg mb-2 max-w-[min(32rem,80vw)] text-[0.8125rem] [overflow-wrap:anywhere]"
            role="alert"
          >
            {active.openError}
          </p>
          <button
            class="add-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-transparent bg-primary px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-primary-foreground hover:brightness-110 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
            type="button"
            onclick={() => void retryRemoteTab(active.root)}
          >
            <Icon icon={iconAdd} size={14} />
            Retry
          </button>
        </main>
      {:else}
        <main class="repo-view flex min-h-0 min-w-0 flex-1 overflow-hidden">
          {#if active}
              {#await import("$lib/components/panels/RepoView.svelte")}
                <div
                  class="repo-loading grid flex-1 place-content-center justify-items-center gap-2.5 text-sm text-muted-foreground"
                  aria-busy="true"
                >
                  <span
                    class="launch-spinner size-[1.4rem] animate-spin rounded-full border-[2.5px] border-primary/25 border-t-primary"
                    aria-hidden="true"
                  ></span>
                  <span>Loading workspace…</span>
                </div>
              {:then RepoViewModule}
                <RepoViewModule.default
                  repoId={active.id}
                  name={active.name}
                  root={active.root}
                  status={active.status}
                  deviceId={active.deviceId}
                />
              {:catch}
                <p
                  class="repo-load-error m-auto text-destructive"
                  role="alert"
                >
                  The repository workspace could not be loaded. Restart the app to try again.
                </p>
              {/await}
          {/if}
        </main>
      {/if}
    {:else}
      <main
        class="home flex min-h-0 flex-1 flex-col overflow-y-auto bg-[radial-gradient(circle_at_50%_-12%,color-mix(in_srgb,var(--primary)_8%,transparent),transparent_46%)] bg-background"
      >
        {#if !tabsOpen}
          <header
            class="home-top flex flex-none items-center justify-between gap-4 border-b border-border bg-muted/80 px-5 py-3 backdrop-blur-[8px]"
          >
            <div class="brand flex min-w-0 items-center gap-2.5">
              <span
                class="brand-mark grid size-9 flex-none place-items-center rounded-[0.65rem] bg-primary/10 text-primary shadow-sm"
                aria-hidden="true"
              >
                <Icon icon={iconBrand} size={20} />
              </span>
              <span class="brand-text flex min-w-0 flex-col leading-[1.15]">
                <span
                  class="brand-name text-[1.0625rem] font-semibold tracking-[-0.01em]"
                  >mygitui</span
                >
                <span
                  class="brand-tag max-[720px]:hidden text-[0.6875rem] text-muted-foreground"
                  >your local git workspace</span
                >
              </span>
            </div>
            <div class="home-utilities flex flex-none items-center gap-1.5">
              <AiHealthChip />
              <ThemeToggle />
            </div>
          </header>
        {/if}

      <div
        class="home-body flex flex-1 flex-col items-center gap-9 px-5 pb-8 pt-11 max-[720px]:pt-7"
      >
        <section
          class="recent w-full min-w-0 max-w-[44rem]"
          aria-labelledby="recent-heading"
        >
          <div class="recent-head mb-4 flex flex-wrap items-end justify-between gap-4">
            <div class="recent-title min-w-0">
              <h1 id="recent-heading" class="recent-heading m-0 text-[1.375rem] font-medium tracking-[-0.01em] text-foreground">
                {repos.length === 0 ? "Repositories" : "Recent repositories"}
              </h1>
              {#if repos.length > 0}
                <p class="recent-sub mb-0 mt-0.5 text-xs text-muted-foreground">
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
              <div class="head-actions flex flex-none flex-wrap gap-2">
                {#if tabsOpen}
                  <button
                    class="back-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-muted px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    onclick={hideRecentRepos}
                  >
                    <Icon icon={iconBack} size={14} />
                    Back to {active?.name ?? "workspace"}
                  </button>
                {/if}
                {#if repos.length > 0}
                  <button
                    class="add-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-transparent bg-primary px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-primary-foreground hover:brightness-110 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    onclick={() => void openFromDisk()}
                  >
                    <Icon icon={iconAdd} size={14} />
                    Open repository…
                  </button>
                  <button
                    class="clone-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-transparent px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    onclick={() => (cloneOpen = true)}
                  >
                    <Icon icon={iconClone} size={14} />
                    Clone repository…
                  </button>
                {/if}
              </div>
            {/if}
          </div>

          {#if showRepoFilter}
            <label
              class="repo-search mb-3 flex h-[2.1rem] items-center gap-1.5 rounded-full border border-border bg-card px-2.5 text-muted-foreground focus-within:border-primary focus-within:ring-1 focus-within:ring-primary"
            >
              <span class="search-icon inline-flex flex-none opacity-80" aria-hidden="true">
                <Icon icon={iconSearch} size={15} />
              </span>
              <input
                class="min-w-0 flex-1 border-0 bg-transparent text-[0.8125rem] text-foreground outline-none placeholder:text-muted-foreground placeholder:opacity-80"
                type="text"
                aria-label="Filter recent repositories"
                placeholder="Filter repositories by name or path"
                bind:value={repoFilter}
              />
              {#if repoFilter}
                <button
                  class="search-clear inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-full border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
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
            <div
              class="empty flex flex-col items-center gap-1.5 rounded-[28px] border border-dashed border-border bg-muted/30 px-6 py-10 text-center"
            >
              <span
                class="empty-icon mb-[0.35rem] grid size-14 place-items-center rounded-full bg-primary/10 text-primary"
                aria-hidden="true"
              >
                <Icon icon={iconFolder} size={30} />
              </span>
              <h2
                class="empty-title m-0 text-base font-semibold text-foreground"
              >
                Open your first repository
              </h2>
              <p
                class="empty-text m-0 max-w-[26rem] text-[0.8125rem] leading-normal text-muted-foreground"
              >
                Choose a folder on disk, clone a remote, or drag a folder onto
                this window to get started.
              </p>
              <div class="empty-actions mt-3 flex flex-wrap justify-center gap-2">
                <button
                  class="add-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-transparent bg-primary px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-primary-foreground hover:brightness-110 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => void openFromDisk()}
                >
                  <Icon icon={iconAdd} size={14} />
                  Open repository…
                </button>
                <button
                  class="clone-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-transparent px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => (cloneOpen = true)}
                >
                  <Icon icon={iconClone} size={14} />
                  Clone repository…
                </button>
              </div>
            </div>
          {:else if filteredRepos.length === 0}
            <div
              class="no-results rounded-2xl border border-dashed border-border px-4 py-8 text-center text-muted-foreground"
            >
              <p
                class="empty-title m-0 mb-2 text-base font-semibold text-foreground"
              >
                No repositories match “{repoFilter}”
              </p>
              <button
                class="clear-search mt-3 inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-transparent px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-primary hover:border-input hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                type="button"
                onclick={() => (repoFilter = "")}
              >
                Clear filter
              </button>
            </div>
          {:else}
            <ul class="repo-list m-0 flex list-none flex-col gap-1.5 p-0">
              {#each filteredRepos as repo (repo.path)}
                {@const when = lastOpenedLabel(repo.lastOpened)}
                <li class="repo-row-wrapper group/row flex items-center gap-0.5 rounded-xl border border-transparent bg-muted/40 pr-[0.35rem] transition-colors hover:border-border hover:bg-muted/70 focus-within:border-border focus-within:bg-muted/70">
                  <button
                    class="repo-row flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-xl border border-transparent bg-transparent px-3 py-[0.55rem] text-left text-foreground hover:border-transparent hover:bg-transparent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                    type="button"
                    onclick={() => void openRepoTab(repo.path)}
                  >
                    <span class="repo-icon grid size-10 flex-none place-items-center rounded-[0.625rem] bg-primary/10 text-primary" aria-hidden="true">
                      <Icon icon={iconFolder} size={20} />
                    </span>
                    <span class="repo-main flex min-w-0 flex-1 flex-col gap-0.5">
                      <span class="repo-title flex min-w-0 items-center gap-[0.3rem]">
                        <span class="repo-name overflow-hidden text-ellipsis whitespace-nowrap text-[0.9375rem] font-semibold">{repoName(repo.path)}</span>
                        {#if repo.pinned}
                          <span
                            class="pin-mark inline-flex flex-none text-primary"
                            title="Pinned"
                          >
                            <Icon icon={iconPin} size={13} />
                          </span>
                        {/if}
                      </span>
                      <span class="repo-path max-w-full overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-muted-foreground">{repo.path}</span>
                    </span>
                    {#if when}
                      <span
                        class="repo-when max-[720px]:hidden flex-none whitespace-nowrap text-[0.6875rem] text-muted-foreground"
                        >{when}</span
                      >
                    {/if}
                  </button>
                  <span class="row-actions flex flex-none items-center gap-0.5 opacity-75 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100">
                    <button
                      class="row-action pin inline-flex size-[1.9rem] cursor-pointer items-center justify-center rounded-lg border-0 bg-transparent p-0 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary {repo.pinned ? 'text-primary' : ''}"
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
                      class="row-action remove inline-flex size-[1.9rem] cursor-pointer items-center justify-center rounded-lg border-0 bg-transparent p-0 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
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

        <!-- Remote devices (SSH): list + detail (browser + settings). -->
        <section
          class="devices w-full min-w-0 max-w-[44rem]"
          aria-labelledby="devices-heading"
        >
          <div class="recent-head mb-4 flex flex-wrap items-end justify-between gap-4">
            <div class="recent-title min-w-0">
              <h2 id="devices-heading" class="recent-heading m-0 text-[1.375rem] font-medium tracking-[-0.01em] text-foreground">
                {deviceDetail ? "Device" : "Devices"}
              </h2>
              {#if !deviceDetail && (remoteDevices.devices?.length ?? 0) > 0}
                <p class="recent-sub mb-0 mt-0.5 text-xs text-muted-foreground">
                  {remoteDevices.devices.length}
                  {remoteDevices.devices.length === 1 ? "machine" : "machines"}
                  registered
                </p>
              {/if}
            </div>
            <div class="head-actions flex flex-none flex-wrap gap-2">
              {#if deviceDetail}
                <button
                  class="back-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-muted px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => (deviceDetailId = null)}
                >
                  <Icon icon={iconBack} size={14} />
                  All devices
                </button>
              {/if}
              <button class="clone-btn inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-transparent px-3.5 py-1.5 text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary" type="button" onclick={openAddDevice}>
                <Icon icon={iconAdd} size={14} />
                Add device…
              </button>
            </div>
          </div>

          {#if deviceDetail}
            <div class="device-detail flex flex-col gap-3 rounded-2xl border border-border bg-muted/40 px-[1.1rem] py-4">
              <div class="detail-head flex min-w-0 items-center gap-2.5">
                <span
                  class="detail-icon grid size-9 flex-none place-items-center rounded-lg bg-primary/10 text-primary"
                  aria-hidden="true"
                >
                  <Icon icon={iconDevices} size={18} />
                </span>
                <span class="detail-main flex min-w-0 flex-1 flex-col gap-0.5">
                  <span
                    class="detail-name overflow-hidden text-ellipsis whitespace-nowrap text-[0.9375rem] font-semibold"
                    >{deviceDetail.profile.name}</span
                  >
                  <span
                    class="detail-host overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-muted-foreground"
                  >
                    {deviceDetail.profile.username}@{deviceDetail.profile.host}:{deviceDetail.profile.port}
                  </span>
                </span>
                <span
                  class="device-status inline-flex flex-none items-center gap-1.5 whitespace-nowrap rounded-full bg-muted px-[0.55rem] py-[0.1rem] text-[0.6875rem] {deviceDetail.status === 'connected'
                    ? 'text-foreground'
                    : 'text-muted-foreground'}"
                  data-status={deviceDetail.status}
                >
                  <span
                    class="status-dot size-[0.45rem] flex-none rounded-full {deviceDetail.status === 'connected'
                      ? 'bg-primary'
                      : deviceDetail.status === 'connecting'
                        ? 'bg-primary/40'
                        : 'bg-muted-foreground/50'}"
                    aria-hidden="true"
                  ></span>
                  {deviceDetail.status}
                </span>
              </div>

              <div class="detail-actions flex flex-wrap items-end gap-2">
                {#if deviceDetail.status === "connected"}
                  <button
                    class="detail-btn inline-flex cursor-pointer items-center gap-[0.3rem] rounded-full border border-border bg-transparent px-[0.85rem] py-[0.35rem] text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    onclick={() => void remoteDevices.disconnect(deviceDetail.profile.id)}
                  >
                    Disconnect
                  </button>
                {:else}
                  <button
                    class="detail-btn inline-flex cursor-pointer items-center gap-[0.3rem] rounded-full border border-border bg-transparent px-[0.85rem] py-[0.35rem] text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    disabled={deviceDetail.status === "connecting"}
                    onclick={() => void remoteDevices.connect(deviceDetail.profile.id)}
                  >
                    {deviceDetail.status === "connecting" ? "Connecting…" : "Connect"}
                  </button>
                {/if}
                <button
                  class="detail-btn inline-flex cursor-pointer items-center gap-[0.3rem] rounded-full border border-border bg-transparent px-[0.85rem] py-[0.35rem] text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => openEditDevice(deviceDetail.profile)}
                >
                  <Icon icon={iconEdit} size={13} />
                  Edit…
                </button>
              </div>

              <div
                class="detail-settings flex flex-wrap items-end gap-2 border-t border-border pt-2"
              >
                <label class="poll-field flex flex-col gap-0.5">
                  <span
                    class="field-label text-[0.6875rem] text-muted-foreground"
                    >Poll interval (seconds)</span
                  >
                  <input
                    class="poll-input w-[5.5rem] rounded border border-border bg-card px-2 py-[0.35rem] text-[0.8125rem] text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
                    type="number"
                    min="1"
                    aria-label="Poll interval in seconds"
                    bind:value={pollDraft}
                  />
                </label>
                <button
                  class="detail-btn inline-flex cursor-pointer items-center gap-[0.3rem] rounded-full border border-border bg-transparent px-[0.85rem] py-[0.35rem] text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => void savePollInterval()}
                >
                  Save
                </button>
                <button
                  class="detail-btn inline-flex cursor-pointer items-center gap-[0.3rem] rounded-full border border-border bg-transparent px-[0.85rem] py-[0.35rem] text-xs font-medium whitespace-nowrap text-foreground hover:border-input hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => void forgetHostKey()}
                >
                  Forget host key
                </button>
                <button
                  class="detail-btn danger inline-flex cursor-pointer items-center gap-[0.3rem] rounded-full border border-border bg-transparent px-[0.85rem] py-[0.35rem] text-xs font-medium whitespace-nowrap text-destructive hover:border-destructive hover:bg-destructive/10 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => (removeDeviceOpen = true)}
                >
                  Delete device
                </button>
              </div>

              <DeviceBrowser deviceId={deviceDetail.profile.id} />
            </div>
          {:else if (remoteDevices.devices?.length ?? 0) === 0}
            <p
              class="devices-empty m-0 rounded-2xl border border-dashed border-border px-6 py-5 text-[0.8125rem] leading-normal text-muted-foreground"
            >
              No devices yet. Register an SSH machine to browse and open its
              repositories as projects.
            </p>
          {:else}
            <ul class="repo-list m-0 flex list-none flex-col gap-1.5 p-0">
              {#each remoteDevices.devices ?? [] as device (device.profile.id)}
                <li class="repo-row-wrapper group/row flex items-center gap-0.5 rounded-xl border border-transparent bg-muted/40 pr-[0.35rem] transition-colors hover:border-border hover:bg-muted/70 focus-within:border-border focus-within:bg-muted/70">
                  <button
                    class="repo-row flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-xl border border-transparent bg-transparent px-3 py-[0.55rem] text-left text-foreground hover:border-transparent hover:bg-transparent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                    type="button"
                    onclick={() => openDeviceDetail(device.profile.id)}
                  >
                    <span class="repo-icon grid size-10 flex-none place-items-center rounded-[0.625rem] bg-primary/10 text-primary" aria-hidden="true">
                      <Icon icon={iconDevices} size={20} />
                    </span>
                    <span class="repo-main flex min-w-0 flex-1 flex-col gap-0.5">
                      <span class="repo-title flex min-w-0 items-center gap-[0.3rem]">
                        <span class="repo-name overflow-hidden text-ellipsis whitespace-nowrap text-[0.9375rem] font-semibold">{device.profile.name}</span>
                      </span>
                      <span class="repo-path max-w-full overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-muted-foreground">
                        {device.profile.username}@{device.profile.host}:{device.profile.port}
                      </span>
                    </span>
                    <span
                      class="device-status inline-flex flex-none items-center gap-1.5 whitespace-nowrap rounded-full bg-muted px-[0.55rem] py-[0.1rem] text-[0.6875rem] {device.status === 'connected'
                        ? 'text-foreground'
                        : 'text-muted-foreground'}"
                      data-status={device.status}
                    >
                      <span
                        class="status-dot size-[0.45rem] flex-none rounded-full {device.status === 'connected'
                          ? 'bg-primary'
                          : device.status === 'connecting'
                            ? 'bg-primary/40'
                            : 'bg-muted-foreground/50'}"
                        aria-hidden="true"
                      ></span>
                      {device.status}
                    </span>
                  </button>
                  <span class="row-actions flex flex-none items-center gap-0.5 opacity-75 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100">
                    <button
                      class="row-action inline-flex size-[1.9rem] cursor-pointer items-center justify-center rounded-lg border-0 bg-transparent p-0 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
                      type="button"
                      title="Edit device"
                      aria-label={`Edit device ${device.profile.name}`}
                      onclick={() => openEditDevice(device.profile)}
                    >
                      <Icon icon={iconEdit} size={15} />
                    </button>
                  </span>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>

      <footer
        class="home-foot flex flex-none items-center gap-5 border-t border-border bg-muted/80 px-5 py-[0.65rem] text-[0.6875rem] text-muted-foreground"
      >
        <span class="hint">
          <kbd class="kbd">Ctrl</kbd><kbd class="kbd">K</kbd> quick switcher
        </span>
        <span class="hint">
          <kbd class="kbd">Ctrl</kbd><kbd class="kbd">Shift</kbd
          ><kbd class="kbd">P</kbd>
          command palette
        </span>
        <span class="foot-note ml-auto opacity-85 max-[720px]:hidden"
          >Tip: drop a folder here to open it</span
        >
      </footer>
      </main>
    {/if}
  {/if}
</div>

<Toaster />

<!-- Lazy-mounted overlays (see the lazy-mounting note in the script): each
     mounts on its first open signal and stays mounted afterwards. -->
{#if isSwitcherOpen() || switcherOverlay.landed}
  {#await switcherOverlay.chunk() then QuickSwitcherModule}
    <QuickSwitcherModule.default />
  {/await}
{/if}
{#if isPaletteOpen() || paletteOverlay.landed}
  {#await paletteOverlay.chunk() then CommandPaletteModule}
    <CommandPaletteModule.default />
  {/await}
{/if}
{#if cloneOpen || cloneOverlay.landed}
  {#await cloneOverlay.chunk() then CloneDialogModule}
    <CloneDialogModule.default bind:open={cloneOpen} />
  {/await}
{/if}
{#if contextMenu.open || contextMenuOverlay.landed}
  {#await contextMenuOverlay.chunk() then ContextMenuModule}
    <ContextMenuModule.default />
  {/await}
{/if}

<!-- Remote devices: add/edit dialog + the connect-prompt dialogs (driven
     by the remote-devices store's pending queues, AuthDialog-style). -->
{#if addDeviceOpen || addDeviceOverlay.landed}
  {#await addDeviceOverlay.chunk() then AddDeviceDialogModule}
    <AddDeviceDialogModule.default bind:open={addDeviceOpen} edit={editingDevice} />
  {/await}
{/if}
{#if remoteDevices.hostkeyPending || hostKeyOverlay.landed}
  {#await hostKeyOverlay.chunk() then HostKeyDialogModule}
    <HostKeyDialogModule.default />
  {/await}
{/if}
{#if remoteDevices.authPending || deviceAuthOverlay.landed}
  {#await deviceAuthOverlay.chunk() then DeviceAuthDialogModule}
    <DeviceAuthDialogModule.default />
  {/await}
{/if}

{#if removeDeviceOpen || confirmOverlay.landed}
  {#await confirmOverlay.chunk() then ConfirmDialogModule}
    <ConfirmDialogModule.default
      bind:open={removeDeviceOpen}
      title={`Delete device ${deviceDetail?.profile.name ?? ""}?`}
      message="The device, its stored secrets and its pinned host key are removed. Projects you opened from it stay in the recent list as ssh:// entries."
      confirmLabel="Delete device"
      danger
      onConfirm={() => void removeDevice()}
    />
  {/await}
{/if}

<!-- M12: AI settings dialog (opened by the AI health chip / palette). -->
{#if aiSettingsOpen || aiSettingsOverlay.landed}
  {#await aiSettingsOverlay.chunk() then AiSettingsModule}
    <AiSettingsModule.default bind:open={aiSettingsOpen} />
  {/await}
{/if}

<!-- Appearance customizer (opened by the palette; see $lib/theme). -->
{#if appearanceOpen || appearanceOverlay.landed}
  {#await appearanceOverlay.chunk() then AppearanceDialogModule}
    <AppearanceDialogModule.default bind:open={appearanceOpen} />
  {/await}
{/if}

