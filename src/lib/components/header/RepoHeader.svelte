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
   * arrive as the `headerActions` data prop from RepoView, which owns them.
   *
   * Pinning (repoActionPins): every menu entry — the two submenus and each
   * command — can be pinned to the header bar for one-click access. Pinned
   * entries render as icon buttons before the ⋯ menu (submenus open their
   * picker; commands run); a trailing pin toggle on each menu row and a
   * "Pin to bar" item inside the submenus flip the state, right-click on a
   * pinned button unpins it.
   *
   * State arrives via props; user intents are emitted through callback
   * props, and header-exclusive reads (layout presets, open-with apps)
   * live here.
   */
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
  import { repoActionPins } from "$lib/stores/repoActionPins.svelte";
  import { showMenuAt } from "$lib/components/menu/contextMenuStore.svelte";
  import type { HeaderAction } from "./headerActions";
  import {
    currentEntry,
    customEntries,
    openWithEntry,
    refreshApps,
    type OpenEntry,
  } from "./openWithApps";
  import { appIcons } from "./appIcons";
  import { tabStore } from "$lib/stores/tabs.svelte";

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
    headerActions = undefined,
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
     * recents) as data — rendered inside "More repository actions" after
     * the header-owned "Layout preset" / "Open with" submenus, and the
     * source for the pinned header buttons (`repoActionPins`).
     */
    headerActions?: HeaderAction[];
  } = $props();

  const iconInfo = $derived(appIcon("info")!);
  const iconHistory = $derived(appIcon("history")!);
  const iconTerminal = $derived(appIcon("terminal")!);
  const iconOpenWith = $derived(appIcon("description")!);
  const iconCustom = $derived(appIcon("terminal")!);
  const iconManage = $derived(appIcon("edit")!);
  const iconPreset = $derived(appIcon("account-tree")!);
  const iconPin = $derived(appIcon("push-pin")!);

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
  /** The device tab strip entry (a device repo opens through SSH remote). */
  const openWithDeviceId = $derived(
    tabStore.tabs.find((tab) => tab.id === repoId)?.deviceId ?? null,
  );
  /**
   * Detected apps that cannot open a device-side folder (no SSH-remote
   * support): disabled in the menu with a why-tooltip on device tabs,
   * instead of a toast after a failed launch. Custom apps stay enabled —
   * the user's own command lines may well know how to reach the device.
   */
  const SSH_REMOTE_APPS = ["vscode", "vscode-insiders", "vscodium", "cursor", "windsurf"];
  const appNeedsLocal = (id: string): boolean =>
    openWithDeviceId !== null && !SSH_REMOTE_APPS.includes(id);

  async function launch(entry: OpenEntry): Promise<void> {
    if (launching) return;
    launching = true;
    await openWithEntry(root, entry, openWithDeviceId);
    launching = false;
  }

  // -- pinnable menu entries (repoActionPins) -----------------------------------

  /**
   * Every "More repository actions" entry can be pinned to the header bar
   * (the railPins idiom): a pinned submenu opens its picker directly, a
   * pinned command runs it. The two header-owned submenu ids share the
   * pin list with RepoView's command ids — one list, one order.
   */
  const LAYOUT_PRESET_PIN_ID = "layout-preset";
  const OPEN_WITH_PIN_ID = "open-with";

  /** Right-click on a pinned bar button unpins it. */
  function pinnedContext(event: MouseEvent, id: string): void {
    showMenuAt(event, [
      {
        id: `unpin-${id}`,
        label: "Unpin from bar",
        icon: iconPin,
        run: () => repoActionPins.unpin(id),
      },
    ]);
  }

  /** The menu row's trailing pin toggle (ContextMenu entry-action idiom). */
  function togglePin(event: Event, id: string): void {
    // Keep the click from activating the menu row under the button.
    event.preventDefault();
    event.stopPropagation();
    repoActionPins.toggle(id);
  }
</script>

<!-- Shared submenu bodies: each renders inside the overflow's submenu AND
     the pinned bar button's picker (repoActionPins pins the whole submenu,
     so both surfaces must offer the same entries). -->
{#snippet presetsEntries()}
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
{/snippet}

{#snippet openWithEntries()}
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
      disabled={launching || appNeedsLocal(app.id)}
      title={appNeedsLocal(app.id)
        ? `${app.name} cannot open repositories on the device — it has no SSH remote support (VS Code, VSCodium, Cursor, and Windsurf do)`
        : undefined}
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
{/snippet}

<!-- The submenu pin rows share the submenu bodies' surface (one per
     submenu, above its entries; flipping keeps the submenu open). -->
{#snippet pinRow(id: string, pinnedLabel: string)}
  <DropdownMenu.Item
    class="cursor-pointer gap-ui-3"
    closeOnSelect={false}
    onclick={() => repoActionPins.toggle(id)}
  >
    <span class="pointer-events-none inline-flex flex-none items-center justify-center">
      <Icon icon={iconPin} size={16} />
    </span>
    {repoActionPins.has(id) ? `Unpin ${pinnedLabel} from bar` : `Pin ${pinnedLabel} to bar`}
  </DropdownMenu.Item>
  <DropdownMenu.Separator />
{/snippet}

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
    <!-- Pinned menu entries (repoActionPins): pin-order icon buttons —
         submenu pins open their picker, command pins run. Right-click
         unpins (railPins idiom). -->
    {#each repoActionPins.pins as pinId (pinId)}
      {#if pinId === LAYOUT_PRESET_PIN_ID}
        <DropdownMenu.Root>
          <Hint label="Layout preset">
            {#snippet child({ props })}
              <DropdownMenu.Trigger
                {...props}
                class="inline-flex size-ui-hit flex-none cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                type="button"
                aria-label="Layout preset"
                aria-haspopup="menu"
                oncontextmenu={(event) => pinnedContext(event, pinId)}
              >
                <Icon icon={iconPreset} size={16} />
              </DropdownMenu.Trigger>
            {/snippet}
          </Hint>
          <DropdownMenu.Content align="end" class="min-w-40">
            {@render presetsEntries()}
          </DropdownMenu.Content>
        </DropdownMenu.Root>
      {:else if pinId === OPEN_WITH_PIN_ID}
        <DropdownMenu.Root onOpenChange={(open) => open && probeApps()}>
          <Hint label="Open with">
            {#snippet child({ props })}
              <DropdownMenu.Trigger
                {...props}
                class="inline-flex size-ui-hit flex-none cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                type="button"
                aria-label="Open with"
                aria-haspopup="menu"
                oncontextmenu={(event) => pinnedContext(event, pinId)}
              >
                <Icon icon={iconOpenWith} size={16} />
              </DropdownMenu.Trigger>
            {/snippet}
          </Hint>
          <DropdownMenu.Content align="end" class="max-h-72 min-w-40 overflow-y-auto">
            {@render openWithEntries()}
          </DropdownMenu.Content>
        </DropdownMenu.Root>
      {:else}
        {@const action = (headerActions ?? []).find((a) => a.id === pinId)}
        {#if action}
          <Hint label={action.label}>
            {#snippet child({ props })}
              <Button
                {...props}
                class="text-muted-foreground hover:bg-accent hover:text-accent-foreground"
                variant="ghost"
                size="icon-sm"
                type="button"
                disabled={action.disabled}
                aria-label={action.label}
                onclick={action.run}
                oncontextmenu={(event) => pinnedContext(event, pinId)}
              >
                <Icon icon={action.icon} size={16} />
              </Button>
            {/snippet}
          </Hint>
        {/if}
      {/if}
    {/each}
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
            {@render pinRow(LAYOUT_PRESET_PIN_ID, "Layout preset")}
            {@render presetsEntries()}
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
            {@render pinRow(OPEN_WITH_PIN_ID, "Open with")}
            {@render openWithEntries()}
          </DropdownMenu.SubContent>
        </DropdownMenu.Sub>
        {#each headerActions ?? [] as action (action.id)}
          {#if action.gapBefore}
            <DropdownMenu.Separator />
          {/if}
          <DropdownMenu.Item
            class="cursor-pointer gap-ui-3"
            disabled={action.disabled}
            onclick={action.run}
          >
            <span class="pointer-events-none inline-flex flex-none items-center justify-center">
              <Icon icon={action.icon} size={16} />
            </span>
            {action.label}
            <Hint label={repoActionPins.has(action.id) ? "Unpin from bar" : "Pin to bar"} side="left">
              {#snippet child({ props })}
                <!-- svelte-ignore a11y_no_interactive_element_to_noninteractive_role -->
                <button
                  {...props}
                  type="button"
                  class="entry-action -mr-1.5 inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 text-xs text-inherit [font:inherit] opacity-65 hover:bg-accent hover:opacity-100 focus-visible:bg-accent focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-ring group-data-[disabled]/dropdown-menu-item:pointer-events-none group-data-[disabled]/dropdown-menu-item:opacity-30"
                  aria-label={repoActionPins.has(action.id)
                    ? `Unpin ${action.label} from the bar`
                    : `Pin ${action.label} to the bar`}
                  onclick={(event) => togglePin(event, action.id)}
                  onkeydown={(event) => {
                    if (event.key === "Enter" || event.key === " ") togglePin(event, action.id);
                  }}
                >
                  <Icon icon={iconPin} size={14} />
                </button>
              {/snippet}
            </Hint>
          </DropdownMenu.Item>
        {/each}
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
