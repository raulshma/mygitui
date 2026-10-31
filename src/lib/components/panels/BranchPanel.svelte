<script lang="ts">
  /**
   * Branch panel (M2, extended in M3 and M9) — local branches +
   * create/switch/rename/delete/reset/merge, a tags section, merged-branch
   * cleanup, and right-click context menus.
   *
   * Rows show name, short sha, upstream with ↑ahead/↓behind, a "gone" badge
   * when the upstream disappeared, and a HEAD marker on the current branch.
   *
   * Switching with a dirty worktree offers a real choice: "Stash changes"
   * (stash_push → switch) or "Proceed anyway" (forced checkout); clean
   * switches stay one click. Deleting previews the operation first
   * (`ops_preview`): merged branches get a ConfirmDialog; unmerged ones open
   * the PreviewDialog — type the branch name, optionally keep the undo
   * checkpoint (guard_checkpoint "pre-branch-delete"), then force-delete.
   * Those flows (and push/rename) live in the shared
   * {@link branchOpsController} (also used by the header switcher); this
   * panel renders their dialogs via BranchActionDialogs and keeps only what
   * is panel-specific: the create form, the merged-cleanup wizard,
   * "Reset to…" (ResetDialog, gitk semantics), "Merge…" (MergeDialog) and
   * the row context menus. Every mutation reloads the list and notifies the
   * owner (`onMutated` → RepoView refreshStatus).
   */
  import { branchCreate, branchDelete, branchIsMerged, branches } from "$lib/ipc/client";
  import type { BranchInfo } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { onUiEvent } from "$lib/palette/events";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import MergeDialog from "$lib/components/merge/MergeDialog.svelte";
  import TagsSection from "$lib/components/panels/TagsSection.svelte";
  import BranchActionDialogs from "$lib/components/header/BranchActionDialogs.svelte";
  import { branchOpsController } from "$lib/git/branchOps.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconCleanup from "@ktibow/iconset-material-symbols/cleaning-services";
  import iconSwitch from "@ktibow/iconset-material-symbols/swap-horiz";
  import iconCheck from "@ktibow/iconset-material-symbols/check";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconAdd from "@ktibow/iconset-material-symbols/add";
  import iconMore from "@ktibow/iconset-material-symbols/more-vert";

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Called after a successful mutation (RepoView refreshes). */
    onMutated?: () => void;
  } = $props();

  let list = $state<BranchInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  /** Contains-filter (branches-focus-switch focuses it). */
  let filter = $state("");
  let filterEl = $state<HTMLInputElement | undefined>(undefined);
  let createNameEl = $state<HTMLInputElement | undefined>(undefined);

  // Create form
  let newName = $state("");
  let newFrom = $state("");
  let newCheckout = $state(true);

  // Inline row editors
  let renaming = $state<{ old: string; value: string } | null>(null);

  // M3 flows
  let resetTo = $state<string | null>(null);

  // M9 flows
  let merging = $state<string | null>(null);
  /** Clean-merged wizard: candidate list + checked set. */
  let cleanupOpen = $state(false);
  let cleanupCandidates = $state<{ name: string; checked: boolean }[]>([]);
  let cleanupBusy = $state(false);

  /**
   * Branch mutation flows (switch/delete dialogs, push, rename), shared with
   * the header switcher. The controller pins its repoId at construction, so
   * it is recreated when the repo switches — discarding any open switch or
   * delete dialog with it (the reset effect below re-wires reload).
   */
  const ops = $derived(branchOpsController(repoId, onMutated));

  const headBranch = $derived(list.find((b) => b.is_head) ?? null);
  const visibleBranches = $derived(
    filter.trim() === ""
      ? list
      : list.filter((b) => b.name.toLowerCase().includes(filter.trim().toLowerCase())),
  );

  $effect(() => {
    // Reload when the repo switches. Reading `ops` re-runs this when the
    // controller was recreated above, pointing the new instance at reload.
    void repoId;
    ops.setReload(reload);
    renaming = null;
    resetTo = null;
    merging = null;
    filter = "";
    void reload();
  });

  // Palette commands ride the typed event bus.
  $effect(() => {
    const unlisteners = [
      onUiEvent("branches-focus-create", () => {
        createNameEl?.focus();
      }),
      onUiEvent("branches-focus-switch", () => {
        filterEl?.focus();
      }),
      onUiEvent("branches-cleanup", () => void onCleanupStart()),
    ];
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      list = await branches(repoId);
      ops.setList(list);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function afterMutation(): void {
    void reload();
    onMutated?.();
  }

  async function run(label: string, run: () => Promise<void>): Promise<void> {
    try {
      await run();
      afterMutation();
    } catch (err) {
      toast(`${label} failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }

  async function onCreate(): Promise<void> {
    const name = newName.trim();
    if (!name) return;
    await run(`Create branch ${name}`, () =>
      branchCreate(repoId, name, newCheckout, newFrom || undefined),
    );
    newName = "";
    newFrom = "";
    newCheckout = true;
  }

  async function onRename(): Promise<void> {
    if (!renaming) return;
    const value = renaming.value.trim();
    const old = renaming.old;
    renaming = null;
    // No-ops (no IPC, no toast) when the name is empty or unchanged.
    await ops.rename(old, value);
  }

  // -- M9: merged-branch cleanup wizard --------------------------------------

  async function onCleanupStart(): Promise<void> {
    const into = headBranch?.name ?? "HEAD";
    const candidates: { name: string; checked: boolean }[] = [];
    for (const branch of list) {
      if (branch.is_head) continue;
      try {
        if (await branchIsMerged(repoId, branch.name, into)) {
          candidates.push({ name: branch.name, checked: true });
        }
      } catch {
        // Unresolvable branch (unborn etc.) — skip it.
      }
    }
    if (candidates.length === 0) {
      toast(`No branches are fully merged into ${into}`, { kind: "info" });
      return;
    }
    cleanupCandidates = candidates;
    cleanupOpen = true;
  }

  async function onCleanupRun(): Promise<void> {
    const names = cleanupCandidates
      .filter((candidate) => candidate.checked)
      .map((candidate) => candidate.name);
    cleanupOpen = false;
    if (names.length === 0) return;
    cleanupBusy = true;
    let deleted = 0;
    const failed: string[] = [];
    for (const name of names) {
      try {
        await branchDelete(repoId, name, false);
        deleted++;
      } catch {
        failed.push(name);
      }
    }
    cleanupBusy = false;
    if (failed.length === 0) {
      toast(`Deleted ${deleted} merged branch${deleted === 1 ? "" : "es"}`, {
        kind: "success",
      });
    } else {
      toast(
        `Deleted ${deleted}; failed: ${failed.join(", ")}`,
        { kind: "error" },
      );
    }
    afterMutation();
  }

  // -- M9 F1: row context menu -------------------------------------------------

  function branchMenu(event: MouseEvent, branch: BranchInfo): void {
    const entries: MenuEntry[] = [];
    if (!branch.is_head) {
      entries.push(
        {
          id: "switch",
          label: "Switch",
          run: () => void ops.requestSwitch(branch.name),
        },
        { id: "merge", label: "Merge into current…", run: () => (merging = branch.name) },
      );
    }
    entries.push(
      { id: "push", label: "Push", run: () => void ops.push(branch, false) },
      { id: "upstream", label: "Push and set upstream", run: () => void ops.push(branch, true) },
      {
        id: "rename",
        label: "Rename…",
        run: () => (renaming = { old: branch.name, value: branch.name }),
      },
      {
        id: "reset",
        label: "Reset current branch to here…",
        run: () => (resetTo = branch.name),
      },
      { id: "copy", label: "Copy branch name", run: () => void navigator.clipboard.writeText(branch.name) },
    );
    if (!branch.is_head) {
      entries.push({
        id: "delete",
        label: "Delete…",
        danger: true,
        run: () => void ops.requestDelete(branch),
      });
    }
    showMenuAt(event, entries);
  }

  function shortSha(sha: string): string {
    return sha.slice(0, 7);
  }
</script>

<aside class="branch-panel flex min-h-0 flex-col text-[13px]" aria-label="Branches">
  <div class="toolbar flex flex-none items-center gap-2 px-2 py-1.5">
    <input
      class="filter min-w-20 flex-1 rounded border border-input bg-background px-2 py-0.5 text-[0.72rem] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
      type="search"
      bind:this={filterEl}
      bind:value={filter}
      placeholder="Filter branches"
      aria-label="Filter branches"
    />
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh branches"
      title={loading ? "Refreshing branches…" : "Refresh branches"}
      disabled={loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Clean merged branches"
      title="Delete every branch already merged into the current one"
      disabled={cleanupBusy}
      onclick={() => void onCleanupStart()}
    >
      <Icon icon={iconCleanup} size={16} />
    </Button>
    {#if ops.busy}<span class="busy text-[0.72rem] text-muted-foreground" role="status">Branch op running…</span>{/if}
  </div>

  <form
    class="create flex flex-none flex-wrap items-center gap-1.5 px-2 pb-1.5"
    aria-label="Create branch"
    onsubmit={(e) => {
      e.preventDefault();
      void onCreate();
    }}
  >
    <div class="name-row flex min-w-24 flex-1 items-center gap-1">
      <input
        class="name min-w-24 flex-1 rounded border border-input bg-background px-2 py-0.5 text-xs text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
        type="text"
        placeholder="New branch name"
        aria-label="New branch name"
        bind:this={createNameEl}
        bind:value={newName}
      />
      <Button
        class="relative after:absolute after:-inset-1 after:content-[''] [&_svg:not([class*='size-'])]:size-4"
        variant="default"
        size="icon-xs"
        type="submit"
        aria-label="Create branch"
        title="Create the branch"
        disabled={!newName.trim()}
      >
        <Icon icon={iconAdd} size={16} />
      </Button>
    </div>
    <label class="from flex items-center gap-1 text-muted-foreground">
      <span class="fl text-[0.6875rem]">from</span>
      <select
        class="max-w-36 rounded border border-input bg-background px-2 py-0.5 text-[0.72rem] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
        bind:value={newFrom}
        aria-label="Create branch from"
      >
        <option value="">HEAD</option>
        {#each list as b (b.name)}
          <option value={b.name}>{b.name}</option>
        {/each}
      </select>
    </label>
    <label
      class="toggle flex cursor-pointer items-center gap-1 text-[0.72rem] text-muted-foreground"
      title="Check out the new branch after creating it"
    >
      <input type="checkbox" class="m-0 accent-primary" bind:checked={newCheckout} />
      <span>checkout</span>
    </label>
  </form>

  {#if error}
    <p class="state py-4 px-3 text-[13px] text-destructive" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state py-4 px-3 text-[13px] text-muted-foreground">No branches.</p>
  {:else}
    <ul class="branches m-0 min-h-0 flex-1 list-none overflow-y-auto p-0 pb-3" role="list" aria-label="Local branches">
      {#each visibleBranches as branch (branch.name)}
        <li
          class="row flex min-w-0 flex-wrap items-center gap-1.5 py-0.5 pl-1 pr-2 {branch.is_head ? "bg-secondary/60" : "hover:bg-muted/60"}"
          class:head={branch.is_head}
          oncontextmenu={(e) => branchMenu(e, branch)}
        >
          {#if renaming?.old === branch.name}
            <form
              class="rename flex min-w-0 flex-1 items-center gap-1.5"
              aria-label={`Rename ${branch.name}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onRename();
              }}
            >
              <input
                class="name min-w-24 flex-1 rounded border border-input bg-background px-2 py-0.5 text-xs text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
                type="text"
                aria-label={`New name for ${branch.name}`}
                bind:value={renaming.value}
                onkeydown={(e) => {
                  if (e.key === "Escape") renaming = null;
                }}
              />
              <Button
                variant="default"
                size="icon-sm"
                type="submit"
                aria-label="Save new branch name"
                title="Save the new branch name"
              >
                <Icon icon={iconCheck} size={16} />
              </Button>
              <Button
                class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
                variant="ghost"
                size="icon-sm"
                type="button"
                aria-label="Cancel rename"
                title="Cancel rename"
                onclick={() => (renaming = null)}
              >
                <Icon icon={iconClose} size={16} />
              </Button>
            </form>
          {:else}
            <span class="marker w-3.5 shrink-0 text-center text-primary" aria-label="Current branch" title="Current branch">
              {branch.is_head ? "●" : ""}
            </span>
            <span class="bname min-w-12 max-w-44 shrink truncate font-mono text-xs font-semibold text-foreground" title={branch.name}>{branch.name}</span>
            <span class="sha shrink-0 font-mono text-[0.6875rem] text-muted-foreground">{shortSha(branch.sha)}</span>
            {#if branch.upstream}
              <span class="upstream min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted-foreground" title={branch.upstream}>
                {branch.upstream}
                {#if branch.ahead > 0 || branch.behind > 0}
                  <span class="ab pl-1.5 text-primary">↑{branch.ahead} ↓{branch.behind}</span>
                {/if}
                {#if branch.gone}<Badge variant="outline" class="gone ml-1.5 h-auto shrink-0 rounded-full border-destructive px-1.5 py-0 text-[0.625rem] text-destructive">gone</Badge>{/if}
              </span>
            {/if}
            <span class="actions ml-auto flex shrink-0 items-center gap-1">
              {#if !branch.is_head}
                <Button
                  class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={`Switch to ${branch.name}`}
                  title={`Switch to ${branch.name}`}
                  onclick={() => void ops.requestSwitch(branch.name)}
                >
                  <Icon icon={iconSwitch} size={16} />
                </Button>
              {:else}
                <Badge variant="secondary" class="head-chip h-auto shrink-0 rounded-full bg-foreground px-1.5 py-0 text-[0.625rem] font-semibold tracking-[0.04em] text-background">HEAD</Badge>
              {/if}
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`More actions for ${branch.name}`}
                title="Push, merge, rename, reset, or delete this branch"
                aria-haspopup="menu"
                onclick={(event) => branchMenu(event, branch)}
              >
                <Icon icon={iconMore} size={16} />
              </Button>
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <TagsSection {repoId} targets={list.map((b) => b.name)} onMutated={afterMutation} />

  {#if merging}
    <MergeDialog
      open={true}
      {repoId}
      branch={merging}
      onClose={() => (merging = null)}
      onDone={() => {
        merging = null;
        afterMutation();
      }}
    />
  {/if}

  {#if cleanupOpen}
    <DialogShell
      bind:open={cleanupOpen}
      labelledBy="cleanup-title"
      width="24rem"
    >
      {#snippet children()}
        <h2 id="cleanup-title" class="sdlg-title title">Delete merged branches</h2>
        <p class="sdlg-message text">
          Fully merged into <strong>{headBranch?.name ?? "HEAD"}</strong> — no
          commits will be lost. Uncheck anything you want to keep.
        </p>
        <ul class="cleanup-list m-0 mb-3 max-h-56 list-none overflow-y-auto p-0">
          {#each cleanupCandidates as candidate (candidate.name)}
            <li>
              <label class="flex cursor-pointer items-center gap-1.5 py-0.5 font-mono text-xs">
                <input type="checkbox" class="m-0 accent-primary" bind:checked={candidate.checked} />
                <span>{candidate.name}</span>
              </label>
            </li>
          {/each}
        </ul>
      {/snippet}
      {#snippet actions()}
        <div class="actions flex flex-wrap justify-end gap-2">
          <button class="secondary sdlg-secondary" type="button" onclick={() => (cleanupOpen = false)}>
            Cancel
          </button>
          <button
            class="primary sdlg-primary"
            type="button"
            disabled={cleanupCandidates.every((c) => !c.checked)}
            onclick={() => void onCleanupRun()}
          >
            Delete checked
          </button>
        </div>
      {/snippet}
    </DialogShell>
  {/if}

  <!-- Shared switch/delete dialogs (state lives on the ops controller). -->
  <BranchActionDialogs controller={ops} />

  <ResetDialog
    {repoId}
    open={resetTo !== null}
    defaultTarget={resetTo ?? undefined}
    onClose={() => (resetTo = null)}
    onDone={() => {
      resetTo = null;
      afterMutation();
    }}
  />
</aside>
