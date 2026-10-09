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
  import { formatRelativeTime } from "$lib/stores/history-logic";

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

<aside class="repo-panel" aria-label="Repository health and maintenance">
  <div class="toolbar">
    <button class="tb" type="button" onclick={() => void reload()} disabled={running !== null}>
      Refresh
    </button>
    {#if running}<span class="busy" role="status">{running} running…</span>{/if}
  </div>

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if health}
    <section class="group" aria-label="Size and objects">
      <h2 class="head">Health</h2>
      <dl class="facts">
        <dt>Git directory</dt>
        <dd>{fmtSize(health.git_size_bytes)}</dd>
        <dt>Worktree</dt>
        <dd>{fmtSize(health.worktree_size_bytes)}</dd>
        <dt>Loose objects</dt>
        <dd>{health.loose_objects}</dd>
        <dt>Pack files</dt>
        <dd>{health.pack_files}</dd>
        <dt>Commit-graph</dt>
        <dd>
          {#if health.has_commit_graph}
            yes ({fmtSize(health.commit_graph_bytes)})
          {:else}
            <span class="warn">none — write one for faster history walks</span>
          {/if}
        </dd>
        <dt>Packed refs</dt>
        <dd>{health.packed_refs ? "yes" : "no"}</dd>
      </dl>
      <div class="ops">
        <button class="tb" type="button" disabled={running !== null} onclick={() => void run("count_objects", "Count objects")}>
          Count objects
        </button>
        <button class="tb" type="button" disabled={running !== null} title="Write the commit-graph (faster log on big repos)" onclick={() => void run("commit_graph", "Commit-graph write")}>
          Write commit-graph
        </button>
        <button class="tb" type="button" disabled={running !== null} onclick={() => void run("pack_refs", "Pack refs")}>
          Pack refs
        </button>
        <button class="tb" type="button" disabled={running !== null} title="git prune — drops unreachable loose objects" onclick={() => void run("prune", "Prune")}>
          Prune
        </button>
        <button class="tb" type="button" disabled={running !== null} title="git gc --auto — pack, repack, and tidy" onclick={() => void run("gc", "GC")}>
          GC
        </button>
        <!-- M12: git fsck — object-store integrity + dangling-object census.
             The result block below stays hidden until the first run
             (fsck_dangling is null in a fresh RepoHealth). -->
        <button class="tb" type="button" disabled={running !== null} title="git fsck — check object connectivity and list dangling objects" onclick={() => void run("fsck", "Check objects")}>
          Check objects (fsck)
        </button>
      </div>

      {#if health.fsck_dangling !== null}
        <div class="fsck" aria-label="Dangling objects">
          <p class="state">
            Dangling objects: {health.fsck_dangling}{#if fsckRanAt}
              (checked {formatRelativeTime(Math.floor(fsckRanAt / 1000))}){/if}
          </p>
          {#if health.fsck_samples.length > 0}
            <div class="fsck-samples">
              {#each health.fsck_samples as sha (sha)}
                <button
                  class="sha"
                  type="button"
                  title="Copy {sha}"
                  onclick={() => void copySha(sha)}
                >
                  {sha.slice(0, 12)}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </section>

    <section class="group" aria-label="Archive export">
      <h2 class="head">Archive</h2>
      <div class="row">
        <input
          class="text"
          bind:value={archiveSpecText}
          placeholder="ref or sha"
          aria-label="Archive ref"
        />
        <select bind:value={archiveFormat} aria-label="Archive format">
          <option value="zip">zip</option>
          <option value="tar">tar</option>
          <option value="tar.gz">tar.gz</option>
        </select>
        <button class="go" type="button" disabled={archiving || !archiveSpecText.trim()} onclick={() => void onArchive()}>
          {archiving ? "Exporting…" : "Export…"}
        </button>
      </div>
    </section>

    <section class="group" aria-label="Sparse checkout">
      <h2 class="head">Sparse checkout (cone mode)</h2>
      {#if sparse}
        <p class="state">
          {sparse.enabled
            ? `${sparse.patterns.length} pattern${sparse.patterns.length === 1 ? "" : "s"}${sparse.cone ? "" : " (non-cone)"}`
            : "Full checkout (sparse disabled)."}
        </p>
      {/if}
      <textarea
        class="patterns"
        rows="4"
        bind:value={sparseText}
        placeholder="one directory pattern per line, e.g.&#10;src/&#10;docs/"
        aria-label="Sparse checkout patterns"
      ></textarea>
      <div class="ops">
        <button class="tb" type="button" disabled={sparseBusy} onclick={() => void onSparseApply(false)}>
          Apply
        </button>
        <button class="tb" type="button" disabled={sparseBusy} onclick={() => void onSparseApply(true)}>
          Add
        </button>
        <button class="tb danger" type="button" disabled={sparseBusy} title="Back to a full checkout" onclick={() => { sparseText = ""; void onSparseApply(false); }}>
          Disable
        </button>
      </div>
    </section>

    <section class="group" aria-label="Git LFS">
      <h2 class="head">Git LFS</h2>
      {#if lfs?.installed}
        <p class="state">{lfs.version}</p>
        {#if lfs.tracked_patterns.length > 0}
          <p class="state">Tracking: {lfs.tracked_patterns.join(", ")}</p>
        {:else}
          <p class="state">No filter=lfs patterns in tracked .gitattributes.</p>
        {/if}
        <div class="ops">
          <button class="tb" type="button" disabled={lfsBusy} onclick={() => void onLfs("pull")}>Pull</button>
          <button class="tb" type="button" disabled={lfsBusy} onclick={() => void onLfs("push")}>Push</button>
          <button class="tb" type="button" disabled={lfsBusy} onclick={() => void onLfs("fetch")}>Fetch</button>
        </div>
      {:else if lfs}
        <p class="state warn">
          git lfs is not installed — install it from <code>git-lfs.com</code>, then re-run setup here.
        </p>
      {:else}
        <p class="state">Detecting…</p>
      {/if}
    </section>
  {:else}
    <p class="state">Loading health…</p>
  {/if}
</aside>

<style>
  .repo-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    font-size: 0.8125rem;
    padding-bottom: 0.75rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .busy {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  .group {
    border-top: 1px solid var(--m3-outline-variant, transparent);
    padding: 0.375rem 0.5rem;
  }

  .head {
    margin: 0 0 0.375rem;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .facts {
    margin: 0 0 0.5rem;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.1rem 0.75rem;
    font-size: 0.75rem;
  }

  .facts dt {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .facts dd {
    margin: 0;
    font-family: ui-monospace, Consolas, monospace;
  }

  .ops {
    display: flex;
    flex-wrap: wrap;
    gap: 0.375rem;
  }

  .tb {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .tb.danger {
    color: var(--m3-error, inherit);
  }

  .go {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .go:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .text {
    flex: 1;
    min-width: 5rem;
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  select {
    font: inherit;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.3rem;
  }

  .patterns {
    width: 100%;
    box-sizing: border-box;
    font: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.3rem 0.45rem;
    margin-bottom: 0.375rem;
    resize: vertical;
  }

  .patterns:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .state {
    margin: 0 0 0.375rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .warn {
    color: var(--m3-tertiary, var(--m3-on-surface-variant));
  }

  code {
    font-family: ui-monospace, Consolas, monospace;
  }

  /* M12: fsck dangling-objects block. */
  .fsck {
    margin-top: 0.375rem;
  }

  .fsck .state {
    margin-bottom: 0.25rem;
  }

  .fsck-samples {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }

  button.sha {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    padding: 0.1rem 0.35rem;
    cursor: pointer;
  }

  button.sha:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  button.sha:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
