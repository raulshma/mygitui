<script lang="ts">
  /**
   * Source-control sidebar — VSCode-style working-copy list.
   *
   * Sections come from the pure model in `statusModel.ts` (Merge Changes /
   * Staged Changes / Changes / Untracked Changes). One path can appear in
   * Staged AND Changes, exactly like `git status`. Rows are compact: status
   * letter, file icon, bold basename on the left, dimmed directory on the
   * right, then hover actions.
   *
   * Interaction: click opens the diff (Conflicted rows go to the optional
   * conflict editor when provided), or opens the submodule as a repository
   * tab when the path is a submodule. Ctrl-click toggles multi-selection;
   * Shift-click extends. Row actions stage/unstage/discard through the same
   * backend commands as before. Keyboard: arrows/Home/End move, Space
   * toggles selection, Enter opens, `s` stages or unstages, `d` discards.
   *
   * Submodules: `submodules(repoId)` supplies checked-out vs recorded state.
   * A submodule that the superproject's status already reports is enriched
   * in place (folder icon + short "new commits"/"modified"/"untracked"
   * tag); modules with no status entry (uninitialized, broken) get their own
   * Submodules section so they can be initialized/updated from here.
   */
  import type { RepoStatus, StatusEntry, SubmoduleInfo } from "$lib/ipc/types";
  import {
    discard,
    gitignoreAdd,
    stage,
    stageAll,
    submoduleSync,
    submoduleUpdate,
    submodules as listSubmodules,
  } from "$lib/ipc/client";
  import { openTab } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { patternFor } from "./gitignore";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import GitignoreGallery from "$lib/components/panels/GitignoreGallery.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { IconButton } from "$lib/components/m3";
  import { fileIconForPath } from "$lib/components/icons/fileIcons";
  import { openPanelPopout } from "$lib/layout/popout";
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
  import {
    buildStatusIndex,
    filterSections,
    kindClass,
    kindLabel,
    kindLetter,
    splitPath,
    submoduleDisplay,
    submoduleNeedsAttention,
    type SectionId,
    type StatusIndex,
    type StatusRow,
    type StatusSectionView,
  } from "./statusModel";

  let {
    status,
    onOpenDiff,
    selected = $bindable([]),
    repoId = null,
    root = "",
    onAfterMutation = undefined,
    onOpenConflict = undefined,
  }: {
    status: RepoStatus | null;
    onOpenDiff: (entry: StatusEntry) => void;
    selected: string[];
    /** Owning repository; staging controls disable while null. */
    repoId?: string | null;
    /** Worktree root (opening a submodule as a tab). */
    root?: string;
    /** Called after a successful mutation (RepoView refreshes the status). */
    onAfterMutation?: () => void;
    /** Called instead of `onOpenDiff` for Conflicted rows (M3 editor). */
    onOpenConflict?: (entry: StatusEntry) => void;
  } = $props();

  let filterText = $state("");
  let collapsed = $state<Record<SectionId, boolean>>({
    conflicted: false,
    staged: false,
    unstaged: false,
    untracked: false,
  });
  let modulesCollapsed = $state(false);

  /** Roving focus: key of the focused row (null → first visible row). */
  let focusKey: string | null = $state(null);
  /** Index (in visibleRows) of the selection anchor for Shift ranges. */
  let anchorIndex = 0;
  /** Row elements by key (roving-tabindex focus targets). */
  const rowEls = $state<Record<string, HTMLElement | undefined>>({});
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

  function toggleIgnore(row: StatusRow): void {
    if (ignoreFor === row.key) {
      ignoreFor = null;
      return;
    }
    ignoreFor = row.key;
    ignoreChoices = choicesFor(row.entry.path);
  }

  /** Adds the pattern to .gitignore and refreshes the status. */
  async function onIgnore(choice: { label: string; pattern: string }): Promise<void> {
    if (!repoId || !choice.pattern) return;
    ignoreFor = null;
    try {
      await gitignoreAdd(repoId, choice.pattern);
      toast(`Ignored ${choice.pattern}`, { kind: "success" });
      onAfterMutation?.();
    } catch (err) {
      toast(`Ignore failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  /** Search index — rebuilt once per status change, not per keystroke. */
  const index: StatusIndex | null = $derived(
    status ? buildStatusIndex(status.entries) : null,
  );
  const sections = $derived(index ? filterSections(index, filterText) : []);
  /** Rows actually on screen (collapsed sections excluded) for key nav. */
  const visibleRows = $derived(
    sections.filter((s) => !collapsed[s.id]).flatMap((s) => s.rows),
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
    if (!status) return [] as SubmoduleInfo[];
    const needle = query.toLowerCase();
    return moduleList
      .filter((info) => !statusPaths.has(info.path))
      .filter((info) => submoduleNeedsAttention(info))
      .filter(
        (info) => needle === "" || info.path.toLowerCase().includes(needle),
      )
      .sort((a, b) => a.path.localeCompare(b.path));
  });

  async function reloadModules(): Promise<void> {
    if (!repoId) {
      moduleToken++;
      moduleList = [];
      moduleError = null;
      moduleLoading = false;
      return;
    }
    const token = ++moduleToken;
    moduleLoading = true;
    try {
      const list = await listSubmodules(repoId);
      if (token !== moduleToken) return;
      moduleList = list;
      moduleError = null;
    } catch (err) {
      if (token !== moduleToken) return;
      moduleError = describeError(err);
    } finally {
      if (token === moduleToken) moduleLoading = false;
    }
  }

  // Repo switch = new submodule list. Manual refresh / mutations reload it
  // explicitly; file-watcher status churn must not scan every submodule.
  $effect(() => {
    void repoId;
    void reloadModules();
  });

  function refreshAll(): void {
    onAfterMutation?.();
    void reloadModules();
  }

  function isSelected(path: string): boolean {
    return selected.includes(path);
  }

  function toggleSelected(path: string): void {
    selected = isSelected(path)
      ? selected.filter((p) => p !== path)
      : [...selected, path];
  }

  function setRangeSelection(from: number, to: number): void {
    const [lo, hi] = from <= to ? [from, to] : [to, from];
    const paths: string[] = [];
    for (let i = lo; i <= hi; i++) {
      const path = visibleRows[i]?.entry.path;
      if (path !== undefined && !paths.includes(path)) paths.push(path);
    }
    selected = paths;
  }

  function focusRow(key: string): void {
    focusKey = key;
    rowEls[key]?.focus();
  }

  function shortSha(sha: string | null): string {
    return sha === null || sha === "" ? "—" : sha.slice(0, 7);
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
    else if (row.section !== "conflicted") lines.push("S stage · D discard");
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
   * Ctrl: toggle. Shift: range.
   */
  function openRow(row: StatusRow): void {
    const info = moduleByPath.get(row.entry.path);
    if (info) {
      void openSubmodule(info);
      return;
    }
    if (row.section === "conflicted" && onOpenConflict !== undefined) {
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
      setRangeSelection(anchorIndex, rowIndex);
      return;
    }
    anchorIndex = rowIndex;
    if (event.ctrlKey || event.metaKey) {
      toggleSelected(row.entry.path);
    } else {
      selected = [row.entry.path];
      openRow(row);
    }
  }

  /**
   * Row action: Staged rows unstage; everything else stages. The visual
   * state follows the refreshed status once the async mutation lands.
   */
  function stageRow(row: StatusRow, event?: MouseEvent): void {
    event?.stopPropagation();
    if (!repoId || row.section === "conflicted") return;
    const unstage = row.section === "staged";
    const isModule = moduleByPath.has(row.entry.path);
    void mutate(
      unstage ? `Unstage ${row.entry.path}` : `Stage ${row.entry.path}`,
      () => stage(repoId, { targets: [{ file: row.entry.path }], unstage }),
    ).then((ok) => {
      if (ok && isModule) void reloadModules();
    });
  }

  /** Section-header action: stage everything in Changes/Untracked, unstage Staged. */
  function stageSection(section: StatusSectionView, event: MouseEvent): void {
    event.stopPropagation();
    if (!repoId || section.id === "conflicted") return;
    const unstage = section.id === "staged";
    const rows = section.rows.filter((row) => row.section !== "conflicted");
    if (rows.length === 0) return;
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

  function onStageAll(unstage: boolean): void {
    if (!repoId) return;
    void mutate(
      unstage ? "Unstage all files" : "Stage all files",
      () => stageAll(repoId, unstage),
    ).then((ok) => {
      if (ok && moduleList.length > 0) void reloadModules();
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
        disabled: !canMutate,
        run: () => onStageAll(false),
      },
      {
        id: "unstage-all",
        label: "Unstage all changes",
        icon: iconUnstageAll,
        disabled: !canMutate,
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
    showMenuAt(event, entries);
  }

  /** Row right-click: the file (or submodule) action menu. */
  function rowMenu(event: MouseEvent, row: StatusRow): void {
    const path = row.entry.path;
    const info = moduleByPath.get(path);
    const entries: MenuEntry[] = [];
    if (!repoId) {
      showMenuAt(event, entries);
      return;
    }

    if (row.section !== "staged" && row.section !== "conflicted") {
      entries.push({
        id: "stage",
        label: "Stage",
        run: () =>
          void mutate(`Stage ${path}`, () =>
            stage(repoId, { targets: [{ file: path }], unstage: false }),
          ).then((ok) => {
            if (ok && info) void reloadModules();
          }),
      });
    }
    if (row.section === "staged") {
      entries.push({
        id: "unstage",
        label: "Unstage",
        run: () =>
          void mutate(`Unstage ${path}`, () =>
            stage(repoId, { targets: [{ file: path }], unstage: true }),
          ).then((ok) => {
            if (ok && info) void reloadModules();
          }),
      });
    }

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
          run: () => onOpenDiff(row.entry),
        },
      );
    } else {
      if (row.section !== "conflicted") {
        entries.push({
          id: "discard",
          label: "Discard changes…",
          danger: true,
          run: () => (discardCandidate = row),
        });
      }
      if (row.section === "untracked") {
        for (const choice of choicesFor(path)) {
          entries.push({
            id: `ignore-${choice.label}`,
            label: `Ignore ${choice.label} (${choice.pattern})`,
            run: () => void onIgnore(choice),
          });
        }
        entries.push({
          id: "ignore-templates",
          label: "Ignore templates…",
          run: () => (galleryOpen = true),
        });
      }
      entries.push(
        {
          id: "history",
          label: "File history",
          run: () =>
            void openPanelPopout("filehistory", repoId, `History: ${path}`, {
              path,
            }),
        },
        {
          id: "open",
          label: "Open diff",
          run: () => openRow(row),
        },
      );
    }
    showMenuAt(event, entries);
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
  /** Row pending a discard confirmation (whole file; checkpointed upstream). */
  let discardCandidate = $state<StatusRow | null>(null);

  function requestDiscard(row: StatusRow, event?: MouseEvent): void {
    event?.stopPropagation();
    if (!canMutate || row.section === "staged" || row.section === "conflicted") {
      return;
    }
    if (moduleByPath.has(row.entry.path)) return;
    discardCandidate = row;
  }

  /** Discard confirmed: the backend snapshots a checkpoint first. */
  async function onDiscardConfirmed(): Promise<void> {
    const row = discardCandidate;
    if (!repoId || !row) return;
    discardCandidate = null;
    await mutate(`Discard ${row.entry.path}`, () =>
      discard(repoId, [{ file: row.entry.path }]),
    );
  }

  function onTreeKeydown(event: KeyboardEvent): void {
    // The Ignore quick-add controls handle their own keys (Enter/Space must
    // not bubble into row open/toggle).
    if ((event.target as HTMLElement).closest(".ignore-wrap")) return;
    const rows = visibleRows;
    if (rows.length === 0) return;
    const current =
      focusKey !== null ? rows.findIndex((r) => r.key === focusKey) : 0;
    const base = current === -1 ? 0 : current;

    let next: number | null = null;
    if (event.key === "ArrowDown") next = (base + 1) % rows.length;
    else if (event.key === "ArrowUp") next = (base - 1 + rows.length) % rows.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = rows.length - 1;

    if (next !== null) {
      event.preventDefault();
      const row = rows[next]!;
      if (event.shiftKey) {
        setRangeSelection(anchorIndex, next);
      } else if (!event.ctrlKey && !event.metaKey) {
        selected = [row.entry.path];
        anchorIndex = next;
      }
      focusRow(row.key);
      return;
    }

    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const row = rows[base];
    if (!row) return;

    if (event.key === " ") {
      event.preventDefault();
      anchorIndex = base;
      toggleSelected(row.entry.path);
    } else if (event.key === "Enter") {
      event.preventDefault();
      anchorIndex = base;
      selected = [row.entry.path];
      openRow(row);
    } else if (event.key === "s" || event.key === "S") {
      if (row.section !== "conflicted") {
        event.preventDefault();
        stageRow(row);
      }
    } else if (event.key === "d" || event.key === "D") {
      if (
        row.section !== "staged" &&
        row.section !== "conflicted" &&
        !moduleByPath.has(row.entry.path)
      ) {
        event.preventDefault();
        requestDiscard(row);
      }
    }
  }

  /** ArrowDown in the filter box drops into the first row. */
  function onFilterKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" && visibleRows.length > 0) {
      event.preventDefault();
      focusRow(visibleRows[0]!.key);
    }
  }

  function toggleSection(id: SectionId): void {
    collapsed[id] = !collapsed[id];
  }

  function toggleModules(): void {
    modulesCollapsed = !modulesCollapsed;
  }
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape") ignoreFor = null;
  }}
/>

<aside aria-label="Source control">
  {#if status}
    <div class="toolbar">
      <span class="filter-wrap">
        <Icon icon={iconSearch} size={13} />
        <input
          class="filter"
          type="text"
          placeholder="Filter changes"
          aria-label="Filter changed files by path"
          bind:value={filterText}
          onkeydown={onFilterKeydown}
        />
        {#if filterText}
          <button
            class="clear-filter"
            type="button"
            aria-label="Clear filter"
            title="Clear filter"
            onclick={() => (filterText = "")}
          >
            <Icon icon={iconClear} size={12} />
          </button>
        {/if}
      </span>
      <IconButton
        icon={iconRefresh}
        label="Refresh changes"
        title="Refresh changes and submodule state"
        dense
        class={mutating || moduleLoading ? "spin" : ""}
        disabled={!repoId || mutating || moduleLoading}
        onclick={refreshAll}
      />
      <IconButton
        icon={iconMore}
        label="More source-control actions"
        title="More actions"
        dense
        onclick={(event) => toolbarMenu(event)}
      />
    </div>

    {#if status.merging || status.rebasing || status.sequencer || status.detached || status.ahead > 0 || status.behind > 0}
      <div class="banners">
        {#if status.merging}<span class="chip chip-state">Merge in progress</span>{/if}
        {#if status.rebasing}<span class="chip chip-state">Rebase in progress</span>{/if}
        {#if status.sequencer}<span class="chip chip-state">Cherry-pick / revert in progress</span>{/if}
        {#if status.detached}<span class="chip chip-state">Detached HEAD</span>{/if}
        {#if status.ahead > 0 || status.behind > 0}
          <span class="chip" title="Commits ahead / behind the upstream">
            ↑{status.ahead} ↓{status.behind}
          </span>
        {/if}
      </div>
    {/if}

    {#if moduleError}
      <button
        class="module-error"
        type="button"
        title={moduleError}
        onclick={() => void reloadModules()}
      >
        Submodule status unavailable — retry
      </button>
    {/if}

    {#if index !== null && index.total === 0 && extraModules.length === 0}
      <div class="empty-state" role="status">
        <span class="empty-icon"><Icon icon={iconCheck} size={26} /></span>
        <p class="empty-title">No changes</p>
        <p class="empty-hint">Your working tree is clean.</p>
      </div>
    {:else if sections.length === 0 && extraModules.length === 0}
      <div class="empty-state" role="status">
        <p class="empty-title">No matching changes</p>
        <p class="empty-hint">Nothing matches “{filterText}”.</p>
      </div>
    {:else}
      <div class="tree" role="tree" aria-label="Changed files" tabindex="-1">
        {#each sections as section (section.id)}
          <div class="section" role="group" aria-labelledby={`section-${section.id}`}>
            <div class="section-header">
              <button
                class="section-toggle"
                type="button"
                id={`section-${section.id}`}
                aria-expanded={!collapsed[section.id]}
                aria-label={`${section.label} (${section.rows.length}), ${collapsed[section.id] ? "expand" : "collapse"}`}
                onclick={() => toggleSection(section.id)}
              >
                <span class="chevron" class:closed={collapsed[section.id]}>▾</span>
                <span class="section-name">{section.label}</span>
                <span class="count">{section.rows.length}</span>
              </button>
              {#if canMutate && section.id !== "conflicted"}
                <span class="section-action">
                  <IconButton
                    icon={section.id === "staged" ? iconUnstage : iconStage}
                    label={`${section.id === "staged" ? "Unstage" : "Stage"} all in ${section.label}`}
                    title={`${section.id === "staged" ? "Unstage" : "Stage"} all files in ${section.label}`}
                    dense
                    onclick={(event) => stageSection(section, event)}
                  />
                </span>
              {/if}
            </div>
            {#if !collapsed[section.id]}
              {#each section.rows as row (row.key)}
                {@const info = moduleByPath.get(row.entry.path)}
                {@const display = info ? submoduleDisplay(info) : null}
                <div
                  class="row file-row"
                  class:selected={isSelected(row.entry.path)}
                  role="treeitem"
                  aria-selected={isSelected(row.entry.path)}
                  aria-label={rowAriaLabel(row, info)}
                  aria-keyshortcuts={row.section === "staged" ? "S" : row.section === "conflicted" ? undefined : "S D"}
                  tabindex={row.key === tabIndexKey ? 0 : -1}
                  data-row-key={row.key}
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
                  <span class={`kind ${kindClass(row.kind)}`} aria-hidden="true">
                    {kindLetter(row.kind)}
                  </span>
                  <Icon
                    icon={info ? iconModule : fileIconForPath(row.entry.path)}
                    size={14}
                  />
                  <span class="path">
                    <span class="base" title={row.entry.path}>{row.base}</span>
                    <span class="meta">
                      {#if row.entry.old_path && (row.kind === "renamed" || row.kind === "copied")}
                        <span class="old" title={`From ${row.entry.old_path}`}>{row.entry.old_path}</span>
                        <span class="arrow" aria-hidden="true">→</span>
                      {/if}
                      {#if row.dir}<span class="dir" title={row.dir}>{row.dir}</span>{/if}
                    </span>
                  </span>
                  {#if info && display && display.tag}
                    <span class={`module-tag ${display.tone}`} title={display.detail}>
                      {display.tag}
                    </span>
                  {/if}
                  <span class="row-actions">
                    {#if row.section === "staged"}
                      <IconButton
                        icon={iconUnstage}
                        label={`Unstage ${row.entry.path}`}
                        title="Unstage this file (S)"
                        dense
                        tabindex={-1}
                        disabled={!canMutate}
                        onclick={(event) => stageRow(row, event)}
                      />
                    {:else if row.section !== "conflicted"}
                      <IconButton
                        icon={iconStage}
                        label={`Stage ${row.entry.path}`}
                        title="Stage this file (S)"
                        dense
                        tabindex={-1}
                        disabled={!canMutate}
                        onclick={(event) => stageRow(row, event)}
                      />
                      {#if !info}
                        <IconButton
                          icon={iconDiscard}
                          label={`Discard changes to ${row.entry.path}`}
                          title="Discard changes to this file (D)"
                          tone="danger"
                          dense
                          tabindex={-1}
                          disabled={!canMutate}
                          onclick={(event) => requestDiscard(row, event)}
                        />
                      {/if}
                    {/if}
                    {#if info}
                      <IconButton
                        icon={iconOpen}
                        label={`Open submodule ${row.entry.path}`}
                        title="Open this submodule as a tab"
                        dense
                        tabindex={-1}
                        disabled={!info.initialized}
                        onclick={(event) => {
                          event.stopPropagation();
                          void openSubmodule(info);
                        }}
                      />
                    {/if}
                    {#if row.section === "untracked" && !info && canMutate}
                      <span class="ignore-wrap">
                        <IconButton
                          icon={iconIgnore}
                          label={`Ignore ${row.entry.path}`}
                          title="Add to .gitignore"
                          dense
                          disabled={mutating}
                          aria-expanded={ignoreFor === row.key}
                          onclick={(event) => {
                            event.stopPropagation();
                            toggleIgnore(row);
                          }}
                        />
                        {#if ignoreFor === row.key}
                          <span class="ignore-menu" role="menu" aria-label="Ignore pattern choices">
                            {#each ignoreChoices as choice (choice.pattern)}
                              <button
                                role="menuitem"
                                type="button"
                                onclick={(event) => {
                                  event.stopPropagation();
                                  void onIgnore(choice);
                                }}
                              >
                                {choice.label}: <code>{choice.pattern}</code>
                              </button>
                            {/each}
                          </span>
                        {/if}
                      </span>
                    {/if}
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/each}

        {#if extraModules.length > 0}
          <div class="section module-section" role="group" aria-labelledby="section-submodules">
            <div class="section-header">
              <button
                class="section-toggle"
                type="button"
                id="section-submodules"
                aria-expanded={!modulesCollapsed}
                aria-label={`Submodules (${extraModules.length}), ${modulesCollapsed ? "expand" : "collapse"}`}
                onclick={toggleModules}
              >
                <span class="chevron" class:closed={modulesCollapsed}>▾</span>
                <span class="section-name">Submodules</span>
                <span class="count">{extraModules.length}</span>
              </button>
              <span class="section-action">
                <IconButton
                  icon={iconRefresh}
                  label="Refresh submodules"
                  title={moduleLoading ? "Refreshing submodules…" : "Refresh submodules"}
                  dense
                  disabled={moduleLoading}
                  onclick={() => void reloadModules()}
                />
              </span>
            </div>
            {#if !modulesCollapsed}
              {#each extraModules as info (info.path)}
                {@const display = submoduleDisplay(info)}
                {@const parts = splitPath(info.path)}
                <div
                  class="row module-row"
                  class:danger={display.tone === "danger"}
                  role="treeitem"
                  aria-selected="false"
                  tabindex={-1}
                  aria-label={`${info.path}, submodule, ${display.detail}`}
                  title={moduleTitle(info)}
                  oncontextmenu={(event) => submoduleMenu(event, info)}
                >
                  <span class="kind k-module" aria-hidden="true">S</span>
                  <Icon icon={iconModule} size={14} />
                  <button
                    class="module-open"
                    type="button"
                    title={moduleTitle(info)}
                    onclick={() => void openSubmodule(info)}
                  >
                    <span class="path">
                      <span class="base" title={info.path}>{parts.base}</span>
                      <span class="meta">
                        {#if parts.dir}<span class="dir" title={parts.dir}>{parts.dir}</span>{/if}
                      </span>
                    </span>
                    {#if display.tag}
                      <span class={`module-tag ${display.tone}`} title={display.detail}>
                        {display.tag}
                      </span>
                    {/if}
                  </button>
                  <span class="row-actions">
                    <IconButton
                      icon={iconUpdate}
                      label={`${info.initialized ? "Update" : "Initialize"} submodule ${info.path}`}
                      title={info.initialized
                        ? "Check out the commit the superproject records"
                        : "Initialize and update this submodule"}
                      dense
                      tabindex={-1}
                      disabled={moduleBusy !== null}
                      onclick={() => void updateSubmodule(info)}
                    />
                    {#if info.initialized}
                      <IconButton
                        icon={iconOpen}
                        label={`Open submodule ${info.path}`}
                        title="Open this submodule as a tab"
                        dense
                        tabindex={-1}
                        onclick={() => void openSubmodule(info)}
                      />
                    {/if}
                    <IconButton
                      icon={iconSync}
                      label={`Sync submodule ${info.path}`}
                      title="Rewrite the URL from .gitmodules into the repo config"
                      dense
                      tabindex={-1}
                      disabled={moduleBusy !== null}
                      onclick={() => void syncSubmodule(info)}
                    />
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  {:else}
    <div class="empty-state" role="status">
      <p class="empty-title">Loading changes…</p>
    </div>
  {/if}
</aside>

<ConfirmDialog
  bind:open={
    () => discardCandidate !== null,
    (v) => {
      if (!v) discardCandidate = null;
    }
  }
  title={discardCandidate ? `Discard changes to ${discardCandidate.entry.path}?` : ""}
  message="Staged and unstaged changes to this file return to HEAD (untracked files are deleted). A checkpoint is created first — restorable from the Undo panel."
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

<style>
  aside {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.125rem;
    padding: 0.3rem 0.4rem;
    flex: none;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
  }

  .filter-wrap {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .filter-wrap > :global(svg) {
    position: absolute;
    left: 0.4rem;
    pointer-events: none;
    opacity: 0.75;
  }

  .filter {
    width: 100%;
    min-width: 0;
    height: 1.6rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.75rem;
    padding: 0 1.5rem;
  }

  .filter::placeholder {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.8;
  }

  .filter:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
    border-color: transparent;
  }

  .clear-filter {
    position: absolute;
    right: 0.2rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.15rem;
    height: 1.15rem;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: transparent;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .clear-filter:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .clear-filter:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  :global(.spin svg) {
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .banners {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    padding: 0.3rem 0.4rem;
    flex: none;
  }

  .chip {
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.0625rem 0.45rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    white-space: nowrap;
  }

  .chip-state {
    background: var(--m3-tertiary-container, var(--m3-surface));
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
    border-color: transparent;
    font-weight: 500;
  }

  .module-error {
    margin: 0 0.4rem 0.25rem;
    padding: 0.2rem 0.4rem;
    border: 1px solid var(--app-deleted, var(--m3-error));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--app-conflicted-bg, var(--m3-error-container));
    color: var(--app-deleted, var(--m3-error));
    font: inherit;
    font-size: 0.6875rem;
    text-align: left;
    cursor: pointer;
  }

  .tree {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-bottom: 0.75rem;
    outline: none;
  }

  .section-header {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    width: 100%;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
  }

  .section-toggle {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.25rem;
    height: 1.65rem;
    border: none;
    background: transparent;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 0 0.4rem;
    cursor: pointer;
    text-align: left;
  }

  .section-toggle:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .chevron {
    display: inline-block;
    width: 0.75rem;
    flex: none;
    font-size: 0.625rem;
    transition: transform 0.1s ease;
  }

  .chevron.closed {
    transform: rotate(-90deg);
  }

  .section-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .count {
    margin-left: auto;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.4rem;
    font-size: 0.625rem;
    font-variant-numeric: tabular-nums;
  }

  .section-action {
    flex: none;
    display: inline-flex;
    padding-right: 0.25rem;
    opacity: 0;
    transition: opacity 0.1s ease;
  }

  .section-header:hover .section-action,
  .section-header:focus-within .section-action {
    opacity: 1;
  }

  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
    min-height: 1.5rem;
    padding: 0 0.35rem 0 0.4rem;
    cursor: pointer;
    content-visibility: auto;
    contain-intrinsic-size: auto 1.5rem;
  }

  .row:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
  }

  .row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .row.selected {
    background: var(--app-active, var(--m3-secondary-container));
    box-shadow: inset 2px 0 var(--m3-primary);
  }

  .row.danger .base {
    color: var(--app-deleted, var(--m3-error));
  }

  .row :global(svg) {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .kind {
    flex: none;
    width: 1.05rem;
    height: 1.05rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 3px;
    font-size: 0.625rem;
    font-weight: 700;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    line-height: 1;
  }

  .k-added {
    color: var(--app-added, var(--m3-primary));
    background: var(--app-added-bg, transparent);
  }

  .k-deleted {
    color: var(--app-deleted, var(--m3-error));
    background: var(--app-deleted-bg, transparent);
  }

  .k-renamed {
    color: var(--app-renamed, var(--m3-tertiary));
    background: var(--app-renamed-bg, transparent);
  }

  .k-modified {
    color: var(--app-modified, var(--m3-secondary));
    background: var(--app-modified-bg, transparent);
  }

  .k-untracked {
    color: var(--app-untracked, var(--m3-on-surface-variant));
    background: var(--app-untracked-bg, transparent);
  }

  .k-conflicted {
    color: var(--app-conflicted, var(--m3-error));
    background: var(--app-conflicted-bg, transparent);
  }

  .k-module {
    color: var(--app-modified, var(--m3-tertiary));
    background: var(--app-modified-bg, transparent);
  }

  .path {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 0.35rem;
    overflow: hidden;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.75rem;
  }

  .base {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
    font-weight: 600;
  }

  .meta {
    flex: 0 1 auto;
    min-width: 0;
    margin-left: auto;
    display: flex;
    align-items: baseline;
    gap: 0.25rem;
  }

  .dir {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: right;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.78;
  }

  .old {
    flex: none;
    max-width: 9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.7;
    text-decoration: line-through;
  }

  .arrow {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 0 0.15rem;
  }

  .module-tag {
    flex: none;
    max-width: 8.5rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.625rem;
    line-height: 1.35;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .module-tag.dirty {
    color: var(--app-modified, var(--m3-tertiary));
  }

  .module-tag.danger {
    color: var(--app-deleted, var(--m3-error));
  }

  .row-actions {
    position: absolute;
    right: 0.2rem;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    gap: 0.0625rem;
    opacity: 0;
    transition: opacity 0.1s ease;
  }

  .row:hover .row-actions,
  .row:focus-within .row-actions,
  .row.selected .row-actions {
    opacity: 1;
  }

  .row-actions::before {
    content: "";
    position: absolute;
    inset: -0.1rem auto -0.1rem -0.9rem;
    width: 0.9rem;
    background: linear-gradient(
      to right,
      transparent,
      var(--m3-surface-container-low, var(--m3-surface))
    );
    pointer-events: none;
  }

  .row:hover .row-actions::before {
    background: linear-gradient(
      to right,
      transparent,
      color-mix(in srgb, var(--m3-on-surface) 5%, var(--m3-surface-container-low, var(--m3-surface)))
    );
  }

  .row.selected .row-actions::before {
    background: linear-gradient(
      to right,
      transparent,
      color-mix(in srgb, var(--m3-primary) 12%, var(--m3-surface-container-low, var(--m3-surface)))
    );
  }

  .module-open {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    padding: 0;
    text-align: left;
    cursor: pointer;
  }

  .module-open:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
    border-radius: 2px;
  }

  .module-row .path {
    flex: 1;
  }

  .empty-state {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.3rem;
    padding: 2rem 1rem;
    text-align: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    user-select: none;
  }

  .empty-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2.75rem;
    height: 2.75rem;
    margin-bottom: 0.2rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    color: var(--app-added, var(--m3-primary));
    opacity: 0.9;
  }

  .empty-title {
    margin: 0;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .empty-hint {
    margin: 0;
    max-width: 15rem;
    font-size: 0.72rem;
    line-height: 1.4;
  }

  /* --- gitignore quick-add (M4): hover-revealed action + pattern menu --- */

  .ignore-wrap {
    position: relative;
    flex: none;
    display: inline-flex;
  }

  .ignore-menu {
    position: absolute;
    right: 0;
    top: 100%;
    z-index: 3;
    display: flex;
    flex-direction: column;
    min-width: 12rem;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-small, 8px);
    box-shadow: var(--m3-elevation-2);
    padding: 0.125rem;
  }

  .ignore-menu button {
    border: none;
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.75rem;
    text-align: left;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
    border-radius: var(--m3-shape-extra-small, 4px);
    white-space: nowrap;
  }

  .ignore-menu button:hover {
    background: var(--m3-surface-container-highest, var(--m3-surface));
  }

  .ignore-menu button:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .ignore-menu code {
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  @media (pointer: coarse) {
    .row-actions {
      position: static;
      transform: none;
      opacity: 1;
    }

    .row-actions::before {
      display: none;
    }
  }
</style>
