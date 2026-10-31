<script lang="ts">
  /**
   * Submodule panel — inspect and manage git submodules.
   *
   * Rows use the same compact source-control vocabulary as StatusPanel:
   * folder icon + dimmed dir + bold basename, a short status tag ("new
   * commits", "modified", "not initialized"), and hover-revealed actions.
   * The second line keeps the checked-out vs recorded shas and the URL
   * available without opening a detail view.
   *
   * Actions: Update checks the module out at the recorded gitlink (and
   * initializes it when needed), Sync rewrites the URL from `.gitmodules`
   * into the repo config, Open (initialized modules only) opens the module
   * worktree as a repository tab.
   */
  import { submoduleSync, submoduleUpdate, submodules } from "$lib/ipc/client";
  import type { SubmoduleInfo } from "$lib/ipc/types";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { splitPath, submoduleDisplay } from "./statusModel";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconUpdate from "@ktibow/iconset-material-symbols/download";
  import iconSync from "@ktibow/iconset-material-symbols/sync";
  import iconOpen from "@ktibow/iconset-material-symbols/open-in-new";
  import iconModule from "@ktibow/iconset-material-symbols/folder-special";
  import iconCheck from "@ktibow/iconset-material-symbols/check-circle";

  let {
    repoId,
    root,
    onMutated = undefined,
  }: {
    repoId: string;
    /** This repository's worktree root (join base for "Open as tab"). */
    root: string;
    /** Called after a mutation that changes repo state (update/sync). */
    onMutated?: () => void;
  } = $props();

  let list = $state<SubmoduleInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  /** Any mutation in flight (buttons disabled while true). */
  let busy = $state(false);
  /** Toolbar flags applied by the per-row Update action. */
  let init = $state(false);
  let recursive = $state(false);

  $effect(() => {
    // Reload when the repo switches.
    void repoId;
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      list = await submodules(repoId);
    } catch (err) {
      error = describeError(err);
    } finally {
      loading = false;
    }
  }

  function describeError(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function fail(what: string, err: unknown): void {
    toast(`${what} failed: ${describeError(err)}`, { kind: "error" });
  }

  function shortSha(sha: string | null): string {
    return sha === null || sha === "" ? "—" : sha.slice(0, 7);
  }

  /** head ≠ recorded: the module has commits not captured by the parent. */
  function updateAvailable(info: SubmoduleInfo): boolean {
    return (
      info.initialized &&
      info.head_sha !== null &&
      info.head_sha !== info.recorded_sha
    );
  }

  /** Absolute worktree path of a submodule (portable separator). */
  function modulePath(info: SubmoduleInfo): string {
    return `${root.replace(/[\\/]+$/, "")}/${info.path}`;
  }

  function title(info: SubmoduleInfo): string {
    const display = submoduleDisplay(info);
    const lines = [info.path, display.detail];
    if (info.url) lines.push(info.url);
    lines.push(
      `HEAD ${shortSha(info.head_sha)} · recorded ${shortSha(info.recorded_sha)}`,
    );
    return lines.join("\n");
  }

  async function onUpdate(info: SubmoduleInfo): Promise<void> {
    if (busy) return;
    busy = true;
    const initialize = init || !info.initialized;
    try {
      await submoduleUpdate(repoId, info.path, initialize, recursive);
      toast(
        initialize
          ? `Initialized submodule ${info.path}`
          : `Updated submodule ${info.path}`,
        { kind: "success" },
      );
      void reload();
      onMutated?.();
    } catch (err) {
      fail("Submodule update", err);
    } finally {
      busy = false;
    }
  }

  async function onSync(info: SubmoduleInfo): Promise<void> {
    if (busy) return;
    busy = true;
    try {
      await submoduleSync(repoId, info.path);
      toast(`Synced submodule ${info.path}`, { kind: "success" });
      void reload();
    } catch (err) {
      fail("Submodule sync", err);
    } finally {
      busy = false;
    }
  }

  async function onOpen(info: SubmoduleInfo): Promise<void> {
    if (!info.initialized) {
      toast("Submodule is not initialized — update it first", {
        kind: "error",
      });
      return;
    }
    try {
      await openTab(modulePath(info));
    } catch (err) {
      fail("Open", err);
    }
  }
</script>

<aside class="submodule-panel flex min-h-0 flex-col bg-card text-[13px] text-foreground" aria-label="Submodules">
  <div class="toolbar flex flex-none items-center gap-2 border-b border-border bg-muted px-1.5 py-1">
    <label
      class="toggle inline-flex cursor-pointer items-center gap-0.5 text-[0.6875rem] text-muted-foreground focus-within:text-foreground"
      title="Also initialize uninitialized submodules"
    >
      <input type="checkbox" class="m-0 size-3.5 accent-primary" bind:checked={init} />
      <span>init</span>
    </label>
    <label
      class="toggle inline-flex cursor-pointer items-center gap-0.5 text-[0.6875rem] text-muted-foreground focus-within:text-foreground"
      title="Update nested submodules too"
    >
      <input type="checkbox" class="m-0 size-3.5 accent-primary" bind:checked={recursive} />
      <span>recursive</span>
    </label>
    <span class="spacer flex-1"></span>
    <span
      class="count inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-muted px-1.5 py-0 text-[0.625rem] font-medium tabular-nums text-muted-foreground"
      title="Submodules in this repository"
    >{list.length}</span>
    <Button
      class={`relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground ${loading ? "[&_svg]:animate-spin" : ""} [&_svg:not([class*='size-'])]:size-4`}
      variant="ghost"
      size="icon-xs"
      type="button"
      aria-label="Refresh submodules"
      title={loading ? "Refreshing submodules…" : "Refresh submodules"}
      disabled={loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
  </div>

  {#if error}
    <div class="state error flex flex-col items-start gap-1.5 py-4 px-3 text-[var(--app-deleted)]" role="alert">
      <p class="m-0">{error}</p>
      <Button
        type="button"
        variant="outline"
        class="h-auto rounded border-[var(--app-deleted)] bg-[var(--app-deleted-bg)] px-2 py-0.5 text-[0.6875rem] font-normal text-[var(--app-deleted)] hover:bg-[var(--app-deleted-bg)] dark:bg-[var(--app-deleted-bg)] dark:text-[var(--app-deleted)] dark:hover:bg-[var(--app-deleted-bg)]"
        onclick={() => void reload()}
      >Retry</Button>
    </div>
  {:else if loading && list.length === 0}
    <p class="state py-4 px-3 text-xs text-muted-foreground" role="status">Loading submodules…</p>
  {:else if list.length === 0}
    <div class="state empty flex flex-1 select-none flex-col items-center justify-center gap-1 px-4 py-8 text-center text-muted-foreground" role="status">
      <span class="empty-icon mb-0.5 inline-flex size-10 items-center justify-center rounded-full border border-border text-[var(--app-added)]"><Icon icon={iconCheck} size={22} /></span>
      <p class="empty-title m-0 text-[13px] font-medium text-foreground">No submodules</p>
      <p class="empty-hint m-0 text-[0.72rem]">Add one with <code class="font-mono">git submodule add</code>.</p>
    </div>
  {:else}
    <ul class="modules m-0 min-h-0 flex-1 list-none overflow-y-auto p-0 pb-2" role="list" aria-label="Submodule list" aria-busy={loading}>
      {#each list as info (info.path)}
        {@const display = submoduleDisplay(info)}
        {@const parts = splitPath(info.path)}
        <li class="module group border-b border-border/55 px-1.5 py-1 hover:bg-muted/60" class:danger={display.tone === "danger"}>
          <div class="line flex min-w-0 items-center gap-1 [&>svg]:shrink-0 [&>svg]:text-muted-foreground">
            <Icon icon={iconModule} size={14} />
            <button
              class="open flex min-w-0 flex-1 cursor-pointer items-center gap-1 border-0 bg-transparent p-0 text-left font-[inherit] text-inherit focus-visible:rounded-sm focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-1"
              type="button"
              title={title(info)}
              onclick={() => void onOpen(info)}
            >
              <span class="path min-w-0 truncate font-mono text-xs">
                {#if parts.dir}<span class="dir text-muted-foreground opacity-80">{parts.dir}</span>{/if}
                <span class="base font-semibold {display.tone === "danger" ? "text-[var(--app-deleted)]" : "text-foreground"}">{parts.base}</span>
              </span>
              {#if display.tag}
                <span
                  class={`tag max-w-36 shrink-0 truncate text-[0.625rem] leading-[1.35] ${display.tone === "danger" ? "text-[var(--app-deleted)]" : display.tone === "dirty" ? "text-[var(--app-modified)]" : "text-muted-foreground"}`}
                  title={display.detail}
                >
                  {display.tag}
                </span>
              {/if}
            </button>
            <span class="actions inline-flex shrink-0 items-center gap-px opacity-0 transition-opacity duration-100 group-hover:opacity-100 group-focus-within:opacity-100 [@media(pointer:coarse)]:opacity-100">
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`${info.initialized ? "Update" : "Initialize"} submodule ${info.path}`}
                title={info.initialized
                  ? "Check out the commit the superproject records"
                  : "Initialize and update this submodule"}
                tabindex={-1}
                disabled={busy}
                onclick={() => void onUpdate(info)}
              >
                <Icon icon={iconUpdate} size={16} />
              </Button>
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Sync submodule ${info.path}`}
                title="Rewrite the URL from .gitmodules into the repo config"
                tabindex={-1}
                disabled={busy}
                onclick={() => void onSync(info)}
              >
                <Icon icon={iconSync} size={16} />
              </Button>
              {#if info.initialized}
                <Button
                  class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={`Open submodule ${info.path} as a tab`}
                  title="Open this submodule as a tab"
                  tabindex={-1}
                  onclick={() => void onOpen(info)}
                >
                  <Icon icon={iconOpen} size={16} />
                </Button>
              {/if}
            </span>
          </div>
          <div class="meta flex min-w-0 items-center gap-1 whitespace-nowrap pl-5 font-mono text-[0.625rem] text-muted-foreground opacity-85" title="Checked-out HEAD vs recorded gitlink">
            {#if updateAvailable(info)}
              <span class="update shrink-0 font-semibold text-[var(--app-modified)]">↓ update available</span>
            {/if}
            <span class="head text-primary">HEAD {shortSha(info.head_sha)}</span>
            <span class="sep opacity-50">·</span>
            <span>recorded {shortSha(info.recorded_sha)}</span>
            {#if info.url}
              <span class="url min-w-0 flex-1 truncate text-right opacity-75" title={info.url}>{info.url}</span>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</aside>
