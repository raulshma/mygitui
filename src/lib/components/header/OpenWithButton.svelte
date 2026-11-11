<!--
  OpenWithButton — repo header "Open with…" split button.

  The primary segment opens the repository root with the last-used app
  (persisted `mygitui.openwith.last`, default: the platform file manager);
  the chevron segment opens the global context menu listing every detected
  app with its brand icon (appIcons.ts), then — after a divider — the
  user-defined custom actions (`$lib/openwith/customApps.svelte`, generic
  terminal glyph) and a "Manage custom apps…" item. Detection re-runs on
  every menu open (cheap stat probes, so newly installed apps appear
  without a restart); ids resolve backend-side against a fresh probe, so
  only machine-verified executables are ever spawned for detected apps.
  Custom ids are namespaced `custom:<uuid>` in the last-used key and spawn
  the user's own command line via `open_with_custom`.
-->
<script lang="ts" module>
  import { detectEditors, openWith, openWithCustom } from "$lib/ipc/client";
  import type { EditorApp } from "$lib/ipc/types";
  import { toast } from "$lib/toast";

  const LAST_KEY = "mygitui.openwith.last";

  /** Detection failure is surfaced once per session, then stays quiet. */
  let detectionToasted = false;

  /**
   * One detection pass. Called on mount and again on every menu open (the
   * probes are a handful of `stat`s); a failure toasts once per session
   * and resolves as "no apps".
   */
  function refreshApps(): Promise<EditorApp[]> {
    return detectEditors()
      .then((found) => (Array.isArray(found) ? found : []))
      .catch((err: unknown) => {
        if (!detectionToasted) {
          detectionToasted = true;
          toast(
            `Could not detect editors: ${describeError(err)}`,
            { kind: "error" },
          );
        }
        return [] as EditorApp[];
      });
  }

  function readLast(): string {
    try {
      return localStorage.getItem(LAST_KEY) ?? "explorer";
    } catch {
      return "explorer";
    }
  }

  function persistLast(id: string): void {
    try {
      localStorage.setItem(LAST_KEY, id);
    } catch {
      // Persistence is best-effort.
    }
  }
</script>

<script lang="ts">
  import type { IconifyIcon } from "@iconify/types";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import CustomAppsDialog from "$lib/components/openWith/CustomAppsDialog.svelte";
  import {
    contextMenu,
    type MenuEntry,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import {
    CUSTOM_ID_PREFIX,
    customApps,
    fromMenuId,
    isCustomId,
    type CustomApp,
  } from "$lib/openwith/customApps.svelte";
  import { appIcons } from "./appIcons";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { describeError } from "$lib/errors";

  let { root }: { root: string } = $props();

  const iconDescription = $derived(appIcon("description")!);
  const iconExpandMore = $derived(appIcon("chevron-down")!);
  /** Distinct generic glyph for user-defined custom actions. */
  const iconCustom = $derived(appIcon("terminal")!);
  const iconManage = $derived(appIcon("edit")!);

  /** Menu/primary entry: a detected app or a user-defined custom action. */
  interface OpenEntry {
    /** `custom:<uuid>` for custom apps, else the detected id. */
    id: string;
    name: string;
    /** Present for custom entries (drives the spawn + the icon). */
    custom?: CustomApp;
  }

  let apps = $state<EditorApp[]>([]);
  let lastId = $state(readLast());
  let launching = $state(false);
  let manageOpen = $state(false);

  $effect(() => {
    void refreshApps().then((found) => {
      apps = found;
    });
  });

  /** Custom actions, namespaced so they never collide with detected ids. */
  const customEntries = $derived<OpenEntry[]>(
    customApps.apps.map((app) => ({
      id: `${CUSTOM_ID_PREFIX}${app.id}`,
      name: app.name,
      custom: app,
    })),
  );

  /** Detected apps first, custom actions after (menu order). */
  const entries = $derived<OpenEntry[]>([
    ...apps.map((a) => ({ id: a.id, name: a.name })),
    ...customEntries,
  ]);

  /** Last-used custom app, when the persisted id points at one that exists. */
  const lastCustom = $derived.by((): OpenEntry | undefined => {
    if (!isCustomId(lastId)) return undefined;
    const app = customApps.byId(fromMenuId(lastId));
    return app ? { id: lastId, name: app.name, custom: app } : undefined;
  });

  /** Last-used app, falling back to the file manager (always present). */
  const current = $derived(
    lastCustom ??
      entries.find((e) => e.id === lastId) ??
      entries.find((e) => e.id === "explorer") ??
      entries[0],
  );

  function iconFor(entry: OpenEntry): IconifyIcon {
    return entry.custom ? iconCustom : (appIcons[entry.id] ?? iconDescription);
  }

  function openWithApp(entry: OpenEntry): void {
    if (launching) return;
    launching = true;
    lastId = entry.id;
    persistLast(entry.id);
    const pending = entry.custom
      ? openWithCustom(root, entry.custom)
      : openWith(root, entry.id);
    pending
      .catch((err: unknown) =>
        toast(
          `Could not open ${entry.name}: ${describeError(err)}`,
          { kind: "error" },
        ),
      )
      .finally(() => {
        launching = false;
      });
  }

  async function openMenu(event: MouseEvent): Promise<void> {
    if (entries.length === 0) return;
    // showMenuAt would normally do these; the menu opens only after the
    // re-detection below, so they happen here instead (synchronously).
    event.preventDefault();
    event.stopPropagation();
    const { clientX: x, clientY: y } = event;
    // Fresh detection per open: a handful of stat probes, so an app
    // installed since the last open is found without an app restart.
    apps = await refreshApps();

    const items: MenuEntry[] = apps.map((app) => ({
      id: app.id,
      label: app.name,
      icon: iconFor({ id: app.id, name: app.name }),
      run: () => openWithApp({ id: app.id, name: app.name }),
    }));
    if (customEntries.length > 0) {
      items.push({ separator: true });
      for (const entry of customEntries) {
        items.push({
          id: entry.id,
          label: entry.name,
          icon: iconFor(entry),
          run: () => openWithApp(entry),
        });
      }
    }
    items.push({ separator: true });
    items.push({
      id: "manage-custom-apps",
      label: "Manage custom apps…",
      icon: iconManage,
      run: () => {
        manageOpen = true;
      },
    });
    contextMenu.show(x, y, items);
  }
</script>

<div
  class="open-with inline-flex flex-none items-stretch overflow-hidden rounded-sm border border-border"
  role="group"
  aria-label="Open repository with an app"
>
  <Hint label={current ? `Open with ${current.name}` : "Detecting apps…"}>
    {#snippet child({ props })}
      <Button
        class="primary text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
        variant="ghost"
        size="icon-xs"
        type="button"
        {...props}
        disabled={!current || launching || contextMenu.open}
        aria-label={current ? `Open with ${current.name}` : "Open with"}
        onclick={() => current && openWithApp(current)}
      >
        {#if current}
          <Icon icon={iconFor(current)} size={14} />
        {:else}
          <Icon icon={iconDescription} size={14} />
        {/if}
      </Button>
    {/snippet}
  </Hint>
  <Button
    class="chev border-l border-border text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
    variant="ghost"
    size="icon-xs"
    type="button"
    aria-label="Choose app to open the repository with"
    aria-haspopup="menu"
    disabled={entries.length === 0}
    onclick={openMenu}
  >
    <Icon icon={iconExpandMore} size={14} />
  </Button>
</div>

{#if manageOpen}
  <CustomAppsDialog bind:open={manageOpen} />
{/if}
