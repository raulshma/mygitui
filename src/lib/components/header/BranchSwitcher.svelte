<script lang="ts">
  /**
   * Header branch switcher — the repo header's read-only branch chip turned
   * into a searchable branch popover (VS Code style).
   *
   * The trigger keeps the `branch-chip` look (RepoView header rhythm + the
   * e2e-visible class) and adds a chevron; clicking it opens a portaled
   * panel (the `floating` action, anchored to the trigger) with:
   * - a search input that filters both sections and doubles as the
   *   create-branch field: a non-empty query that is not an exact existing
   *   local branch shows a `+ Create branch "…"` row (Enter with nothing
   *   highlighted runs it too);
   * - "Local branches" rows in BranchPanel's visual language (HEAD dot,
   *   mono name, short sha, upstream ↑n ↓n, "gone" badge). Click/Enter
   *   switches through the shared {@link branchOpsController}; the ⋮ menu
   *   and right-click offer Switch / Push / Push and set upstream / Rename
   *   (PromptDialog, old name preselected) / Copy / Delete;
   * - "Remote branches" rows (ones a local branch already tracks are
   *   hidden). Click/Enter checks out through the controller; the menu
   *   offers Checkout / Copy / Delete remote branch (local ConfirmDialog →
   *   `push --delete`, mirroring RemotePanel's flow and toast texts);
   * - a footer with Fetch (same shared verb + toasts as RepoQuickActions),
   *   Refresh (reload both lists), and "Manage branches" (focus-panel
   *   event → the Branches panel tab).
   *
   * Every branch mutation goes through the shared per-repo
   * {@link branchOpsController}; its dialogs (dirty-worktree choice,
   * merged-delete confirm, unmerged-delete preview) render via the shared
   * BranchActionDialogs HERE — outside the popover — so closing the
   * popover right after an action fires never orphans an open dialog
   * (rename uses this component's own PromptDialog, and the remote delete
   * its own ConfirmDialog). While `controller.busy`, the row list ignores
   * pointer events (dialogs stay operable).
   *
   * Keyboard: the search input is a combobox driving the listbox via
   * `aria-activedescendant` (↑/↓/Home/End/Enter/Esc, like the command
   * palette); rows highlight on hover. Outside clicks and Escape close the
   * popover and return focus to the trigger.
   */
  import type {
    BranchInfo,
    RepoStatus,
    RemoteBranchInfo,
  } from "$lib/ipc/types";
  import { branches, pushRepo, remoteBranches } from "$lib/ipc/client";
  import { branchOpsController } from "$lib/git/branchOps.svelte";
  import {
    fetchDefaultRemote,
    syncSkipMessage,
    syncSuccessMessage,
  } from "$lib/git/sync";
  import { toast } from "$lib/toast";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";
  import {
    showMenuAt,
    type MenuEntry,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import { floating } from "$lib/components/overlay/floating";
  import BranchActionDialogs from "$lib/components/header/BranchActionDialogs.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { IconButton } from "$lib/components/m3";
  import iconBranch from "@ktibow/iconset-material-symbols/account-tree";
  import iconChevronDown from "@ktibow/iconset-material-symbols/expand-more";
  import iconSearch from "@ktibow/iconset-material-symbols/search";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconAdd from "@ktibow/iconset-material-symbols/add";
  import iconMore from "@ktibow/iconset-material-symbols/more-vert";
  import iconFetch from "@ktibow/iconset-material-symbols/cloud-download";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";

  let {
    repoId,
    status,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Latest status snapshot; `null` while the first fetch is in flight. */
    status: RepoStatus | null;
    /** Called after a mutation (RepoView refreshes the repo status). */
    onMutated?: () => void;
  } = $props();

  /** One activatable row in the flat keyboard order. */
  type Row =
    | { kind: "create"; name: string }
    | { kind: "local"; branch: BranchInfo }
    | { kind: "remote"; rb: RemoteBranchInfo };

  let open = $state(false);
  let query = $state("");
  let activeIndex = $state(0);
  let localList = $state<BranchInfo[]>([]);
  let remoteList = $state<RemoteBranchInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let fetchBusy = $state(false);

  let triggerEl = $state<HTMLButtonElement | undefined>(undefined);
  let inputEl = $state<HTMLInputElement | undefined>(undefined);
  let popoverEl = $state<HTMLElement | undefined>(undefined);

  /** Row-menu-driven rename (PromptDialog keyed on the branch's old name). */
  let renaming = $state<{ old: string } | null>(null);
  // PromptDialog submits close() (which runs the functional bind's setter,
  // nulling `renaming`) BEFORE onSubmit fires, so the old name is captured
  // here at menu time instead of read from `renaming` afterwards.
  let renameOld = $state("");
  /** Remote-deletion confirm state (branch to `push --delete`). */
  let remoteDelete = $state<RemoteBranchInfo | null>(null);

  // One controller per repository, recreated when the repoId prop changes:
  // RepoView is mounted un-keyed in App.svelte, so switching repo tabs
  // changes `repoId` while this instance stays alive — a controller pinned
  // to the first id would send mutations to the previous repository. The
  // $derived recreation (BranchPanel's model) also drops any dialog state
  // left open by the old repo. `setReload` wires list refreshes: after
  // every completed mutation the controller re-runs `reload` here and
  // fires the owner's `onMutated` (RepoView status refresh) itself.
  const controller = $derived(branchOpsController(repoId, onMutated));

  $effect(() => {
    controller.setReload(reload);
  });

  /** Chip label — same semantics as RepoView's former `branchLabel`. */
  const chipLabel = $derived.by(() => {
    if (status?.branch) return status.branch;
    if (status?.detached && status.head) {
      return `detached @ ${status.head.slice(0, 7)}`;
    }
    return status?.detached ? "detached" : "unknown";
  });

  const q = $derived(query.trim().toLowerCase());

  const visibleLocal = $derived(
    q === ""
      ? localList
      : localList.filter((b) => b.name.toLowerCase().includes(q)),
  );

  /** Remote branches, minus ones a local branch already tracks. */
  const visibleRemote = $derived.by(() => {
    const tracked = new Set(
      localList.flatMap((b) => (b.upstream ? [b.upstream] : [])),
    );
    return remoteList.filter((rb) => {
      if (tracked.has(`${rb.remote}/${rb.name}`)) return false;
      return (
        q === "" || `${rb.remote}/${rb.name}`.toLowerCase().includes(q)
      );
    });
  });

  /** A non-empty query that isn't an exact local branch offers creation. */
  const showCreate = $derived(
    query.trim() !== "" && !localList.some((b) => b.name === query.trim()),
  );

  /** Flat activation order: create row, locals, remotes. */
  const rows = $derived.by<Row[]>(() => {
    const out: Row[] = [];
    if (showCreate) out.push({ kind: "create", name: query.trim() });
    for (const branch of visibleLocal) out.push({ kind: "local", branch });
    for (const rb of visibleRemote) out.push({ kind: "remote", rb });
    return out;
  });

  // Keep the highlighted option inside the (possibly filtered) list.
  $effect(() => {
    if (activeIndex >= rows.length) {
      activeIndex = Math.max(0, rows.length - 1);
    }
  });

  // Autofocus the search input on open (the floating action has no
  // data-autofocus support — that convention belongs to DialogShell).
  $effect(() => {
    if (open) inputEl?.focus();
  });

  // The controller reads the list for HEAD context (delete previews name
  // the merge base); feed it after every load.
  $effect(() => {
    controller.setList(localList);
  });

  // Repo-tab switch (un-keyed RepoView, prop-only change): drop the
  // previous repository's lists immediately so the chip's popover never
  // renders — or acts on — another repo's branches, and refetch when the
  // switch happened while the popover was open. The effect also depends on
  // `open` (it must fire for switches-while-open), so the guard keeps
  // plain open/close toggles from wiping freshly loaded lists.
  let listedRepo: string | null = null;
  $effect(() => {
    if (repoId === listedRepo) return;
    listedRepo = repoId;
    localList = [];
    remoteList = [];
    error = null;
    if (open) void reload();
  });

  /** Loads both branch lists in parallel (open + after mutations). */
  async function reload(): Promise<void> {
    const forRepo = repoId;
    loading = true;
    error = null;
    try {
      const [local, remote] = await Promise.all([
        branches(repoId),
        remoteBranches(repoId),
      ]);
      // A repo-tab switch while the requests were in flight: the response
      // belongs to the previous repository — discard it (the reset effect
      // has already cleared the lists and started a fresh load).
      if (repoId !== forRepo) return;
      localList = local;
      remoteList = remote;
    } catch (err) {
      if (repoId !== forRepo) return;
      error = err instanceof Error ? err.message : String(err);
    } finally {
      if (repoId === forRepo) loading = false;
    }
  }

  function openPopover(): void {
    query = "";
    activeIndex = 0;
    error = null;
    open = true;
    // Refresh on EVERY open: branches move outside this component too
    // (BranchPanel, the terminal, autofetch), and the chip's status prop
    // alone doesn't keep the HEAD dot / upstream / sha columns honest.
    // The controller additionally refreshes after its own mutations.
    void reload();
  }

  /** Closes, clears the transient query, and restores trigger focus. */
  function close(): void {
    query = "";
    activeIndex = 0;
    open = false;
    triggerEl?.focus();
  }

  function toggle(): void {
    if (open) close();
    else openPopover();
  }

  // The panel is portaled to <body>, so outside-click detection lives on
  // document; presses on the trigger or inside the panel are ignored.
  $effect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (popoverEl?.contains(target) || triggerEl?.contains(target)) return;
      close();
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  });

  // Palette commands / other surfaces ride the typed event bus; events for
  // other repositories are ignored. mode "create" opens the same way — the
  // search input doubles as the create field and is focused on open.
  $effect(() => {
    return onUiEvent("branch-switcher-open", ({ repoId: target }) => {
      if (target !== undefined && target !== repoId) return;
      openPopover();
    });
  });

  function rowKey(row: Row): string {
    switch (row.kind) {
      case "create":
        return `create:${row.name}`;
      case "local":
        return row.branch.name;
      case "remote":
        return `${row.rb.remote}/${row.rb.name}`;
    }
  }

  /** VS Code create row (query isn't an existing local branch). */
  async function createFromQuery(): Promise<void> {
    const name = query.trim();
    if (name === "" || !showCreate) return;
    const ok = await controller.create(name);
    // On success the controller already reloaded the list (setReload) and
    // notified the repo view; only the popover remains to close.
    if (ok) close();
  }

  /**
   * Row activation: the controller's dialogs (dirty-worktree choice, delete
   * confirm/preview) are rendered by this component via
   * BranchActionDialogs, so closing the popover immediately never orphans
   * an open dialog.
   */
  function switchTo(name: string): void {
    void controller.requestSwitch(name);
    close();
  }

  async function checkoutRemote(rb: RemoteBranchInfo): Promise<void> {
    close();
    // The controller toasts + reloads (setReload) + notifies onMutated.
    await controller.checkoutRemote(rb.remote, rb.name);
  }

  /** `push --delete` on the remote branch (RemotePanel's flow + texts). */
  async function deleteRemoteBranch(): Promise<void> {
    const rb = remoteDelete;
    if (!rb) return;
    try {
      await pushRepo(repoId, {
        remote: rb.remote,
        branch: "",
        force: false,
        force_with_lease: false,
        set_upstream: false,
        refs: [rb.name],
        tags: false,
        delete: true,
      });
      toast(`Deleted remote branch ${rb.remote}/${rb.name}`, {
        kind: "success",
      });
      // Not routed through the controller (it is a push --delete), so
      // refresh the lists and the repo status here.
      void reload();
      onMutated?.();
    } catch (err) {
      toast(
        `Delete failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  function renameSubmitted(value: string): void {
    const old = renameOld;
    renaming = null;
    renameOld = "";
    if (old === "" || value === old) return;
    void controller.rename(old, value);
  }

  /** Right-click / ⋮ menu for a local row (BranchPanel's mutation set). */
  function localMenu(event: MouseEvent, branch: BranchInfo): void {
    const entries: MenuEntry[] = [
      {
        id: "switch",
        label: "Switch",
        disabled: branch.is_head,
        run: () => switchTo(branch.name),
      },
      {
        id: "push",
        label: "Push",
        run: () => void controller.push(branch, false),
      },
      {
        id: "upstream",
        label: "Push and set upstream",
        run: () => void controller.push(branch, true),
      },
      {
        id: "rename",
        label: "Rename…",
        run: () => {
          renameOld = branch.name;
          renaming = { old: branch.name };
        },
      },
      {
        id: "copy",
        label: "Copy branch name",
        run: () => void navigator.clipboard.writeText(branch.name),
      },
      { separator: true },
      {
        id: "delete",
        label: "Delete…",
        danger: true,
        disabled: branch.is_head,
        run: () => void controller.requestDelete(branch),
      },
    ];
    showMenuAt(event, entries);
  }

  /** Right-click / ⋮ menu for a remote row. */
  function remoteMenu(event: MouseEvent, rb: RemoteBranchInfo): void {
    const entries: MenuEntry[] = [
      { id: "checkout", label: "Checkout", run: () => void checkoutRemote(rb) },
      {
        id: "copy",
        label: "Copy name",
        run: () =>
          void navigator.clipboard.writeText(`${rb.remote}/${rb.name}`),
      },
      { separator: true },
      {
        id: "delete",
        label: "Delete remote branch…",
        danger: true,
        run: () => (remoteDelete = rb),
      },
    ];
    showMenuAt(event, entries);
  }

  /** Footer Fetch: same shared verb + toast texts as RepoQuickActions. */
  async function onFetch(): Promise<void> {
    if (fetchBusy) return;
    fetchBusy = true;
    try {
      const outcome = await fetchDefaultRemote(repoId);
      if (outcome.kind === "skipped") {
        toast(syncSkipMessage(outcome.reason, "fetch"), { kind: "error" });
        return;
      }
      toast(syncSuccessMessage("fetch", outcome), { kind: "success" });
      // New remote branches may have appeared.
      void reload();
    } catch (err) {
      toast(
        `Fetch failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      fetchBusy = false;
    }
  }

  function manageBranches(): void {
    close();
    emitUiEvent("focus-panel", { panel: "branches", repoId });
  }

  function activate(row: Row): void {
    if (row.kind === "create") void createFromQuery();
    else if (row.kind === "local") switchTo(row.branch.name);
    else void checkoutRemote(row.rb);
  }

  /** Combobox keyboard: ↑/↓/Home/End move, Enter activates, Esc closes. */
  function onInputKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (rows.length === 0) return;
      const delta = event.key === "ArrowDown" ? 1 : -1;
      activeIndex = Math.min(
        rows.length - 1,
        Math.max(0, activeIndex + delta),
      );
    } else if (event.key === "Home") {
      event.preventDefault();
      activeIndex = 0;
    } else if (event.key === "End") {
      event.preventDefault();
      activeIndex = Math.max(0, rows.length - 1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const row = rows[activeIndex];
      if (row) activate(row);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }
</script>

<button
  bind:this={triggerEl}
  class="branch-chip"
  type="button"
  title="Switch branch"
  aria-label="Switch branch"
  aria-haspopup="dialog"
  aria-expanded={open}
  onclick={toggle}
>
  <Icon icon={iconBranch} size={12} />
  <span class="branch-label">{chipLabel}</span>
  <Icon icon={iconChevronDown} size={10} />
</button>

{#if open}
  <div
    bind:this={popoverEl}
    class="popover"
    role="dialog"
    aria-label="Branch switcher"
    use:floating={{ anchor: triggerEl, gap: 6 }}
  >
    <div class="search-row">
      <Icon icon={iconSearch} size={14} />
      <input
        class="search"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls="branch-switcher-list"
        aria-activedescendant={rows.length > 0
          ? `branch-opt-${activeIndex}`
          : undefined}
        aria-autocomplete="list"
        aria-label="Search or create branch"
        placeholder="Search or create branch"
        bind:this={inputEl}
        bind:value={query}
        onkeydown={onInputKeydown}
      />
      {#if query !== ""}
        <button
          class="clear"
          type="button"
          aria-label="Clear search"
          title="Clear search"
          onclick={() => {
            query = "";
            inputEl?.focus();
          }}
        >
          <Icon icon={iconClose} size={12} />
        </button>
      {/if}
    </div>

    <ul
      id="branch-switcher-list"
      class="rows"
      class:busy={controller.busy}
      role="listbox"
      aria-label="Branches"
    >
      {#if error}
        <li class="state error" role="alert">
          {error} — use Refresh to retry.
        </li>
      {:else if loading && localList.length === 0 && remoteList.length === 0}
        <li class="state" role="status">Loading branches…</li>
      {:else}
        {#each rows as row, i (rowKey(row))}
          {#if row.kind === "local" && rows[i - 1]?.kind !== "local"}
            <li class="section" role="presentation">Local branches</li>
          {:else if row.kind === "remote" && rows[i - 1]?.kind !== "remote"}
            <li class="section" role="presentation">Remote branches</li>
          {/if}
          {#if row.kind === "create"}
            <li
              id={`branch-opt-${i}`}
              class="row create-row"
              class:active={i === activeIndex}
              role="option"
              aria-selected={i === activeIndex}
              onclick={() => activate(row)}
              onkeydown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  activate(row);
                }
              }}
              onmouseenter={() => (activeIndex = i)}
            >
              <span class="marker"></span>
              <span class="create-icon"><Icon icon={iconAdd} size={14} /></span>
              <span class="bname">Create branch "{row.name}"</span>
            </li>
          {:else if row.kind === "local"}
            <li
              id={`branch-opt-${i}`}
              class="row"
              class:head={row.branch.is_head}
              class:active={i === activeIndex}
              role="option"
              aria-selected={i === activeIndex}
              onclick={() => activate(row)}
              onkeydown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  activate(row);
                }
              }}
              onmouseenter={() => (activeIndex = i)}
              oncontextmenu={(event) => localMenu(event, row.branch)}
            >
              <span
                class="marker"
                aria-label={row.branch.is_head ? "Current branch" : undefined}
                title={row.branch.is_head ? "Current branch" : undefined}
              >
                {row.branch.is_head ? "●" : ""}
              </span>
              <span class="bname" title={row.branch.name}>{row.branch.name}</span>
              <span class="sha">{row.branch.sha.slice(0, 7)}</span>
              {#if row.branch.upstream}
                <span class="upstream" title={row.branch.upstream}>
                  {row.branch.upstream}
                  {#if row.branch.ahead > 0 || row.branch.behind > 0}
                    <span class="ab">↑{row.branch.ahead} ↓{row.branch.behind}</span>
                  {/if}
                  {#if row.branch.gone}<span class="gone">gone</span>{/if}
                </span>
              {/if}
              <span class="row-actions">
                <IconButton
                  icon={iconMore}
                  label={`More actions for ${row.branch.name}`}
                  title="Switch, push, rename, or delete this branch"
                  dense
                  aria-haspopup="menu"
                  onclick={(event) => {
                    event.stopPropagation();
                    localMenu(event, row.branch);
                  }}
                />
              </span>
            </li>
          {:else}
            <li
              id={`branch-opt-${i}`}
              class="row"
              class:active={i === activeIndex}
              role="option"
              aria-selected={i === activeIndex}
              onclick={() => activate(row)}
              onkeydown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  activate(row);
                }
              }}
              onmouseenter={() => (activeIndex = i)}
              oncontextmenu={(event) => remoteMenu(event, row.rb)}
            >
              <span class="marker" aria-hidden="true"></span>
              <span class="bname" title="{row.rb.remote}/{row.rb.name}">
                {row.rb.remote}/{row.rb.name}
              </span>
              <span class="sha">{row.rb.sha.slice(0, 7)}</span>
              <span class="row-actions">
                <IconButton
                  icon={iconMore}
                  label={`More actions for ${row.rb.remote}/${row.rb.name}`}
                  title="Checkout, copy, or delete this remote branch"
                  dense
                  aria-haspopup="menu"
                  onclick={(event) => {
                    event.stopPropagation();
                    remoteMenu(event, row.rb);
                  }}
                />
              </span>
            </li>
          {/if}
        {/each}
        {#if rows.length === 0}
          <li class="state" role="option" aria-selected="false" aria-disabled="true">
            No branches.
          </li>
        {/if}
      {/if}
    </ul>

    <footer class="foot">
      <button
        class="foot-btn"
        type="button"
        disabled={fetchBusy}
        onclick={() => void onFetch()}
      >
        <Icon icon={iconFetch} size={13} />
        Fetch
      </button>
      <button class="foot-btn" type="button" onclick={() => void reload()}>
        <Icon icon={iconRefresh} size={13} />
        Refresh
      </button>
      <span class="foot-spacer"></span>
      <button class="foot-btn manage" type="button" onclick={manageBranches}>
        Manage branches
      </button>
    </footer>
  </div>
{/if}

<!-- The controller's dialogs live OUTSIDE the popover so they survive
     closing it (every activation closes the popover immediately);
     BranchActionDialogs is the shared rendering of the controller's
     dirty-switch choice, merged-delete confirm, and force-delete preview. -->
<BranchActionDialogs {controller} />

<!-- Rename (row menu): old name preselected. -->
<PromptDialog
  bind:open={
    () => renaming !== null,
    (v) => {
      if (!v) renaming = null;
    }
  }
  title={renaming !== null ? `Rename branch ${renameOld}` : ""}
  message="Pick a new name for the branch."
  initial={renameOld}
  confirmLabel="Rename"
  onSubmit={renameSubmitted}
/>

<!-- Remote branch delete (row menu). -->
<ConfirmDialog
  bind:open={
    () => remoteDelete !== null,
    (v) => {
      if (!v) remoteDelete = null;
    }
  }
  title={remoteDelete
    ? `Delete remote branch ${remoteDelete.remote}/${remoteDelete.name}?`
    : ""}
  message="The branch is deleted on the remote; local clones update on their next fetch."
  confirmLabel="Delete remote branch"
  danger
  onConfirm={() => void deleteRemoteBranch()}
/>

<style>
  /* Trigger — the former read-only .branch-chip (RepoView) plus button
     resets and the chevron affordance. */
  .branch-chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    max-width: 16rem;
    min-width: 0;
    padding: 0.1rem 0.45rem;
    border: none;
    cursor: pointer;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-secondary));
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.6875rem;
  }

  .branch-chip:hover {
    filter: brightness(1.08);
  }

  .branch-chip:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .branch-chip :global(svg) {
    flex: none;
  }

  .branch-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .popover {
    position: fixed;
    /* Below dialogs (70) and the context menu / toasts (1000). */
    z-index: 60;
    display: flex;
    flex-direction: column;
    width: 22rem;
    max-width: calc(100vw - 1rem);
    max-height: min(32rem, calc(100vh - 1.5rem));
    overflow: hidden;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: var(--m3-elevation-3, 0 8px 24px rgba(0, 0, 0, 0.3));
    font-size: 0.8125rem;
  }

  .search-row {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.55rem 0.7rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .search {
    flex: 1;
    min-width: 0;
    border: none;
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    outline: none;
  }

  .search::placeholder {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .clear {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.25rem;
    height: 1.25rem;
    border: none;
    border-radius: 50%;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .clear:hover {
    background: var(--m3-surface-container-highest, var(--m3-surface));
  }

  .clear:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .rows {
    flex: 1 1 auto;
    min-height: 0;
    margin: 0;
    padding: 0.25rem 0 0.4rem;
    list-style: none;
    overflow-y: auto;
  }

  /* While a branch op runs, rows are inert; the controller's dialogs
     (outside this list) stay fully operable. */
  .rows.busy {
    pointer-events: none;
  }

  .rows.busy .row {
    opacity: 0.55;
  }

  .section {
    padding: 0.5rem 0.75rem 0.2rem;
    font-size: 0.6875rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--m3-primary);
    user-select: none;
  }

  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.375rem;
    margin: 0 0.25rem;
    padding: 0.25rem 0.4rem;
    border-radius: var(--m3-shape-small, 8px);
    min-width: 0;
    cursor: pointer;
  }

  .row:hover,
  .row.active {
    background: var(--m3-surface-container-highest, var(--m3-surface));
  }

  .row.head {
    background: var(--m3-secondary-container, transparent);
    color: var(--m3-on-secondary-container, inherit);
  }

  .marker {
    flex: none;
    width: 0.875rem;
    text-align: center;
    color: var(--m3-primary);
  }

  .bname {
    flex: 0 1 auto;
    min-width: 3rem;
    max-width: 11rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .create-row {
    color: var(--m3-primary);
  }

  .create-row .bname {
    max-width: none;
    flex: 1 1 auto;
    font-weight: 500;
    color: var(--m3-primary);
  }

  .create-icon {
    flex: none;
    display: inline-flex;
    align-items: center;
  }

  .sha {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .upstream {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
    font-family: ui-monospace, Consolas, monospace;
  }

  .ab {
    color: var(--m3-primary);
    padding-left: 0.375rem;
  }

  .gone {
    margin-left: 0.375rem;
    padding: 0 0.35rem;
    border: 1px solid var(--m3-error, currentColor);
    border-radius: var(--m3-shape-full, 9999px);
    color: var(--m3-error, inherit);
    font-size: 0.625rem;
  }

  .row-actions {
    margin-left: auto;
    flex: none;
    display: flex;
    align-items: center;
  }

  .state {
    padding: 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
  }

  .error {
    color: var(--m3-error, inherit);
  }

  .foot {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.35rem 0.5rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .foot-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.25rem 0.55rem;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    cursor: pointer;
  }

  .foot-btn:hover:not(:disabled) {
    background: var(--m3-surface-container-highest, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .foot-btn:disabled {
    color: var(--app-disabled-fg, var(--m3-on-surface-variant));
    cursor: not-allowed;
  }

  .foot-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .foot-spacer {
    flex: 1;
  }

  .foot-btn.manage {
    color: var(--m3-primary);
  }
</style>
