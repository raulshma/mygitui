<script lang="ts">
  /**
   * Source-control sidebar — VSCode-style working-copy list.
   *
   * Sections come from the pure model in `statusModel.ts` (Staged Changes /
   * Changes). One path can appear in both groups when it has staged and
   * unstaged edits. The right-side status letter identifies added, modified,
   * deleted, untracked, or conflicted files.
   *
   * Interaction is 1-1 with the VS Code source-control view. A row's
   * selection identity is its key (`section:path`), so a path with staged
   * and unstaged edits selects independently in each group. Plain click
   * selects and opens the diff (Conflicted rows go to the optional conflict
   * editor when provided; a submodule path opens as a repository tab);
   * Ctrl-click toggles the row in the multi-selection; Shift-click extends
   * the range from the anchor without opening. Keyboard: arrows / PageUp /
   * PageDown / Home / End move the focus (Shift extends, no wraparound),
   * Ctrl+A selects all, Space toggles the focused row, Enter opens, `s`
   * stages or unstages and `d` discards the selection. Stage/unstage/
   * discard — hover buttons, context menu and keys alike — apply to the
   * whole selection when the target row is part of it, and right-clicking
   * an unselected row collapses the selection to that row first. After a
   * mutation the selection follows files across the group move on refresh.
   *
   * Submodules: `submodules(repoId)` supplies checked-out vs recorded state.
   * A submodule that the superproject's status already reports is enriched
   * in place (folder icon + short "new commits"/"modified"/"untracked"
   * tag); modules with no status entry (uninitialized, broken) appear in the
   * Changes group so they can be initialized/updated from here.
   */
  import { untrack } from "svelte";
  import type {
    ChangeKind,
    RepoStatus,
    StatusEntry,
    SubmoduleInfo,
  } from "$lib/ipc/types";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import {
    closeRepo,
    discard,
    gitignoreAdd,
    openRepo,
    remoteRepoOpen,
    repoStatus,
    stage,
    stageAll,
    submoduleSync,
    submoduleUpdate,
    submodules as listSubmodules,
  } from "$lib/ipc/client";
  import { onRepoChange } from "$lib/events/repoEvents";
  import { openTab, parseSshUri, tabStore } from "$lib/stores/tabs.svelte";
  import { normalizeRepoPath } from "$lib/stores/switcher.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import iconBranch from "@ktibow/iconset-material-symbols/account-tree";
  import { toast } from "$lib/toast";
  import { patternFor } from "./gitignore";
  import {
    contextMenu,
    showMenuAt,
    type MenuEntry,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import GitignoreGallery from "$lib/components/panels/GitignoreGallery.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
import {
  fileIconColorForPath,
  fileIconForPath,
  FILE_ICON_THEMES,
} from "$lib/components/icons/fileIcons";
import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import { openPanelPopout } from "$lib/layout/popout";
  import { visibleRange } from "$lib/components/graph/layout";
  import iconStage from "@ktibow/iconset-material-symbols/add";
  import iconUnstage from "@ktibow/iconset-material-symbols/remove";
  import iconDiscard from "@ktibow/iconset-material-symbols/undo";
  import iconModule from "@ktibow/iconset-material-symbols/folder-special";
  import iconOpen from "@ktibow/iconset-material-symbols/open-in-new";
  import iconUpdate from "@ktibow/iconset-material-symbols/download";
  import iconSync from "@ktibow/iconset-material-symbols/sync";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconMore from "@ktibow/iconset-material-symbols/more-vert";
  import iconSearch from "@ktibow/iconset-material-symbols/search";
  import iconClear from "@ktibow/iconset-material-symbols/close";
  import iconCheck from "@ktibow/iconset-material-symbols/check-circle";
  import iconStageAll from "@ktibow/iconset-material-symbols/done-all";
  import iconUnstageAll from "@ktibow/iconset-material-symbols/remove-done";
  import iconTemplates from "@ktibow/iconset-material-symbols/description";
  import iconIgnore from "@ktibow/iconset-material-symbols/visibility-off";
  import iconChevron from "@ktibow/iconset-material-symbols/chevron-right";
  import iconAhead from "@ktibow/iconset-material-symbols/arrow-upward";
  import iconBehind from "@ktibow/iconset-material-symbols/arrow-downward";
import iconPalette from "@ktibow/iconset-material-symbols/palette";
  import {
    buildStatusIndex,
    filterSections,
    kindLabel,
    kindLetter,
    splitPath,
    submoduleBranchLabel,
    submoduleChangeCounts,
    submoduleDisplay,
    submoduleNeedsAttention,
    type SectionId,
    type StatusIndex,
    type StatusRow,
    type StatusSectionView,
  } from "./statusModel";
  import {
    actionRows,
    allSelection,
    countLabel,
    pruneSelection,
    rangeSelection,
    remapSelection,
    selectionActions,
    toggleSelection,
    type SelectionActions,
  } from "./statusSelection";

  let {
    status,
    onOpenDiff,
    selected = $bindable([]),
    repoId = null,
    root = "",
    repoName = "",
    hasSubmodules = $bindable(false),
    onAfterMutation = undefined,
    onOpenConflict = undefined,
  }: {
    status: RepoStatus | null;
    onOpenDiff: (
      entry: StatusEntry,
      targetRepoId?: string,
      targetRoot?: string,
      targetRepoName?: string,
    ) => void;
    /** Selected row keys (`section:path` or `prefix:section:path`), VS Code multi-select style. */
    selected: string[];
    /** Owning repository; staging controls disable while null. */
    repoId?: string | null;
    /** Worktree root (opening a submodule as a tab). */
    root?: string;
    /** Display name of the repository. */
    repoName?: string;
    /** Whether this repository has any submodules. */
    hasSubmodules?: boolean;
    /** Called after a successful mutation (RepoView refreshes the status). */
    onAfterMutation?: () => void;
    /** Called instead of `onOpenDiff` for Conflicted rows (M3 editor). */
    onOpenConflict?: (entry: StatusEntry) => void;
  } = $props();

  type DisplaySection = StatusSectionView & { modules: SubmoduleInfo[] };

  let filterText = $state("");
  let collapsed = $state<Record<SectionId, boolean>>({
    staged: false,
    changes: false,
  });

  /** Roving focus: key of the focused row (null → first visible row). */
  let focusKey: string | null = $state(null);
  /** Index (in visibleRows) of the selection anchor for Shift ranges. */
  let anchorIndex = 0;
  /** Row elements by key (roving-tabindex focus targets). */
  const rowEls = $state<Record<string, HTMLElement | undefined>>({});
  const STATUS_ROW_HEIGHT = 24;
  const STATUS_SECTION_HEIGHT = 26;
  const STATUS_ROW_OVERSCAN = 12;
  const STATUS_VIRTUALIZE_THRESHOLD = 150;
  let treeElement: HTMLDivElement | undefined = $state();
  let treeScrollTop = $state(0);
  let treeViewportHeight = $state(640);
  /** Staging mutation in flight (disables the controls momentarily). */
  let mutating = $state(false);

  /** Submodule metadata for the current repo (paths, state, URLs). */
  let moduleList = $state<SubmoduleInfo[]>([]);
  let moduleError = $state<string | null>(null);
  let moduleLoading = $state(false);
  /** Path of the submodule mutation in flight, if any. */
  let moduleBusy = $state<string | null>(null);
  /** Guards stale submodule refreshes. */
  let moduleToken = 0;

  /**
   * Gitignore quick-add (M4): the row key whose "Ignore" dropdown is open
   * plus the candidate patterns computed from that row's path.
   */
  let ignoreFor = $state<string | null>(null);
  let ignoreChoices = $state<{ label: string; pattern: string }[]>([]);

  function describeError(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  /** Candidate ignore patterns for one path: exact, `*.ext`, `dir/`. */
  function choicesFor(path: string): { label: string; pattern: string }[] {
    const opts = patternFor(path);
    const out = [{ label: "exact", pattern: opts.exact }];
    if (opts.ext) out.push({ label: `all ${opts.ext}`, pattern: opts.ext });
    if (opts.dir) out.push({ label: `dir ${opts.dir}`, pattern: opts.dir });
    return out;
  }

  /** Adds the pattern to .gitignore and refreshes the status. */
  async function onIgnore(
    choice: { label: string; pattern: string },
    row?: StatusRow,
  ): Promise<void> {
    const targetRepoId =
      (row?.submodulePath ? submodulesMap[row.submodulePath]?.repoId : repoId) ??
      repoId;
    if (!targetRepoId || !choice.pattern) return;
    const subPath = row?.submodulePath;
    ignoreFor = null;
    try {
      await gitignoreAdd(targetRepoId, choice.pattern);
      toast(`Ignored ${choice.pattern}`, { kind: "success" });
      if (subPath) {
        void loadSubmoduleStatus(subPath);
      } else {
        onAfterMutation?.();
      }
    } catch (err) {
      toast(`Ignore failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  /** Search index — rebuilt once per status change, not per keystroke. */
  const index: StatusIndex | null = $derived(
    status ? buildStatusIndex(status.entries) : null,
  );
  /** Filter text without surrounding whitespace. */
  const query = $derived(filterText.trim());

  /** Submodule rows keyed by repo path. */
  const moduleByPath = $derived(
    new Map(moduleList.map((info) => [info.path, info])),
  );
  /** Every path the superproject status already reports. */
  const statusPaths = $derived(
    new Set((status?.entries ?? []).map((entry) => entry.path)),
  );
  /**
   * Modules that git status does not already list but that need attention:
   * uninitialized/broken modules (no status entry exists) and dirty modules
   * whose status libgit2 did not surface. Clean modules stay hidden.
   */
  const extraModules = $derived.by(() => {
    if (hasSubmodules || !status) return [] as SubmoduleInfo[];
    const needle = query.toLowerCase();
    return moduleList
      .filter((info) => !statusPaths.has(info.path))
      .filter((info) => submoduleNeedsAttention(info))
      .filter(
        (info) => needle === "" || info.path.toLowerCase().includes(needle),
      )
      .sort((a, b) => a.path.localeCompare(b.path));
  });

  /** Status sections with exceptional submodules folded into Changes. */
  const sections = $derived.by<DisplaySection[]>(() => {
    const filtered = index ? filterSections(index, filterText) : [];
    const withModules = filtered.map((section) => ({
      ...section,
      modules: section.id === "changes" ? extraModules : [],
    }));
    if (extraModules.length === 0 || filtered.some((s) => s.id === "changes")) {
      return withModules;
    }
    return [
      ...withModules,
      {
        id: "changes",
        label: "Changes",
        rows: [],
        modules: extraModules,
      },
    ];
  });
  interface SubmoduleData {
    info: SubmoduleInfo;
    repoId: string | null;
    root: string;
    status: RepoStatus | null;
    index: StatusIndex | null;
    loading: boolean;
    error: string | null;
  }

  let submodulesMap = $state<Record<string, SubmoduleData>>({});
  const openedRepoIds = new Set<string>();

  let mainRepoCollapsed = $state(false);
  let submoduleCollapsed = $state<Record<string, boolean>>({});
  let subSectionCollapsed = $state<Record<string, boolean>>({});

  function isSubSectionCollapsed(subPath: string, sectionId: SectionId): boolean {
    return subSectionCollapsed[`${subPath}:${sectionId}`] ?? false;
  }

  function toggleSubSection(subPath: string, sectionId: SectionId): void {
    const key = `${subPath}:${sectionId}`;
    subSectionCollapsed[key] = !subSectionCollapsed[key];
  }

  function toggleSubmodule(path: string): void {
    submoduleCollapsed[path] = !submoduleCollapsed[path];
  }

  function mainBranchLabel(s: RepoStatus): string {
    if (s.branch) return s.branch;
    if (s.detached && s.head) return `detached @ ${s.head.slice(0, 7)}`;
    return s.detached ? "detached" : "HEAD";
  }

  /** Rows actually on screen (collapsed sections excluded) for key nav. */
  const visibleRows = $derived.by<StatusRow[]>(() => {
    const rows: StatusRow[] = [];
    if (!hasSubmodules) {
      for (const s of sections) {
        if (!collapsed[s.id]) {
          rows.push(...s.rows);
        }
      }
      return rows;
    }
    if (!mainRepoCollapsed) {
      for (const s of sections) {
        if (!collapsed[s.id]) {
          rows.push(...s.rows);
        }
      }
    }
    for (const info of moduleList) {
      if (!submoduleCollapsed[info.path]) {
        const sub = submodulesMap[info.path];
        if (sub && sub.index) {
          const subFiltered = filterSections(sub.index, filterText);
          for (const s of subFiltered) {
            if (!isSubSectionCollapsed(info.path, s.id)) {
              rows.push(...s.rows);
            }
          }
        }
      }
    }
    return rows;
  });

  /** Keep the full status model, but mount only the viewport for large lists. */
  const sectionWindows = $derived.by(() => {
    let sectionTop = 0;
    return sections.map((section) => {
      const isCollapsed = collapsed[section.id];
      const rowsTop = sectionTop + STATUS_SECTION_HEIGHT;
      let first = 0;
      let last = isCollapsed ? 0 : section.rows.length;

      if (!isCollapsed && section.rows.length > STATUS_VIRTUALIZE_THRESHOLD) {
        const rowsBottom = rowsTop + section.rows.length * STATUS_ROW_HEIGHT;
        const viewportBottom = treeScrollTop + treeViewportHeight;
        if (rowsBottom <= treeScrollTop + STATUS_SECTION_HEIGHT) {
          first = section.rows.length;
          last = section.rows.length;
        } else if (rowsTop >= viewportBottom) {
          first = 0;
          last = 0;
        } else {
          const range = visibleRange(
            treeScrollTop - rowsTop,
            treeViewportHeight,
            section.rows.length,
            STATUS_ROW_HEIGHT,
            STATUS_ROW_OVERSCAN,
          );
          first = Math.min(section.rows.length, range.first);
          last = Math.max(first, Math.min(section.rows.length, range.last));
        }
      }

      sectionTop = rowsTop +
        (isCollapsed
          ? 0
          : (section.rows.length + section.modules.length) * STATUS_ROW_HEIGHT);

      return {
        section,
        first,
        last,
        rows:
          first === 0 && last === section.rows.length
            ? section.rows
            : section.rows.slice(first, last),
      };
    });
  });
  const renderedRows = $derived(
    hasSubmodules ? visibleRows : sectionWindows.flatMap((window) => window.rows),
  );
  /** The single tab-reachable row (falls back to the first visible row when
   *  the focused one is filtered out or collapsed away). */
  const tabIndexKey = $derived(
    focusKey !== null && visibleRows.some((r) => r.key === focusKey)
      ? focusKey
      : (visibleRows[0]?.key ?? null),
  );
  /** Staging controls need a repo and no in-flight mutation. */
  const canMutate = $derived(repoId !== null && !mutating);
  /** At least one non-conflicted working-tree row can be staged. */
  const canStageAll = $derived(
    canMutate &&
      (index?.bySection.changes.some((row) => row.kind !== "conflicted") ?? false),
  );
  /** At least one index-side change can be unstaged. */
  const canUnstageAll = $derived(
    canMutate && index !== null && index.bySection.staged.length > 0,
  );

  async function loadSubmoduleStatus(subPath: string): Promise<void> {
    const entry = submodulesMap[subPath];
    if (!entry) return;
    try {
      entry.loading = true;
      let subRepoId = entry.repoId;
      if (!subRepoId) {
        const normRoot = normalizeRepoPath(entry.root);
        const openTab = tabStore.tabs.find(
          (t) => normalizeRepoPath(t.root) === normRoot,
        );
        if (openTab) {
          subRepoId = openTab.id;
        } else {
          // Remote roots are `ssh://<device-id>/<path>` URIs: they must open
          // through `remote_repo_open` — `repo_open` would run a local git2
          // discover on the URI and fail with an OS path error.
          const remote = parseSshUri(entry.root);
          const res = remote
            ? await remoteRepoOpen(remote.deviceId, remote.path)
            : await openRepo(entry.root);
          subRepoId = res.repo_id;
          openedRepoIds.add(res.repo_id);
        }
        entry.repoId = subRepoId;
      }
      const st = await repoStatus(subRepoId);
      entry.status = st;
      entry.index = buildStatusIndex(st.entries, entry.info.path, entry.info.path);
      entry.error = null;
    } catch (err) {
      entry.error = describeError(err);
      entry.status = null;
      entry.index = null;
    } finally {
      entry.loading = false;
    }
  }

  function cleanupOpenedRepos(): void {
    for (const id of openedRepoIds) {
      if (!tabStore.tabs.some((t) => t.id === id)) {
        void closeRepo(id);
      }
    }
    openedRepoIds.clear();
  }

  async function reloadModules(): Promise<void> {
    if (!repoId) {
      moduleToken++;
      moduleList = [];
      moduleError = null;
      moduleLoading = false;
      hasSubmodules = false;
      submodulesMap = {};
      return;
    }
    const token = ++moduleToken;
    moduleLoading = true;
    try {
      const list = await listSubmodules(repoId);
      if (token !== moduleToken) return;
      moduleList = list;
      moduleError = null;
      hasSubmodules = list.length > 0;

      const nextMap: Record<string, SubmoduleData> = {};
      for (const info of list) {
        const subRoot = modulePath(info);
        const existing = submodulesMap[info.path];
        nextMap[info.path] = {
          info,
          repoId: existing?.repoId ?? null,
          root: subRoot,
          status: existing?.status ?? null,
          index: existing?.index ?? null,
          loading: existing ? existing.loading : info.initialized,
          error: existing?.error ?? null,
        };
      }
      submodulesMap = nextMap;

      await Promise.all(
        list.map(async (info) => {
          if (!info.initialized) return;
          await loadSubmoduleStatus(info.path);
        }),
      );
    } catch (err) {
      if (token !== moduleToken) return;
      moduleError = describeError(err);
    } finally {
      if (token === moduleToken) moduleLoading = false;
    }
  }

  // Repo switch = new submodule list. Manual refresh / mutations reload it
  // explicitly; untracked so tabStore.tabs mutations don't trigger reloads.
  $effect(() => {
    void repoId;
    untrack(() => {
      cleanupOpenedRepos();
      void reloadModules();
    });
  });

  $effect(() => {
    return onRepoChange({
      handle: (change) => {
        if (change.repo_id === repoId) {
          if (
            change.full ||
            change.paths.some(
              (p) => p === ".gitmodules" || moduleByPath.has(p),
            )
          ) {
            void reloadModules();
          }
          return;
        }
        for (const [subPath, entry] of Object.entries(submodulesMap)) {
          if (entry.repoId && change.repo_id === entry.repoId) {
            void loadSubmoduleStatus(subPath);
          }
        }
      },
    });
  });

  $effect(() => {
    return () => {
      cleanupOpenedRepos();
    };
  });

  function refreshAll(): void {
    onAfterMutation?.();
    void reloadModules();
  }

  /** Whether the row (by key) is in the multi-selection. */
  function isSelected(key: string): boolean {
    return selected.includes(key);
  }

  /** All live status rows across the main repository and all submodules. */
  const allRows = $derived.by<StatusRow[]>(() => {
    const list: StatusRow[] = [];
    if (index) list.push(...index.rows);
    if (hasSubmodules) {
      for (const info of moduleList) {
        const sub = submodulesMap[info.path];
        if (sub?.index) list.push(...sub.index.rows);
      }
    }
    return list;
  });

  /**
   * Stage/unstage remaps queued by mutations: once the refreshed status
   * arrives, selected keys flip section (a staged file moves Changes →
   * Staged Changes) so the selection follows the files, VS Code style.
   */
  let pendingRemaps: { prefix?: string; paths: ReadonlySet<string>; unstage: boolean }[] = [];

  // Runs on every status refresh: apply queued remaps, then drop keys whose
  // rows vanished (discarded, committed elsewhere, filtered out upstream).
  $effect(() => {
    const rows = allRows;
    if (rows.length === 0 && pendingRemaps.length === 0) return;
    let next = selected;
    for (const remap of pendingRemaps) {
      next = remapSelection(next, remap.paths, remap.unstage, rows, remap.prefix);
    }
    pendingRemaps = [];
    next = pruneSelection(next, rows);
    const changed =
      next.length !== selected.length ||
      next.some((key, i) => key !== selected[i]);
    if (changed) selected = next;
  });

  function focusRow(key: string): void {
    focusKey = key;
    const mounted = rowEls[key];
    if (mounted?.isConnected) {
      mounted.focus({ preventScroll: true });
      const element = treeElement;
      if (element) {
        const elRect = mounted.getBoundingClientRect();
        const treeRect = element.getBoundingClientRect();
        if (elRect.top < treeRect.top + STATUS_SECTION_HEIGHT) {
          element.scrollTop -= treeRect.top + STATUS_SECTION_HEIGHT - elRect.top;
        } else if (elRect.bottom > treeRect.bottom) {
          element.scrollTop += elRect.bottom - treeRect.bottom;
        }
      }
      return;
    }

    const element = treeElement;
    if (!element) return;

    let offset = 0;
    let rowTop: number | null = null;
    for (const section of sections) {
      offset += STATUS_SECTION_HEIGHT;
      if (collapsed[section.id]) continue;
      const index = section.rows.findIndex((row) => row.key === key);
      if (index !== -1) {
        rowTop = offset + index * STATUS_ROW_HEIGHT;
        break;
      }
      offset += (section.rows.length + section.modules.length) * STATUS_ROW_HEIGHT;
    }
    if (rowTop === null) return;

    const stickyTop = element.scrollTop + STATUS_SECTION_HEIGHT;
    const viewportBottom = element.scrollTop + element.clientHeight;
    if (rowTop < stickyTop) {
      element.scrollTop = Math.max(0, rowTop - STATUS_SECTION_HEIGHT);
    } else if (rowTop + STATUS_ROW_HEIGHT > viewportBottom) {
      element.scrollTop = rowTop + STATUS_ROW_HEIGHT - element.clientHeight;
    }
    treeScrollTop = element.scrollTop;
    treeViewportHeight = Math.max(1, element.clientHeight);
    requestAnimationFrame(() => rowEls[key]?.focus({ preventScroll: true }));
  }

  function trackTreeViewport(node: HTMLDivElement) {
    treeElement = node;
    const update = () => {
      treeScrollTop = node.scrollTop;
      treeViewportHeight = Math.max(1, node.clientHeight);
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

  function shortSha(sha: string | null): string {
    return sha === null || sha === "" ? "—" : sha.slice(0, 7);
  }

  /** `splitPath` keeps the separator; rows show the folder without it. */
  function dirText(dir: string): string {
    return dir.replace(/[\\/]+$/, "");
  }

  /** Change-kind letter color, mapped straight to the app status tokens
   *  (replaces the former `k-*` scoped classes). */
  const KIND_COLOR: Record<string, string> = {
    added: "text-[color:var(--app-added)]",
    deleted: "text-[color:var(--app-deleted)]",
    modified: "text-[color:var(--app-modified)]",
    renamed: "text-[color:var(--app-renamed)]",
    copied: "text-[color:var(--app-renamed)]",
    untracked: "text-[color:var(--app-untracked)]",
    conflicted: "text-[color:var(--app-conflicted)]",
  };

  function kindColorClass(kind: ChangeKind): string {
    return KIND_COLOR[kind] ?? "text-[color:var(--app-modified)]";
  }

  /** Submodule tag tone color (`submoduleDisplay().tone` → status token). */
  function toneColorClass(tone: "clean" | "dirty" | "danger"): string {
    return tone === "dirty"
      ? "text-[color:var(--app-modified)]"
      : tone === "danger"
        ? "text-[color:var(--app-deleted)]"
        : "";
  }

  /** Absolute worktree path of a submodule (portable separator). */
  function modulePath(info: SubmoduleInfo): string {
    return `${root.replace(/[\\/]+$/, "")}/${info.path}`;
  }

  function moduleTitle(info: SubmoduleInfo): string {
    const display = submoduleDisplay(info);
    const lines = [info.path, display.detail];
    if (info.url) lines.push(info.url);
    lines.push(
      `HEAD ${shortSha(info.head_sha)} · recorded ${shortSha(info.recorded_sha)}`,
    );
    return lines.join("\n");
  }

  function rowTitle(row: StatusRow, info: SubmoduleInfo | undefined): string {
    const lines = [row.entry.path, kindLabel(row.kind)];
    if (row.entry.old_path) lines.push(`from ${row.entry.old_path}`);
    if (info) lines.push(`Submodule: ${submoduleDisplay(info).detail}`);
    if (row.section === "staged") lines.push("S unstage");
    else if (row.kind !== "conflicted") lines.push("S stage · D discard");
    return lines.join("\n");
  }

  function rowAriaLabel(row: StatusRow, info: SubmoduleInfo | undefined): string {
    const parts = [row.entry.path, kindLabel(row.kind)];
    if (row.entry.old_path) parts.push(`renamed from ${row.entry.old_path}`);
    if (info) parts.push(`submodule, ${submoduleDisplay(info).detail}`);
    return parts.join(", ");
  }

  async function openSubmodule(info: SubmoduleInfo): Promise<void> {
    if (!info.initialized) {
      toast("Submodule is not initialized — update it first", {
        kind: "error",
      });
      return;
    }
    if (!root) {
      toast("This view has no worktree path", { kind: "error" });
      return;
    }
    try {
      await openTab(modulePath(info));
    } catch (err) {
      toast(`Open failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  async function updateSubmodule(info: SubmoduleInfo): Promise<void> {
    if (!repoId || moduleBusy !== null) return;
    moduleBusy = info.path;
    try {
      await submoduleUpdate(repoId, info.path, !info.initialized, false);
      toast(
        info.initialized
          ? `Updated submodule ${info.path}`
          : `Initialized submodule ${info.path}`,
        { kind: "success" },
      );
      onAfterMutation?.();
      await reloadModules();
    } catch (err) {
      toast(`Submodule update failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      moduleBusy = null;
    }
  }

  async function syncSubmodule(info: SubmoduleInfo): Promise<void> {
    if (!repoId || moduleBusy !== null) return;
    moduleBusy = info.path;
    try {
      await submoduleSync(repoId, info.path);
      toast(`Synced submodule ${info.path}`, { kind: "success" });
      await reloadModules();
    } catch (err) {
      toast(`Submodule sync failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      moduleBusy = null;
    }
  }

  /**
   * Plain click: select + open (submodule tab, conflict editor, or diff).
   * Ctrl: toggle in the selection (no open). Shift: extend the range from
   * the anchor (no open). All VS Code list behaviors.
   */
  function openRow(row: StatusRow): void {
    if (row.submodulePath) {
      const sub = submodulesMap[row.submodulePath];
      if (sub && sub.repoId) {
        onOpenDiff(
          row.entry,
          sub.repoId,
          sub.root,
          sub.info.name || sub.info.path,
        );
        return;
      }
    }
    const info = moduleByPath.get(row.entry.path);
    if (info) {
      void openSubmodule(info);
      return;
    }
    if (row.kind === "conflicted" && onOpenConflict !== undefined) {
      onOpenConflict(row.entry);
    } else {
      onOpenDiff(row.entry);
    }
  }

  function onRowClick(event: MouseEvent, row: StatusRow, rowIndex: number): void {
    if ((event.target as HTMLElement).closest(".row-actions, .ignore-wrap")) {
      return;
    }
    focusKey = row.key;
    if (event.shiftKey) {
      selected = rangeSelection(visibleRows, anchorIndex, rowIndex);
      return;
    }
    anchorIndex = rowIndex;
    if (event.ctrlKey || event.metaKey) {
      selected = toggleSelection(selected, row.key);
    } else {
      selected = [row.key];
      openRow(row);
    }
  }

  /**
   * Row action (hover button, context menu, `s`): Staged rows unstage,
   * Changes rows stage. VS Code applies it to the whole selection when the
   * target row is part of it — a selection spanning both groups stages and
   * unstages in one go, each file in its own direction. The visual state
   * follows the refreshed status once the async mutation lands.
   */
  function stageAction(row: StatusRow, event?: MouseEvent): void {
    event?.stopPropagation();
    if (row.kind === "conflicted") return;
    const targetRepoId = row.submodulePath
      ? submodulesMap[row.submodulePath]?.repoId
      : repoId;
    if (!targetRepoId || mutating) return;

    const targets = actionRows(row, selected, visibleRows);
    const byRepo = new Map<string | undefined, StatusRow[]>();
    for (const t of targets) {
      const key = t.submodulePath;
      const list = byRepo.get(key) ?? [];
      list.push(t);
      byRepo.set(key, list);
    }

    for (const [subPath, repoTargets] of byRepo.entries()) {
      const rid = subPath ? submodulesMap[subPath]?.repoId : repoId;
      if (!rid) continue;
      const actions = selectionActions(repoTargets);
      const paths = [...actions.unstage, ...actions.stage];
      if (paths.length === 0) continue;
      if (actions.unstage.length > 0) {
        pendingRemaps.push({ prefix: subPath ?? "", paths: new Set(actions.unstage), unstage: true });
      }
      if (actions.stage.length > 0) {
        pendingRemaps.push({ prefix: subPath ?? "", paths: new Set(actions.stage), unstage: false });
      }
      const hasModule = !subPath && paths.some((path) => moduleByPath.has(path));
      void mutate(
        stageLabel(actions),
        () =>
          Promise.all([
            actions.unstage.length > 0
              ? stage(rid, {
                  targets: actions.unstage.map((file) => ({ file })),
                  unstage: true,
                })
              : Promise.resolve(),
            actions.stage.length > 0
              ? stage(rid, {
                  targets: actions.stage.map((file) => ({ file })),
                  unstage: false,
                })
              : Promise.resolve(),
          ]).then(() => undefined),
      ).then((ok) => {
        if (ok) {
          if (subPath) {
            void loadSubmoduleStatus(subPath);
            onAfterMutation?.();
            void reloadModules();
          } else {
            if (hasModule) void reloadModules();
          }
        }
      });
    }
  }

  function stageAllSubmodule(subPath: string, unstage: boolean): void {
    const sub = submodulesMap[subPath];
    if (!sub || !sub.repoId) return;
    if (unstage) {
      const stagedRows = sub.index?.bySection.staged ?? [];
      if (stagedRows.length > 0) {
        pendingRemaps.push({
          prefix: subPath,
          paths: new Set(stagedRows.map((r) => r.entry.path)),
          unstage: true,
        });
      }
      void mutate(`Unstage all in ${subPath}`, () => stageAll(sub.repoId!, true)).then((ok) => {
        if (ok) {
          void loadSubmoduleStatus(subPath);
          onAfterMutation?.();
          void reloadModules();
        }
      });
    } else {
      const idx = sub.index;
      if (!idx) return;
      const files = idx.bySection.changes
        .filter((r) => r.kind !== "conflicted")
        .map((r) => ({ file: r.entry.path }));
      if (files.length === 0) return;
      pendingRemaps.push({
        prefix: subPath,
        paths: new Set(files.map((f) => f.file)),
        unstage: false,
      });
      void mutate(`Stage all in ${subPath}`, () =>
        stage(sub.repoId!, { targets: files, unstage: false }),
      ).then((ok) => {
        if (ok) {
          void loadSubmoduleStatus(subPath);
          onAfterMutation?.();
          void reloadModules();
        }
      });
    }
  }

  function stageSubmoduleSection(
    subPath: string,
    sectionId: SectionId,
    event: MouseEvent,
  ): void {
    event.stopPropagation();
    const sub = submodulesMap[subPath];
    if (!sub || !sub.repoId) return;
    const unstage = sectionId === "staged";
    const subIdx = sub.index;
    if (!subIdx) return;
    const rows = subIdx.bySection[sectionId].filter(
      (r) => r.kind !== "conflicted",
    );
    if (rows.length === 0) return;
    pendingRemaps.push({
      prefix: subPath,
      paths: new Set(rows.map((r) => r.entry.path)),
      unstage,
    });
    void mutate(
      `${unstage ? "Unstage" : "Stage"} all in ${subPath} (${sectionId})`,
      () =>
        stage(sub.repoId!, {
          targets: rows.map((r) => ({ file: r.entry.path })),
          unstage,
        }),
    ).then((ok) => {
      if (ok) {
        void loadSubmoduleStatus(subPath);
        onAfterMutation?.();
        void reloadModules();
      }
    });
  }

  /** Error-toast label: the verb plus the file count when it is not 1. */
  function stageLabel(actions: SelectionActions): string {
    const verbs: string[] = [];
    if (actions.unstage.length > 0) {
      verbs.push(countLabel("Unstage", actions.unstage.length));
    }
    if (actions.stage.length > 0) {
      verbs.push(countLabel("Stage", actions.stage.length));
    }
    return verbs.join(" + ") || "Stage";
  }

  /** Section-header action: stage Changes or unstage Staged Changes. */
  function stageSection(section: StatusSectionView, event: MouseEvent): void {
    event.stopPropagation();
    if (!repoId) return;
    const unstage = section.id === "staged";
    const rows = section.rows.filter((row) => row.kind !== "conflicted");
    if (rows.length === 0) return;
    pendingRemaps.push({
      prefix: "",
      paths: new Set(rows.map((row) => row.entry.path)),
      unstage,
    });
    const hasModule = rows.some((row) => moduleByPath.has(row.entry.path));
    void mutate(
      `${unstage ? "Unstage" : "Stage"} all in ${section.label}`,
      () =>
        stage(repoId, {
          targets: rows.map((row) => ({ file: row.entry.path })),
          unstage,
        }),
    ).then((ok) => {
      if (ok && hasModule) void reloadModules();
    });
  }

  /**
   * Toolbar action. Unstage goes through `stage_all` (index := HEAD); stage
   * targets every non-conflicted Changes row explicitly so conflicts are
   * never auto-resolved by `git add -A`.
   */
  function onStageAll(unstage: boolean): void {
    if (!repoId) return;
    if (unstage) {
      const source = index;
      if (source !== null && source.bySection.staged.length > 0) {
        pendingRemaps.push({
          prefix: "",
          paths: new Set(
            source.bySection.staged.map((row) => row.entry.path),
          ),
          unstage: true,
        });
      }
      void mutate("Unstage all files", () => stageAll(repoId, true)).then(
        (ok) => {
          if (ok && moduleList.length > 0) void reloadModules();
        },
      );
      return;
    }
    const source = index;
    if (source === null) return;
    const paths = new Set<string>();
    for (const row of source.bySection.changes) {
      const path = row.entry.path;
      if (row.kind !== "conflicted") paths.add(path);
    }
    if (paths.size === 0) return;
    pendingRemaps.push({ prefix: "", paths, unstage: false });
    const files = [...paths];
    const hasModule = files.some((path) => moduleByPath.has(path));
    void mutate("Stage all files", () =>
      stage(repoId, {
        targets: files.map((file) => ({ file })),
        unstage: false,
      }),
    ).then((ok) => {
      if (ok && hasModule) void reloadModules();
    });
  }

  /** Runs one staging mutation: error toast on failure, refresh on success. */
  async function mutate(
    label: string,
    run: () => Promise<void>,
  ): Promise<boolean> {
    mutating = true;
    try {
      await run();
      onAfterMutation?.();
      return true;
    } catch (err) {
      toast(`${label} failed: ${describeError(err)}`, { kind: "error" });
      return false;
    } finally {
      mutating = false;
    }
  }

  // -- menus -----------------------------------------------------------------

  function toolbarMenu(event: MouseEvent): void {
    const entries: MenuEntry[] = [
      {
        id: "stage-all",
        label: "Stage all changes",
        icon: iconStageAll,
        disabled: !canStageAll,
        run: () => onStageAll(false),
      },
      {
        id: "unstage-all",
        label: "Unstage all changes",
        icon: iconUnstageAll,
        disabled: !canUnstageAll,
        run: () => onStageAll(true),
      },
    ];
    if (repoId) {
      entries.push(
        { separator: true },
        {
          id: "ignore-templates",
          label: "Ignore templates…",
          icon: iconTemplates,
          run: () => (galleryOpen = true),
        },
        {
          id: "refresh",
          label: "Refresh changes",
          icon: iconRefresh,
          run: refreshAll,
        },
      );
    }
    entries.push({ separator: true });
    for (const option of FILE_ICON_THEMES) {
      entries.push({
        id: `file-icons-${option.id}`,
        label: `${fileIconPreferences.theme === option.id ? "✓ " : ""}${option.label} file icons`,
        icon: iconPalette,
        run: () => fileIconPreferences.setTheme(option.id),
      });
    }
    showMenuAt(event, entries);
  }

  /**
   * Right-click, kebab button and Menu-key entry point share one menu. VS
   * Code syncs the selection first — opening the menu on an unselected row
   * collapses the selection to that row — and every command then acts on the
   * whole selection; per-file extras (ignore, history, submodule ops) stay
   * single-selection only.
   */
  function syncRowSelection(row: StatusRow): void {
    if (isSelected(row.key)) return;
    selected = [row.key];
    const at = visibleRows.findIndex((r) => r.key === row.key);
    anchorIndex = at === -1 ? 0 : at;
    focusKey = row.key;
  }

  function rowMenuEntries(row: StatusRow): MenuEntry[] {
    const rid = row.submodulePath
      ? submodulesMap[row.submodulePath]?.repoId
      : repoId;
    if (!rid) return [];
    const targets = actionRows(row, selected, visibleRows);
    const actions = selectionActions(targets);
    const entries: MenuEntry[] = [];

    if (actions.unstage.length > 0) {
      entries.push({
        id: "unstage",
        label: countLabel("Unstage", actions.unstage.length),
        run: () => stageAction(row),
      });
    }
    if (actions.stage.length > 0) {
      entries.push({
        id: "stage",
        label: countLabel("Stage", actions.stage.length),
        run: () => stageAction(row),
      });
    }

    const discardable = targets.filter(
      (t) =>
        t.section !== "staged" &&
        t.kind !== "conflicted" &&
        (t.submodulePath ? true : !moduleByPath.has(t.entry.path)),
    );
    if (discardable.length > 0) {
      entries.push({
        id: "discard",
        label: discardable.length > 1
          ? `Discard changes in ${discardable.length} files…`
          : "Discard changes…",
        danger: true,
        run: () => (discardCandidates = discardable),
      });
    }

    const single = targets.length === 1 ? targets[0]! : null;
    if (single !== null) {
      const info = !single.submodulePath
        ? moduleByPath.get(single.entry.path)
        : undefined;
      if (info) {
        entries.push({ separator: true });
        if (info.initialized) {
          entries.push({
            id: "module-open",
            label: "Open submodule",
            run: () => void openSubmodule(info),
          });
        }
        entries.push(
          {
            id: "module-update",
            label: info.initialized
              ? "Update submodule"
              : "Initialize and update submodule",
            run: () => void updateSubmodule(info),
          },
          {
            id: "module-sync",
            label: "Sync submodule URL",
            run: () => void syncSubmodule(info),
          },
          { separator: true },
          {
            id: "open-diff",
            label: "Open gitlink diff",
            run: () => onOpenDiff(single.entry),
          },
        );
      } else {
        if (single.kind === "untracked") {
          for (const choice of choicesFor(single.entry.path)) {
            entries.push({
              id: `ignore-${choice.label}`,
              label: `Ignore ${choice.label} (${choice.pattern})`,
              run: () => void onIgnore(choice, single),
            });
          }
          if (!single.submodulePath) {
            entries.push({
              id: "ignore-templates",
              label: "Ignore templates…",
              run: () => (galleryOpen = true),
            });
          }
        }
        entries.push(
          {
            id: "history",
            label: "File history",
            run: () =>
              void openPanelPopout(
                "filehistory",
                rid,
                `History: ${single.entry.path}`,
                { path: single.entry.path },
              ),
          },
          {
            id: "open",
            label: "Open diff",
            run: () => openRow(single),
          },
        );
      }
    }
    return entries;
  }

  /** Context-menu event: sync the selection, then open beside the pointer. */
  function rowMenu(event: MouseEvent, row: StatusRow): void {
    syncRowSelection(row);
    showMenuAt(event, rowMenuEntries(row));
  }

  /**
   * Kebab button / Menu key: open the same menu anchored to the row.
   * Keyboard users reach it with the Menu key (or Shift+F10) on a focused
   * row — the hover buttons alone are not a keyboard path.
   */
  function openRowMenu(row: StatusRow): void {
    syncRowSelection(row);
    const entries = rowMenuEntries(row);
    if (entries.length === 0) return;
    const el = rowEls[row.key];
    const rect = el?.getBoundingClientRect();
    contextMenu.show(
      rect ? rect.right : window.innerWidth / 2,
      rect ? rect.bottom : window.innerHeight / 2,
      entries,
    );
  }

  /** Extra submodule row menu (modules not represented in git status). */
  function submoduleMenu(event: MouseEvent, info: SubmoduleInfo): void {
    if (!repoId) return;
    const entries: MenuEntry[] = [];
    if (info.initialized) {
      entries.push({
        id: "module-open",
        label: "Open submodule",
        run: () => void openSubmodule(info),
      });
    }
    entries.push(
      {
        id: "module-update",
        label: info.initialized
          ? "Update submodule"
          : "Initialize and update submodule",
        run: () => void updateSubmodule(info),
      },
      {
        id: "module-sync",
        label: "Sync submodule URL",
        run: () => void syncSubmodule(info),
      },
      { separator: true },
      {
        id: "module-refresh",
        label: "Refresh submodules",
        run: () => void reloadModules(),
      },
    );
    showMenuAt(event, entries);
  }

  // -- discard / ignore gallery ------------------------------------------------

  let galleryOpen = $state(false);
  /** Rows pending a discard confirmation (whole files; checkpointed upstream). */
  let discardCandidates = $state<StatusRow[]>([]);

  /**
   * Hover button / `d`: discard the selection when the target row is part
   * of it, otherwise just the row. Staged, conflicted and submodule rows
   * never discard (conflict resolution is an explicit edit).
   */
  function requestDiscard(row: StatusRow, event?: MouseEvent): void {
    event?.stopPropagation();
    if (row.section === "staged" || row.kind === "conflicted") {
      return;
    }
    if (!row.submodulePath && moduleByPath.has(row.entry.path)) return;
    const targetRepoId = row.submodulePath
      ? submodulesMap[row.submodulePath]?.repoId
      : repoId;
    if (!targetRepoId || mutating) return;

    discardCandidates = actionRows(row, selected, visibleRows).filter(
      (candidate) =>
        candidate.section !== "staged" &&
        candidate.kind !== "conflicted" &&
        (candidate.submodulePath ? true : !moduleByPath.has(candidate.entry.path)),
    );
  }

  /** Discard confirmed: the backend snapshots a checkpoint first. */
  async function onDiscardConfirmed(): Promise<void> {
    const candidates = discardCandidates;
    if (candidates.length === 0) return;
    discardCandidates = [];

    const byRepo = new Map<string | undefined, StatusRow[]>();
    for (const c of candidates) {
      const list = byRepo.get(c.submodulePath) ?? [];
      list.push(c);
      byRepo.set(c.submodulePath, list);
    }

    for (const [subPath, repoCandidates] of byRepo.entries()) {
      const rid = subPath ? submodulesMap[subPath]?.repoId : repoId;
      if (!rid) continue;
      await mutate(
        repoCandidates.length > 1
          ? `Discard ${repoCandidates.length} files`
          : `Discard ${repoCandidates[0]!.entry.path}`,
        () =>
          discard(
            rid,
            repoCandidates.map((row) => ({ file: row.entry.path })),
          ),
      );
      if (subPath) {
        void loadSubmoduleStatus(subPath);
      }
    }
    onAfterMutation?.();
    void reloadModules();
  }

  /**
   * VS Code list keyboard model: plain movement replaces the selection and
   * moves the anchor; Shift extends the range from the anchor; focus clamps
   * at the ends (no wraparound). Ctrl+A selects all; Space toggles the
   * focused row without disturbing the rest; Enter opens without touching
   * the selection; `s` stages/unstages and `d` discards the selection.
   */
  function onTreeKeydown(event: KeyboardEvent): void {
    // The Ignore quick-add controls handle their own keys (Enter/Space must
    // not bubble into row open/toggle).
    if ((event.target as HTMLElement).closest(".ignore-wrap")) return;
    const rows = visibleRows;
    if (rows.length === 0) return;
    const current =
      focusKey !== null ? rows.findIndex((r) => r.key === focusKey) : -1;
    const base = current === -1 ? 0 : current;

    const pageRows = Math.max(
      1,
      Math.floor(treeViewportHeight / STATUS_ROW_HEIGHT) - 1,
    );
    let next: number | null = null;
    if (event.key === "ArrowDown") next = Math.min(base + 1, rows.length - 1);
    else if (event.key === "ArrowUp") next = Math.max(base - 1, 0);
    else if (event.key === "PageDown") {
      next = Math.min(base + pageRows, rows.length - 1);
    } else if (event.key === "PageUp") {
      next = Math.max(base - pageRows, 0);
    } else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = rows.length - 1;

    if (next !== null) {
      event.preventDefault();
      const row = rows[next]!;
      if (event.shiftKey) {
        selected = rangeSelection(rows, anchorIndex, next);
      } else {
        selected = [row.key];
        anchorIndex = next;
      }
      focusRow(row.key);
      return;
    }

    if ((event.ctrlKey || event.metaKey) && event.key === "a") {
      event.preventDefault();
      selected = allSelection(rows);
      return;
    }

    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const row = rows[base];
    if (!row) return;

    if (event.key === " ") {
      event.preventDefault();
      anchorIndex = base;
      selected = toggleSelection(selected, row.key);
    } else if (event.key === "Enter") {
      event.preventDefault();
      openRow(row);
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      openRowMenu(row);
    } else if (event.key === "s" || event.key === "S") {
      if (row.kind !== "conflicted") {
        event.preventDefault();
        stageAction(row);
      }
    } else if (event.key === "d" || event.key === "D") {
      if (
        row.section !== "staged" &&
        row.kind !== "conflicted" &&
        (row.submodulePath ? true : !moduleByPath.has(row.entry.path))
      ) {
        event.preventDefault();
        requestDiscard(row);
      }
    }
  }

  /** ArrowDown in the filter box selects and focuses the first row. */
  function onFilterKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" && visibleRows.length > 0) {
      event.preventDefault();
      const row = visibleRows[0]!;
      selected = [row.key];
      anchorIndex = 0;
      focusRow(row.key);
    }
  }

  function toggleSection(id: SectionId): void {
    collapsed[id] = !collapsed[id];
  }

</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape") ignoreFor = null;
  }}
/>

<aside
  aria-label="Source control"
  class="flex flex-1 flex-col h-full min-h-0 min-w-0 overflow-hidden bg-background text-foreground text-[0.8125rem]"
>
  {#if status}
    <div class="toolbar flex items-center gap-0.5 px-[0.4rem] py-[0.3rem] flex-none bg-card border-b border-border">
      <span class="relative flex-1 min-w-0 flex items-center text-muted-foreground">
        <span class="absolute left-1.5 inline-flex pointer-events-none opacity-75">
          <Icon icon={iconSearch} size={13} />
        </span>
        <Input
          class="h-[1.6rem] rounded-sm bg-background dark:bg-background px-6 text-xs md:text-xs placeholder:text-muted-foreground/80"
          type="text"
          placeholder="Filter changes"
          aria-label="Filter changed files by path"
          bind:value={filterText}
          onkeydown={onFilterKeydown}
        />
        {#if filterText}
          <Button
            class="absolute right-[0.2rem] size-[1.15rem] rounded-full p-0 text-muted-foreground hover:bg-accent hover:text-foreground"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label="Clear filter"
            title="Clear filter"
            onclick={() => (filterText = "")}
          >
            <Icon icon={iconClear} size={12} />
          </Button>
        {/if}
      </span>
      <Button
        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Stage all files"
        title="Stage every file in Changes"
        disabled={!canStageAll}
        onclick={() => onStageAll(false)}
      >
        <Icon icon={iconStageAll} size={16} />
      </Button>
      <Button
        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Unstage all files"
        title="Unstage every file in Staged Changes (all sections)"
        disabled={!canUnstageAll}
        onclick={() => onStageAll(true)}
      >
        <Icon icon={iconUnstageAll} size={16} />
      </Button>
      <Button
        class={`relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground ${mutating || moduleLoading ? "[&_svg]:animate-spin" : ""} [&_svg:not([class*='size-'])]:size-4`}
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Refresh changes"
        title="Refresh changes and submodule state"
        disabled={!repoId || mutating || moduleLoading}
        onclick={refreshAll}
      >
        <Icon icon={iconRefresh} size={16} />
      </Button>
      <Button
        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="More source-control actions"
        title="More actions"
        onclick={(event) => toolbarMenu(event)}
      >
        <Icon icon={iconMore} size={16} />
      </Button>
    </div>

    {#if status.merging || status.rebasing || status.sequencer || status.detached || status.ahead > 0 || status.behind > 0}
      <div class="banners flex flex-wrap gap-1 px-[0.4rem] py-[0.3rem] flex-none">
        {#if status.merging}<Badge variant="secondary" class="gap-[0.2rem] rounded-full px-[0.45rem] py-px h-auto text-[0.6875rem] bg-muted text-foreground">Merge in progress</Badge>{/if}
        {#if status.rebasing}<Badge variant="secondary" class="gap-[0.2rem] rounded-full px-[0.45rem] py-px h-auto text-[0.6875rem] bg-muted text-foreground">Rebase in progress</Badge>{/if}
        {#if status.sequencer}<Badge variant="secondary" class="gap-[0.2rem] rounded-full px-[0.45rem] py-px h-auto text-[0.6875rem] bg-muted text-foreground">Cherry-pick / revert in progress</Badge>{/if}
        {#if status.detached}<Badge variant="secondary" class="gap-[0.2rem] rounded-full px-[0.45rem] py-px h-auto text-[0.6875rem] bg-muted text-foreground">Detached HEAD</Badge>{/if}
        {#if status.ahead > 0}
          <Badge
            variant="secondary"
            class="gap-[0.2rem] rounded-full px-[0.45rem] py-px h-auto text-[0.6875rem] bg-primary/10 text-primary tabular-nums"
            role="img"
            aria-label={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}
            title={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}
          >
            <Icon icon={iconAhead} size={11} />
            {status.ahead} to push
          </Badge>
        {/if}
        {#if status.behind > 0}
          <Badge
            variant="secondary"
            class="gap-[0.2rem] rounded-full px-[0.45rem] py-px h-auto text-[0.6875rem] bg-accent text-accent-foreground tabular-nums"
            role="img"
            aria-label={`${status.behind} ${status.behind === 1 ? "commit" : "commits"} to pull`}
            title={`${status.behind} ${status.behind === 1 ? "commit" : "commits"} to pull`}
          >
            <Icon icon={iconBehind} size={11} />
            {status.behind} to pull
          </Badge>
        {/if}
      </div>
    {/if}

    {#if moduleError}
      <Button
        variant="ghost"
        class="mx-[0.4rem] mb-1 h-auto justify-start border border-[color:var(--app-deleted)] rounded-sm px-1 py-0.5 bg-[color:var(--app-conflicted-bg)] text-[color:var(--app-deleted)] text-left text-[0.6875rem] hover:bg-[color:var(--app-conflicted-bg)] hover:text-[color:var(--app-deleted)] dark:hover:bg-[color:var(--app-conflicted-bg)] dark:hover:text-[color:var(--app-deleted)]"
        title={moduleError}
        onclick={() => void reloadModules()}
      >
        Submodule status unavailable — retry
      </Button>
    {/if}

    {#snippet renderRow(
      row: StatusRow,
      targetRepoId: string | null,
      targetRoot: string,
      subPath?: string,
    )}
      {@const info = !subPath ? moduleByPath.get(row.entry.path) : undefined}
      {@const display = info ? submoduleDisplay(info) : null}
      <div
        class="row file-row group/row relative flex items-center gap-[0.4rem] min-w-0 h-6 min-h-6 box-border pl-[0.45rem] pr-[0.4rem] cursor-pointer [content-visibility:auto] [contain-intrinsic-size:auto_1.375rem] hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2 [&.selected]:bg-[color:var(--app-active)] [&.selected]:shadow-[inset_2px_0_0_var(--primary)]"
        class:selected={isSelected(row.key)}
        role="treeitem"
        aria-selected={isSelected(row.key)}
        aria-label={rowAriaLabel(row, info)}
        aria-keyshortcuts={row.section === "staged"
          ? "S"
          : row.kind === "conflicted"
            ? undefined
            : "S D"}
        tabindex={row.key === tabIndexKey ? 0 : -1}
        data-row-key={row.key}
        data-kind={row.kind}
        bind:this={rowEls[row.key]}
        title={rowTitle(row, info)}
        onkeydown={onTreeKeydown}
        oncontextmenu={(event) => rowMenu(event, row)}
        onclick={(event) =>
          onRowClick(
            event,
            row,
            visibleRows.findIndex((r) => r.key === row.key),
          )}
      >
        {#if info}
          <span class="shrink-0 inline-flex text-muted-foreground opacity-90">
            <Icon icon={iconModule} size={14} />
          </span>
        {:else}
          <span
            class="file-icon shrink-0 inline-flex items-center justify-center leading-none"
            style:color={fileIconColorForPath(row.entry.path, fileIconPreferences.theme)}
          >
            <Icon
              icon={fileIconForPath(row.entry.path, fileIconPreferences.theme)}
              size={14}
            />
          </span>
        {/if}
        <span class="path flex-1 basis-auto min-w-0 flex items-baseline gap-[0.35rem] overflow-hidden text-xs leading-[1.25]">
          <span
            class="base min-w-8 overflow-hidden text-ellipsis whitespace-nowrap text-foreground font-normal group-data-[kind=added]/row:text-[color:var(--app-added)] group-data-[kind=untracked]/row:text-[color:var(--app-untracked)] group-data-[kind=modified]/row:text-[color:var(--app-modified)] group-data-[kind=renamed]/row:text-[color:var(--app-renamed)] group-data-[kind=copied]/row:text-[color:var(--app-renamed)] group-data-[kind=conflicted]/row:text-[color:var(--app-conflicted)] group-data-[kind=deleted]/row:text-[color:var(--app-deleted)] group-data-[kind=deleted]/row:line-through group-data-[kind=deleted]/row:decoration-1"
            title={row.entry.path}
          >{row.base}</span>
          {#if row.dir}
            <span
              class="dir grow min-w-0 max-w-[48%] ml-auto overflow-hidden text-ellipsis whitespace-nowrap text-right text-muted-foreground opacity-70 text-[0.6875rem]"
              title={dirText(row.dir)}
            >{dirText(row.dir)}</span>
          {/if}
        </span>
        {#if info && display && display.tag}
          <span class={`module-tag shrink-0 max-w-[6.5rem] overflow-hidden text-ellipsis whitespace-nowrap text-[0.625rem] leading-[1.35] text-muted-foreground ${toneColorClass(display.tone)}`} title={display.detail}>
            {display.tag}
          </span>
        {/if}
        <span
          class={`kind shrink-0 w-4 inline-flex items-center justify-center text-[0.6875rem] font-bold font-mono leading-none relative z-[1] cursor-help ${kindColorClass(row.kind)}`}
          title={`${row.entry.path} · ${kindLabel(row.kind)}`}
          aria-hidden="true"
        >
          {kindLetter(row.kind)}
        </span>
        <span class="row-actions absolute right-[1.35rem] top-1/2 -translate-y-1/2 flex items-center gap-px opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-visible/row:opacity-100 group-has-[:focus-visible]/row:opacity-100 pointer-coarse:static pointer-coarse:transform-none pointer-coarse:opacity-100">
          <span
            aria-hidden="true"
            class={`pointer-events-none absolute -left-[0.9rem] -top-[0.1rem] -bottom-[0.1rem] w-[0.9rem] pointer-coarse:hidden ${isSelected(row.key)
              ? "bg-[linear-gradient(to_right,transparent,color-mix(in_srgb,var(--primary)_12%,var(--background)))]"
              : "bg-[linear-gradient(to_right,transparent,var(--background))] group-hover/row:bg-[linear-gradient(to_right,transparent,color-mix(in_srgb,var(--foreground)_5%,var(--background)))]"}`}
          ></span>
          {#if row.section === "staged"}
            <Button
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label={`Unstage ${row.entry.path}`}
              title="Unstage this file (S)"
              tabindex={-1}
              disabled={!targetRepoId || mutating}
              onclick={(event) => stageAction(row, event)}
            >
              <Icon icon={iconUnstage} size={16} />
            </Button>
          {:else if row.kind !== "conflicted"}
            <Button
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label={`Stage ${row.entry.path}`}
              title="Stage this file (S)"
              tabindex={-1}
              disabled={!targetRepoId || mutating}
              onclick={(event) => stageAction(row, event)}
            >
              <Icon icon={iconStage} size={16} />
            </Button>
            {#if !info}
              <Button
                class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Discard changes to ${row.entry.path}`}
                title="Discard changes to this file (D)"
                tabindex={-1}
                disabled={!targetRepoId || mutating}
                onclick={(event) => requestDiscard(row, event)}
              >
                <Icon icon={iconDiscard} size={16} />
              </Button>
            {/if}
          {/if}
          {#if info}
            <Button
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label={`Open submodule ${row.entry.path}`}
              title="Open this submodule as a tab"
              tabindex={-1}
              disabled={!info.initialized}
              onclick={(event) => {
                event.stopPropagation();
                void openSubmodule(info);
              }}
            >
              <Icon icon={iconOpen} size={16} />
            </Button>
          {/if}
          {#if row.kind === "untracked" && !info && targetRepoId && !mutating}
            <span class="ignore-wrap relative shrink-0 inline-flex">
              <DropdownMenu.Root
                open={ignoreFor === row.key}
                onOpenChange={(open) => {
                  if (open) {
                    ignoreFor = row.key;
                    ignoreChoices = choicesFor(row.entry.path);
                  } else if (ignoreFor === row.key) {
                    ignoreFor = null;
                  }
                }}
              >
                <DropdownMenu.Trigger
                  class="relative inline-flex shrink-0 cursor-pointer items-center justify-center rounded-md border border-transparent bg-transparent p-0 text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1 aria-expanded:bg-accent aria-expanded:text-accent-foreground size-6 disabled:cursor-not-allowed disabled:text-muted-foreground/50 disabled:hover:bg-transparent"
                  title="Add to .gitignore"
                  aria-label={`Ignore ${row.entry.path}`}
                  disabled={mutating}
                  tabindex={-1}
                  onclick={(event) => event.stopPropagation()}
                >
                  <Icon icon={iconIgnore} size={16} />
                </DropdownMenu.Trigger>
                <DropdownMenu.Content
                  class="min-w-48 max-w-[calc(100vw-1rem)] rounded-md p-0.5"
                  align="end"
                  aria-label="Ignore pattern choices"
                >
                  {#each ignoreChoices as choice (choice.pattern)}
                    <DropdownMenu.Item
                      class="whitespace-nowrap rounded-sm px-2 py-1 text-xs"
                      onSelect={() => void onIgnore(choice, row)}
                    >
                      {choice.label}: <code class="font-mono text-[0.6875rem] text-muted-foreground">{choice.pattern}</code>
                    </DropdownMenu.Item>
                  {/each}
                </DropdownMenu.Content>
              </DropdownMenu.Root>
            </span>
          {/if}
          {#if targetRepoId}
            <Button
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label={`More actions for ${row.entry.path}`}
              title="More actions"
              aria-haspopup="menu"
              tabindex={-1}
              onclick={(event) => {
                event.stopPropagation();
                openRowMenu(row);
              }}
            >
              <Icon icon={iconMore} size={16} />
            </Button>
          {/if}
        </span>
      </div>
    {/snippet}

    {#if hasSubmodules}
      <div
        class="tree multi-repo-tree flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable] pb-3 outline-none"
        role="tree"
        aria-label="Repositories and submodules"
        tabindex="-1"
        bind:this={treeElement}
        use:trackTreeViewport
      >
        <!-- Main repository accordion -->
        <div class="repo-accordion border-b border-border">
          <div class="repo-accordion-header group/acc flex items-center w-full min-h-7 px-1 py-[0.15rem] box-border bg-card border-y border-border select-none gap-1">
            <button
              class="repo-accordion-toggle flex-1 min-w-0 flex items-center gap-[0.35rem] h-[1.6rem] bg-transparent border-0 text-foreground text-xs font-semibold px-[0.2rem] cursor-pointer text-left hover:text-primary focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2"
              type="button"
              aria-expanded={!mainRepoCollapsed}
              onclick={() => (mainRepoCollapsed = !mainRepoCollapsed)}
            >
              <span
                class="chevron inline-flex items-center justify-center w-4 shrink-0 opacity-80 transition-transform {mainRepoCollapsed
                  ? 'rotate-0'
                  : 'rotate-90'}"
                class:closed={mainRepoCollapsed}
              >
                <Icon icon={iconChevron} size={13} />
              </span>
              <span class="repo-title overflow-hidden text-ellipsis whitespace-nowrap">{repoName || "repository"}</span>
              {#if status}
                <span class="branch-pill shrink-0 inline-flex items-center gap-[0.2rem] px-[0.35rem] py-px rounded-full bg-muted text-muted-foreground text-[0.625rem] font-medium max-w-28 overflow-hidden text-ellipsis whitespace-nowrap" title={`Branch: ${mainBranchLabel(status)}`}>
                  <Icon icon={iconBranch} size={10} />
                  <span>{mainBranchLabel(status)}</span>
                </span>
              {/if}
              {#if index && index.total > 0}
                <span class="count-pill shrink-0 min-w-4 px-[0.35rem] rounded-full bg-muted text-muted-foreground text-[0.625rem] font-semibold leading-[1.05rem] text-center tabular-nums">{index.total}</span>
              {/if}
            </button>
            <div class="header-actions flex items-center gap-[0.1rem] shrink-0 opacity-0 transition-opacity group-hover/acc:opacity-100 group-focus-within/acc:opacity-100">
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label="Stage all repository files"
                title="Stage all repository files"
                disabled={!canStageAll}
                onclick={() => onStageAll(false)}
              >
                <Icon icon={iconStageAll} size={16} />
              </Button>
              <Button
                class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label="Unstage all repository files"
                title="Unstage all repository files"
                disabled={!canUnstageAll}
                onclick={() => onStageAll(true)}
              >
                <Icon icon={iconUnstageAll} size={16} />
              </Button>
            </div>
          </div>

          {#if !mainRepoCollapsed}
            {#if repoId}
              <div class="embedded-commit-bar px-[0.4rem] pt-[0.3rem] pb-[0.4rem] bg-background border-b border-border">
                <CommitBar {repoId} {status} onCommitted={refreshAll} />
              </div>
            {/if}

            {#if index && index.total === 0}
              <div class="sub-clean-hint px-3 py-2 text-[0.72rem] text-muted-foreground italic">No changes in main repository</div>
            {:else if sections.length === 0 && filterText}
              <div class="sub-clean-hint px-3 py-2 text-[0.72rem] text-muted-foreground italic">No matching changes in main repository</div>
            {:else}
              {#each sections as section, sectionIndex (section.id)}
                <div class="section" role="group" aria-labelledby={`main-section-${section.id}`}>
                  <div class="section-header group/header sticky top-0 z-[2] flex items-center w-full h-[26px] box-border bg-card border-b border-border select-none" class:border-t={sectionIndex > 0}>
                    <button
                      class="section-toggle flex-1 min-w-0 flex items-center gap-[0.3rem] h-6 bg-transparent border-0 text-muted-foreground text-[0.6875rem] font-bold tracking-[0.045em] uppercase px-2 cursor-pointer text-left hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2"
                      type="button"
                      id={`main-section-${section.id}`}
                      aria-expanded={!collapsed[section.id]}
                      onclick={() => toggleSection(section.id)}
                    >
                      <span
                        class="chevron inline-flex items-center justify-center w-4 shrink-0 opacity-80 transition-transform {collapsed[section.id]
                          ? 'rotate-0'
                          : 'rotate-90'}"
                        class:closed={collapsed[section.id]}
                      >
                        <Icon icon={iconChevron} size={13} />
                      </span>
                      <span class="section-name min-w-0 overflow-hidden text-ellipsis whitespace-nowrap leading-[1.2]">{section.label}</span>
                      <span class="count shrink-0 min-w-[1.15rem] ml-auto px-1.5 rounded-full bg-muted text-muted-foreground text-[0.625rem] font-semibold leading-[1.075rem] text-center tabular-nums transition-opacity group-hover/header:opacity-0 group-focus-within/header:opacity-0 pointer-coarse:hidden">{section.rows.length}</span>
                    </button>
                    {#if canMutate && (section.id === "staged" || section.rows.some((row) => row.kind !== "conflicted"))}
                      <span class="section-action absolute right-[0.3rem] top-1/2 -translate-y-1/2 inline-flex opacity-0 transition-opacity group-hover/header:opacity-100 group-focus-within/header:opacity-100 pointer-coarse:static pointer-coarse:transform-none pointer-coarse:opacity-100 pointer-coarse:pr-1">
                        <Button
                          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                          variant="ghost"
                          size="icon-xs"
                          type="button"
                          aria-label={`${section.id === "staged" ? "Unstage" : "Stage"} all in ${section.label}`}
                          title={`${section.id === "staged" ? "Unstage" : "Stage"} all files in ${section.label}`}
                          onclick={(event) => stageSection(section, event)}
                        >
                          <Icon icon={section.id === "staged" ? iconUnstage : iconStage} size={16} />
                        </Button>
                      </span>
                    {/if}
                  </div>
                  {#if !collapsed[section.id]}
                    {#each section.rows as row (row.key)}
                      {@render renderRow(row, repoId, root)}
                    {/each}
                  {/if}
                </div>
              {/each}
            {/if}
          {/if}
        </div>

        <!-- Submodule accordions -->
        {#each moduleList as info (info.path)}
          {@const sub = submodulesMap[info.path]}
          {@const isCollapsed = submoduleCollapsed[info.path] ?? false}
          {@const display = submoduleDisplay(info)}
          {@const counts = submoduleChangeCounts(sub?.status ?? null)}
          <div class="repo-accordion submodule-accordion border-b border-border">
            <div class="repo-accordion-header group/acc flex items-center w-full min-h-7 px-1 py-[0.15rem] box-border bg-card border-y border-border select-none gap-1">
              <button
                class="repo-accordion-toggle flex-1 min-w-0 flex items-center gap-[0.35rem] h-[1.6rem] bg-transparent border-0 text-foreground text-xs font-semibold px-[0.2rem] cursor-pointer text-left hover:text-primary focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2"
                type="button"
                aria-expanded={!isCollapsed}
                onclick={() => toggleSubmodule(info.path)}
              >
                <span
                  class="chevron inline-flex items-center justify-center w-4 shrink-0 opacity-80 transition-transform {isCollapsed
                    ? 'rotate-0'
                    : 'rotate-90'}"
                  class:closed={isCollapsed}
                >
                  <Icon icon={iconChevron} size={13} />
                </span>
                <Icon icon={iconModule} size={14} />
                <span class="repo-title overflow-hidden text-ellipsis whitespace-nowrap" title={info.path}>{info.path}</span>
                <span class="branch-pill shrink-0 inline-flex items-center gap-[0.2rem] px-[0.35rem] py-px rounded-full bg-muted text-muted-foreground text-[0.625rem] font-medium max-w-28 overflow-hidden text-ellipsis whitespace-nowrap" title={`Branch: ${submoduleBranchLabel(info, sub?.status ?? null)}`}>
                  <Icon icon={iconBranch} size={10} />
                  <span>{submoduleBranchLabel(info, sub?.status ?? null)}</span>
                </span>
                {#if display.tag}
                  <span class={`module-tag shrink-0 max-w-[6.5rem] overflow-hidden text-ellipsis whitespace-nowrap text-[0.625rem] leading-[1.35] text-muted-foreground ${toneColorClass(display.tone)}`} title={display.detail}>
                    {display.tag}
                  </span>
                {/if}
                {#if info.initialized && counts.total > 0}
                  <span class="count-pill shrink-0 min-w-4 px-[0.35rem] rounded-full bg-muted text-muted-foreground text-[0.625rem] font-semibold leading-[1.05rem] text-center tabular-nums">{counts.total}</span>
                {/if}
              </button>
              <div class="header-actions flex items-center gap-[0.1rem] shrink-0 opacity-0 transition-opacity group-hover/acc:opacity-100 group-focus-within/acc:opacity-100">
                {#if !info.initialized}
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Initialize submodule ${info.path}`}
                    title="Initialize and update submodule"
                    disabled={moduleBusy !== null}
                    onclick={() => void updateSubmodule(info)}
                  >
                    <Icon icon={iconUpdate} size={16} />
                  </Button>
                {:else}
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Stage all in ${info.path}`}
                    title="Stage all files in this submodule"
                    disabled={mutating || counts.changes === 0}
                    onclick={() => stageAllSubmodule(info.path, false)}
                  >
                    <Icon icon={iconStageAll} size={16} />
                  </Button>
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Unstage all in ${info.path}`}
                    title="Unstage all files in this submodule"
                    disabled={mutating || counts.staged === 0}
                    onclick={() => stageAllSubmodule(info.path, true)}
                  >
                    <Icon icon={iconUnstageAll} size={16} />
                  </Button>
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Update submodule ${info.path}`}
                    title="Update submodule (git submodule update)"
                    disabled={moduleBusy !== null}
                    onclick={() => void updateSubmodule(info)}
                  >
                    <Icon icon={iconUpdate} size={16} />
                  </Button>
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Sync submodule ${info.path}`}
                    title="Sync submodule URL"
                    disabled={moduleBusy !== null}
                    onclick={() => void syncSubmodule(info)}
                  >
                    <Icon icon={iconSync} size={16} />
                  </Button>
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Open submodule ${info.path}`}
                    title="Open submodule as a tab"
                    onclick={() => void openSubmodule(info)}
                  >
                    <Icon icon={iconOpen} size={16} />
                  </Button>
                {/if}
              </div>
            </div>

            {#if !isCollapsed}
              {#if !info.initialized}
                <div class="uninitialized-banner flex flex-col items-center gap-[0.4rem] px-2 py-3 bg-background border-b border-border">
                  <p class="uninit-text m-0 text-xs text-muted-foreground">Submodule is not initialized.</p>
                  <Button
                    variant="outline"
                    size="xs"
                    class="gap-[0.35rem] rounded-sm px-[0.6rem] py-1 bg-muted text-primary hover:bg-accent hover:text-primary text-[0.72rem] dark:bg-muted dark:hover:bg-accent [&_svg:not([class*='size-'])]:size-[13px]"
                    onclick={() => void updateSubmodule(info)}
                    disabled={moduleBusy !== null}
                  >
                    <Icon icon={iconUpdate} size={13} />
                    Initialize and update submodule
                  </Button>
                </div>
              {:else if sub?.loading && !sub.status}
                <div class="sub-clean-hint px-3 py-2 text-[0.72rem] text-muted-foreground italic">
                  <Spinner class="size-3" role="presentation" aria-hidden="true" /> Loading submodule changes…
                </div>
              {:else if sub?.error}
                <div class="module-error mx-[0.4rem] mb-1 px-1 py-0.5 border border-[color:var(--app-deleted)] rounded-sm bg-[color:var(--app-conflicted-bg)] text-[color:var(--app-deleted)] text-left text-[0.6875rem]" title={sub.error}>
                  Submodule error: {sub.error}
                </div>
              {:else if sub?.status}
                <div class="embedded-commit-bar px-[0.4rem] pt-[0.3rem] pb-[0.4rem] bg-background border-b border-border">
                  <CommitBar
                    repoId={sub.repoId!}
                    status={sub.status}
                    onCommitted={() => {
                      void loadSubmoduleStatus(info.path);
                      refreshAll();
                    }}
                  />
                </div>

                {@const subFilteredSections = sub.index ? filterSections(sub.index, filterText) : []}
                {#if counts.total === 0}
                  <div class="sub-clean-hint px-3 py-2 text-[0.72rem] text-muted-foreground italic">No changes in submodule</div>
                {:else if subFilteredSections.length === 0 && filterText}
                  <div class="sub-clean-hint px-3 py-2 text-[0.72rem] text-muted-foreground italic">No matching changes in submodule</div>
                {:else}
                  {#each subFilteredSections as subSection, subSectionIndex (subSection.id)}
                    {@const subSecCollapsed = isSubSectionCollapsed(info.path, subSection.id)}
                    <div class="section" role="group" aria-labelledby={`sub-${info.path}-${subSection.id}`}>
                      <div class="section-header group/header sticky top-0 z-[2] flex items-center w-full h-[26px] box-border bg-card border-b border-border select-none" class:border-t={subSectionIndex > 0}>
                        <button
                          class="section-toggle flex-1 min-w-0 flex items-center gap-[0.3rem] h-6 bg-transparent border-0 text-muted-foreground text-[0.6875rem] font-bold tracking-[0.045em] uppercase px-2 cursor-pointer text-left hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2"
                          type="button"
                          id={`sub-${info.path}-${subSection.id}`}
                          aria-expanded={!subSecCollapsed}
                          onclick={() => toggleSubSection(info.path, subSection.id)}
                        >
                          <span
                            class="chevron inline-flex items-center justify-center w-4 shrink-0 opacity-80 transition-transform {subSecCollapsed
                              ? 'rotate-0'
                              : 'rotate-90'}"
                            class:closed={subSecCollapsed}
                          >
                            <Icon icon={iconChevron} size={13} />
                          </span>
                          <span class="section-name min-w-0 overflow-hidden text-ellipsis whitespace-nowrap leading-[1.2]">{subSection.label}</span>
                          <span class="count shrink-0 min-w-[1.15rem] ml-auto px-1.5 rounded-full bg-muted text-muted-foreground text-[0.625rem] font-semibold leading-[1.075rem] text-center tabular-nums transition-opacity group-hover/header:opacity-0 group-focus-within/header:opacity-0 pointer-coarse:hidden">{subSection.rows.length}</span>
                        </button>
                        {#if sub.repoId && (subSection.id === "staged" || subSection.rows.some((row) => row.kind !== "conflicted"))}
                          <span class="section-action absolute right-[0.3rem] top-1/2 -translate-y-1/2 inline-flex opacity-0 transition-opacity group-hover/header:opacity-100 group-focus-within/header:opacity-100 pointer-coarse:static pointer-coarse:transform-none pointer-coarse:opacity-100 pointer-coarse:pr-1">
                            <Button
                              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                              variant="ghost"
                              size="icon-xs"
                              type="button"
                              aria-label={`${subSection.id === "staged" ? "Unstage" : "Stage"} all in ${subSection.label}`}
                              title={`${subSection.id === "staged" ? "Unstage" : "Stage"} all files in ${subSection.label}`}
                              disabled={mutating}
                              onclick={(event) => stageSubmoduleSection(info.path, subSection.id, event)}
                            >
                              <Icon icon={subSection.id === "staged" ? iconUnstage : iconStage} size={16} />
                            </Button>
                          </span>
                        {/if}
                      </div>
                      {#if !subSecCollapsed}
                        {#each subSection.rows as row (row.key)}
                          {@render renderRow(row, sub.repoId, sub.root, info.path)}
                        {/each}
                      {/if}
                    </div>
                  {/each}
                {/if}
              {/if}
            {/if}
          </div>
        {/each}
      </div>
    {:else if index !== null && index.total === 0 && extraModules.length === 0}
      <div class="empty-state flex-1 min-h-0 flex flex-col items-center justify-center gap-[0.3rem] px-4 py-8 text-center text-muted-foreground select-none" role="status">
        <span class="empty-icon inline-flex items-center justify-center size-11 mb-0.5 rounded-full border border-border text-[color:var(--app-added)] opacity-90"><Icon icon={iconCheck} size={26} /></span>
        <p class="empty-title m-0 text-[0.8125rem] font-medium text-foreground">No changes</p>
        <p class="empty-hint m-0 max-w-[15rem] text-[0.72rem] leading-[1.4]">Your working tree is clean.</p>
      </div>
    {:else if sections.length === 0 && extraModules.length === 0}
      <div class="empty-state flex-1 min-h-0 flex flex-col items-center justify-center gap-[0.3rem] px-4 py-8 text-center text-muted-foreground select-none" role="status">
        <p class="empty-title m-0 text-[0.8125rem] font-medium text-foreground">No matching changes</p>
        <p class="empty-hint m-0 max-w-[15rem] text-[0.72rem] leading-[1.4]">Nothing matches “{filterText}”.</p>
      </div>
    {:else}
      <div
        class="tree flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable] pb-3 outline-none"
        role="tree"
        aria-label="Changed files"
        tabindex="-1"
        bind:this={treeElement}
        use:trackTreeViewport
      >
        {#each sectionWindows as window, windowIndex (window.section.id)}
          {@const section = window.section}
          <div class="section" role="group" aria-labelledby={`section-${section.id}`}>
            <div class="section-header group/header sticky top-0 z-[2] flex items-center w-full h-[26px] box-border bg-card border-b border-border select-none" class:border-t={windowIndex > 0}>
              <button
                class="section-toggle flex-1 min-w-0 flex items-center gap-[0.3rem] h-6 bg-transparent border-0 text-muted-foreground text-[0.6875rem] font-bold tracking-[0.045em] uppercase px-2 cursor-pointer text-left hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2"
                type="button"
                id={`section-${section.id}`}
                aria-expanded={!collapsed[section.id]}
                aria-label={`${section.label} (${section.rows.length + section.modules.length}), ${collapsed[section.id] ? "expand" : "collapse"}`}
                onclick={() => toggleSection(section.id)}
              >
                <span
                  class="chevron inline-flex items-center justify-center w-4 shrink-0 opacity-80 transition-transform {collapsed[section.id]
                    ? 'rotate-0'
                    : 'rotate-90'}"
                  class:closed={collapsed[section.id]}
                >
                  <Icon icon={iconChevron} size={13} />
                </span>
                <span class="section-name min-w-0 overflow-hidden text-ellipsis whitespace-nowrap leading-[1.2]">{section.label}</span>
                <span class="count shrink-0 min-w-[1.15rem] ml-auto px-1.5 rounded-full bg-muted text-muted-foreground text-[0.625rem] font-semibold leading-[1.075rem] text-center tabular-nums transition-opacity group-hover/header:opacity-0 group-focus-within/header:opacity-0 pointer-coarse:hidden">{section.rows.length + section.modules.length}</span>
              </button>
              {#if canMutate &&
                (section.id === "staged" || section.rows.some((row) => row.kind !== "conflicted"))}
                <span class="section-action absolute right-[0.3rem] top-1/2 -translate-y-1/2 inline-flex opacity-0 transition-opacity group-hover/header:opacity-100 group-focus-within/header:opacity-100 pointer-coarse:static pointer-coarse:transform-none pointer-coarse:opacity-100 pointer-coarse:pr-1">
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`${section.id === "staged" ? "Unstage" : "Stage"} all in ${section.label}`}
                    title={`${section.id === "staged" ? "Unstage" : "Stage"} all files in ${section.label}`}
                    onclick={(event) => stageSection(section, event)}
                  >
                    <Icon icon={section.id === "staged" ? iconUnstage : iconStage} size={16} />
                  </Button>
                </span>
              {/if}
            </div>
            {#if !collapsed[section.id]}
              {#if window.first > 0}
                <div
                  class="row-spacer"
                  aria-hidden="true"
                  style:height={`${window.first * STATUS_ROW_HEIGHT}px`}
                ></div>
              {/if}
              {#each window.rows as row (row.key)}
                {@render renderRow(row, repoId, root)}
              {/each}
              {#if window.last < section.rows.length}
                <div
                  class="row-spacer"
                  aria-hidden="true"
                  style:height={`${(section.rows.length - window.last) * STATUS_ROW_HEIGHT}px`}
                ></div>
              {/if}

              {#each section.modules as info, moduleIndex (info.path)}
                {@const display = submoduleDisplay(info)}
                {@const parts = splitPath(info.path)}
                <div
                  class="row module-row group/row relative flex items-center gap-[0.4rem] min-w-0 h-6 min-h-6 box-border px-[0.4rem] cursor-pointer [content-visibility:auto] [contain-intrinsic-size:auto_1.375rem] hover:bg-[color:var(--app-hover)]"
                  role="treeitem"
                  aria-selected="false"
                  aria-setsize={section.rows.length + section.modules.length}
                  aria-posinset={section.rows.length + moduleIndex + 1}
                  tabindex={-1}
                  aria-label={`${info.path}, submodule, ${display.detail}`}
                  title={moduleTitle(info)}
                  oncontextmenu={(event) => submoduleMenu(event, info)}
                >
                  <Icon icon={iconModule} size={14} />
                  <button
                    class="module-open flex-1 basis-auto min-w-0 flex items-center gap-[0.4rem] bg-transparent border-0 p-0 text-left text-inherit cursor-pointer rounded-xs focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-1"
                    type="button"
                    title={moduleTitle(info)}
                    onclick={() => void openSubmodule(info)}
                  >
                    <span class="path flex-1 basis-auto min-w-0 flex items-baseline gap-[0.35rem] overflow-hidden text-xs leading-[1.25]">
                      <span class={`base min-w-8 overflow-hidden text-ellipsis whitespace-nowrap font-normal ${display.tone === "danger" ? "text-[color:var(--app-deleted)]" : "text-foreground"}`} title={info.path}>{parts.base}</span>
                      {#if parts.dir}
                        <span class="dir grow min-w-0 max-w-[48%] ml-auto overflow-hidden text-ellipsis whitespace-nowrap text-right text-muted-foreground opacity-70 text-[0.6875rem]" title={dirText(parts.dir)}>{dirText(parts.dir)}</span>
                      {/if}
                    </span>
                    {#if display.tag}
                      <span class={`module-tag shrink-0 max-w-[6.5rem] overflow-hidden text-ellipsis whitespace-nowrap text-[0.625rem] leading-[1.35] text-muted-foreground ${toneColorClass(display.tone)}`} title={display.detail}>
                        {display.tag}
                      </span>
                    {/if}
                  </button>
                  <span
                    class="kind shrink-0 w-4 inline-flex items-center justify-center text-[0.6875rem] font-bold font-mono leading-none relative z-[1] cursor-help text-[color:var(--app-modified)]"
                    title={`${info.path} · ${display.detail}`}
                    aria-hidden="true"
                  >
                    S
                  </span>
                  <span class="row-actions absolute right-[1.35rem] top-1/2 -translate-y-1/2 flex items-center gap-px opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-visible/row:opacity-100 group-has-[:focus-visible]/row:opacity-100 pointer-coarse:static pointer-coarse:transform-none pointer-coarse:opacity-100">
                    <span
                      aria-hidden="true"
                      class="pointer-events-none absolute -left-[0.9rem] -top-[0.1rem] -bottom-[0.1rem] w-[0.9rem] pointer-coarse:hidden bg-[linear-gradient(to_right,transparent,var(--background))] group-hover/row:bg-[linear-gradient(to_right,transparent,color-mix(in_srgb,var(--foreground)_5%,var(--background)))]"
                    ></span>
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
                      disabled={moduleBusy !== null}
                      onclick={() => void updateSubmodule(info)}
                    >
                      <Icon icon={iconUpdate} size={16} />
                    </Button>
                    {#if info.initialized}
                      <Button
                        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                        variant="ghost"
                        size="icon-xs"
                        type="button"
                        aria-label={`Open submodule ${info.path}`}
                        title="Open this submodule as a tab"
                        tabindex={-1}
                        onclick={() => void openSubmodule(info)}
                      >
                        <Icon icon={iconOpen} size={16} />
                      </Button>
                    {/if}
                    <Button
                      class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                      variant="ghost"
                      size="icon-xs"
                      type="button"
                      aria-label={`Sync submodule ${info.path}`}
                      title="Rewrite the URL from .gitmodules into the repo config"
                      tabindex={-1}
                      disabled={moduleBusy !== null}
                      onclick={() => void syncSubmodule(info)}
                    >
                      <Icon icon={iconSync} size={16} />
                    </Button>
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  {:else}
    <div class="empty-state flex-1 min-h-0 flex flex-col items-center justify-center gap-[0.3rem] px-4 py-8 text-center text-muted-foreground select-none" role="status">
      <Spinner class="size-[18px]" role="presentation" aria-hidden="true" />
      <p class="empty-title m-0 text-[0.8125rem] font-medium text-foreground">Loading changes…</p>
    </div>
  {/if}
</aside>

<ConfirmDialog
  bind:open={
    () => discardCandidates.length > 0,
    (v) => {
      if (!v) discardCandidates = [];
    }
  }
  title={discardCandidates.length > 1
    ? `Discard changes to ${discardCandidates.length} files?`
    : discardCandidates[0]
      ? `Discard changes to ${discardCandidates[0].submodulePath ? `${discardCandidates[0].submodulePath}/${discardCandidates[0].entry.path}` : discardCandidates[0].entry.path}?`
      : ""}
  message={discardCandidates.length > 1
    ? `Staged and unstaged changes to these ${discardCandidates.length} files return to HEAD (untracked files are deleted). A checkpoint is created first — restorable from the Undo panel.`
    : "Staged and unstaged changes to this file return to HEAD (untracked files are deleted). A checkpoint is created first — restorable from the Undo panel."}
  confirmLabel="Discard changes"
  danger
  onConfirm={() => void onDiscardConfirmed()}
/>

{#if repoId}
  <GitignoreGallery
    bind:open={galleryOpen}
    {repoId}
    onApplied={onAfterMutation}
  />
{/if}

