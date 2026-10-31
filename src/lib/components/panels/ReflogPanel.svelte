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
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
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

<aside class="reflog-panel flex min-h-0 flex-col text-[0.8125rem]" aria-label="Reflog">
  <div class="toolbar flex shrink-0 items-center gap-1.5 px-2 py-1.5">
    <label class="ref-pick flex items-center gap-1.5">
      <span class="sr-only">Reflog ref</span>
      <select
        class="max-w-56 rounded-md border border-input bg-background px-1.5 py-0.5 font-[inherit] text-xs text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
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
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh reflog"
      title={loading ? "Refreshing reflog…" : "Refresh reflog"}
      disabled={loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
  </div>

  <p
    class="hint m-0 shrink-0 border-b border-border bg-primary/10 px-2 py-[0.3rem] text-[0.72rem] text-foreground"
  >
    Lost commits? Find them here — every HEAD move is recorded.
  </p>

  {#if error}
    <p class="state m-0 px-3 py-4 text-[0.8125rem] text-destructive" role="alert"
      >{error}</p
    >
  {:else if !loading && entries.length === 0}
    <p class="state m-0 px-3 py-4 text-[0.8125rem] text-muted-foreground">
      No reflog entries for this ref.</p
    >
  {:else}
    <div class="log min-h-0 flex-1 overflow-y-auto px-2 pb-2 pt-1" role="list" aria-label="Reflog entries">
      {#each visibleGroups as group (group.label + group.entries[0].new_sha)}
        <h3
          class="day mb-1 mt-2 text-[0.6875rem] font-semibold uppercase tracking-[0.05em] text-muted-foreground"
        >
          {group.label}
        </h3>
        <ul class="entries m-0 flex list-none flex-col p-0" role="list" aria-label={`Reflog entries for ${group.label}`}>
          {#each group.entries as entry, i (`${entry.new_sha}:${i}`)}
            <li>
              <button
                class="entry flex w-full cursor-pointer items-baseline gap-2 rounded-sm border-0 bg-transparent px-1.5 py-1 text-left font-[inherit] text-foreground hover:bg-muted/60 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                type="button"
                title={`${entry.message} — click to copy ${shortSha(entry.new_sha)}`}
                onclick={() => copySha(entry)}
                oncontextmenu={(e) => entryMenu(e, entry)}
              >
                <span class="sha shrink-0 font-mono text-[0.72rem] text-primary"
                  >{shortSha(entry.new_sha)}</span
                >
                <span class="what flex min-w-0 flex-col">
                  <span class="message overflow-hidden text-ellipsis whitespace-nowrap"
                    >{entry.message}</span
                  >
                  <span class="meta text-[0.6875rem] text-muted-foreground"
                    >{entry.signature.name} · {relativeAge(entry.signature.time)}</span
                  >
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/each}
      {#if entries.length > REFLOG_DISPLAY_CAP}
        <p class="cap mb-0 mt-2 text-[0.6875rem] text-muted-foreground" role="note">
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

