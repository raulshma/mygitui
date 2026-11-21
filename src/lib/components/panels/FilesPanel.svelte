<!--
  FilesPanel — the workdir file browser (the `files` panel).

  VS Code-explorer interaction model:
  - Lazy gitignore-aware tree: expanding a directory lists exactly that
    level (`repo_list_dir`); listings are cached in the tree model.
  - Status badges roll up from the repo status (built once per snapshot,
    never per render) with VS Code-style folder dots.
  - Click selects; clicking a file opens it (typed `files-open-path` event
    handled by RepoView); chevron/Enter toggles dirs; submodules open as
    their own repo tab.
  - Keyboard: arrows/Home/End navigate, Right/Left expand/collapse,
    Enter opens, F2 renames, Delete deletes, typing jumps to the filter
    (Ctrl+F too, routed through the keybinds engine while the tree holds
    focus).
  - Virtualized: fixed 24px rows via the shared VirtualList (spacer +
    absolutely-positioned window, overscan), like BlameView.
  - Context menu: open, diff (when changed), file history, blame, copy
    paths, reveal in status, external editor, new/rename/delete.
  - Workstream A §5.6: one PanelHeader row (title + inline filter on
    toggle + refresh + overflow) replaced the toolbar stack; per-row file
    sizes are hidden by default (toggle in the overflow, persisted).
-->
<script lang="ts">
  import { tick } from "svelte";
  import type { RepoStatus } from "$lib/ipc/types";
  import { onRepoChange } from "$lib/events/repoEvents";
  import {
    createEntry,
    deleteEntry,
    listDir,
    renameEntry,
  } from "$lib/ipc/client";
  import {
    buildStatusBadges,
    createRoot,
    filterRows,
    findNode,
    flattenVisible,
    insertEntry,
    insertListing,
    removeEntry,
    renameEntry as renameNode,
    restoreExpanded,
    type BadgeKind,
    type FileNode,
    type FlatRow,
  } from "./fileTreeModel";
  import { fileTreePrefs } from "./fileTreePrefs.svelte";
  import VirtualList from "$lib/components/layout/VirtualList.svelte";
  import PanelHeader from "$lib/components/layout/PanelHeader.svelte";
  import OverflowMenu from "$lib/components/layout/OverflowMenu.svelte";
  import { fileIconForPath, fileIconColorForPath, type FileIconTheme } from "$lib/components/icons/fileIcons";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import EmptyState from "../layout/EmptyState.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import { contextMenu, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { showMenuAt } from "$lib/components/menu/contextMenuStore.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";
  import { openPath } from "@tauri-apps/plugin-opener";
  import { isTauri } from "$lib/entry/dragdrop";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { copyText } from "$lib/clipboard";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { closedDialog, openDialog, type DialogState } from "$lib/state/dialog";

  let {
    repoId,
    root,
    status = null,
    /** Path of the file open in the center workspace (reveal button). */
    activeFile = null,
  }: {
    repoId: string;
    /** Worktree root (absolute; for opening submodules as repo tabs). */
    root: string;
    status?: RepoStatus | null;
    activeFile?: string | null;
  } = $props();

  const iconAddFile = $derived(appIcon("post-add")!);
  const iconAddFolder = $derived(appIcon("create-new-folder")!);
  const iconChevronDown = $derived(appIcon("chevron-down")!);
  const iconChevronRight = $derived(appIcon("chevron-right")!);
  const iconFolder = $derived(appIcon("folder")!);
  const iconRefresh = $derived(appIcon("refresh")!);
  const iconReveal = $derived(appIcon("file-open")!);
  const iconSearch = $derived(appIcon("search")!);
  const iconSubmodule = $derived(appIcon("alt-route")!);

  // --- Tree state -----------------------------------------------------------

  const FILE_ROW_HEIGHT = 24;
  const ROW_OVERSCAN = 12;
  const INDENT_PX = 12;

  let tree = $state<FileNode>(createRoot());
  /** Local filter query ("" = plain tree view). */
  let filter = $state("");
  /**
   * Filter input visibility (Workstream A §5.6): the input expands inline
   * in the PanelHeader row on toggle instead of squatting in the toolbar.
   */
  let filterOn = $state(false);
  /** The filter input element (focus hand-off from Ctrl+F / typed keys). */
  let filterInputEl: HTMLInputElement | undefined = $state(undefined);
  /** The panel root (Ctrl+F routing: only the focused panel reacts). */
  let panelEl: HTMLElement | undefined = $state(undefined);
  let rootLoading = $state(false);
  let loadError = $state<string | null>(null);
  /** Path of the single selected row (selection model = one). */
  let selectedPath = $state("");
  /** Flash highlight target for reveal actions (cleared after a beat). */
  let revealHighlight = $state<string | null>(null);

  /** Status badges per path/dir — one build per status snapshot. */
  const badges = $derived(buildStatusBadges(status?.entries ?? []));

  /** Rows on display: filter view or the flattened expanded tree. */
  const rows = $derived.by(() => {
    if (filter.trim() !== "") return filterRows(tree, filter);
    return flattenVisible(tree);
  });

  // --- Viewport (virtualization) --------------------------------------------
  //
  // The shared VirtualList owns the viewport tracking + windowing; the
  // panel only drives it for keyboard nav (scrollToIndex) and focus
  // hand-off from the filter box (element()). Rows render through its
  // snippet with the original ids (files-row-<index>) so the
  // aria-activedescendant tree keeps working.

  /** The VirtualList instance surface this panel drives. */
  interface VirtualListApi {
    scrollToIndex(index: number, align?: "nearest" | "center"): void;
    viewportSize(): number;
    viewportScrollTop(): number;
    element(): HTMLDivElement | null;
  }

  let list = $state<VirtualListApi | undefined>(undefined);

  // --- Listing --------------------------------------------------------------

  /** Stale-response guard: listings from an older repo (or a superseded
   * refresh) never touch the tree. */
  let listToken = 0;

  async function listInto(path: string, expand: boolean): Promise<void> {
    const token = ++listToken;
    try {
      const result = await listDir(repoId, path, fileTreePrefs.showIgnored);
      if (token !== listToken) return;
      const node = insertListing(tree, result);
      if (node && expand) {
        node.expanded = true;
        fileTreePrefs.setExpanded(root, path, true);
      }
      loadError = null;
    } catch (err) {
      if (token !== listToken) return;
      loadError = String(err);
    }
  }

  /** Re-list the root and every already-listed expanded dir (refresh). */
  async function refreshTree(): Promise<void> {
    const token = ++listToken;
    rootLoading = true;
    try {
      const result = await listDir(repoId, "", fileTreePrefs.showIgnored);
      if (token !== listToken) return;
      insertListing(tree, result);
      loadError = null;
    } catch (err) {
      if (token !== listToken) return;
      loadError = String(err);
    } finally {
      if (token === listToken) rootLoading = false;
    }
    // Expanded dirs refresh in parallel after the root lands: a deep tree
    // used to pay 1 sequential IPC round-trip per dir per refresh. Errors
    // are silent (a dir may have been deleted externally — it disappears
    // on collapse/expand). One mutation pass afterwards keeps the rows
    // derivation from re-running once per dir.
    const expandedPaths = collectExpanded(tree);
    if (expandedPaths.length === 0) return;
    const results = await Promise.all(
      expandedPaths.map(async (path) => {
        try {
          return { path, result: await listDir(repoId, path, fileTreePrefs.showIgnored) };
        } catch {
          return { path, result: null };
        }
      }),
    );
    if (token !== listToken) return;
    for (const entry of results) {
      if (entry.result === null) {
        removeEntry(tree, entry.path);
      } else {
        insertListing(tree, entry.result);
      }
    }
  }

  function collectExpanded(node: FileNode): string[] {
    const out: string[] = [];
    if (node.children === null) return out;
    for (const child of node.children) {
      if (
        (child.kind === "dir") &&
        child.expanded &&
        child.children !== null
      ) {
        out.push(child.path);
        out.push(...collectExpanded(child));
      }
    }
    return out;
  }

  function toggleDir(node: FileNode): void {
    if (node.kind !== "dir" && node.kind !== "submodule") return;
    if (node.kind === "submodule") {
      // Submodules are separate repos: open them as a tab (SubmodulePanel
      // precedent) instead of browsing another repo's tree inline.
      void openTab(`${root}/${node.path}`);
      return;
    }
    if (node.children === null) {
      void listInto(node.path, true);
    } else {
      node.expanded = !node.expanded;
      fileTreePrefs.setExpanded(root, node.path, node.expanded);
    }
  }

  function openFile(node: FileNode): void {
    emitUiEvent("files-open-path", { repoId, path: node.path });
  }

  function activate(node: FileNode): void {
    selectedPath = node.path;
    if (node.kind === "dir" || node.kind === "submodule") {
      toggleDir(node);
    } else {
      openFile(node);
    }
  }

  // --- Hydration / external refresh ----------------------------------------

  // Rebuild when the repo tab changes; hydrate the persisted expanded set
  // first (never inside `$derived` — the store writes `$state`).
  $effect(() => {
    fileTreePrefs.ensure(root);
  });

  let hydratedRootFor = $state<string | null>(null);
  $effect(() => {
    // New repo tab: fresh tree, then expand whatever was persisted.
    if (hydratedRootFor === repoId) return;
    hydratedRootFor = repoId;
    listToken++;
    tree = createRoot();
    selectedPath = "";
    filter = "";
    void (async () => {
      rootLoading = true;
      await refreshTree();
      // Persisted expansion restores to a fixed point: after the root
      // lists, only level-1 dirs are visible and each restored level
      // reveals the next (restoreExpanded lists levels in parallel).
      const token = listToken;
      await restoreExpanded(
        tree,
        (path) => fileTreePrefs.isExpanded(root, path),
        async (path) => {
          try {
            return await listDir(repoId, path, fileTreePrefs.showIgnored);
          } catch {
            return null; // dir deleted since it was persisted — skip
          }
        },
        () => token !== listToken,
      );
    })();
  });

  // External changes (backend watcher/poller): re-list the visible tree.
  // Debounced — one mutation can surface as a burst of events, and each
  // refresh costs one listing per expanded dir — AND throttled to one full
  // refresh per window: under continuous writes the watcher force-flushes a
  // batch every 2s (watcher.rs MAX_WINDOW), so an SSH-backed repo would
  // otherwise pay one SFTP round-trip per expanded dir every ~2.5s for a
  // tree that rarely changed. `full` resyncs and an unlisted (empty) tree
  // bypass the throttle; manual refreshes call refreshTree directly and are
  // never throttled.
  const WATCHER_REFRESH_MIN_MS = 3_000;
  let repoChangeTimer: ReturnType<typeof setTimeout> | null = null;
  /** When the last watcher-driven refresh ran (throttle window start). */
  let lastWatcherRefreshAt = 0;
  /** The current burst contained a `full` resync (lifts the throttle). */
  let burstHasFull = false;

  function runWatcherRefresh(): void {
    repoChangeTimer = null;
    const treeEmpty = tree.children === null || tree.children.length === 0;
    const sinceLast = Date.now() - lastWatcherRefreshAt;
    if (!burstHasFull && !treeEmpty && sinceLast < WATCHER_REFRESH_MIN_MS) {
      // Inside the window: hold the refresh for the trailing edge so the
      // burst is still applied, just not at watcher speed.
      repoChangeTimer = setTimeout(
        runWatcherRefresh,
        WATCHER_REFRESH_MIN_MS - sinceLast,
      );
      return;
    }
    burstHasFull = false;
    lastWatcherRefreshAt = Date.now();
    void refreshTree();
  }

  $effect(() => {
    const unregister = onRepoChange({
      filter: (change) => change.repo_id === repoId,
      handle: (change) => {
        burstHasFull = burstHasFull || change.full;
        if (repoChangeTimer) clearTimeout(repoChangeTimer);
        repoChangeTimer = setTimeout(runWatcherRefresh, 500);
      },
    });
    return () => {
      unregister();
      if (repoChangeTimer) {
        clearTimeout(repoChangeTimer);
        repoChangeTimer = null;
      }
    };
  });

  // Status refreshes (watcher-driven) re-roll badges automatically via the
  // derived; the tree itself only refreshes on explicit user action.

  // --- Reveal ---------------------------------------------------------------

  /** Reveal + flash-highlight a path (palette command / status menus). */
  async function revealPath(path: string): Promise<void> {
    if (filter.trim() !== "") filter = "";
    const segments = path.split("/");
    for (let i = 1; i < segments.length; i++) {
      const dir = segments.slice(0, i).join("/");
      const dirNode = findNode(tree, dir);
      if (!dirNode) return;
      if (dirNode.children === null) {
        await listInto(dir, true);
      } else if (!dirNode.expanded) {
        dirNode.expanded = true;
        fileTreePrefs.setExpanded(root, dir, true);
      }
    }
    selectedPath = path;
    const index = rows.findIndex((row) => row.node.path === path);
    if (index !== -1 && list) {
      const target = index * FILE_ROW_HEIGHT;
      const viewTop = list.viewportScrollTop();
      const viewBottom = viewTop + list.viewportSize();
      if (target < viewTop || target + FILE_ROW_HEIGHT > viewBottom) {
        list.scrollToIndex(index, "center");
      }
    }
    revealHighlight = path;
    setTimeout(() => {
      if (revealHighlight === path) revealHighlight = null;
    }, 1600);
  }

  $effect(() => {
    return onUiEvent("files-reveal-path", (detail) => {
      if (detail.repoId !== repoId) return;
      const target = detail.path ?? activeFile;
      if (target) void revealPath(target);
    });
  });

  // Ctrl+F follows focus (the keybinds engine owns the combo globally —
  // see the `diff.find` command): inside this panel it filters files. Keep
  // -alive mounting means several instances are live, so the focus guard
  // picks the one the user is working in.
  $effect(() => {
    return onUiEvent("files-filter", () => {
      if (!panelEl?.contains(document.activeElement)) return;
      void openFilter();
    });
  });

  // --- File ops ---------------------------------------------------------------

  /** One file-tree prompt dialog: args (incl. the submit action) travel with open. */
  let prompt = $state<
    DialogState<{
      title: string;
      message: string;
      placeholder: string;
      initial: string;
      confirmLabel: string;
      onSubmit: (value: string) => void | Promise<void>;
    }>
  >(closedDialog());
  /** One file-tree confirm dialog: args travel with open. */
  let confirm = $state<
    DialogState<{ title: string; message: string; onConfirm: () => void | Promise<void> }>
  >(closedDialog());
  /** Where a pending New File/Folder should land. */
  let newEntryDir = $state("");

  function parentDirOf(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx === -1 ? "" : path.slice(0, idx);
  }

  function startCreate(kind: "file" | "dir"): void {
    // Land in the selected dir (when a dir is selected) or its parent.
    const selected = selectedPath !== "" ? findNode(tree, selectedPath) : null;
    let dir = "";
    if (selected) {
      dir =
        selected.kind === "dir" && selected.children !== null
          ? selected.path
          : parentDirOf(selected.path);
    }
    newEntryDir = dir;
    prompt = openDialog({
      title: kind === "file" ? "New file" : "New folder",
      message:
        dir === "" ? "Created at the repository root" : `Created in ${dir}/`,
      placeholder: kind === "file" ? "path/to/file.ts" : "folder-name",
      initial: "",
      confirmLabel: "Create",
      onSubmit: async (value: string) => {
        const name = value.replace(/\\/g, "/").replace(/\/+$/, "");
        if (!name) return;
        const target = dir === "" ? name : `${dir}/${name}`;
        try {
          await createEntry(repoId, target, kind);
          insertEntry(tree, dir, { name, kind });
          selectedPath = target;
          if (kind === "file") openFileByPath(target);
        } catch (err) {
          toast(String(err), { kind: "error" });
        }
      },
    });
  }

  function openFileByPath(path: string): void {
    emitUiEvent("files-open-path", { repoId, path });
  }

  function startRename(node: FileNode): void {
    prompt = openDialog({
      title: `Rename ${node.name}`,
      message: node.path,
      placeholder: "",
      initial: node.name,
      confirmLabel: "Rename",
      onSubmit: async (value: string) => {
        const name = value.replace(/\\/g, "/").replace(/\/+$/, "");
        if (!name || name === node.name) return;
        const newPath =
          parentDirOf(node.path) === ""
            ? name
            : `${parentDirOf(node.path)}/${name}`;
        try {
          await renameEntry(repoId, node.path, newPath);
          renameEntryInTree(node.path, newPath);
          selectedPath = newPath;
        } catch (err) {
          toast(String(err), { kind: "error" });
        }
      },
    });
  }

  function renameEntryInTree(path: string, newPath: string): void {
    renameNode(tree, path, newPath);
  }

  function startDelete(node: FileNode): void {
    confirm = openDialog({
      title: `Delete ${node.name}?`,
      message:
        node.kind === "dir"
          ? "`" + node.path + "` and everything inside it will be deleted. A checkpoint is created first, so this is undoable from the Undo panel."
          : "`" + node.path + "` will be deleted. A checkpoint is created first, so this is undoable from the Undo panel.",
      onConfirm: async () => {
        try {
          await deleteEntry(repoId, node.path);
          removeEntry(tree, node.path);
          if (selectedPath === node.path) selectedPath = "";
          toast(`Deleted ${node.path}`);
        } catch (err) {
          toast(String(err), { kind: "error" });
        }
      },
    });
  }

  // --- Context menu -----------------------------------------------------------

  function rowMenu(event: MouseEvent, node: FileNode): void {
    event.preventDefault();
    event.stopPropagation();
    selectedPath = node.path;
    const entries: MenuEntry[] = [];
    const changed = badges.has(node.path);
    if (node.kind === "file" || node.kind === "symlink") {
      entries.push(
        { id: "open", label: "Open", run: () => openFile(node) },
        {
          id: "diff",
          label: "Open diff",
          disabled: !changed,
          run: () => emitUiEvent("files-open-diff", { repoId, path: node.path }),
        },
        { separator: true },
        {
          id: "history",
          label: "File history",
          run: () =>
            emitUiEvent("open-file-history", { path: node.path, repoId }),
        },
        { separator: true },
      );
    } else if (node.kind === "dir") {
      entries.push(
        {
          id: "new-file",
          label: "New file…",
          run: () => {
            selectedPath = node.path;
            startCreate("file");
          },
        },
        {
          id: "new-dir",
          label: "New folder…",
          run: () => {
            selectedPath = node.path;
            startCreate("dir");
          },
        },
        { separator: true },
      );
    } else if (node.kind === "submodule") {
      entries.push({
        id: "open-sub",
        label: "Open as repository",
        run: () => void openTab(`${root}/${node.path}`),
      });
    }
    entries.push(
      {
        id: "external",
        label: "Open with system app",
        run: () => {
          if (!isTauri()) return;
          openPath(`${root}/${node.path}`).catch((err) =>
            toast(String(err), { kind: "error" }),
          );
        },
      },
      { separator: true },
      {
        id: "copy-path",
        label: "Copy path",
        run: () => void copyText(`${root}/${node.path}`),
      },
      {
        id: "copy-rel",
        label: "Copy relative path",
        run: () => void copyText(node.path),
      },
      { separator: true },
      {
        id: "rename",
        label: "Rename…",
        disabled: node.kind === "submodule",
        run: () => startRename(node),
      },
      {
        id: "delete",
        label: "Delete…",
        danger: true,
        disabled: node.kind === "submodule",
        run: () => startDelete(node),
      },
    );
    showMenuAt(event, entries);
  }

  // --- Keyboard ---------------------------------------------------------------

  /** Shows the filter input (if hidden) and focuses it once mounted. */
  async function openFilter(): Promise<void> {
    filterOn = true;
    await tick();
    filterInputEl?.focus();
  }

  /** PanelHeader filter toggle: off also drops the query. */
  async function toggleFilter(next: boolean): Promise<void> {
    filterOn = next;
    if (!next) {
      filter = "";
      return;
    }
    await openFilter();
  }

  function focusFilter(): void {
    if (!filterOn) {
      void openFilter();
      return;
    }
    filterInputEl?.focus();
  }

  function onTreeKeydown(event: KeyboardEvent): void {
    if (rows.length === 0) return;
    const currentIndex = rows.findIndex((row) => row.node.path === selectedPath);
    const at = currentIndex === -1 ? 0 : currentIndex;
    const move = (index: number) => {
      const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
      if (row) selectAndScroll(row.node.path);
    };
    switch (event.key) {
      case "ArrowDown":
        move(at + 1);
        break;
      case "ArrowUp":
        move(at - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(rows.length - 1);
        break;
      case "PageDown":
        move(at + Math.floor((list?.viewportSize() ?? 0) / FILE_ROW_HEIGHT));
        break;
      case "PageUp":
        move(at - Math.floor((list?.viewportSize() ?? 0) / FILE_ROW_HEIGHT));
      case "ArrowRight": {
        const row = rows[at]!;
        if (row.node.kind === "dir" && !row.node.expanded) toggleDir(row.node);
        else move(at + 1);
        break;
      }
      case "ArrowLeft": {
        const row = rows[at]!;
        if (row.node.kind === "dir" && row.node.expanded) toggleDir(row.node);
        else {
          const parent = parentDirOf(row.node.path);
          if (parent !== "") selectAndScroll(parent);
        }
        break;
      }
      case "Enter": {
        const row = rows[at]!;
        activate(row.node);
        break;
      }
      case "F2": {
        const row = rows[at]!;
        if (row.node.kind !== "submodule") startRename(row.node);
        break;
      }
      case "Delete": {
        const row = rows[at]!;
        if (row.node.kind !== "submodule") startDelete(row.node);
        break;
      }
      case "Escape":
        if (filter !== "") {
          filter = "";
          event.stopPropagation();
        }
        break;
      default:
        // Printable characters: jump to the filter box (VS Code behavior).
        if (event.key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey) {
          focusFilter();
        }
        return;
    }
    event.preventDefault();
  }

  function selectAndScroll(path: string): void {
    selectedPath = path;
    const index = rows.findIndex((row) => row.node.path === path);
    if (index !== -1) list?.scrollToIndex(index);
  }

  // --- Render helpers ----------------------------------------------------------

  // Fallbacks mirror the light-scheme values in app.css — the tokens are
  // always defined, so these only matter if the stylesheet failed to load.
  const BADGE_VAR: Record<BadgeKind, string> = {
    added: "var(--app-added, #1a7f37)",
    modified: "var(--app-modified, #9a6700)",
    deleted: "var(--app-deleted, #cf222e)",
    renamed: "var(--app-renamed, #8250df)",
    conflicted: "var(--app-conflicted, #cf222e)",
    untracked: "var(--app-untracked, #1a7f37)",
  };

  function formatSize(size: number | null): string {
    if (size === null) return "";
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }

  const activeFilePath = $derived(selectedPath);</script>

<div class="files flex h-full min-h-0 min-w-0 flex-col bg-background text-ui-base text-foreground" bind:this={panelEl}>
  <!-- Workstream A §5.6: one PanelHeader row (title + filter-match count +
       inline filter + refresh + overflow) replaced the toolbar/filter
       stack. New file/folder + reveal + sizes + icon themes live in the
       overflow; the tree itself is unchanged. -->
  <PanelHeader
    title="Files"
    count={filter.trim() !== "" ? rows.length : undefined}
    filterOn={filterOn}
    onFilterToggle={(next) => void toggleFilter(next)}
    class="bg-card"
  >
    {#snippet filterInput()}
      {#if filterOn}
        <div
          class="filter flex min-w-16 max-w-56 flex-1 items-center gap-1 rounded-full border border-border bg-background px-1.5 py-px text-muted-foreground"
          role="search"
        >
          <Icon icon={iconSearch} size={14} />
          <!-- svelte-ignore a11y_autofocus -->
          <Input
            type="text"
            bind:ref={filterInputEl}
            class="h-auto min-w-0 flex-1 rounded-none border-none bg-transparent px-0 py-0 text-xs md:text-xs text-foreground shadow-none outline-none focus-visible:ring-0 dark:bg-transparent placeholder:text-muted-foreground"
            placeholder="Filter files"
            bind:value={filter}
            aria-label="Filter files"
            onkeydown={(event) => {
              if (event.key === "Escape") {
                if (filter !== "") {
                  filter = "";
                } else {
                  filterOn = false;
                }
                event.stopPropagation();
              }
              if (event.key === "ArrowDown") {
                list?.element()?.focus();
                event.preventDefault();
              }
            }}
          />
          {#if filter !== ""}
            <span class="fcount flex-none text-ui-micro tabular-nums text-muted-foreground">{rows.length}</span>
          {/if}
        </div>
      {/if}
    {/snippet}
    {#snippet trailing()}
      <Hint label="Refresh tree">
        {#snippet child({ props })}
          <Button
            class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
            variant="ghost"
            size="icon-xs"
            type="button"
            {...props}
            aria-label="Refresh"
            onclick={() => void refreshTree()}
          >
            <Icon icon={iconRefresh} size={14} />
          </Button>
        {/snippet}
      </Hint>
      <OverflowMenu label="File options">
        {#snippet content()}
          <DropdownMenu.Item
            class="cursor-pointer gap-ui-3"
            onclick={() => startCreate("file")}
          >
            <span class="pointer-events-none inline-flex flex-none items-center justify-center">
              <Icon icon={iconAddFile} size={16} />
            </span>
            New file…
          </DropdownMenu.Item>
          <DropdownMenu.Item
            class="cursor-pointer gap-ui-3"
            onclick={() => startCreate("dir")}
          >
            <span class="pointer-events-none inline-flex flex-none items-center justify-center">
              <Icon icon={iconAddFolder} size={16} />
            </span>
            New folder…
          </DropdownMenu.Item>
          <DropdownMenu.Item
            class="cursor-pointer gap-ui-3"
            disabled={activeFile === null}
            onclick={() => {
              if (activeFile !== null) {
                emitUiEvent("files-reveal-path", { repoId, path: activeFile });
              }
            }}
          >
            <span class="pointer-events-none inline-flex flex-none items-center justify-center">
              <Icon icon={iconReveal} size={16} />
            </span>
            Reveal active file
          </DropdownMenu.Item>
          <DropdownMenu.Separator />
          <DropdownMenu.CheckboxItem
            class="cursor-pointer"
            checked={fileTreePrefs.showIgnored}
            onCheckedChange={(next) => {
              fileTreePrefs.setShowIgnored(next === true);
              void refreshTree();
            }}
          >
            Show ignored files
          </DropdownMenu.CheckboxItem>
          <DropdownMenu.CheckboxItem
            class="cursor-pointer"
            checked={fileTreePrefs.showSizes}
            onCheckedChange={(next) => fileTreePrefs.setShowSizes(next === true)}
          >
            Show file sizes
          </DropdownMenu.CheckboxItem>
          <DropdownMenu.Separator />
          <DropdownMenu.RadioGroup
            value={fileIconPreferences.theme}
            onValueChange={(value) => fileIconPreferences.setTheme(value as FileIconTheme)}
          >
            <DropdownMenu.RadioItem class="cursor-pointer" value="colorful">
              Colorful file icons
            </DropdownMenu.RadioItem>
            <DropdownMenu.RadioItem class="cursor-pointer" value="material">
              Material file icons
            </DropdownMenu.RadioItem>
            <DropdownMenu.RadioItem class="cursor-pointer" value="minimal">
              Minimal file icons
            </DropdownMenu.RadioItem>
          </DropdownMenu.RadioGroup>
        {/snippet}
      </OverflowMenu>
    {/snippet}
  </PanelHeader>

  {#snippet row(entry: FlatRow, index: number)}
    {@const node = entry.node}
    {@const badge = badges.get(node.path)}
    {@const isDir = node.kind === "dir" || node.kind === "submodule"}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- aria-activedescendant tree: the container carries focus + keys -->
    <div
      id={`files-row-${index}`}
      class="row flex h-full cursor-pointer items-center gap-1 whitespace-nowrap pr-2 text-foreground hover:bg-[color:var(--app-hover)] aria-selected:bg-[color:var(--app-active)] [&.disabled]:text-muted-foreground {node.path === revealHighlight ? 'animate-[files-flash_1.5s_ease-out]' : ''}"
      class:sel={node.path === selectedPath}
      class:disabled={node.kind === "submodule"}
      role="treeitem"
      aria-selected={node.path === selectedPath}
      aria-expanded={isDir && node.kind === "dir" ? node.expanded : undefined}
      aria-level={entry.depth + 1}
      style={`padding-left:${entry.depth * INDENT_PX + 6}px`}
      onclick={() => activate(node)}
      ondblclick={() => node.kind === "file" && openFile(node)}
      oncontextmenu={(event) => rowMenu(event, node)}
    >
      <span class="chev inline-flex w-3.5 flex-none justify-center">
        {#if node.kind === "dir"}
          <Button
            variant="ghost"
            size="icon-xs"
            class="chev-btn size-3.5 rounded-none p-0 text-muted-foreground hover:bg-transparent hover:text-foreground dark:hover:bg-transparent [&_svg:not([class*='size-'])]:size-3.5"
            tabindex={-1}
            aria-label={node.expanded ? `Collapse ${node.name}` : `Expand ${node.name}`}
            onclick={(event) => {
              event.stopPropagation();
              toggleDir(node);
            }}
          >
            <Icon icon={node.expanded ? iconChevronDown : iconChevronRight} size={14} />
          </Button>
        {/if}
      </span>
      <span
        class="ficon inline-flex flex-none items-center"
        style={`color:${node.kind === "dir" || node.kind === "submodule" ? "var(--primary)" : fileIconColorForPath(node.path, fileIconPreferences.theme)}`}
      >
        <Icon
          icon={node.kind === "dir" || node.kind === "submodule" ? (node.kind === "submodule" ? iconSubmodule : iconFolder) : fileIconForPath(node.path, fileIconPreferences.theme)}
          size={14}
        />
      </span>
      <Hint label={node.path}>
        {#snippet child({ props })}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex (aria-activedescendant tree: rows must stay out of tab order) -->
          <span class="name min-w-0 flex-1 overflow-hidden text-ellipsis" {...props} tabindex={-1}>{node.name}</span>
        {/snippet}
      </Hint>
      {#if node.kind === "file" && node.size !== null && !isDir && fileTreePrefs.showSizes}
        <!-- Sizes are opt-in (§5.6: per-row sizes are noise; the toggle
             lives in the panel header's overflow, persisted). -->
        <span class="size flex-none text-ui-meta tabular-nums text-muted-foreground">{formatSize(node.size)}</span>
      {/if}
      {#if node.truncated}
        <Hint label="Listing capped — some entries are missing">
          {#snippet child({ props })}
            <!-- svelte-ignore a11y_no_noninteractive_tabindex (aria-activedescendant tree: rows must stay out of tab order) -->
            <span class="trunc flex-none text-muted-foreground" {...props} tabindex={-1}>…</span>
          {/snippet}
        </Hint>
      {/if}
      {#if badge}
        <Hint label={`Git: ${badge}`}>
          {#snippet child({ props })}
            <!-- svelte-ignore a11y_no_noninteractive_tabindex (aria-activedescendant tree: rows must stay out of tab order) -->
            <span
              class="badge size-2 flex-none rounded-full"
              {...props}
              tabindex={-1}
              style={`background:${BADGE_VAR[badge]}`}
            ></span>
          {/snippet}
        </Hint>
      {/if}
    </div>
  {/snippet}

  <VirtualList
    items={rows}
    rowHeight={FILE_ROW_HEIGHT}
    overscan={ROW_OVERSCAN}
    scrollerClass="tree min-h-0 flex-1 select-none overflow-auto pb-3 outline-none focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
    scrollerProps={{
      role: "tree",
      "aria-label": "Repository files",
      tabindex: 0,
      "aria-activedescendant":
        selectedPath !== ""
          ? `files-row-${rows.findIndex((r) => r.node.path === selectedPath)}`
          : undefined,
      onkeydown: onTreeKeydown,
    }}
    {row}
  >
    {#snippet empty()}
      {#if rootLoading}
        <p class="state inline-flex items-center gap-1.5 p-4 text-xs text-muted-foreground" role="status"><Spinner class="size-4" role="presentation" aria-hidden="true" /> Reading files…</p>
      {:else if loadError !== null}
        <p class="state state-error inline-flex items-center gap-1.5 p-4 text-xs text-destructive" role="alert">{loadError}</p>
      {:else}
        <EmptyState title={filter !== "" ? "No matches" : "Empty repository"} />
      {/if}
    {/snippet}
  </VirtualList>

  <PromptDialog
    bind:open={
      () => prompt.open,
      (v) => {
        if (!v) prompt = closedDialog();
      }
    }
    title={prompt.open ? prompt.title : ""}
    message={prompt.open ? prompt.message : ""}
    placeholder={prompt.open ? prompt.placeholder : ""}
    initial={prompt.open ? prompt.initial : ""}
    confirmLabel={prompt.open ? prompt.confirmLabel : "Create"}
    onSubmit={(value) => {
      if (prompt.open) void prompt.onSubmit(value);
    }}
  />
  <ConfirmDialog
    bind:open={
      () => confirm.open,
      (v) => {
        if (!v) confirm = closedDialog();
      }
    }
    title={confirm.open ? confirm.title : ""}
    message={confirm.open ? confirm.message : ""}
    confirmLabel="Delete"
    danger
    onConfirm={() => {
      if (confirm.open) void confirm.onConfirm();
    }}
  />
</div>

