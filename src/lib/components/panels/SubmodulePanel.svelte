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
  import { describeError } from "$lib/errors";
  import { submoduleAdd, submoduleDeinit, submoduleSync, submoduleUpdate, submodules } from "$lib/ipc/client";
  import type { SubmoduleInfo } from "$lib/ipc/types";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import EmptyState from "../layout/EmptyState.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { splitPath, submoduleDisplay } from "./statusModel";
  import { appIcon } from "$lib/components/icons/library.svelte";

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

  const iconRefresh = $derived(appIcon("refresh")!);
  const iconUpdate = $derived(appIcon("download")!);
  const iconSync = $derived(appIcon("sync")!);
  const iconOpen = $derived(appIcon("open-in-new")!);
  const iconModule = $derived(appIcon("folder-special")!);
  const iconCheck = $derived(appIcon("check-circle")!);
  const iconAdd = $derived(appIcon("add")!);
  const iconDeinit = $derived(appIcon("delete-sweep")!);

  // "Add submodule" inline form (Phase 5 `submodule_add`): url + path +
  // optional module name; client-side URL sanity (no leading dash — an
  // option-shaped value would be parsed as a git flag backend-side).
  let formOpen = $state(false);
  let addUrl = $state("");
  let addPath = $state("");
  let addName = $state("");

  /** True when a value is empty or starts with "-" (option-shaped). */
  function optionShaped(value: string): boolean {
    const trimmed = value.trim();
    return trimmed === "" || trimmed.startsWith("-");
  }

  /** All three fields non-empty, sane, path without a leading dash. */
  const addValid = $derived(
    !optionShaped(addUrl) &&
      !optionShaped(addPath) &&
      (addName.trim() === "" || !optionShaped(addName)),
  );

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
    // Stale-response guard: the panel is keep-alive across repo switches —
    // a slow submodule list from repo A must never land after the switch.
    const id = repoId;
    loading = true;
    error = null;
    try {
      const fresh = await submodules(id);
      if (id !== repoId) return;
      list = fresh;
    } catch (err) {
      if (id !== repoId) return;
      error = describeError(err);
    } finally {
      if (id === repoId) loading = false;
    }
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

  // -- add submodule (Phase 5) --------------------------------------------------

  async function onAdd(): Promise<void> {
    if (!addValid || busy) return;
    busy = true;
    try {
      await submoduleAdd(
        repoId,
        addUrl.trim(),
        addPath.trim(),
        addName.trim() || undefined,
      );
      toast(`Added submodule ${addPath.trim()}`, { kind: "success" });
      addUrl = "";
      addPath = "";
      addName = "";
      formOpen = false;
      void reload();
      onMutated?.();
    } catch (err) {
      fail("Add submodule", err);
    } finally {
      busy = false;
    }
  }

  // -- deinit (Phase 5) ---------------------------------------------------------

  /** Submodule pending a deinit confirmation. */
  let deinitCandidate = $state<SubmoduleInfo | null>(null);

  async function onDeinitConfirmed(): Promise<void> {
    const info = deinitCandidate;
    if (!info || busy) return;
    deinitCandidate = null;
    busy = true;
    try {
      await submoduleDeinit(repoId, info.path);
      toast(`Deinitialized submodule ${info.path}`, { kind: "success" });
      void reload();
      onMutated?.();
    } catch (err) {
      fail("Submodule deinit", err);
    } finally {
      busy = false;
    }
  }
</script>

<aside class="submodule-panel flex min-h-0 flex-col bg-card text-ui-base text-foreground" aria-label="Submodules">
  <div class="toolbar flex flex-none items-center gap-2 border-b border-border bg-muted px-1.5 py-1">
    <Hint label="Also initialize uninitialized submodules">
      {#snippet child({ props })}
        <label
          class="toggle inline-flex cursor-pointer items-center gap-0.5 text-ui-micro text-muted-foreground focus-within:text-foreground"
          {...props}
        >
          <input type="checkbox" class="m-0 size-3.5 accent-primary" bind:checked={init} />
          <span>init</span>
        </label>
      {/snippet}
    </Hint>
    <Hint label="Update nested submodules too">
      {#snippet child({ props })}
        <label
          class="toggle inline-flex cursor-pointer items-center gap-0.5 text-ui-micro text-muted-foreground focus-within:text-foreground"
          {...props}
        >
          <input type="checkbox" class="m-0 size-3.5 accent-primary" bind:checked={recursive} />
          <span>recursive</span>
        </label>
      {/snippet}
    </Hint>
    <span class="spacer flex-1"></span>
    <Hint label="Submodules in this repository">
      {#snippet child({ props })}
        <span
          class="count inline-flex h-auto shrink-0 items-center rounded-full border border-transparent bg-muted px-1.5 py-0 text-ui-micro font-medium tabular-nums text-muted-foreground"
          {...props}
        >{list.length}</span>
      {/snippet}
    </Hint>
    <Button
      class={`relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground ${loading ? "[&_svg]:animate-spin" : ""} aria-pressed:bg-accent aria-pressed:text-accent-foreground [&_svg:not([class*='size-'])]:size-4`}
      variant="ghost"
      size="icon-xs"
      type="button"
      aria-label={formOpen ? "Close add-submodule form" : "Add submodule"}
      aria-pressed={formOpen}
      title={formOpen ? "Close the add-submodule form" : "Add a submodule"}
      disabled={busy}
      onclick={() => (formOpen = !formOpen)}
    >
      <Icon icon={iconAdd} size={16} />
    </Button>
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

  {#if formOpen}
    <!-- Add-submodule form: url + path + optional name. Client-side URL
         sanity (no leading dash) keeps option-shaped values off the wire. -->
    <form
      class="add-form flex flex-col gap-1.5 border-b border-border bg-muted/50 px-1.5 py-1.5"
      aria-label="Add submodule"
      onsubmit={(e) => {
        e.preventDefault();
        void onAdd();
      }}
    >
      <Input
        class="h-auto w-full rounded px-2 py-0.5 font-mono text-ui-meta"
        type="text"
        placeholder="repository URL"
        aria-label="Submodule repository URL"
        bind:value={addUrl}
      />
      <div class="flex gap-1.5">
        <Input
          class="h-auto min-w-0 flex-1 rounded px-2 py-0.5 font-mono text-ui-meta"
          type="text"
          placeholder="path, e.g. vendor/lib"
          aria-label="Submodule path"
          bind:value={addPath}
        />
        <Input
          class="h-auto w-32 shrink-0 rounded px-2 py-0.5 font-mono text-ui-meta"
          type="text"
          placeholder="name (optional)"
          aria-label="Submodule name (optional)"
          bind:value={addName}
        />
      </div>
      <div class="flex items-center justify-end gap-1.5">
        <Button
          class="h-auto rounded-full px-3 py-0.5 text-ui-meta"
          type="submit"
          disabled={busy || !addValid}
          title={addValid ? "Add the submodule" : "URL and path are required and may not start with a dash"}
        >
          Add
        </Button>
      </div>
    </form>
  {/if}

  {#if error}
    <div class="state error flex flex-col items-start gap-1.5 py-4 px-3 text-[var(--app-deleted)]" role="alert">
      <p class="m-0">{error}</p>
      <Button
        type="button"
        variant="outline"
        class="h-auto rounded border-[var(--app-deleted)] bg-[var(--app-deleted-bg)] px-2 py-0.5 text-ui-micro font-normal text-[var(--app-deleted)] hover:bg-[var(--app-deleted-bg)] dark:bg-[var(--app-deleted-bg)] dark:text-[var(--app-deleted)] dark:hover:bg-[var(--app-deleted-bg)]"
        onclick={() => void reload()}
      >Retry</Button>
    </div>
  {:else if loading && list.length === 0}
    <p class="state py-4 px-3 text-xs text-muted-foreground" role="status">Loading submodules…</p>
  {:else if list.length === 0}
    <EmptyState icon={iconCheck} title="No submodules" hint="Add one with git submodule add." />
  {:else}
    <ul class="modules m-0 min-h-0 flex-1 list-none overflow-y-auto p-0 pb-2" role="list" aria-label="Submodule list" aria-busy={loading}>
      {#each list as info (info.path)}
        {@const display = submoduleDisplay(info)}
        {@const parts = splitPath(info.path)}
        <li class="module group border-b border-border/55 px-1.5 py-1 hover:bg-muted/60" class:danger={display.tone === "danger"}>
          <div class="line flex min-w-0 items-center gap-1 [&>svg]:shrink-0 [&>svg]:text-muted-foreground">
            <Icon icon={iconModule} size={14} />
            <Hint label={title(info)}>
              {#snippet child({ props })}
                <button
                  class="open flex min-w-0 flex-1 cursor-pointer items-center gap-1 border-0 bg-transparent p-0 text-left font-[inherit] text-inherit focus-visible:rounded-sm focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-1"
                  type="button"
                  {...props}
                  onclick={() => void onOpen(info)}
                >
                  <span class="path min-w-0 truncate font-mono text-xs">
                    {#if parts.dir}<span class="dir text-muted-foreground opacity-80">{parts.dir}</span>{/if}
                    <span class="base font-semibold {display.tone === "danger" ? "text-[var(--app-deleted)]" : "text-foreground"}">{parts.base}</span>
                  </span>
                  {#if display.tag}
                    <Hint label={display.detail}>
                      {#snippet child({ props: tagProps })}
                        <span
                          class={`tag max-w-36 shrink-0 truncate text-ui-micro leading-[1.35] ${display.tone === "danger" ? "text-[var(--app-deleted)]" : display.tone === "dirty" ? "text-[var(--app-modified)]" : "text-muted-foreground"}`}
                          {...tagProps}
                        >
                          {display.tag}
                        </span>
                      {/snippet}
                    </Hint>
                  {/if}
                </button>
              {/snippet}
            </Hint>
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
                <!-- Phase 5 deinit: clears the module's workdir contents
                     (confirm first — not checkpointed upstream). -->
                <Hint label="Deinit this submodule (clears its working tree contents)">
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                      variant="ghost"
                      size="icon-xs"
                      type="button"
                      aria-label={`Deinit submodule ${info.path}`}
                      tabindex={-1}
                      disabled={busy}
                      onclick={() => (deinitCandidate = info)}
                    >
                      <Icon icon={iconDeinit} size={16} />
                    </Button>
                  {/snippet}
                </Hint>
              {/if}
              {#if info.initialized}
                <Hint label="Open this submodule as a tab">
                  {#snippet child({ props })}
                    <Button
                      class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                      variant="ghost"
                      size="icon-xs"
                      type="button"
                      {...props}
                      aria-label={`Open submodule ${info.path} as a tab`}
                      tabindex={-1}
                      onclick={() => void onOpen(info)}
                    >
                      <Icon icon={iconOpen} size={16} />
                    </Button>
                  {/snippet}
                </Hint>
              {/if}
            </span>
          </div>
          <Hint label="Checked-out HEAD vs recorded gitlink">
            {#snippet child({ props })}
              <div class="meta flex min-w-0 items-center gap-1 whitespace-nowrap pl-5 font-mono text-ui-meta text-muted-foreground opacity-85" {...props}>
                {#if updateAvailable(info)}
                  <span class="update shrink-0 font-semibold text-[var(--app-modified)]">↓ update available</span>
                {/if}
                <span class="head text-primary">HEAD {shortSha(info.head_sha)}</span>
                <span class="sep opacity-50">·</span>
                <span>recorded {shortSha(info.recorded_sha)}</span>
                {#if info.url}
                  <Hint label={info.url}>
                    {#snippet child({ props: urlProps })}
                      <span class="url min-w-0 flex-1 truncate text-right opacity-75" {...urlProps}>{info.url}</span>
                    {/snippet}
                  </Hint>
                {/if}
              </div>
            {/snippet}
          </Hint>
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<ConfirmDialog
  bind:open={
    () => deinitCandidate !== null,
    (v) => {
      if (!v) deinitCandidate = null;
    }
  }
  title={deinitCandidate ? `Deinit submodule “${deinitCandidate.path}”?` : ""}
  message={deinitCandidate
    ? `${deinitCandidate.url}\nThe module's working tree contents are removed and it becomes uninitialized (the .gitmodules entry stays). This cannot be undone via the app's checkpoints.`
    : ""}
  confirmLabel="Deinit submodule"
  danger
  onConfirm={() => void onDeinitConfirmed()}
/>
