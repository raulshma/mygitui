<script lang="ts">
  /**
   * Repo header — the repository workspace's breadcrumb row (36px, plan
   * §5.3): repo name · branch switcher · Sync split button on the left, the
   * remote-path info affordance after them, and history / terminal / one
   * "More repository actions" overflow on the right.
   *
   * Workstream A demotions (every control reachable in ≤2 clicks):
   * - ssh/root path → an info icon: the tooltip shows the ssh host (the
   *   full path for local roots), a popover shows the full path + copy.
   * - layout preset select → "Layout preset" submenu in the overflow (the
   *   current preset checked; picking one applies it, like the old select).
   * - `OpenWithButton` → "Open with" submenu in the overflow (detected +
   *   custom apps, "Manage custom apps…"), via `openWithApps.svelte.ts`.
   * - remote connection badge → removed; the status bar owns connection
   *   state (plan §5.3 / §8.6), and remote tabs carry a cloud glyph.
   * - history toggle + terminal stay one-click.
   *
   * The "More repository actions" menu itself replaced the plain overflow
   * button with the shared OverflowMenu primitive (bits-ui dropdown-menu):
   * same accessible name, same `role="menu"` content — but nested. The
   * repo-specific commands (bisect, save layout, clean, reset, recents)
   * arrive as the `headerOverflow` snippet from RepoView, which owns them.
   *
   * State arrives via props; user intents are emitted through callback
   * props, and header-exclusive reads (layout presets, open-with apps)
   * live here.
   */
  import type { Snippet } from "svelte";
  import type { EditorApp, RepoStatus } from "$lib/ipc/types";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import OverflowMenu from "$lib/components/layout/OverflowMenu.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { layouts } from "$lib/layout/layout.svelte";
  import { copyText } from "$lib/clipboard";
  import { floating } from "$lib/components/overlay/floating";
  import BranchSwitcher from "$lib/components/header/BranchSwitcher.svelte";
  import RepoQuickActions from "$lib/components/header/RepoQuickActions.svelte";
  import CustomAppsDialog from "$lib/components/openWith/CustomAppsDialog.svelte";
  import {
    currentEntry,
    customEntries,
    openWithEntry,
    refreshApps,
    type OpenEntry,
  } from "./openWithApps";
  import { appIcons } from "./appIcons";

  let {
    repoId,
    name,
    root,
    status,
    presetId,
    historyVisible,
    canHideHistory,
    onMutated = undefined,
    onToggleHistory,
    onNewTerminal,
    headerOverflow = undefined,
  }: {
    repoId: string;
    /** Repository display name (leftmost, truncated with a tooltip). */
    name: string;
    /** Absolute repository path — the info popover's copyable subject. */
    root: string;
    /** Latest status snapshot; `null` while the first fetch is in flight. */
    status: RepoStatus | null;
    /** Id of the layout preset the workspace currently renders. */
    presetId: string;
    /** Whether the history panel is actually on screen (not behind a rail/tab). */
    historyVisible: boolean;
    /** Hiding history must not empty the workspace. */
    canHideHistory: boolean;
    /** Called after a sync/branch mutation (RepoView refreshes the status). */
    onMutated?: () => void;
    /** History toggle button (RepoView owns the layout mutation). */
    onToggleHistory: () => void;
    /** "＋ Terminal" button: focuses or creates the terminal panel. */
    onNewTerminal: () => void;
    /**
     * Repo-specific overflow commands (bisect, save layout, clean, reset,
     * recents) — rendered inside "More repository actions" after the
     * header-owned "Layout preset" / "Open with" submenus.
     */
    headerOverflow?: Snippet;
  } = $props();

  const iconInfo = $derived(appIcon("info")!);
  const iconHistory = $derived(appIcon("history")!);
  const iconTerminal = $derived(appIcon("terminal")!);
  const iconOpenWith = $derived(appIcon("description")!);
  const iconCustom = $derived(appIcon("terminal")!);
  const iconManage = $derived(appIcon("edit")!);
  const iconPreset = $derived(appIcon("account-tree")!);

  /** The preset's display name (the submenu trigger's summary). */
  const presetName = $derived(layouts.presetById(presetId)?.name ?? "Default");

  // -- remote / root path info ------------------------------------------------

  /**
   * ssh roots show their host in the tooltip (`ssh://user@host:port/path`
   * → `host`); local paths already are the interesting bit. The popover
   * always carries the full, copyable path.
   */
  const rootHost = $derived.by(() => {
    if (!root.startsWith("ssh://")) return root;
    const rest = root.slice("ssh://".length);
    const authority = rest.slice(0, rest.indexOf("/") === -1 ? rest.length : rest.indexOf("/"));
    const at = authority.lastIndexOf("@");
    const hostPort = at === -1 ? authority : authority.slice(at + 1);
    const colon = hostPort.lastIndexOf(":");
    return colon === -1 ? hostPort : hostPort.slice(0, colon);
  });

  let pathInfoOpen = $state(false);
  let pathInfoAnchor = $state<HTMLElement | null>(null);
  let pathInfoEl = $state<HTMLElement | null>(null);

  function togglePathInfo(): void {
    if (pathInfoOpen) {
      pathInfoOpen = false;
      pathInfoAnchor?.focus();
    } else {
      pathInfoOpen = true;
    }
  }

  // The popover is portaled to <body>, so outside-click detection lives on
  // document (BranchSwitcher idiom); Escape closes and restores focus.
  $effect(() => {
    if (!pathInfoOpen) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (pathInfoEl?.contains(target) || pathInfoAnchor?.contains(target)) return;
      pathInfoOpen = false;
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        pathInfoOpen = false;
        pathInfoAnchor?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  });

  // -- open with (overflow submenu) --------------------------------------------

  /** Detected apps; re-probed on mount and on every menu open. */
  let apps = $state<EditorApp[]>([]);
  let launching = $state(false);
  let manageOpen = $state(false);

  $effect(() => {
    void refreshApps().then((found) => {
      apps = found;
    });
  });

  /** Fresh detection per menu open (cheap stat probes). */
  function probeApps(): void {
    void refreshApps().then((found) => {
      apps = found;
    });
  }

  const openWithCurrent = $derived(currentEntry(apps));
  const openWithCustoms = $derived(customEntries());

  async function launch(entry: OpenEntry): Promise<void> {
    if (launching) return;
    launching = true;
    await openWithEntry(root, entry);
    launching = false;
  }
</script>

<header
  class="repo-header flex h-9 min-h-9 flex-none items-center gap-x-2 border-b border-border bg-popover px-2.5 text-ui-base"
>
  <span class="repo-identity flex min-w-0 flex-1 items-center gap-ui-4">
    <Hint label={name}>
      {#snippet child({ props })}
        <span
          class="repo-name max-w-[15rem] min-w-0 flex-[0_1_auto] overflow-hidden text-ellipsis whitespace-nowrap font-semibold"
          {...props}
        >{name}</span>
      {/snippet}
    </Hint>
    <BranchSwitcher {repoId} {status} onMutated={onMutated} />
    <RepoQuickActions {repoId} {status} onMutated={onMutated} />
    <Hint label={root.startsWith("ssh://") ? `Remote host: ${rootHost}` : root}>
      {#snippet child({ props })}
        <Button
          bind:ref={pathInfoAnchor}
          class="root-info wm-narrow:hidden wm-stacked:hidden text-muted-foreground hover:bg-accent hover:text-accent-foreground"
          variant="ghost"
          size="icon-sm"
          type="button"
          {...props}
          aria-label="Repository path"
          aria-haspopup="dialog"
          aria-expanded={pathInfoOpen}
          onclick={togglePathInfo}
        >
          <Icon icon={iconInfo} size={14} />
        </Button>
      {/snippet}
    </Hint>
  </span>
  <span class="layout-controls ml-auto flex flex-none items-center gap-1">
    <Button
      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label={historyVisible ? "Hide history" : "Show history"}
      aria-pressed={historyVisible}
      title={historyVisible
        ? canHideHistory
          ? "Hide the history sidebar"
          : "Expand the primary sidebar before hiding history"
        : "Show the history sidebar"}
      disabled={historyVisible && !canHideHistory}
      onclick={onToggleHistory}
    >
      <Icon icon={iconHistory} size={16} />
    </Button>
    <Hint label="Open (or focus) this repository's terminal panel">
      {#snippet child({ props })}
        <Button
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground"
          variant="ghost"
          size="icon-sm"
          type="button"
          {...props}
          aria-label="Terminal"
          onclick={onNewTerminal}
        >
          <Icon icon={iconTerminal} size={16} />
        </Button>
      {/snippet}
    </Hint>
    <OverflowMenu label="More repository actions" onOpenChange={(open) => open && probeApps()}>
      {#snippet content()}
        <DropdownMenu.Sub>
          <DropdownMenu.SubTrigger class="cursor-pointer gap-ui-3">
            <span class="pointer-events-none inline-flex flex-none items-center justify-center">
              <Icon icon={iconPreset} size={16} />
            </span>
            Layout preset
          </DropdownMenu.SubTrigger>
          <DropdownMenu.SubContent>
            <DropdownMenu.RadioGroup
              value={presetId}
              onValueChange={(value) => layouts.applyPreset(root, value)}
            >
              {#each layouts.listPresets() as preset (preset.id)}
                <DropdownMenu.RadioItem class="cursor-pointer" value={preset.id}>
                  {preset.name}
                </DropdownMenu.RadioItem>
              {/each}
            </DropdownMenu.RadioGroup>
          </DropdownMenu.SubContent>
        </DropdownMenu.Sub>
        <DropdownMenu.Sub>
          <DropdownMenu.SubTrigger
            class="cursor-pointer gap-ui-3"
            disabled={openWithCurrent === undefined && openWithCustoms.length === 0}
          >
            <span class="pointer-events-none inline-flex flex-none items-center justify-center">
              <Icon icon={iconOpenWith} size={16} />
            </span>
            Open with
          </DropdownMenu.SubTrigger>
          <DropdownMenu.SubContent class="max-h-72 overflow-y-auto">
            {#if openWithCurrent}
              <DropdownMenu.Item
                class="cursor-pointer gap-ui-3"
                disabled={launching}
                onclick={() => void launch(openWithCurrent)}
              >
                <span class="pointer-events-none inline-flex flex-none items-center justify-center">
                  <Icon icon={openWithCurrent.custom ? iconCustom : (appIcons[openWithCurrent.id] ?? iconOpenWith)} size={16} />
                </span>
                Open with {openWithCurrent.name}
              </DropdownMenu.Item>
              <DropdownMenu.Separator />
            {/if}
            {#each apps as app (app.id)}
              <DropdownMenu.Item
                class="cursor-pointer gap-ui-3"
                disabled={launching}
                onclick={() => void launch({ id: app.id, name: app.name })}
              >
                <span class="pointer-events-none inline-flex flex-none items-center justify-center">
                  <Icon icon={appIcons[app.id] ?? iconOpenWith} size={16} />
                </span>
                {app.name}
              </DropdownMenu.Item>
            {/each}
            {#if openWithCustoms.length > 0}
              <DropdownMenu.Separator />
              {#each openWithCustoms as entry (entry.id)}
                <DropdownMenu.Item
                  class="cursor-pointer gap-ui-3"
                  disabled={launching}
                  onclick={() => void launch(entry)}
                >
                  <span class="pointer-events-none inline-flex flex-none items-center justify-center">
                    <Icon icon={iconCustom} size={16} />
                  </span>
                  {entry.name}
                </DropdownMenu.Item>
              {/each}
            {/if}
            <DropdownMenu.Separator />
            <DropdownMenu.Item
              class="cursor-pointer gap-ui-3"
              onclick={() => (manageOpen = true)}
            >
              <span class="pointer-events-none inline-flex flex-none items-center justify-center">
                <Icon icon={iconManage} size={16} />
              </span>
              Manage custom apps…
            </DropdownMenu.Item>
          </DropdownMenu.SubContent>
        </DropdownMenu.Sub>
        {#if headerOverflow}
          <DropdownMenu.Separator />
          {@render headerOverflow()}
        {/if}
      {/snippet}
    </OverflowMenu>
  </span>
</header>

{#if pathInfoOpen}
  <!-- Remote/path info popover (BranchSwitcher idiom): the full root path,
       copyable — the header row itself only carries the tooltip host. -->
  <div
    bind:this={pathInfoEl}
    class="popover fixed z-60 flex w-max max-w-[min(40rem,calc(100vw-1rem))] flex-col gap-2 rounded-lg border border-border bg-popover p-3 text-ui-base text-popover-foreground shadow-md"
    role="dialog"
    aria-label="Repository path"
    use:floating={{ anchor: pathInfoAnchor ?? undefined, gap: 6 }}
  >
    <div class="flex items-center gap-2">
      <span class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-ui-meta">
        {root}
      </span>
      <Button
        class="h-6 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
        variant="outline"
        type="button"
        onclick={() => void copyText(root)}
      >
        Copy path
      </Button>
    </div>
  </div>
{/if}

{#if manageOpen}
  <CustomAppsDialog bind:open={manageOpen} />
{/if}
