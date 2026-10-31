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
    lfsRun,
    lfsStatus,
    maintenanceRun,
    pickSaveFile,
    repoHealth,
    sparseApply,
    sparseInfo,
  } from "$lib/ipc/client";
  import type { LfsStatus, RepoHealth, SparseInfo } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { formatRelativeTime } from "$lib/git/format";
  import { Button } from "$lib/components/ui/button/index.js";

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

  // Sparse
  let sparse = $state<SparseInfo | null>(null);
  let sparseText = $state("");
  let sparseBusy = $state(false);

  // LFS
  let lfs = $state<LfsStatus | null>(null);
  let lfsBusy = $state(false);

  $effect(() => {
    void repoId;
    health = null;
    sparse = null;
    lfs = null;
    void reload();
  });

  async function reload(): Promise<void> {
    error = null;
    try {
      health = await repoHealth(repoId);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
    sparseInfo(repoId)
      .then((info) => {
        sparse = info;
        sparseText = info.patterns.join("\n");
      })
      .catch(() => {});
    lfsStatus(repoId)
      .then((status) => (lfs = status))
      .catch(() => {});
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
</script>

<aside
  class="repo-panel flex min-h-0 flex-col overflow-y-auto bg-background pb-3 text-[13px] text-foreground"
  aria-label="Repository health and maintenance"
>
  <div class="toolbar flex shrink-0 items-center gap-2 px-2 py-1.5">
    <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" onclick={() => void reload()} disabled={running !== null}>
      Refresh
    </Button>
    {#if running}<span class="busy text-[11.5px] text-muted-foreground" role="status">{running} running…</span>{/if}
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
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} onclick={() => void run("count_objects", "Count objects")}>
          Count objects
        </Button>
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="Write the commit-graph (faster log on big repos)" onclick={() => void run("commit_graph", "Commit-graph write")}>
          Write commit-graph
        </Button>
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} onclick={() => void run("pack_refs", "Pack refs")}>
          Pack refs
        </Button>
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="git prune — drops unreachable loose objects" onclick={() => void run("prune", "Prune")}>
          Prune
        </Button>
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="git gc --auto — pack, repack, and tidy" onclick={() => void run("gc", "GC")}>
          GC
        </Button>
        <!-- M12: git fsck — object-store integrity + dangling-object census.
             The result block below stays hidden until the first run
             (fsck_dangling is null in a fresh RepoHealth). -->
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={running !== null} title="git fsck — check object connectivity and list dangling objects" onclick={() => void run("fsck", "Check objects")}>
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
                <Button
                  class="h-auto rounded px-[5px] py-[2px] font-mono text-[11px] font-normal text-muted-foreground"
                  variant="outline"
                  type="button"
                  title="Copy {sha}"
                  onclick={() => void copySha(sha)}
                >
                  {sha.slice(0, 12)}
                </Button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Archive export">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Archive</h2>
      <div class="row flex items-center gap-1.5">
        <input
          class="text min-w-20 flex-1 rounded border border-border bg-background px-[7px] py-[3px] text-xs text-foreground"
          bind:value={archiveSpecText}
          placeholder="ref or sha"
          aria-label="Archive ref"
        />
        <select
          class="rounded border border-border bg-background px-1 py-[2px] text-[11.5px] text-foreground"
          bind:value={archiveFormat}
          aria-label="Archive format"
        >
          <option value="zip">zip</option>
          <option value="tar">tar</option>
          <option value="tar.gz">tar.gz</option>
        </select>
        <Button
          class="h-auto rounded-full px-3 py-[3px] text-[11.5px]"
          type="button"
          disabled={archiving || !archiveSpecText.trim()}
          onclick={() => void onArchive()}
        >
          {archiving ? "Exporting…" : "Export…"}
        </Button>
      </div>
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Sparse checkout">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Sparse checkout (cone mode)</h2>
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
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={sparseBusy} onclick={() => void onSparseApply(false)}>
          Apply
        </Button>
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={sparseBusy} onclick={() => void onSparseApply(true)}>
          Add
        </Button>
        <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal" variant="destructive" type="button" disabled={sparseBusy} title="Back to a full checkout" onclick={() => { sparseText = ""; void onSparseApply(false); }}>
          Disable
        </Button>
      </div>
    </section>

    <section class="group border-t border-border px-2 py-1.5" aria-label="Git LFS">
      <h2 class="head mb-1.5 text-xs font-semibold uppercase tracking-[0.05em] text-muted-foreground">Git LFS</h2>
      {#if lfs?.installed}
        <p class="state mb-1.5 text-xs text-muted-foreground">{lfs.version}</p>
        {#if lfs.tracked_patterns.length > 0}
          <p class="state mb-1.5 text-xs text-muted-foreground">Tracking: {lfs.tracked_patterns.join(", ")}</p>
        {:else}
          <p class="state mb-1.5 text-xs text-muted-foreground">No filter=lfs patterns in tracked .gitattributes.</p>
        {/if}
        <div class="ops flex flex-wrap gap-1.5">
          <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={lfsBusy} onclick={() => void onLfs("pull")}>Pull</Button>
          <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={lfsBusy} onclick={() => void onLfs("push")}>Push</Button>
          <Button class="h-auto rounded px-2 py-[3px] text-[11.5px] font-normal text-muted-foreground" variant="outline" type="button" disabled={lfsBusy} onclick={() => void onLfs("fetch")}>Fetch</Button>
        </div>
      {:else if lfs}
        <p class="state warn mb-1.5 text-xs text-amber-600 dark:text-amber-400">
          git lfs is not installed — install it from <code class="font-mono">git-lfs.com</code>, then re-run setup here.
        </p>
      {:else}
        <p class="state mb-1.5 text-xs text-muted-foreground">Detecting…</p>
      {/if}
    </section>
  {:else}
    <p class="state mb-1.5 text-xs text-muted-foreground">Loading health…</p>
  {/if}
</aside>
