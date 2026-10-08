<script lang="ts">
  /**
   * Reflog panel (M3, real actions in M9) — the safety net for lost commits.
   *
   * A ref selector (HEAD + local branches) drives `reflog`; entries are
   * grouped by calendar day and show short sha, message, author and
   * relative date. Rendering caps at 500 entries (the full list stays in
   * memory; virtualization lands if it ever matters). Clicking an entry
   * copies its sha; the right-click menu (and per-entry buttons) offer the
   * recovery actions: select the commit in history, reset the current
   * branch to it (ResetDialog), or branch from here.
   */
  import { branchCreate, branches, reflog } from "$lib/ipc/client";
  import type { BranchInfo, ReflogEntry } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import { emitUiEvent } from "$lib/palette/events";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import {
    groupReflogByDay,
    REFLOG_DISPLAY_CAP,
    relativeAge,
    shortSha,
  } from "./panelModel";

  let {
    repoId,
  }: {
    repoId: string;
  } = $props();

  let branchList = $state<BranchInfo[]>([]);
  /** Selected ref: "HEAD" (default) or a local branch name. */
  let selectedRef = $state<string>("HEAD");
  let entries = $state<ReflogEntry[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Recovery flows
  let resetTo = $state<string | null>(null);
  let branchFrom = $state<string | null>(null);

  const visible = $derived(
    entries.length > REFLOG_DISPLAY_CAP ? entries.slice(0, REFLOG_DISPLAY_CAP) : entries,
  );
  const visibleGroups = $derived(groupReflogByDay(visible));

  $effect(() => {
    // Reload (back to HEAD) when the repo switches.
    void repoId;
    selectedRef = "HEAD";
    resetTo = null;
    branchFrom = null;
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      const refName = selectedRef === "HEAD" ? undefined : selectedRef;
      const [log, br] = await Promise.all([reflog(repoId, refName), branches(repoId)]);
      entries = log;
      branchList = br;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      entries = [];
    } finally {
      loading = false;
    }
  }

  function onSelectRef(): void {
    void reload();
  }

  function copySha(entry: ReflogEntry): void {
    void navigator.clipboard?.writeText(entry.new_sha).then(() =>
      toast(`${shortSha(entry.new_sha)} copied`, { kind: "success" }),
    );
  }

  function entryMenu(event: MouseEvent, entry: ReflogEntry): void {
    const entries: MenuEntry[] = [
      {
        id: "copy",
        label: "Copy sha",
        run: () => copySha(entry),
      },
      {
        id: "show",
        label: "Show in history",
        run: () => emitUiEvent("history-select-commit", { sha: entry.new_sha }),
      },
      {
        id: "reset",
        label: "Reset current branch to here…",
        run: () => (resetTo = entry.new_sha),
      },
      {
        id: "branch",
        label: "Create branch here…",
        run: () => (branchFrom = entry.new_sha),
      },
    ];
    showMenuAt(event, entries);
  }

  async function onCreateBranch(name: string): Promise<void> {
    const sha = branchFrom;
    branchFrom = null;
    if (!sha) return;
    try {
      await branchCreate(repoId, name, true, sha);
      toast(`Created branch ${name} at ${shortSha(sha)} (checked out)`, {
        kind: "success",
      });
      void reload();
    } catch (err) {
      toast(
        `Branch create failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }
</script>

<aside class="reflog-panel" aria-label="Reflog">
  <div class="toolbar">
    <label class="ref-pick">
      <span class="sr-only">Reflog ref</span>
      <select
        aria-label="Reflog ref"
        bind:value={selectedRef}
        onchange={onSelectRef}
      >
        <option value="HEAD">HEAD</option>
        {#each branchList as branch (branch.name)}
          <option value={branch.name}>{branch.name}</option>
        {/each}
      </select>
    </label>
    <button class="tb" type="button" onclick={() => void reload()} disabled={loading}>
      {loading ? "Loading…" : "Refresh"}
    </button>
  </div>

  <p class="hint">
    Lost commits? Find them here — every HEAD move is recorded.
  </p>

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if !loading && entries.length === 0}
    <p class="state">No reflog entries for this ref.</p>
  {:else}
    <div class="log" role="list" aria-label="Reflog entries">
      {#each visibleGroups as group (group.label + group.entries[0].new_sha)}
        <h3 class="day">{group.label}</h3>
        <ul class="entries" role="list" aria-label={`Reflog entries for ${group.label}`}>
          {#each group.entries as entry, i (`${entry.new_sha}:${i}`)}
            <li>
              <button
                class="entry"
                type="button"
                title={`${entry.message} — click to copy ${shortSha(entry.new_sha)}`}
                onclick={() => copySha(entry)}
                oncontextmenu={(e) => entryMenu(e, entry)}
              >
                <span class="sha">{shortSha(entry.new_sha)}</span>
                <span class="what">
                  <span class="message">{entry.message}</span>
                  <span class="meta">{entry.signature.name} · {relativeAge(entry.signature.time)}</span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/each}
      {#if entries.length > REFLOG_DISPLAY_CAP}
        <p class="cap" role="note">
          Showing the {REFLOG_DISPLAY_CAP} most recent of {entries.length} entries.
        </p>
      {/if}
    </div>
  {/if}
</aside>

<ResetDialog
  {repoId}
  open={resetTo !== null}
  defaultTarget={resetTo ?? undefined}
  onClose={() => (resetTo = null)}
  onDone={() => {
    resetTo = null;
    void reload();
  }}
/>

<PromptDialog
  open={branchFrom !== null}
  title="Create branch here"
  message={branchFrom ? `New branch at ${shortSha(branchFrom)} (checked out).` : ""}
  placeholder="Branch name"
  confirmLabel="Create"
  onCancel={() => (branchFrom = null)}
  onSubmit={(name) => void onCreateBranch(name)}
/>

<style>
  .reflog-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .ref-pick {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  select {
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.3rem;
    max-width: 14rem;
  }

  select:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .hint {
    flex: none;
    margin: 0;
    padding: 0.3rem 0.5rem;
    font-size: 0.72rem;
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
    background: var(--m3-secondary-container, var(--m3-surface-container-low));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0.25rem 0.5rem 0.5rem;
  }

  .day {
    margin: 0.5rem 0 0.25rem;
    font-size: 0.6875rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .entries {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
  }

  .entry {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    font: inherit;
    color: var(--m3-on-surface);
    padding: 0.25rem 0.375rem;
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
  }

  .entry:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .entry:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .sha {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    color: var(--m3-primary);
  }

  .what {
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .message {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .cap {
    margin: 0.5rem 0 0;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .tb {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .tb:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .tb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .state {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 1rem 0.75rem;
    margin: 0;
    font-size: 0.8125rem;
  }

  .error {
    color: var(--m3-error, inherit);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
