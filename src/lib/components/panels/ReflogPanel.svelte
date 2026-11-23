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
   *
   * The ref selector's branch feed comes from the shared refs cache
   * (`$lib/stores/refs.svelte`) — the four other ref-listing surfaces use
   * the same cache, so a repo open scans once, not five times; the reflog
   * itself is panel-specific and stays uncached. External ref moves
   * (terminal, outside editors) reload through the debounced repo-change
   * effect below — the reflog is exactly the surface that must follow
   * HEAD moves it did not cause.
   */
  import { branchCreate, reflog } from "$lib/ipc/client";
  import type { BranchInfo, ReflogEntry } from "$lib/ipc/types";
  import { isSignificant, onRepoChange } from "$lib/events/repoEvents";
  import { branchesFor, invalidateRefs } from "$lib/stores/refs.svelte";
  import { copyText } from "$lib/clipboard";
  import { toast } from "$lib/toast";
  import { emitUiEvent } from "$lib/palette/events";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import EmptyState from "../layout/EmptyState.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
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

  const iconRefresh = $derived(appIcon("refresh")!);

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

  // External ref moves (the terminal panel, an outside editor, the header
  // switcher) never reached this panel before — it reloaded only on repo
  // switch and after its OWN mutations. Reload on significant repo
  // changes, debounced 500 ms like FilesPanel's tree refresh so a watcher
  // burst coalesces into one reload. The selected ref is kept: a HEAD move
  // elsewhere should refresh the reflog the user is reading, not yank the
  // selector back to HEAD (that reset stays a repo-switch behavior). The
  // panel is keep-alive mounted (TabGroup), so this also reloads while
  // hidden — accepted: the reads are cheap and shared (the refs cache was
  // already dropped by its own repoEvents subscription; every panel's
  // debounced reload then shares ONE rescan). StatusPanel's cleanup
  // pattern: the pending debounce dies with the panel, or it would fire
  // reload() IPC against a repo that may since have been switched away.
  $effect(() => {
    let reloadTimer: ReturnType<typeof setTimeout> | null = null;
    const unregister = onRepoChange({
      filter: (change) => change.repo_id === repoId && isSignificant(change),
      handle: () => {
        if (reloadTimer) clearTimeout(reloadTimer);
        reloadTimer = setTimeout(() => {
          reloadTimer = null;
          void reload();
        }, 500);
      },
    });
    return () => {
      unregister();
      if (reloadTimer) {
        clearTimeout(reloadTimer);
        reloadTimer = null;
      }
    };
  });

  async function reload(): Promise<void> {
    // Stale-response guard: the panel is keep-alive across repo switches —
    // a slow reflog from repo A must never land after the switch.
    const id = repoId;
    loading = true;
    error = null;
    try {
      const refName = selectedRef === "HEAD" ? undefined : selectedRef;
      // reflog is panel-specific (uncached); the ref selector's branch
      // feed rides the shared refs cache (module header).
      const [log, br] = await Promise.all([reflog(id, refName), branchesFor(id)]);
      if (id !== repoId) return;
      entries = log;
      branchList = br;
    } catch (err) {
      if (id !== repoId) return;
      error = err instanceof Error ? err.message : String(err);
      entries = [];
    } finally {
      if (id === repoId) loading = false;
    }
  }

  function onSelectRef(): void {
    void reload();
  }

  function copySha(entry: ReflogEntry): void {
    void copyText(entry.new_sha, shortSha(entry.new_sha));
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
      // Refs moved (new branch + HEAD), so the shared refs cache must be
      // dropped FIRST — a plain reload could still be served the
      // pre-mutation branch list from the cache's TTL window (the watcher
      // event that invalidates it may still be batched/in flight).
      invalidateRefs(repoId);
      void reload();
    } catch (err) {
      toast(
        `Branch create failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }
</script>

<aside class="reflog-panel flex min-h-0 flex-col text-ui-base" aria-label="Reflog">
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
    class="hint m-0 shrink-0 border-b border-border bg-primary/10 px-2 py-1 text-ui-meta text-foreground"
  >
    Lost commits? Find them here — every HEAD move is recorded.
  </p>

  {#if error}
    <p class="state m-0 px-3 py-4 text-ui-base text-destructive" role="alert"
      >{error}</p
    >
  {:else if loading && entries.length === 0}
    <p class="state m-auto px-3 py-4 text-ui-base text-muted-foreground" role="status">
      <span class="inline-flex items-center gap-ui-4">
        <Spinner class="size-4" role="presentation" aria-hidden="true" />
        Loading reflog…
      </span>
    </p>
  {:else if !loading && entries.length === 0}
    <EmptyState title="No reflog entries for this ref." />
  {:else}
    <div class="log min-h-0 flex-1 overflow-y-auto px-2 pb-2 pt-1" role="list" aria-label="Reflog entries">
      {#each visibleGroups as group (group.label + group.entries[0].new_sha)}
        <h3
          class="day mb-1 mt-2 text-ui-micro font-semibold uppercase tracking-[0.05em] text-muted-foreground"
        >
          {group.label}
        </h3>
        <ul class="entries m-0 flex list-none flex-col p-0" role="list" aria-label={`Reflog entries for ${group.label}`}>
          {#each group.entries as entry, i (`${entry.new_sha}:${i}`)}
            <li>
              <Hint label={`${entry.message} — click to copy ${shortSha(entry.new_sha)}`}>
                {#snippet child({ props })}
                  <button
                    {...props}
                    class="entry flex w-full cursor-pointer items-baseline gap-2 rounded-sm border-0 bg-transparent px-1.5 py-1 text-left font-[inherit] text-foreground hover:bg-muted/60 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    onclick={() => copySha(entry)}
                    oncontextmenu={(e) => entryMenu(e, entry)}
                  >
                    <span class="sha shrink-0 font-mono text-ui-meta text-primary-ink"
                      >{shortSha(entry.new_sha)}</span
                    >
                    <span class="what flex min-w-0 flex-col">
                      <span class="message overflow-hidden text-ellipsis whitespace-nowrap"
                        >{entry.message}</span
                      >
                      <span class="meta text-ui-micro text-muted-foreground"
                        >{entry.signature.name} · {relativeAge(entry.signature.time)}</span
                      >
                    </span>
                  </button>
                {/snippet}
              </Hint>
            </li>
          {/each}
        </ul>
      {/each}
      {#if entries.length > REFLOG_DISPLAY_CAP}
        <p class="cap mb-0 mt-2 text-ui-micro text-muted-foreground" role="note">
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

