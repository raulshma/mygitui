<!--
  RepoPanel (M11, fsck added in M12) — repository health, maintenance,
  archive export, sparse-checkout (cone mode), and LFS in one place.

  Health refreshes on mount + after every maintenance run. Maintenance ops
  run on the backend op queue and toast their output. Archive asks for the
  destination via the save-file picker. Sparse edits apply cone-mode
  patterns (empty list = full checkout). LFS degrades gracefully when the
  binary is absent. "Check objects (fsck)" runs the fsck op and reveals a
  dangling-objects block (hidden until the first run).
-->
<script lang="ts">
  import {
    archiveSpec,
    hooksList,
    lfsLock,
    lfsLocks,
    lfsRun,
    lfsStatus,
    lfsUnlock,
    maintenanceRun,
    pickSaveFile,
    readFile,
    repoBundle,
    repoHealth,
    repoRefs,
    sparseApply,
    sparseInfo,
    statFile,
    writeFile,
  } from "$lib/ipc/client";
  import type { HookInfo, LfsLock, LfsStatus, RepoHealth, SparseInfo } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { formatRelativeTime } from "$lib/git/format";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { copyText } from "$lib/clipboard";
  import { isTauri } from "$lib/entry/dragdrop";
  import { openPath } from "@tauri-apps/plugin-opener";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { hookPathFor, hookRowsFor, hooksSummary } from "./hooksModel";
  import {
    parseGitAttributes,
    ruleIsLfs,
    serializeGitAttributes,
    setLfsRule,
    setLockableRule,
    upsertRule,
    type AttributesLine,
  } from "./attributesModel";

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    onMutated?: () => void;
  } = $props();

  let health = $state<RepoHealth | null>(null);
  let error = $state<string | null>(null);
  let running = $state<string | null>(null);
  /** M12: local record of when the last fsck ran (no backend timestamp in
   *  RepoHealth — the FE timestamp covers this session). */
  let fsckRanAt = $state<number | null>(null);

  // Archive form
  let archiveSpecText = $state("HEAD");
  let archiveFormat = $state<"zip" | "tar" | "tar.gz">("zip");
  let archiving = $state(false);

  // Bundle export (Phase 5 `repo_bundle`): all-refs checkbox + optional
  // ref picker; destination via the same save-file picker as archive.
  let bundleOpen = $state(false);
  let bundleAll = $state(true);
  /** Full ref list ([name, sha] pairs), fetched once per dialog open. */
  let bundleRefList = $state<[string, string][]>([]);
  let bundleLoadingRefs = $state(false);
  /** Refs ticked in the picker (used when `bundleAll` is off). */
  let bundleSelected = $state<string[]>([]);
  let bundling = $state(false);

  // Sparse
  let sparse = $state<SparseInfo | null>(null);
  let sparseText = $state("");
  let sparseBusy = $state(false);
  /** The sparse read failed (muted error line + Retry in its section). */
  let sparseError = $state<string | null>(null);

  // LFS
  let lfs = $state<LfsStatus | null>(null);
  let lfsBusy = $state(false);
  /** The LFS read failed (muted error line + Retry in its section). */
  let lfsError = $state<string | null>(null);

  // LFS file locks (`git lfs locks`; Phase 5). Best-effort like the rest of
  // the LFS section — the sub-section stays hidden when the list is null.
  let lfsLockList = $state<LfsLock[] | null>(null);
  let lfsLockError = $state<string | null>(null);
  /** Path to lock: free text with the tracked patterns as suggestions. */
  let lfsLockPathInput = $state("");
  let lfsLockBusy = $state(false);

  // Hooks (`.git/hooks` has no backend read/write; the section lists
  // kind/present/executable and derives each hook file's path for
  // open-in-editor / copy — see hooksModel).
  let hooks = $state<HookInfo[] | null>(null);
  let hooksError = $state<string | null>(null);

  // .gitattributes manager (batch B item 40): the root file is a normal
  // workdir file, so repo_read_file / repo_write_file reach it. Edits are
  // read-modify-write with a stale check (re-read before write; a file
  // changed underneath aborts and reloads instead of clobbering).
  let attrBaseline = $state("");
  let attrDraft = $state("");
  let attrMtime = $state<number | null>(null);
  let attrBusy = $state(false);
  let attrError = $state<string | null>(null);
  // Add-entry form state.
  let newPattern = $state("");
  let newLfs = $state(false);
  let newText = $state(false);
  let newEolLf = $state(false);
  let newEolCrlf = $state(false);
  let newBinary = $state(false);

  /** The repo root this panel's tab belongs to (null for unknown ids). */
  const repoRoot = $derived(tabStore.tabs.find((tab) => tab.id === repoId)?.root ?? null);

  const hookRows = $derived(hookRowsFor(hooks ?? [], repoRoot));

  /** Parsed view of the draft (rules + preserved comments/blanks). */
  const attrLines = $derived.by<AttributesLine[]>(() => parseGitAttributes(attrDraft));

  /** Unsaved edits pending (the Save button's gate). */
  const attrDirty = $derived(attrDraft !== attrBaseline);

  $effect(() => {
    void repoId;
    health = null;
    sparse = null;
    sparseText = "";
    lfs = null;
    lfsLockList = [];
    lfsLockError = null;
    lfsLockPathInput = "";
    bundleOpen = false;
    sparseError = null;
    lfsError = null;
    hooks = null;
    hooksError = null;
    attrBaseline = "";
    attrDraft = "";
    attrMtime = null;
    attrError = null;
    void reload();
  });

  async function reload(): Promise<void> {
    // Stale-response guard: the panel is keep-alive across repo switches —
    // a slow health/sparse/LFS response from repo A must never land after
    // the switch to repo B.
    const id = repoId;
    error = null;
    try {
      const fresh = await repoHealth(id);
      if (id !== repoId) return;
      health = fresh;
    } catch (err) {
      if (id !== repoId) return;
      error = err instanceof Error ? err.message : String(err);
    }
    // Best-effort side reads: they refine the sparse + LFS sections and
    // degrade to a muted error line (with Retry) when they fail.
    sparseInfo(id)
      .then((info) => {
        if (id !== repoId) return;
        sparseError = null;
        sparse = info;
        sparseText = info.patterns.join("\n");
      })
      .catch((err) => {
        if (id !== repoId) return;
        sparseError = err instanceof Error ? err.message : String(err);
      });
    lfsStatus(id)
      .then((status) => {
        if (id !== repoId) return;
        lfsError = null;
        lfs = status;
        if (status.installed) void loadLfsLocks(id);
      })
      .catch((err) => {
        if (id !== repoId) return;
        lfsError = err instanceof Error ? err.message : String(err);
      });
    hooksList(id)
      .then((list) => {
        if (id !== repoId) return;
        hooksError = null;
        hooks = list;
      })
      .catch((err) => {
        if (id !== repoId) return;
        hooksError = err instanceof Error ? err.message : String(err);
      });
    loadAttributes(id);
  }

  /** Reads .gitattributes (a missing file reads as empty) + its mtime. */
  async function loadAttributes(id: string): Promise<void> {
    try {
      const bytes = await readFile(id, ".gitattributes");
      if (id !== repoId) return;
      const text = new TextDecoder().decode(bytes);
      attrBaseline = text;
      attrDraft = text;
      attrError = null;
      const meta = await statFile(id, ".gitattributes").catch(() => null);
      if (id !== repoId || meta === null) return;
      attrMtime = meta.mtime_ms;
    } catch (err) {
      if (id !== repoId) return;
      attrError = err instanceof Error ? err.message : String(err);
    }
  }

  /**
   * Applies `edit` to the parsed draft and saves: read-modify-write with a
   * stale check — the file is re-read before the write, and if it changed
   * underneath (another process/git) the write is dropped with an error
   * toast and the fresh content loaded instead (nothing partial applied).
   */
  async function saveAttributes(edit: (lines: AttributesLine[]) => AttributesLine[]): Promise<void> {
    if (attrBusy) return;
    attrBusy = true;
    try {
      const fresh = new TextDecoder().decode(await readFile(repoId, ".gitattributes"));
      if (fresh !== attrBaseline) {
        attrBaseline = fresh;
        attrDraft = fresh;
        const meta = await statFile(repoId, ".gitattributes").catch(() => null);
        if (meta !== null) attrMtime = meta.mtime_ms;
        toast(
          ".gitattributes changed on disk — the latest version is loaded; re-apply your edit.",
          { kind: "error" },
        );
        return;
      }
      const next = serializeGitAttributes(edit(parseGitAttributes(attrBaseline)));
      await writeFile(repoId, ".gitattributes", new TextEncoder().encode(next), attrMtime);
      attrBaseline = next;
      attrDraft = next;
      const meta = await statFile(repoId, ".gitattributes").catch(() => null);
      if (meta !== null) attrMtime = meta.mtime_ms;
      toast(".gitattributes updated", { kind: "success" });
    } catch (err) {
      toast(
        `.gitattributes update failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      attrBusy = false;
    }
  }

  /** The LFS toggle on one rule's pattern. */
  function toggleLfs(pattern: string, enabled: boolean): void {
    void saveAttributes((lines) => setLfsRule(lines, pattern, enabled));
  }

  /** The lockable toggle on one rule's pattern. */
  function toggleLockable(pattern: string, enabled: boolean): void {
    void saveAttributes((lines) => setLockableRule(lines, pattern, enabled));
  }

  /** Adds the form's entry (pattern + checked attribute tokens). */
  function addAttributeEntry(): void {
    const pattern = newPattern.trim();
    if (pattern === "") return;
    const attrs: string[] = [];
    if (newLfs) attrs.push("filter=lfs");
    if (newText) attrs.push("text");
    if (newEolLf) attrs.push("eol=lf");
    if (newEolCrlf) attrs.push("eol=crlf");
    if (newBinary) attrs.push("binary");
    if (attrs.length === 0) {
      toast("Check at least one attribute for the new pattern", { kind: "error" });
      return;
    }
    void saveAttributes((lines) => upsertRule(lines, pattern, attrs));
    newPattern = "";
    newLfs = newText = newEolLf = newEolCrlf = newBinary = false;
  }

  function fmtSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }

  async function run(op: string, label: string): Promise<void> {
    if (running) return;
    running = op;
    try {
      const output = await maintenanceRun(repoId, op);
      if (op === "fsck") fsckRanAt = Date.now();
      toast(`${label} done${output ? `: ${output}` : ""}`, { kind: "success" });
      await reload();
      onMutated?.();
    } catch (err) {
      toast(
        `${label} failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      running = null;
    }
  }

  /** Copies a dangling-object sha to the clipboard (M12). */
  async function copySha(sha: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(sha);
      toast("SHA copied to clipboard", { kind: "success" });
    } catch {
      toast("Could not copy the SHA", { kind: "error" });
    }
  }

  async function onArchive(): Promise<void> {
    if (archiving) return;
    const ext = archiveFormat === "zip" ? "zip" : archiveFormat === "tar" ? "tar" : "tar.gz";
    const destination = await pickSaveFile("Export archive", `archive.${ext}`, [
      archiveFormat === "zip"
        ? { name: "Zip archive", extensions: ["zip"] }
        : { name: "Tar archive", extensions: ["tar", "gz"] },
    ]);
    if (!destination) return;
    archiving = true;
    try {
      await archiveSpec(repoId, archiveSpecText.trim(), archiveFormat, destination);
      toast(`Archive written to ${destination}`, { kind: "success" });
    } catch (err) {
      toast(
        `Archive failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      archiving = false;
    }
  }

  // -- Bundle export ----------------------------------------------------------

  /** Opens the dialog and (re)fetches the ref list for the picker. */
  function openBundle(): void {
    bundleOpen = true;
    bundleAll = true;
    bundleSelected = [];
    bundleRefList = [];
    bundleLoadingRefs = true;
    repoRefs(repoId)
      .then((refs) => {
        bundleRefList = refs;
        bundleLoadingRefs = false;
      })
      .catch(() => {
        // The picker degrades to empty — all-refs export still works.
        bundleRefList = [];
        bundleLoadingRefs = false;
      });
  }

  /** `<repo-name>-2026-11-18.bundle` — the save-dialog default name. */
  const bundleDefaultName = $derived.by(() => {
    const name = tabStore.tabs.find((tab) => tab.id === repoId)?.name ?? "repo";
    const day = new Date().toISOString().slice(0, 10);
    return `${name}-${day}.bundle`;
  });

  async function onBundleExport(): Promise<void> {
    if (bundling) return;
    const destination = await pickSaveFile("Export bundle", bundleDefaultName, [
      { name: "Git bundle", extensions: ["bundle"] },
    ]);
    if (!destination) return;
    bundling = true;
    try {
      await repoBundle(
        repoId,
        destination,
        bundleAll ? [] : bundleSelected,
        bundleAll,
      );
      bundleOpen = false;
      toast(`Bundle written to ${destination}`, { kind: "success" });
    } catch (err) {
      toast(
        `Bundle export failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      bundling = false;
    }
  }

  async function onSparseApply(add: boolean): Promise<void> {
    if (sparseBusy) return;
    const patterns = sparseText
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "");
    sparseBusy = true;
    try {
      await sparseApply(repoId, patterns, add);
      toast(
        patterns.length === 0
          ? "Sparse checkout disabled (full checkout)"
          : `Sparse checkout updated (${patterns.length} pattern${patterns.length === 1 ? "" : "s"})`,
        { kind: "success" },
      );
      await reload();
      onMutated?.();
    } catch (err) {
      toast(
        `Sparse update failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      sparseBusy = false;
    }
  }

  async function onLfs(subcommand: string): Promise<void> {
    if (lfsBusy) return;
    lfsBusy = true;
    try {
      await lfsRun(repoId, subcommand);
      toast(`git lfs ${subcommand} done`, { kind: "success" });
      await reload();
    } catch (err) {
      toast(
        `git lfs ${subcommand} failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      lfsBusy = false;
    }
  }

  /** Best-effort lock list (needs git-lfs + a reachable LFS server). */
  async function loadLfsLocks(id: string): Promise<void> {
    try {
      const locks = await lfsLocks(id);
      if (id !== repoId) return;
      lfsLockError = null;
      lfsLockList = locks;
    } catch (err) {
      if (id !== repoId) return;
      lfsLockError = err instanceof Error ? err.message : String(err);
    }
  }

  async function onLfsLock(): Promise<void> {
    const path = lfsLockPathInput.trim();
    if (!path || lfsLockBusy) return;
    lfsLockBusy = true;
    try {
      await lfsLock(repoId, path);
      toast(`Locked ${path}`, { kind: "success" });
      lfsLockPathInput = "";
      await loadLfsLocks(repoId);
    } catch (err) {
      toast(
        `Lock failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      lfsLockBusy = false;
    }
  }

  async function onLfsUnlock(path: string): Promise<void> {
    if (lfsLockBusy) return;
    lfsLockBusy = true;
    try {
      await lfsUnlock(repoId, path);
      toast(`Unlocked ${path}`, { kind: "success" });
      await loadLfsLocks(repoId);
    } catch (err) {
      toast(
        `Unlock failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      lfsLockBusy = false;
    }
  }

  // -- Hooks -------------------------------------------------------------------

  /** Opens a hook file in the OS default handler (opener plugin). */
  async function openHook(path: string): Promise<void> {
    if (!isTauri()) {
      toast("Opening files needs the desktop app", { kind: "info" });
      return;
    }
    try {
      await openPath(path);
    } catch (err) {
      toast(
        `Could not open ${path}: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  function copyHookPath(path: string): void {
    void copyText(path, "hook path");
  }
</script>

<aside
  class="repo-panel flex min-h-0 flex-col overflow-y-auto bg-background pb-3 text-ui-base text-foreground"
  aria-label="Repository health and maintenance"
>
  <div class="toolbar flex shrink-0 items-center gap-2 px-2 py-1.5">
    <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" onclick={() => void reload()} disabled={running !== null}>
      Refresh
    </Button>
    {#if running}<span class="busy text-ui-meta text-muted-foreground" role="status">{running} running…</span>{/if}
  </div>

  {#if error}
    <p class="state error mb-1.5 text-xs text-destructive" role="alert">{error}</p>
  {:else if health}
    <section class="group border-t border-border px-2 py-1.5" aria-label="Size and objects">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Health</h2>
      <dl class="facts mb-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-[2px] text-xs">
        <dt class="text-muted-foreground">Git directory</dt>
        <dd class="m-0 font-mono">{fmtSize(health.git_size_bytes)}</dd>
        <dt class="text-muted-foreground">Worktree</dt>
        <dd class="m-0 font-mono">{fmtSize(health.worktree_size_bytes)}</dd>
        <dt class="text-muted-foreground">Loose objects</dt>
        <dd class="m-0 font-mono">{health.loose_objects}</dd>
        <dt class="text-muted-foreground">Pack files</dt>
        <dd class="m-0 font-mono">{health.pack_files}</dd>
        <dt class="text-muted-foreground">Commit-graph</dt>
        <dd class="m-0 font-mono">
          {#if health.has_commit_graph}
            yes ({fmtSize(health.commit_graph_bytes)})
          {:else}
            <span class="warn text-amber-600 dark:text-amber-400">none — write one for faster history walks</span>
          {/if}
        </dd>
        <dt class="text-muted-foreground">Packed refs</dt>
        <dd class="m-0 font-mono">{health.packed_refs ? "yes" : "no"}</dd>
      </dl>
      <div class="ops flex flex-wrap gap-1.5">
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} onclick={() => void run("count_objects", "Count objects")}>
          Count objects
        </Button>
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="Write the commit-graph (faster log on big repos)" onclick={() => void run("commit_graph", "Commit-graph write")}>
          Write commit-graph
        </Button>
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} onclick={() => void run("pack_refs", "Pack refs")}>
          Pack refs
        </Button>
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="git prune — drops unreachable loose objects" onclick={() => void run("prune", "Prune")}>
          Prune
        </Button>
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="git gc --auto — pack, repack, and tidy" onclick={() => void run("gc", "GC")}>
          GC
        </Button>
        <!-- M12: git fsck — object-store integrity + dangling-object census.
             The result block below stays hidden until the first run
             (fsck_dangling is null in a fresh RepoHealth). -->
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="git fsck — check object connectivity and list dangling objects" onclick={() => void run("fsck", "Check objects")}>
          Check objects (fsck)
        </Button>
      </div>

      {#if health.fsck_dangling !== null}
        <div class="fsck mt-1.5" aria-label="Dangling objects">
          <p class="state mb-1.5 text-xs text-muted-foreground">
            Dangling objects: {health.fsck_dangling}{#if fsckRanAt}
              (checked {formatRelativeTime(Math.floor(fsckRanAt / 1000))}){/if}
          </p>
          {#if health.fsck_samples.length > 0}
            <div class="fsck-samples flex flex-wrap gap-1">
              {#each health.fsck_samples as sha (sha)}
                <Hint label={`Copy ${sha}`}>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      class="h-auto rounded px-[5px] py-px font-mono text-ui-meta font-normal text-muted-foreground"
                      variant="outline"
                      type="button"
                      onclick={() => void copySha(sha)}
                    >
                      {sha.slice(0, 12)}
                    </Button>
                  {/snippet}
                </Hint>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Git hooks">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Hooks</h2>
      {#if hooksError}
        <p class="state mb-1.5 flex flex-wrap items-center gap-2 text-xs text-destructive" role="alert">
          {hooksError}
          <Button
            class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
            variant="outline"
            type="button"
            onclick={() => void reload()}
          >
            Retry
          </Button>
        </p>
      {:else if hooks === null}
        <p class="state mb-1.5 text-xs text-muted-foreground">Loading hooks…</p>
      {:else if hookRows.length === 0}
        <p class="state mb-1.5 text-xs text-muted-foreground">No hooks reported.</p>
      {:else}
        <p class="state mb-1.5 text-xs text-muted-foreground">{hooksSummary(hooks)}</p>
        <ul class="m-0 flex list-none flex-col gap-0.5 p-0" role="list" aria-label="Git hooks">
          {#each hookRows as row (row.kind)}
            <li class="hook-row flex min-w-0 items-center justify-between gap-2 rounded border border-transparent px-1 py-0.5 hover:border-border">
              <span class="flex min-w-0 items-center gap-1.5">
                <span class="min-w-0 truncate font-mono text-ui-meta text-foreground" title={row.kind}>{row.kind}</span>
                {#if row.present}
                  <span class="shrink-0 rounded-full bg-muted px-1.5 py-px text-ui-micro leading-normal text-muted-foreground">present</span>
                {/if}
                {#if row.executable}
                  <span class="shrink-0 rounded-full bg-primary/10 px-1.5 py-px text-ui-micro leading-normal text-primary-ink">executable</span>
                {/if}
              </span>
              <span class="flex flex-none items-center gap-1">
                <Hint label={row.canOpen ? `Open ${row.path} in the default editor` : "The hook file does not exist yet"}>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
                      variant="outline"
                      type="button"
                      disabled={!row.canOpen}
                      onclick={() => void openHook(row.path)}
                    >
                      Open in editor
                    </Button>
                  {/snippet}
                </Hint>
                <Hint label="Copy the hook file's path">
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
                      variant="outline"
                      type="button"
                      disabled={!row.canCopy}
                      onclick={() => copyHookPath(row.path)}
                    >
                      Copy path
                    </Button>
                  {/snippet}
                </Hint>
              </span>
            </li>
          {/each}
        </ul>
        <p class="state mt-1 mb-0 text-ui-meta text-muted-foreground">
          Hook files live under <code class="font-mono">.git/hooks</code> in the repository — edit them with “Open in editor”.
        </p>
      {/if}
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Archive export">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Archive</h2>
      <div class="row flex items-center gap-1.5">
        <input
          class="text min-w-20 flex-1 rounded border border-border bg-background px-[7px] py-px text-xs text-foreground"
          bind:value={archiveSpecText}
          placeholder="ref or sha"
          aria-label="Archive ref"
        />
        <select
          class="rounded border border-border bg-background px-1 py-px text-ui-meta text-foreground"
          bind:value={archiveFormat}
          aria-label="Archive format"
        >
          <option value="zip">zip</option>
          <option value="tar">tar</option>
          <option value="tar.gz">tar.gz</option>
        </select>
        <Button
          class="h-auto rounded-full px-3 py-px text-ui-meta"
          type="button"
          disabled={archiving || !archiveSpecText.trim()}
          onclick={() => void onArchive()}
        >
          {archiving ? "Exporting…" : "Export…"}
        </Button>
      </div>
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Bundle export">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Bundle</h2>
      <div class="row flex items-center gap-1.5">
        <p class="state m-0 min-w-0 flex-1 text-xs text-muted-foreground">
          One-file pack of the selected refs (`git bundle create`) — clone or fetch from it later.
        </p>
        <Button
          class="h-auto rounded-full px-3 py-px text-ui-meta"
          type="button"
          disabled={bundling}
          onclick={openBundle}
        >
          {bundling ? "Exporting…" : "Export…"}
        </Button>
      </div>
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Sparse checkout">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Sparse checkout (cone mode)</h2>
      {#if sparseError}
        <p class="state mb-1.5 flex flex-wrap items-center gap-2 text-xs text-destructive" role="alert">
          {sparseError}
          <Button
            class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
            variant="outline"
            type="button"
            onclick={() => void reload()}
          >
            Retry
          </Button>
        </p>
      {/if}
      {#if sparse}
        <p class="state mb-1.5 text-xs text-muted-foreground">
          {sparse.enabled
            ? `${sparse.patterns.length} pattern${sparse.patterns.length === 1 ? "" : "s"}${sparse.cone ? "" : " (non-cone)"}`
            : "Full checkout (sparse disabled)."}
        </p>
      {/if}
      <textarea
        class="patterns mb-1.5 w-full resize-y rounded border border-border bg-background px-[7px] py-1 font-mono text-xs text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
        rows="4"
        bind:value={sparseText}
        placeholder="one directory pattern per line, e.g.&#10;src/&#10;docs/"
        aria-label="Sparse checkout patterns"
      ></textarea>
      <div class="ops flex flex-wrap gap-1.5">
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={sparseBusy} onclick={() => void onSparseApply(false)}>
          Apply
        </Button>
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={sparseBusy} onclick={() => void onSparseApply(true)}>
          Add
        </Button>
        <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal" variant="destructive" type="button" disabled={sparseBusy} title="Back to a full checkout" onclick={() => { sparseText = ""; void onSparseApply(false); }}>
          Disable
        </Button>
      </div>
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Git LFS">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Git LFS</h2>
      {#if lfsError}
        <p class="state mb-1.5 flex flex-wrap items-center gap-2 text-xs text-destructive" role="alert">
          {lfsError}
          <Button
            class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
            variant="outline"
            type="button"
            onclick={() => void reload()}
          >
            Retry
          </Button>
        </p>
      {/if}
      {#if lfs?.installed}
        <p class="state mb-1.5 text-xs text-muted-foreground">{lfs.version}</p>
        {#if lfs.tracked_patterns.length > 0}
          <p class="state mb-1.5 text-xs text-muted-foreground">Tracking: {lfs.tracked_patterns.join(", ")}</p>
        {:else}
          <p class="state mb-1.5 text-xs text-muted-foreground">No filter=lfs patterns yet — add them in the .gitattributes section below.</p>
        {/if}
        <div class="ops flex flex-wrap gap-1.5">
          <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={lfsBusy} onclick={() => void onLfs("pull")}>Pull</Button>
          <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={lfsBusy} onclick={() => void onLfs("push")}>Push</Button>
          <Button class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" type="button" disabled={lfsBusy} onclick={() => void onLfs("fetch")}>Fetch</Button>
        </div>
        <!-- Locks sub-section (Phase 5 `git lfs locks`): list + lock/unlock.
             Hidden until the first list read lands (best-effort). -->
        {#if lfsLockError}
          <p class="state mb-1.5 mt-2 text-ui-meta text-muted-foreground">
            Locks unavailable: {lfsLockError}
          </p>
        {/if}
        {#if lfsLockList}
          <div class="lfs-locks mt-2" aria-label="LFS file locks">
            <p class="state mb-1 text-ui-micro font-semibold uppercase tracking-[0.05em] text-muted-foreground">
              Locks {lfsLockList.length > 0 ? `(${lfsLockList.length})` : ""}
            </p>
            {#if lfsLockList.length === 0}
              <p class="state mb-1 text-xs text-muted-foreground">No locks held on the LFS server.</p>
            {:else}
              <ul class="m-0 mb-1 flex list-none flex-col gap-0.5 p-0" role="list" aria-label="LFS locks">
                {#each lfsLockList as lock (lock.id)}
                  <li class="flex min-w-0 items-center gap-2 rounded border border-transparent px-1 py-0.5 hover:border-border">
                    <span class="min-w-0 flex-1 truncate font-mono text-ui-meta text-foreground" title={lock.path}>{lock.path}</span>
                    <span class="shrink-0 text-ui-meta text-muted-foreground">{lock.owner || "unknown owner"}</span>
                    <Button
                      class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
                      variant="outline"
                      type="button"
                      disabled={lfsLockBusy}
                      title="Release this lock (git lfs unlock)"
                      onclick={() => void onLfsUnlock(lock.path)}
                    >
                      Unlock
                    </Button>
                  </li>
                {/each}
              </ul>
            {/if}
            <div class="lock-form flex items-center gap-1.5">
              <input
                class="min-w-20 flex-1 rounded border border-border bg-background px-[7px] py-px font-mono text-xs text-foreground"
                list="lfs-tracked-patterns"
                bind:value={lfsLockPathInput}
                placeholder="path to lock, e.g. assets/model.psd"
                aria-label="LFS path to lock"
              />
              <datalist id="lfs-tracked-patterns">
                {#each lfs.tracked_patterns as pattern (pattern)}
                  <option value={pattern}></option>
                {/each}
              </datalist>
              <Button
                class="h-auto rounded-full px-3 py-px text-ui-meta"
                type="button"
                disabled={lfsLockBusy || lfsLockPathInput.trim() === ""}
                onclick={() => void onLfsLock()}
              >
                Lock
              </Button>
            </div>
          </div>
        {/if}
      {:else if lfs}
        <p class="state warn mb-1.5 text-xs text-amber-600 dark:text-amber-400">
          git lfs is not installed — install it from <code class="font-mono">git-lfs.com</code>, then re-run setup here.
        </p>
      {:else}
        <p class="state mb-1.5 text-xs text-muted-foreground">Detecting…</p>
      {/if}
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label=".gitattributes manager">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">.gitattributes</h2>
      {#if attrError}
        <p class="state mb-1.5 flex flex-wrap items-center gap-2 text-xs text-destructive" role="alert">
          {attrError}
          <Button
            class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
            variant="outline"
            type="button"
            onclick={() => void reload()}
          >
            Retry
          </Button>
        </p>
      {:else}
        {#if attrLines.length === 0 || attrLines.every((line) => line.kind === "raw" && line.text.trim() === "")}
          <p class="state mb-1.5 text-xs text-muted-foreground">
            No .gitattributes in the repository root yet — the first entry below creates it.
          </p>
        {:else}
          <ul class="m-0 mb-1.5 flex list-none flex-col gap-0.5 p-0" role="list" aria-label=".gitattributes entries">
            {#each attrLines as line, i (i)}
              {#if line.kind === "raw"}
                {#if line.text.trim() !== ""}
                  <li class="text-ui-meta text-muted-foreground" aria-label="Comment">{line.text.trim()}</li>
                {/if}
              {:else}
                <li class="flex min-w-0 flex-wrap items-center justify-between gap-x-2 gap-y-0.5 rounded border border-transparent px-1 py-0.5 hover:border-border">
                  <span class="flex min-w-0 flex-wrap items-center gap-1.5">
                    <span class="min-w-0 truncate font-mono text-ui-meta text-foreground" title={line.pattern}>{line.pattern}</span>
                    {#each line.attrs as attr (attr)}
                      <span class="shrink-0 rounded-full bg-muted px-1.5 py-px font-mono text-ui-micro leading-normal text-muted-foreground">{attr}</span>
                    {/each}
                  </span>
                  <span class="flex flex-none items-center gap-1">
                    <Button
                      class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
                      variant="outline"
                      type="button"
                      disabled={attrBusy}
                      title={ruleIsLfs(line) ? "Remove the filter=lfs pattern (-filter=lfs)" : "Track this pattern with git lfs (filter=lfs)"}
                      onclick={() => toggleLfs(line.pattern, !ruleIsLfs(line))}
                    >
                      {ruleIsLfs(line) ? "Untrack LFS" : "Track LFS"}
                    </Button>
                    <Button
                      class="h-auto rounded px-2 py-px text-ui-meta font-normal text-muted-foreground"
                      variant="outline"
                      type="button"
                      disabled={attrBusy}
                      title="Toggle the lockable attribute (git lfs lock/unlock needs it)"
                      onclick={() => {
                        const mention = line.attrs.includes("lockable");
                        toggleLockable(line.pattern, !mention);
                      }}
                    >
                      {line.attrs.includes("lockable") ? "Unlockable" : "Lockable"}
                    </Button>
                  </span>
                </li>
              {/if}
            {/each}
          </ul>
        {/if}
        <div class="add flex flex-col gap-1">
          <div class="flex items-center gap-1.5">
            <input
              class="min-w-20 flex-1 rounded border border-border bg-background px-[7px] py-px font-mono text-xs text-foreground"
              bind:value={newPattern}
              placeholder="pattern, e.g. *.psd or assets/"
              aria-label="New .gitattributes pattern"
            />
            <Button
              class="h-auto rounded-full px-3 py-px text-ui-meta"
              type="button"
              disabled={attrBusy || newPattern.trim() === ""}
              onclick={addAttributeEntry}
            >
              Add entry
            </Button>
          </div>
          <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-ui-meta text-muted-foreground">
            <label class="flex cursor-pointer items-center gap-1">
              <input type="checkbox" bind:checked={newLfs} /> <span class="font-mono">filter=lfs</span>
            </label>
            <label class="flex cursor-pointer items-center gap-1">
              <input type="checkbox" bind:checked={newText} /> <span class="font-mono">text</span>
            </label>
            <label class="flex cursor-pointer items-center gap-1">
              <input type="checkbox" bind:checked={newEolLf} /> <span class="font-mono">eol=lf</span>
            </label>
            <label class="flex cursor-pointer items-center gap-1">
              <input type="checkbox" bind:checked={newEolCrlf} /> <span class="font-mono">eol=crlf</span>
            </label>
            <label class="flex cursor-pointer items-center gap-1">
              <input type="checkbox" bind:checked={newBinary} /> <span class="font-mono">binary</span>
            </label>
          </div>
          {#if attrDirty}
            <p class="m-0 text-ui-meta text-muted-foreground">
              Edits apply per entry with a re-read-before-write guard (a file changed underneath reloads instead of saving).
            </p>
          {/if}
        </div>
      {/if}
    </section>
  {:else}
    <p class="state mb-1.5 text-xs text-muted-foreground">Loading health…</p>
  {/if}
</aside>

{#if bundleOpen}
  <!-- Bundle export dialog: all-refs checkbox + optional ref picker; the
       save-file flow reuses pickSaveFile like the archive export. -->
  <DialogShell
    bind:open={
      () => bundleOpen,
      (v) => {
        if (!v) bundleOpen = false;
      }
    }
    labelledBy="bundle-title"
    width="26rem"
    onsubmit={() => void onBundleExport()}
  >
    {#snippet children()}
      <h2 id="bundle-title" class="sdlg-title">Export bundle</h2>
      <label class="mb-2 flex cursor-pointer items-center gap-1.5 text-xs">
        <input type="checkbox" class="m-0 accent-primary" data-autofocus bind:checked={bundleAll} />
        <span>all refs (--all)</span>
      </label>
      {#if !bundleAll}
        <div class="mb-2 max-h-48 overflow-y-auto rounded border border-border bg-background px-2 py-1">
          {#if bundleLoadingRefs}
            <p class="m-0 py-1 text-xs text-muted-foreground">Loading refs…</p>
          {:else if bundleRefList.length === 0}
            <p class="m-0 py-1 text-xs text-muted-foreground">No refs found.</p>
          {:else}
            {#each bundleRefList as [ref] (ref)}
              <label class="flex cursor-pointer items-center gap-1.5 py-0.5 text-xs">
                <input
                  type="checkbox"
                  class="m-0 accent-primary"
                  checked={bundleSelected.includes(ref)}
                  onchange={(e) => {
                    const on = e.currentTarget.checked;
                    bundleSelected = on
                      ? [...bundleSelected, ref]
                      : bundleSelected.filter((r) => r !== ref);
                  }}
                />
                <span class="min-w-0 truncate font-mono">{ref}</span>
              </label>
            {/each}
          {/if}
        </div>
      {/if}
      <p class="sdlg-message mb-0">
        Saved via the file picker as a single <code class="font-mono">.bundle</code> file.
      </p>
    {/snippet}
    {#snippet actions()}
      <button class="sdlg-secondary" type="button" onclick={() => (bundleOpen = false)}>
        Cancel
      </button>
      <button
        class="sdlg-primary"
        type="submit"
        disabled={bundling || (!bundleAll && bundleSelected.length === 0)}
      >
        {bundling ? "Exporting…" : "Export"}
      </button>
    {/snippet}
  </DialogShell>
{/if}
