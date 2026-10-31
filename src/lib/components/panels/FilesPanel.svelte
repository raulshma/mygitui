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
    Enter opens, F2 renames, Delete deletes, typing jumps to the filter.
  - Virtualized: fixed 24px rows, windowed via `visibleRange` (spacer
    divs + overscan), matching StatusPanel's approach.
  - Context menu: open, diff (when changed), file history, blame, copy
    paths, reveal in status, external editor, new/rename/delete.
-->
<script lang="ts">
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
    type BadgeKind,
    type FileNode,
  } from "./fileTreeModel";
  import { fileTreePrefs } from "./fileTreePrefs.svelte";
  import { visibleRange } from "$lib/components/graph/layout";
  import { fileIconForPath, fileIconColorForPath } from "$lib/components/icons/fileIcons";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { contextMenu, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { showMenuAt } from "$lib/components/menu/contextMenuStore.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";
  import { openPath } from "@tauri-apps/plugin-opener";
  import { isTauri } from "$lib/entry/dragdrop";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import iconAddFile from "@ktibow/iconset-material-symbols/post-add";
  import iconAddFolder from "@ktibow/iconset-material-symbols/create-new-folder";
  import iconChevronDown from "@ktibow/iconset-material-symbols/keyboard-arrow-down";
  import iconChevronRight from "@ktibow/iconset-material-symbols/chevron-right";
  import iconFolder from "@ktibow/iconset-material-symbols/folder";
  import iconMore from "@ktibow/iconset-material-symbols/more-horiz";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconReveal from "@ktibow/iconset-material-symbols/file-open";
  import iconSearch from "@ktibow/iconset-material-symbols/search";
  import iconSubmodule from "@ktibow/iconset-material-symbols/alt-route";

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

  // --- Tree state -----------------------------------------------------------

  const FILE_ROW_HEIGHT = 24;
  const ROW_OVERSCAN = 12;
  const VIRTUALIZE_THRESHOLD = 150;
  const INDENT_PX = 12;

  let tree = $state<FileNode>(createRoot());
  /** Local filter query ("" = plain tree view). */
  let filter = $state("");
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

  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  let treeElement: HTMLDivElement | undefined;

  const virtualized = $derived(rows.length > VIRTUALIZE_THRESHOLD);
  const { first, last } = $derived(
    virtualized
      ? visibleRange(scrollTop, viewportHeight, rows.length, FILE_ROW_HEIGHT, ROW_OVERSCAN)
      : { first: 0, last: rows.length },
  );
  const topSpacer = $derived(first * FILE_ROW_HEIGHT);
  const bottomSpacer = $derived(Math.max(0, (rows.length - last) * FILE_ROW_HEIGHT));
  const visibleRows = $derived(rows.slice(first, last));

  function trackViewport(node: HTMLDivElement) {
    treeElement = node;
    const update = () => {
      scrollTop = node.scrollTop;
      viewportHeight = Math.max(1, node.clientHeight);
    };
    const observer = new ResizeObserver(update);
    observer.observe(node);
    node.addEventListener("scroll", update, { passive: true });
    update();
    return {
      destroy() {
        observer.disconnect();
        node.removeEventListener("scroll", update);
        if (treeElement === node) treeElement = undefined;
      },
    };
  }

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
    // Expanded dirs refresh after the root lands; errors are silent (a dir
    // may have been deleted externally — it disappears on collapse/expand).
    const expandedPaths = collectExpanded(tree);
    for (const path of expandedPaths) {
      if (token !== listToken) return;
      try {
        const result = await listDir(repoId, path, fileTreePrefs.showIgnored);
        if (token !== listToken) return;
        insertListing(tree, result);
      } catch {
        if (token === listToken) removeEntry(tree, path);
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
      // Re-expand persisted dirs (breadth-first through the levels).
      const wanted = persistedExpanded(tree);
      for (const path of wanted) {
        await listInto(path, true);
      }
    })();
  });

  // External changes (backend watcher/poller): re-list the visible tree.
  // Debounced — one mutation can surface as a burst of events, and each
  // refresh costs one listing per expanded dir.
  let repoChangeTimer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    const unregister = onRepoChange({
      filter: (change) => change.repo_id === repoId,
      handle: () => {
        if (repoChangeTimer) clearTimeout(repoChangeTimer);
        repoChangeTimer = setTimeout(() => {
          repoChangeTimer = null;
          void refreshTree();
        }, 500);
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

  function persistedExpanded(node: FileNode): string[] {
    // The prefs store is async-persisted but synchronously readable; walk
    // the current tree and expand any node the store says was expanded.
    const out: string[] = [];
    const queue: FileNode[] = [node];
    while (queue.length > 0) {
      const current = queue.shift()!;
      if (current.children === null) continue;
      for (const child of current.children) {
        if (child.kind === "dir" && fileTreePrefs.isExpanded(root, child.path)) {
          out.push(child.path);
        }
        if (child.kind === "dir" && child.children !== null) {
          queue.push(child);
        }
      }
    }
    return out;
  }

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
    if (index !== -1 && treeElement) {
      const target = index * FILE_ROW_HEIGHT;
      const viewportTop = treeElement.scrollTop;
      const viewportBottom = viewportTop + treeElement.clientHeight;
      if (target < viewportTop || target + FILE_ROW_HEIGHT > viewportBottom) {
        treeElement.scrollTop = Math.max(0, target - treeElement.clientHeight / 2);
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

  // --- File ops ---------------------------------------------------------------

  let promptOpen = $state(false);
  let promptTitle = $state("");
  let promptMessage = $state("");
  let promptInitial = $state("");
  let promptConfirm = $state("Create");
  let promptSubmit = $state<(value: string) => void>(() => {});
  let confirmOpen = $state(false);
  let confirmTitle = $state("");
  let confirmMessage = $state("");
  let confirmSubmit = $state<() => void>(() => {});
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
    promptTitle = kind === "file" ? "New file" : "New folder";
    promptMessage =
      dir === "" ? "Created at the repository root" : `Created in ${dir}/`;
    promptConfirm = "Create";
    promptInitial = "";
    promptSubmit = async (value: string) => {
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
    };
    promptOpen = true;
  }

  function openFileByPath(path: string): void {
    emitUiEvent("files-open-path", { repoId, path });
  }

  function startRename(node: FileNode): void {
    promptTitle = `Rename ${node.name}`;
    promptMessage = node.path;
    promptConfirm = "Rename";
    promptInitial = node.name;
    promptSubmit = async (value: string) => {
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
    };
    promptOpen = true;
  }

  function renameEntryInTree(path: string, newPath: string): void {
    renameNode(tree, path, newPath);
  }

  function startDelete(node: FileNode): void {
    confirmTitle = `Delete ${node.name}?`;
    confirmMessage =
      node.kind === "dir"
        ? "`" + node.path + "` and everything inside it will be deleted. A checkpoint is created first, so this is undoable from the Undo panel."
        : "`" + node.path + "` will be deleted. A checkpoint is created first, so this is undoable from the Undo panel.";
    confirmSubmit = async () => {
      try {
        await deleteEntry(repoId, node.path);
        removeEntry(tree, node.path);
        if (selectedPath === node.path) selectedPath = "";
        toast(`Deleted ${node.path}`);
      } catch (err) {
        toast(String(err), { kind: "error" });
      }
    };
    confirmOpen = true;
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
        run: () => {
          void navigator.clipboard.writeText(`${root}/${node.path}`);
        },
      },
      {
        id: "copy-rel",
        label: "Copy relative path",
        run: () => {
          void navigator.clipboard.writeText(node.path);
        },
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

  function overflowMenu(event: MouseEvent): void {
    const iconTheme = fileIconPreferences.theme;
    showMenuAt(event, [
      {
        id: "ignored",
        label: fileTreePrefs.showIgnored ? "Hide ignored files" : "Show ignored files",
        run: () => {
          fileTreePrefs.setShowIgnored(!fileTreePrefs.showIgnored);
          void refreshTree();
        },
      },
      { separator: true },
      {
        id: "icons-colorful",
        label: `${iconTheme === "colorful" ? "✓ " : ""}Colorful file icons`,
        run: () => fileIconPreferences.setTheme("colorful"),
      },
      {
        id: "icons-material",
        label: `${iconTheme === "material" ? "✓ " : ""}Material file icons`,
        run: () => fileIconPreferences.setTheme("material"),
      },
      {
        id: "icons-minimal",
        label: `${iconTheme === "minimal" ? "✓ " : ""}Minimal file icons`,
        run: () => fileIconPreferences.setTheme("minimal"),
      },
    ]);
  }

  // --- Keyboard ---------------------------------------------------------------

  function focusFilter(): void {
    treeElement?.closest(".files")?.querySelector("input")?.focus();
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
        move(at + Math.floor(viewportHeight / FILE_ROW_HEIGHT));
        break;
      case "PageUp":
        move(at - Math.floor(viewportHeight / FILE_ROW_HEIGHT));
        break;
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
    if (index === -1 || !treeElement) return;
    const top = index * FILE_ROW_HEIGHT;
    const viewTop = treeElement.scrollTop;
    const viewBottom = viewTop + treeElement.clientHeight;
    if (top < viewTop) treeElement.scrollTop = top;
    else if (top + FILE_ROW_HEIGHT > viewBottom) {
      treeElement.scrollTop = top + FILE_ROW_HEIGHT - treeElement.clientHeight;
    }
  }

  // --- Render helpers ----------------------------------------------------------

  const BADGE_VAR: Record<BadgeKind, string> = {
    added: "var(--app-added, #3fb950)",
    modified: "var(--app-modified, #d29922)",
    deleted: "var(--app-deleted, #f85149)",
    renamed: "var(--app-renamed, #8492ff)",
    conflicted: "var(--app-conflicted, #f85149)",
    untracked: "var(--app-untracked, #7ee787)",
  };

  function formatSize(size: number | null): string {
    if (size === null) return "";
    if (size < 1024) return `${size} B`;
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }

  const activeFilePath = $derived(selectedPath);</script>

<div class="files flex h-full min-h-0 min-w-0 flex-col bg-background text-[13px] text-foreground">
  <div class="toolbar flex shrink-0 items-center gap-[2px] border-b border-border bg-card px-1.5 py-1">
    <Button
      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[15px]"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="New file"
      title="New file"
      onclick={() => startCreate("file")}
    >
      <Icon icon={iconAddFile} size={15} />
    </Button>
    <Button
      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[15px]"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="New folder"
      title="New folder"
      onclick={() => startCreate("dir")}
    >
      <Icon icon={iconAddFolder} size={15} />
    </Button>
    <Button
      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[15px]"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh"
      title="Refresh tree"
      onclick={() => void refreshTree()}
    >
      <Icon icon={iconRefresh} size={15} />
    </Button>
    <Button
      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[15px]"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Reveal active file"
      title="Reveal the open file in the tree"
      disabled={activeFile === null}
      onclick={() => {
        if (activeFile !== null) {
          emitUiEvent("files-reveal-path", { repoId, path: activeFile });
        }
      }}
    >
      <Icon icon={iconReveal} size={15} />
    </Button>
    <span class="spacer flex-1"></span>
    <div
      class="filter flex min-w-16 max-w-56 flex-1 items-center gap-1 rounded-full border border-border bg-background px-1.5 py-[2px] text-muted-foreground"
      role="search"
    >
      <Icon icon={iconSearch} size={13} />
      <!-- svelte-ignore a11y_autofocus -->
      <Input
        type="text"
        class="h-auto min-w-0 flex-1 rounded-none border-none bg-transparent px-0 py-0 text-xs md:text-xs text-foreground shadow-none outline-none focus-visible:ring-0 dark:bg-transparent placeholder:text-muted-foreground"
        placeholder="Filter files"
        bind:value={filter}
        aria-label="Filter files"
        onkeydown={(event) => {
          if (event.key === "Escape" && filter !== "") {
            filter = "";
            event.stopPropagation();
          }
          if (event.key === "ArrowDown") {
            treeElement?.focus();
            event.preventDefault();
          }
        }}
      />
      {#if filter !== ""}
        <span class="fcount flex-none text-[10px] tabular-nums text-muted-foreground">{rows.length}</span>
      {/if}
    </div>
    <Button
      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[15px]"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="More options"
      title="File icons and ignored files"
      onclick={(event) => overflowMenu(event)}
    >
      <Icon icon={iconMore} size={15} />
    </Button>
  </div>

  <div
    class="tree min-h-0 flex-1 select-none overflow-auto pb-12 outline-none focus-visible:shadow-[inset_0_0_0_2px_var(--ring)]"
    role="tree"
    aria-label="Repository files"
    tabindex="0"
    aria-activedescendant={selectedPath !== "" ? `files-row-${rows.findIndex((r) => r.node.path === selectedPath)}` : undefined}
    use:trackViewport
    onkeydown={onTreeKeydown}
  >
    {#if rootLoading && rows.length === 0}
      <p class="state inline-flex items-center gap-1.5 p-4 text-xs text-muted-foreground" role="status"><Spinner class="size-[14px]" role="presentation" aria-hidden="true" /> Reading files…</p>
    {:else if loadError !== null && rows.length === 0}
      <p class="state state-error inline-flex items-center gap-1.5 p-4 text-xs text-destructive" role="alert">{loadError}</p>
    {:else if rows.length === 0}
      <p class="state inline-flex items-center gap-1.5 p-4 text-xs text-muted-foreground">{filter !== "" ? "No matches" : "Empty repository"}</p>
    {:else}
      {#if virtualized}
        <div class="row-spacer w-full" style={`height:${topSpacer}px`}></div>
      {/if}
      {#each visibleRows as row, i (row.node.path)}
        {@const index = first + i}
        {@const node = row.node}
        {@const badge = badges.get(node.path)}
        {@const isDir = node.kind === "dir" || node.kind === "submodule"}
        <!-- svelte-ignore a11y_interactive_supports_focus -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- aria-activedescendant tree: the container carries focus + keys -->
        <div
          id={`files-row-${index}`}
          class="row flex h-6 cursor-pointer items-center gap-1 whitespace-nowrap pr-2 text-foreground hover:bg-[color:var(--app-hover)] aria-selected:bg-[color:var(--app-active)] [&.disabled]:text-muted-foreground {node.path === revealHighlight ? 'animate-[files-flash_1.5s_ease-out]' : ''}"
          class:sel={node.path === selectedPath}
          class:disabled={node.kind === "submodule"}
          role="treeitem"
          aria-selected={node.path === selectedPath}
          aria-expanded={isDir && node.kind === "dir" ? node.expanded : undefined}
          aria-level={row.depth + 1}
          style={`padding-left:${row.depth * INDENT_PX + 6}px`}
          onclick={() => activate(node)}
          ondblclick={() => node.kind === "file" && openFile(node)}
          oncontextmenu={(event) => rowMenu(event, node)}
        >
          <span class="chev inline-flex w-3.5 flex-none justify-center">
            {#if node.kind === "dir"}
              <Button
                variant="ghost"
                size="icon-xs"
                class="chev-btn size-3.5 rounded-none p-0 text-muted-foreground hover:bg-transparent hover:text-foreground dark:hover:bg-transparent [&_svg:not([class*='size-'])]:size-[13px]"
                tabindex={-1}
                aria-label={node.expanded ? `Collapse ${node.name}` : `Expand ${node.name}`}
                onclick={(event) => {
                  event.stopPropagation();
                  toggleDir(node);
                }}
              >
                <Icon icon={node.expanded ? iconChevronDown : iconChevronRight} size={13} />
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
          <span class="name min-w-0 flex-1 overflow-hidden text-ellipsis" title={node.path}>{node.name}</span>
          {#if node.kind === "file" && node.size !== null && !isDir}
            <span class="size flex-none text-[10px] tabular-nums text-muted-foreground">{formatSize(node.size)}</span>
          {/if}
          {#if node.truncated}
            <span class="trunc flex-none text-muted-foreground" title="Listing capped — some entries are missing">…</span>
          {/if}
          {#if badge}
            <span
              class="badge h-[7px] w-[7px] flex-none rounded-full"
              title={`Git: ${badge}`}
              style={`background:${BADGE_VAR[badge]}`}
            ></span>
          {/if}
        </div>
      {/each}
      {#if virtualized}
        <div class="row-spacer w-full" style={`height:${bottomSpacer}px`}></div>
      {/if}
    {/if}
  </div>

  <PromptDialog
    bind:open={promptOpen}
    title={promptTitle}
    message={promptMessage}
    placeholder={promptTitle === "New file" ? "path/to/file.ts" : "folder-name"}
    initial={promptInitial}
    confirmLabel={promptConfirm}
    onSubmit={promptSubmit}
  />
  <ConfirmDialog
    bind:open={confirmOpen}
    title={confirmTitle}
    message={confirmMessage}
    confirmLabel="Delete"
    danger
    onConfirm={confirmSubmit}
  />
</div>

