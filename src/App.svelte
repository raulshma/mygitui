<script lang="ts">
  /**
   * mygitui application shell (M1): tab strip over open repositories + a
   * per-tab status summary stub (full panels — working copy, diff, history —
   * land later in M1). With no tabs open, the home area lists recent
   * repositories; clicking one (or dropping a folder / passing CLI paths)
   * opens it as a tab via the tab store.
   */
  import { onMount, untrack } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { initTheme } from "$lib/theme";
  import { appearance } from "$lib/theme/appearance.svelte";
  import { startMenuColor } from "$lib/theme/menu-color";
  import { startIconSets } from "$lib/components/icons/library.svelte";
  import { isTauri, listenDragDrop } from "$lib/entry/dragdrop";
  import { initDeepLinks } from "$lib/entry/deeplink";
  import { pickFolder, remotePollSetPaused } from "$lib/ipc/client";
  import {
    openTab,
    refreshStatus,
    restoreSession,
    retryRemoteTab,
    startTabEvents,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import { opStore } from "$lib/stores/ops.svelte";
  import { syncWindowTitle } from "$lib/stores/windowTitle.svelte";
  import { flushSplits, layouts } from "$lib/layout";
  import { startWidthMode, stopWidthMode } from "$lib/layout/widthMode.svelte";
  import { flushHistoryPrefs } from "$lib/stores/historyPrefs";
  import { flushWorkspaceSessions } from "$lib/stores/workspaceSession.svelte";
  import { issueSubscriptions } from "$lib/issues/subscriptions.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import { syncGithubSettings } from "$lib/state/githubSync";
  import OverviewView from "$lib/overview/OverviewView.svelte";
  import OverviewToggle from "$lib/overview/OverviewToggle.svelte";
  import { overviewStore, wipeLegacyAgentKeys } from "$lib/overview/overviewStore.svelte";
  import {
    deviceInputFromProfile,
    parsePollSecs,
    remoteDevices,
    startRemoteDeviceEvents,
    POLL_SECS_DEFAULT,
    POLL_SECS_MAX,
    POLL_SECS_MIN,
    PROMPT_TIMEOUT_MS,
  } from "$lib/stores/remoteDevices.svelte";
  import { toast } from "$lib/toast";
  import Toaster from "$lib/components/Toaster.svelte";
  import TabStrip from "$lib/components/TabStrip.svelte";
  import StatusBar from "$lib/components/layout/StatusBar.svelte";
  import TitleBar from "$lib/components/layout/TitleBar.svelte";
  import AppBarMenu from "$lib/components/layout/AppBarMenu.svelte";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { formatRelativeTime } from "$lib/git/format";
  import iconBrand from "@ktibow/iconset-material-symbols/account-tree";
  import iconAdd from "@ktibow/iconset-material-symbols/add";
  import iconDevices from "@ktibow/iconset-material-symbols/devices";
  import iconEdit from "@ktibow/iconset-material-symbols/edit";
  import iconChevronDown from "@ktibow/iconset-material-symbols/expand-more";
  import iconSettings from "@ktibow/iconset-material-symbols/settings";
  import iconBack from "@ktibow/iconset-material-symbols/arrow-back-rounded";
  // Startup: the popout-only views below are dynamically imported inside
  // the popout branch — the main window never renders them, so their
  // module trees (history graph, file-history, popout panels) stay out of
  // the launch-critical graph.
  // M4 F1: panel popout query contract (?panel=diff|history&repo=<id>).
  import { parsePopoutQuery } from "$lib/layout/popout";
  // M4 F2: global keybind engine + palette execution entry point.
  import { startKeybinds, primaryModifierLabel } from "$lib/palette/keybinds";
  import { executeCommandById, isPaletteOpen, togglePalette } from "$lib/palette/palette.svelte";
  import { isSwitcherOpen } from "$lib/stores/switcher.svelte";
  import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
  import { isShortcutsOpen } from "$lib/components/help/shortcuts.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import AiHealthChip from "$lib/components/ai/AiHealthChip.svelte";
  import UpdateChip from "$lib/components/UpdateChip.svelte";
  import { updater } from "$lib/stores/updater.svelte";
  import DeviceBrowser from "$lib/components/DeviceBrowser.svelte";
  import type { DeviceProfile } from "$lib/ipc/types";
  import { closedDialog, openDialog, type DialogState } from "$lib/state/dialog";
  import type { SectionId } from "$lib/components/settings/SettingsDialog.svelte";
  import { describeError } from "$lib/errors";

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
   *
   * `landed` flips from the chunk's resolution callback, never from inside
   * `chunk()` itself: the template calls `chunk()` in its `{#await}`
   * expression, and a state write there throws `state_unsafe_mutation`
   * (Svelte forbids writes during template evaluation).
   */
  function lazyOverlay<T>(load: () => Promise<T>) {
    let promise: Promise<T> | null = null;
    let landed = $state(false);
    return {
      chunk: (): Promise<T> => {
        if (!promise) {
          promise = load();
          void promise.then(() => {
            landed = true;
          });
        }
        return promise;
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
  const addDeviceOverlay = lazyOverlay(() => import("$lib/components/AddDeviceDialog.svelte"));
  const hostKeyOverlay = lazyOverlay(() => import("$lib/components/HostKeyDialog.svelte"));
  const deviceAuthOverlay = lazyOverlay(() => import("$lib/components/DeviceAuthDialog.svelte"));
  const confirmOverlay = lazyOverlay(() => import("$lib/components/safety/ConfirmDialog.svelte"));
  const appearanceOverlay = lazyOverlay(
    () => import("$lib/components/AppearanceDialog.svelte"),
  );
  const settingsOverlay = lazyOverlay(
    () => import("$lib/components/settings/SettingsDialog.svelte"),
  );
  const shortcutsOverlay = lazyOverlay(
    () => import("$lib/components/help/ShortcutsOverlay.svelte"),
  );
  const aiResultOverlay = lazyOverlay(
    () => import("$lib/components/ai/AiResultDialog.svelte"),
  );

  /** Overlay chunks fetched one per idle slot, launch graph untouched. */
  const overlayWarmers: Array<() => Promise<unknown>> = [
    switcherOverlay.chunk,
    paletteOverlay.chunk,
    contextMenuOverlay.chunk,
    cloneOverlay.chunk,
    addDeviceOverlay.chunk,
    hostKeyOverlay.chunk,
    deviceAuthOverlay.chunk,
    confirmOverlay.chunk,
    appearanceOverlay.chunk,
    settingsOverlay.chunk,
    shortcutsOverlay.chunk,
    aiResultOverlay.chunk,
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
  if (popout !== null) {
    // Popouts render one panel and never restore the session — clear the
    // launch-hold flags so a popout-side `launching` reader can never
    // wedge (registry #13; `restoreSession` is main-window-only).
    tabStore.skipRestore();
  }

  // Customizer (shadcn style/base/theme/fonts/...): storage → DOM during
  // component init — before any paint — so a persisted appearance never
  // flashes the defaults. Must never break startup; failures log and move
  // on. (Menu/icon wiring follows in onMount below.)
  try {
    appearance.init();
  } catch (err) {
    console.error("[mygitui] appearance init failed:", err);
  }

  /** Clone-dialog visibility (Overview's "Clone repository…" button). */
  let cloneOpen = $state(false);

  /** Shared AI result dialog (M13, `ai-show-result`): headless AI runs
   * (palette commands, context-menu features without a home panel) present
   * their output here.
   */
  let aiResultOpen = $state(false);
  let aiResult = $state<{ title: string; text: string; meta?: string } | null>(null);

  /** Appearance customizer (opened by the palette / ThemeToggle menu). */
  let appearanceOpen = $state(false);

  /** Settings hub (opened by the palette / TabStrip gear / Ctrl+Comma / AI shortcuts). */
  let settingsOpen = $state(false);
  let settingsSection = $state<SectionId>("general");

  // --- Remote devices (SSH) -------------------------------------------------

  /** The device whose detail view (browser + settings) the home shows. */
  let deviceDetailId = $state<string | null>(null);
  /**
   * Add/edit-device dialog — one state, the edited device travels with
   * open (null device = "add"), so no open/close site can hand-reset it.
   */
  let addDevice = $state<DialogState<{ device: DeviceProfile | null }>>(
    closedDialog(),
  );
  /** Delete-device confirmation. */
  let removeDeviceOpen = $state(false);
  /**
   * Draft of the detail view's poll-interval field (seconds; blank = the
   * global default — parsed through `parsePollSecs`, 2–300 clamped).
   */
  let pollDraft = $state("");
  /** True while the detail view's poll-interval save RPC is in flight. */
  let savingPoll = $state(false);
  /** Draft of the detail view's auto-connect-on-launch toggle. */
  let autoConnectDraft = $state(false);
  /**
   * Sync-status detail rows (item 60): which device list rows are expanded
   * (deviceId → open). Per-row richer state: last status change, poll
   * interval, auth kind, open repo tabs, last connection error.
   */
  let expandedDeviceRows = $state<Record<string, boolean>>({});

  /** The focused device entry, or `null` when the list is shown. */
  const deviceDetail = $derived(
    deviceDetailId !== null ? remoteDevices.byId(deviceDetailId) : null,
  );

  // Follow profile changes (update responses, edit-dialog saves) so the
  // toggle never shows a stale value while the detail view is open.
  $effect(() => {
    autoConnectDraft = deviceDetail?.profile.auto_connect ?? false;
  });

  function openDeviceDetail(deviceId: string): void {
    deviceDetailId = deviceId;
    const secs = remoteDevices.byId(deviceId)?.profile.poll_secs;
    pollDraft = typeof secs === "number" ? String(secs) : "";
  }

  function openAddDevice(): void {
    addDevice = openDialog({ device: null });
  }

  function openEditDevice(device: DeviceProfile): void {
    addDevice = openDialog({ device });
  }

  /** Persists the detail view's poll-interval draft via device_update. */
  async function savePollInterval(): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile || savingPoll) return;
    savingPoll = true;
    // Blank = the global default (null); otherwise clamped to 2–300 s —
    // the backend clamps the stored profile to the same range.
    const seconds = parsePollSecs(pollDraft);
    try {
      await remoteDevices.update({
        ...deviceInputFromProfile(profile),
        poll_secs: seconds,
      });
      toast(
        seconds === null
          ? `Poll interval for ${profile.name} reset to the global default`
          : `Poll interval for ${profile.name} set to ${seconds}s`,
        {
          kind: "success",
        },
      );
    } catch (err) {
      const message = describeError(err);
      toast(`Failed to update ${profile.name}: ${message}`, { kind: "error" });
    } finally {
      savingPoll = false;
    }
  }

  /** Persists the detail view's auto-connect toggle via device_update. */
  async function saveAutoConnect(enabled: boolean): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile) return;
    try {
      await remoteDevices.update({
        ...deviceInputFromProfile(profile),
        auto_connect: enabled,
      });
      toast(
        enabled
          ? `${profile.name} will connect on launch`
          : `${profile.name} will not connect on launch`,
        { kind: "success" },
      );
    } catch (err) {
      // Revert the visual toggle — the backend kept the old value.
      autoConnectDraft = profile.auto_connect;
      const message = describeError(err);
      toast(`Failed to update ${profile.name}: ${message}`, { kind: "error" });
    }
  }

  async function forgetHostKey(): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile) return;
    const outcome = await remoteDevices.forgetHostKey(profile.id);
    if (outcome === "forgotten") {
      toast(`Host key for ${profile.name} forgotten — next connect re-prompts`);
    } else if (outcome === "none") {
      toast(`${profile.name} has no pinned host key`);
    }
    // "failed": the store already toasted the error — stay silent here.
  }

  async function removeDevice(): Promise<void> {
    const profile = deviceDetail?.profile;
    if (!profile) return;
    try {
      await remoteDevices.remove(profile.id);
      deviceDetailId = null;
      toast(`Removed device ${profile.name}`);
    } catch (err) {
      const message = describeError(err);
      toast(`Failed to remove ${profile.name}: ${message}`, { kind: "error" });
    }
  }

  /** The focused repository tab, or `null` when nothing is open. */
  const active = $derived(tabStore.active);

  /** Tabs exist (workspace or the recents overlay above it). */
  const tabsOpen = $derived(tabStore.tabs.length > 0);

  /**
   * Launch restore is still resolving the stored active tab: hold the launch
   * view instead of painting the Overview (or a non-active tab) first.
   */
  const launching = $derived(tabStore.restoring && tabStore.activeId === null);

  /**
   * The Overview: the zero-tabs landing, and (via the persisted store) the
   * header-toggle overlay over open tabs. Replaced the recents home.
   */
  const showOverview = $derived(overviewStore.enabled || !tabsOpen);

  /** The workspace (repo view + status bar), hidden while the Overview shows. */
  const showWorkspace = $derived(tabsOpen && !showOverview);

  /** Repository name shown while the last session reopens. */
  const restoreName = $derived(
    tabStore.restoreRoot ? repoName(tabStore.restoreRoot) : null,
  );

  // Dynamic window title: one effect reads the active tab (+ its status's
  // dirty marker) through the title store, which guards the Tauri call.
  $effect(() => {
    syncWindowTitle();
  });

  // GitHub feature switches: mirrored into the backend spawn gate (the IPC
  // client also rejects gated commands, this is defense-in-depth) and read
  // by the issues lane effect below so turning Issues off drops its pollers.
  $effect(() => {
    void appSettings.githubEnabled;
    void appSettings.githubPullRequests;
    void appSettings.githubActions;
    void appSettings.githubIssues;
    void appSettings.githubNotifications;
    void appSettings.githubStatusChips;
    syncGithubSettings();
  });

  // Issues polling has one application-owned coordinator. Track the open
  // repositories and focused tab so activation promotes one lane to 60 s;
  // the coordinator itself owns visibility/focus listeners and coalescing.
  $effect(() => {
    if (popout !== null) return;
    void appSettings.githubEnabled;
    void appSettings.githubIssues;
    const repoIds = tabStore.tabs.map((tab) => tab.id);
    const activeId = tabStore.activeId;
    untrack(() => {
      void repoIds.length;
      void activeId;
      issueSubscriptions.syncLanes();
      // Lanes created after startup (notably restored tabs) need an immediate
      // visible catch-up rather than waiting for their first interval.
      void issueSubscriptions.catchUp();
    });
  });

  // -- focus refresh ---------------------------------------------------------
  //
  // The backend FS watcher gives up permanently after 3 restart attempts;
  // when it dies, `repo-changed` events stop and the UI would stay stale
  // forever. Re-run the tab store's coalesced status refresh (the same
  // `refreshStatus` path watcher events use) when the window regains focus
  // or becomes visible again. Debounced so rapid alt-tabs fire one cycle;
  // skipped while ops run — a mutation refreshes status on completion.

  /** Debounce window for focus-triggered refreshes. */
  const FOCUS_REFRESH_DEBOUNCE_MS = 1000;

  let focusRefreshTimer: ReturnType<typeof setTimeout> | null = null;

  function queueFocusRefresh(): void {
    if (focusRefreshTimer !== null) clearTimeout(focusRefreshTimer);
    focusRefreshTimer = setTimeout(() => {
      focusRefreshTimer = null;
      if (tabStore.tabs.some((tab) => opStore.busy(tab.id))) return;
      // The focused repo only: regaining focus says "this window's state may
      // be stale", not "re-scan every open repo". Background tabs refresh on
      // activation (setActive), so they are never stale when shown.
      const active = tabStore.active;
      if (active) void refreshStatus(active.id);
    }, FOCUS_REFRESH_DEBOUNCE_MS);
  }

  function onVisibilityRefresh(): void {
    if (document.visibilityState === "visible") queueFocusRefresh();
  }

  function repoName(path: string): string {
    const trimmed = path.replace(/[\\/]+$/, "");
    const segments = trimmed.split(/[\\/]/).filter(Boolean);
    return segments.length > 0 ? segments[segments.length - 1]! : path;
  }

  /** Tab-strip "+" / Overview "Add folder…": folder picker → open as tab. */
  async function openFromDisk(): Promise<void> {
    const path = await pickFolder();
    if (path) await openRepoTab(path);
  }

  async function openRepoTab(path: string): Promise<void> {
    try {
      await openTab(path);
    } catch (err) {
      const message = describeError(err);
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

    // One-time hygiene: the Agent mode this Overview replaced left its
    // storage behind (`mygitui.agent`, `mygitui.agent.<root>` sessions).
    wipeLegacyAgentKeys();

    // Remote devices: registry + prompt/status event wiring (idempotent),
    // plus pruning of prompts the backend already timed out (120 s). The
    // registry lands BEFORE any dial starts (restore / CLI arg below):
    // their device-status events must patch a loaded list, not race a
    // stale snapshot replace — that race used to pin "disconnected" on
    // launch until a reload. Opted-in devices then auto-connect.
    startRemoteDeviceEvents();
    void remoteDevices.load().then(() => remoteDevices.autoConnectOnLaunch());
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

    // Layout + split-pref + history/workspace-session persistence run on a
    // 150 ms debounce; closing or reloading the window inside that window
    // would silently drop the last change (a pending timer dies with the
    // document). Flush on the way out — `pagehide` fires on both navigation
    // and app close, popouts included (their splitter ratios persist too).
    const flushPendingLayoutWrites = () => {
      layouts.flush();
      flushSplits();
      flushHistoryPrefs();
      flushWorkspaceSessions();
    };
    window.addEventListener("pagehide", flushPendingLayoutWrites);
    cleanups.push(() => {
      window.removeEventListener("pagehide", flushPendingLayoutWrites);
    });

    // Window width modes (plan §4.2): resolve once and mirror the window
    // width onto <html data-width-mode="…"> on resize — the `wm-*` Tailwind
    // variants key off it. Popouts track their own (often narrower) window.
    // Resize work is change-guarded (one attribute write per mode flip), so
    // no debounce is needed.
    startWidthMode();
    cleanups.push(stopWidthMode);

    // Relaunch polish: reopen the tabs (and the focused one) from the last
    // session. Runs before the CLI-arg/deep-link listeners below, so an
    // explicit `mygitui <path>` still wins the focus when it lands.
    if (!popout) {
      // Restore dials every stored remote tab's device; once it settles,
      // all launch dials have too — re-sync the statuses (the first load's
      // pre-dial snapshot may have pinned "disconnected").
      void restoreSession()
        .then(() => remoteDevices.load())
        .catch((err) =>
          console.warn("[mygitui] post-restore device sync failed:", err),
        );
      // Overlay dialogs mount lazily (see the lazy-mounting note); warm
      // their chunks once the launch settles.
      warmNextOverlay();
      // Self-update: schedule the gated auto-check (settings from
      // localStorage; polls only when autoCheck and the last check is
      // ~12h stale, 15s after launch). Popouts never check.
      updater.init();
    }

    // Popout windows render one panel only — no shell-level listeners.
    if (popout) {
      return () => {
        for (const unlisten of cleanups) unlisten();
      };
    }

    // Main-window Issues subscriptions: the coordinator owns its visibility
    // and focus listeners, while the effect above owns lane reconciliation.
    issueSubscriptions.start();
    cleanups.push(() => issueSubscriptions.stop());

    // M4 F2: global keyboard shortcuts (Ctrl/Cmd+K palette et al).
    cleanups.push(startKeybinds((commandId) => {
      void executeCommandById(commandId);
    }));

    // Focus refresh (see the block above): watcher death fallback. Popouts
    // returned early — they render one panel and hold no tabs, so their
    // tabStore is empty and the cycle would be a no-op.
    window.addEventListener("focus", queueFocusRefresh);
    document.addEventListener("visibilitychange", onVisibilityRefresh);
    cleanups.push(() => {
      window.removeEventListener("focus", queueFocusRefresh);
      document.removeEventListener("visibilitychange", onVisibilityRefresh);
      if (focusRefreshTimer !== null) clearTimeout(focusRefreshTimer);
    });

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

    // M12: the AI health chip and AI configure buttons open settings in the AI section.
    cleanups.push(
      onUiEvent("open-ai-settings", () => {
        settingsSection = "ai";
        settingsOpen = true;
      }),
    );

    // M13: headless AI runs (palette commands) present results here.
    cleanups.push(
      onUiEvent("ai-show-result", (detail) => {
        aiResult = detail;
        aiResultOpen = true;
      }),
    );

    cleanups.push(
      onUiEvent("open-appearance", () => {
        appearanceOpen = true;
      }),
    );

    cleanups.push(
      onUiEvent("open-settings", () => {
        settingsSection = "general";
        settingsOpen = true;
      }),
    );

    return () => {
      for (const unlisten of cleanups) unlisten();
    };
  });
</script>

<div class="app-shell flex h-full flex-col overflow-hidden bg-background text-foreground">
  <!-- Custom window chrome (decorations are off on Windows/Linux; macOS
       overlays its traffic lights over this bar). First child so every
       view — popout, tab strip, launch, home — sits below it. Workstream A:
       the repo tab strip renders INSIDE the bar (one ~40px top row); with
       no workspace (popout/home/launch) the bar stays a plain title bar. -->
  {#snippet repoTabs()}
    <TabStrip openFolder={openFromDisk}>
      {#snippet trailing()}
        <!-- Status cluster (AI health dot + agent toggle) — the update
             chip self-hides while idle and shows only when there is
             update state, so it stays a visible attention badge. -->
        <UpdateChip compact />
        <AiHealthChip compact />
        <OverviewToggle />
        <AppBarMenu onOpenSettings={() => { settingsSection = "general"; settingsOpen = true; }} />
      {/snippet}
    </TabStrip>
  {/snippet}
  <TitleBar tabs={!launching && tabsOpen ? repoTabs : undefined} />
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

    {#if launching}
      <!-- Launch restore is still opening the stored repo: hold this frame
           instead of flashing the recents list (or a non-active tab). -->
      <main
        class="launch flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.65rem] bg-[radial-gradient(circle_at_50%_-12%,color-mix(in_srgb,var(--primary)_8%,transparent),transparent_46%)] bg-background text-muted-foreground"
        aria-busy="true"
      >
        <span
          class="launch-mark mb-ui-35 grid size-[3.25rem] place-items-center rounded-[0.95rem] bg-primary/10 text-primary-ink shadow-sm"
          aria-hidden="true"
        >
          <Icon icon={iconBrand} size={26} />
        </span>
        <span class="launch-title text-sm font-medium text-foreground">
          Opening {restoreName ?? "your last workspace"}…
        </span>
        {#if tabStore.restoreRoot}
          <Hint label={tabStore.restoreRoot} side="bottom">
            {#snippet child({ props })}
              <span
                class="launch-sub max-w-[min(32rem,80vw)] overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs opacity-75"
                {...props}
              >
                {tabStore.restoreRoot}
              </span>
            {/snippet}
          </Hint>
        {/if}
        <span
          class="launch-spinner mt-ui-35 size-[1.4rem] animate-spin rounded-full border-[2.5px] border-primary/25 border-t-primary"
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
            class="offline-icon mb-ui-35 grid size-[3.25rem] place-items-center rounded-[0.95rem] bg-muted text-muted-foreground shadow-sm"
            aria-hidden="true"
          >
            <Icon icon={iconDevices} size={26} />
          </span>
          <h2 class="offline-title m-0 text-base font-medium text-foreground">
            {active.name} is not reachable
          </h2>
          <Hint label={active.root} side="bottom">
            {#snippet child({ props })}
              <span
                class="offline-path max-w-[min(32rem,80vw)] overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs opacity-75"
                {...props}>{active.root}</span
              >
            {/snippet}
          </Hint>
          <p
            class="offline-msg mb-2 max-w-[min(32rem,80vw)] text-ui-base [overflow-wrap:anywhere]"
            role="alert"
          >
            {active.openError}
          </p>
          <button
            class="add-btn pill-btn pill-btn-md pill-btn-primary"
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
      <!-- VS Code-style bottom chrome: a fixed-height strip whose indicators
           appear/disappear inside it, so the workspace above never shifts
           (the old in-flow op strip twitched on every auto-fetch tick). -->
      <StatusBar />
    {:else if showOverview}
      <!-- The Overview: the zero-tabs landing, and the header-toggle overlay
           over open tabs (it replaced the old recents home — the remote
           devices section rides along as the children snippet). -->
      <main class="overview-host flex min-h-0 min-w-0 flex-1 overflow-hidden">
        <OverviewView
          onOpenSettings={() => { settingsSection = "general"; settingsOpen = true; }}
          onClone={() => (cloneOpen = true)}
        >
          <!-- Remote devices (SSH): list + detail (browser + settings). -->
          <section
            class="devices w-full min-w-0"
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
                    class="back-btn pill-btn pill-btn-md pill-btn-secondary"
                    type="button"
                    onclick={() => (deviceDetailId = null)}
                  >
                    <Icon icon={iconBack} size={14} />
                    All devices
                  </button>
                {/if}
                <button class="clone-btn pill-btn pill-btn-md pill-btn-ghost" type="button" onclick={openAddDevice}>
                  <Icon icon={iconAdd} size={14} />
                  Add device…
                </button>
              </div>
            </div>

            {#if deviceDetail}
              <div class="device-detail flex flex-col gap-3 rounded-2xl border border-border bg-muted/40 px-[1.1rem] py-4">
                <div class="detail-head flex min-w-0 items-center gap-2.5">
                  <span
                    class="detail-icon grid size-9 flex-none place-items-center rounded-lg bg-primary/10 text-primary-ink"
                    aria-hidden="true"
                  >
                    <Icon icon={iconDevices} size={18} />
                  </span>
                  <span class="detail-main flex min-w-0 flex-1 flex-col gap-0.5">
                  <span
                    class="detail-name overflow-hidden text-ellipsis whitespace-nowrap text-sm font-semibold"
                    >{deviceDetail.profile.name}</span
                  >
                    <span
                      class="detail-host overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-muted-foreground"
                    >
                      {deviceDetail.profile.username}@{deviceDetail.profile.host}:{deviceDetail.profile.port}
                    </span>
                  </span>
                  <span
                    class="device-status inline-flex flex-none items-center gap-1.5 whitespace-nowrap rounded-full bg-muted px-2 py-px text-ui-micro {deviceDetail.status === 'connected'
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
                      class="detail-btn pill-btn pill-btn-sm pill-btn-ghost"
                      type="button"
                      onclick={() => void remoteDevices.disconnect(deviceDetail.profile.id)}
                    >
                      Disconnect
                    </button>
                  {:else}
                    <button
                      class="detail-btn pill-btn pill-btn-sm pill-btn-ghost"
                      type="button"
                      disabled={deviceDetail.status === "connecting"}
                      onclick={() => void remoteDevices.connect(deviceDetail.profile.id)}
                    >
                      {deviceDetail.status === "connecting" ? "Connecting…" : "Connect"}
                    </button>
                  {/if}
                  <button
                    class="detail-btn pill-btn pill-btn-sm pill-btn-ghost"
                    type="button"
                    onclick={() => openEditDevice(deviceDetail.profile)}
                  >
                    <Icon icon={iconEdit} size={14} />
                    Edit…
                  </button>
                </div>

                <div
                  class="detail-settings flex flex-wrap items-end gap-2 border-t border-border pt-2"
                >
                  <label class="poll-field flex flex-col gap-0.5">
                    <span
                      class="field-label text-ui-micro text-muted-foreground"
                      >Poll interval (seconds)</span
                    >
                    <input
                      class="poll-input w-[5.5rem] rounded border border-border bg-card px-2 py-ui-35 text-ui-base text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
                      type="number"
                      min={POLL_SECS_MIN}
                      max={POLL_SECS_MAX}
                      placeholder={`${POLL_SECS_DEFAULT} (default)`}
                      aria-label="Poll interval in seconds (blank = global default)"
                      bind:value={pollDraft}
                    />
                  </label>
                  <label class="check-field flex cursor-pointer items-center gap-1.5 pb-[0.3rem]">
                    <input
                      class="m-0 accent-primary"
                      type="checkbox"
                      bind:checked={autoConnectDraft}
                      onchange={() => void saveAutoConnect(autoConnectDraft)}
                      aria-label="Auto-connect on launch"
                    />
                    <span
                      class="field-label text-ui-micro text-muted-foreground"
                      >Auto-connect on launch</span
                    >
                  </label>
                  <button
                    class="detail-btn pill-btn pill-btn-sm pill-btn-ghost"
                    type="button"
                    disabled={savingPoll}
                    onclick={() => void savePollInterval()}
                  >
                    {savingPoll ? "Saving…" : "Save"}
                  </button>
                  <button
                    class="detail-btn pill-btn pill-btn-sm pill-btn-ghost"
                    type="button"
                    onclick={() => void forgetHostKey()}
                  >
                    Forget host key
                  </button>
                  <button
                    class="detail-btn danger pill-btn pill-btn-sm pill-btn-danger"
                    type="button"
                    onclick={() => (removeDeviceOpen = true)}
                  >
                    Delete device
                  </button>
                </div>

                <DeviceBrowser deviceId={deviceDetail.profile.id} />
              </div>
            {:else if remoteDevices.loadError && (remoteDevices.devices?.length ?? 0) === 0}
              <!-- device_list failed: never dress a backend hiccup up as
                   "no devices yet" — show the error with a Retry instead. -->
              <div
                class="devices-error m-0 flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-dashed border-border px-6 py-5"
              >
                <p
                  class="devices-error-msg m-0 min-w-0 flex-1 text-ui-base leading-normal text-destructive"
                  role="alert"
                >
                  Couldn't load the device list: {remoteDevices.loadError}
                </p>
                <button
                  class="add-btn pill-btn pill-btn-md pill-btn-primary"
                  type="button"
                  onclick={() => void remoteDevices.load()}
                >
                  <Icon icon={iconAdd} size={14} />
                  Retry
                </button>
              </div>
            {:else if (remoteDevices.devices?.length ?? 0) === 0}
              <p
                class="devices-empty m-0 rounded-2xl border border-dashed border-border px-6 py-5 text-ui-base leading-normal text-muted-foreground"
              >
                No devices yet. Register an SSH machine to browse and open its
                repositories as projects.
              </p>
            {:else}
              <ul class="repo-list m-0 flex list-none flex-col gap-1.5 p-0">
                {#each remoteDevices.devices ?? [] as device (device.profile.id)}
                  <li class="repo-row-wrapper group/row flex flex-wrap items-center gap-0.5 rounded-xl border border-transparent bg-muted/40 pr-ui-35 transition-colors hover:border-border hover:bg-muted/70 focus-within:border-border focus-within:bg-muted/70">
                    <button
                      class="repo-row flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-xl border border-transparent bg-transparent px-3 py-[0.55rem] text-left text-foreground hover:border-transparent hover:bg-transparent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                      type="button"
                      onclick={() => openDeviceDetail(device.profile.id)}
                    >
                      <span class="repo-icon grid size-10 flex-none place-items-center rounded-[0.625rem] bg-primary/10 text-primary-ink" aria-hidden="true">
                        <Icon icon={iconDevices} size={20} />
                      </span>
                      <span class="repo-main flex min-w-0 flex-1 flex-col gap-0.5">
                        <span class="repo-title flex min-w-0 items-center gap-ui-3">
                          <span class="repo-name overflow-hidden text-ellipsis whitespace-nowrap text-sm font-semibold">{device.profile.name}</span>
                        </span>
                        <span class="repo-path max-w-full overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-muted-foreground">
                          {device.profile.username}@{device.profile.host}:{device.profile.port}
                        </span>
                      </span>
                      <span
                        class="device-status inline-flex flex-none items-center gap-1.5 whitespace-nowrap rounded-full bg-muted px-2 py-px text-ui-micro {device.status === 'connected'
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
                      <Hint label={expandedDeviceRows[device.profile.id] ? "Hide sync details" : "Show sync details"}>
                        {#snippet child({ props })}
                          <button
                            class="row-action inline-flex size-7 cursor-pointer items-center justify-center rounded-lg border-0 bg-transparent p-0 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
                            type="button"
                            {...props}
                            aria-label={`${expandedDeviceRows[device.profile.id] ? "Hide" : "Show"} sync details for ${device.profile.name}`}
                            aria-expanded={expandedDeviceRows[device.profile.id] ?? false}
                            onclick={() =>
                              (expandedDeviceRows = {
                                ...expandedDeviceRows,
                                [device.profile.id]: !(expandedDeviceRows[device.profile.id] ?? false),
                              })}
                          >
                            <span
                              class="inline-flex transition-transform {expandedDeviceRows[device.profile.id]
                                ? 'rotate-180'
                                : ''}"
                              aria-hidden="true"
                            >
                              <Icon icon={iconChevronDown} size={14} />
                            </span>
                          </button>
                        {/snippet}
                      </Hint>
                      <Hint label="Edit device">
                        {#snippet child({ props })}
                          <button
                            class="row-action inline-flex size-7 cursor-pointer items-center justify-center rounded-lg border-0 bg-transparent p-0 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
                            type="button"
                            {...props}
                            aria-label={`Edit device ${device.profile.name}`}
                            onclick={() => openEditDevice(device.profile)}
                          >
                            <Icon icon={iconEdit} size={14} />
                          </button>
                        {/snippet}
                      </Hint>
                    </span>
                    {#if expandedDeviceRows[device.profile.id] ?? false}
                      <!-- Sync-status detail (item 60): richer per-device state
                           from what already flows (profile + device-status
                           events + open tabs) — no extra backend calls. -->
                      <div
                        class="device-details grid w-full grid-cols-[max-content_1fr] gap-x-3 gap-y-0.5 rounded-xl border border-border bg-card px-3 py-2 text-ui-micro"
                        data-testid={`device-details-${device.profile.id}`}
                      >
                        <span class="text-muted-foreground">connection</span>
                        <span class="text-foreground">
                          {device.status}
                          {#if remoteDevices.lastStatusAt[device.profile.id]}
                            · changed {formatRelativeTime(Math.floor(remoteDevices.lastStatusAt[device.profile.id] / 1000))}
                          {/if}
                        </span>
                        <span class="text-muted-foreground">poll</span>
                        <span class="text-foreground">
                          every {device.profile.poll_secs}s (per-device; 2–300 s, blank = 5s global default)
                        </span>
                        <span class="text-muted-foreground">auth</span>
                        <span class="text-foreground">
                          {device.profile.auth.kind === "key_file" ? "key file" : "password"}
                          {#if device.profile.auto_connect}· auto-connect on launch{/if}
                        </span>
                        <span class="text-muted-foreground">open repos</span>
                        <span class="text-foreground tabular-nums">
                          {tabStore.tabs.filter((tab) => tab.deviceId === device.profile.id).length}
                        </span>
                        {#if remoteDevices.lastError[device.profile.id]}
                          <span class="text-muted-foreground">last error</span>
                          <span class="text-destructive">{remoteDevices.lastError[device.profile.id]}</span>
                        {/if}
                      </div>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </section>
        </OverviewView>
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
{#if addDevice.open || addDeviceOverlay.landed}
  {#await addDeviceOverlay.chunk() then AddDeviceDialogModule}
    <AddDeviceDialogModule.default
      bind:open={
        () => addDevice.open,
        (v) => {
          if (!v) addDevice = closedDialog();
        }
      }
      edit={addDevice.open ? addDevice.device : null}
    />
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

<!-- M13: shared AI result dialog (headless runs land here). -->
{#if aiResultOpen || aiResultOverlay.landed}
  {#await aiResultOverlay.chunk() then AiResultDialogModule}
    <AiResultDialogModule.default
      bind:open={aiResultOpen}
      title={aiResult?.title ?? "AI result"}
      text={aiResult?.text ?? ""}
      meta={aiResult?.meta ?? null}
    />
  {/await}
{/if}

<!-- Appearance customizer (opened by the palette; see $lib/theme). -->
{#if appearanceOpen || appearanceOverlay.landed}
  {#await appearanceOverlay.chunk() then AppearanceDialogModule}
    <AppearanceDialogModule.default bind:open={appearanceOpen} />
  {/await}
{/if}

<!-- Settings hub (opened by the palette / TabStrip gear / Ctrl+Comma / AI shortcuts). -->
{#if settingsOpen || settingsOverlay.landed}
  {#await settingsOverlay.chunk() then SettingsDialogModule}
    <SettingsDialogModule.default
      bind:open={settingsOpen}
      initialSection={settingsSection}
    />
  {/await}
{/if}

<!-- `?` shortcuts cheatsheet (palette command help.shortcuts; store-driven). -->
{#if isShortcutsOpen() || shortcutsOverlay.landed}
  {#await shortcutsOverlay.chunk() then ShortcutsOverlayModule}
    <ShortcutsOverlayModule.default />
  {/await}
{/if}

