<!--
  OpenWithButton — repo header "Open with…" split button.

  The primary segment opens the repository root with the last-used app
  (persisted `mygitui.openwith.last`, default: the platform file manager);
  the chevron segment opens the global context menu listing every detected
  app with its brand icon (appIcons.ts). Detection is lazy + session-cached
  (module scope: one backend probe per window); ids resolve backend-side
  against a fresh probe, so only machine-verified executables are ever
  spawned.
-->
<script lang="ts" module>
  import type { IconifyIcon } from "@iconify/types";
  import { detectEditors, openWith } from "$lib/ipc/client";
  import type { EditorApp } from "$lib/ipc/types";
  import { toast } from "$lib/toast";

  const LAST_KEY = "mygitui.openwith.last";

  /** Session-wide detection cache: one backend probe per window. */
  let cachedApps: Promise<EditorApp[]> | null = null;

  function detectOnce(): Promise<EditorApp[]> {
    cachedApps ??= detectEditors().catch((err: unknown) => {
      // Detection failure is surfaced once, then treated as "no apps".
      toast(
        `Could not detect editors: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
      return [] as EditorApp[];
    });
    return cachedApps;
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
  import iconDescription from "@ktibow/iconset-material-symbols/description";
  import iconExpandMore from "@ktibow/iconset-material-symbols/expand-more";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { contextMenu, showMenuAt } from "$lib/components/menu/contextMenuStore.svelte";
  import { appIcons } from "./appIcons";

  let { root }: { root: string } = $props();

  let apps = $state<EditorApp[]>([]);
  let lastId = $state(readLast());
  let launching = $state(false);

  $effect(() => {
    void detectOnce().then((found) => {
      apps = Array.isArray(found) ? found : [];
    });
  });

  /** Last-used app, falling back to the file manager (always present). */
  const current = $derived(
    apps.find((a) => a.id === lastId) ?? apps.find((a) => a.id === "explorer") ?? apps[0],
  );

  function iconFor(app: EditorApp): IconifyIcon {
    return appIcons[app.id] ?? iconDescription;
  }

  function openWithApp(app: EditorApp): void {
    if (launching) return;
    launching = true;
    lastId = app.id;
    persistLast(app.id);
    openWith(root, app.id)
      .catch((err: unknown) =>
        toast(
          `Could not open ${app.name}: ${err instanceof Error ? err.message : String(err)}`,
          { kind: "error" },
        ),
      )
      .finally(() => {
        launching = false;
      });
  }

  function openMenu(event: MouseEvent): void {
    if (apps.length === 0) return;
    showMenuAt(
      event,
      apps.map((app) => ({
        id: app.id,
        label: app.name,
        icon: iconFor(app),
        run: () => openWithApp(app),
      })),
    );
  }
</script>

<div class="open-with" role="group" aria-label="Open repository with an app">
  <button
    class="primary"
    type="button"
    disabled={!current || launching || contextMenu.open}
    title={current ? `Open with ${current.name}` : "Detecting apps…"}
    aria-label={current ? `Open with ${current.name}` : "Open with"}
    onclick={() => current && openWithApp(current)}
  >
    {#if current}
      <Icon icon={iconFor(current)} size={15} />
    {/if}
  </button>
  <button
    class="chev"
    type="button"
    aria-label="Choose app to open the repository with"
    aria-haspopup="menu"
    disabled={apps.length === 0}
    onclick={openMenu}
  >
    <Icon icon={iconExpandMore} size={14} />
  </button>
</div>

<style>
  .open-with {
    display: inline-flex;
    align-items: stretch;
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    overflow: hidden;
  }

  button {
    display: grid;
    place-items: center;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
    padding: 0;
  }
  button:disabled {
    cursor: default;
    opacity: 0.5;
  }

  .primary {
    width: 1.65rem;
    height: 1.45rem;
  }

  .chev {
    width: 1.1rem;
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .open-with:not(:has(button:disabled)) button:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  button:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }
</style>
