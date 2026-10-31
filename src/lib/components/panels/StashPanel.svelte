<script lang="ts">
  /**
   * Stash panel (M3) — the stash stack plus every stash mutation.
   *
   * Rows show `stash@{n}`, the message, short sha, relative age and author;
   * each row carries Apply (keeps the stash), Pop (apply + drop), Drop
   * (confirm) and Branch… (inline name prompt → `stash_branch`, which
   * checks the stash out as a new branch). The toolbar's "Stash changes"
   * opens an inline push form (optional message, keep-index and
   * include-untracked checkboxes — untracked on by default). Every
   * mutation refreshes the repo status (stashing changes the worktree)
   * and reloads the list.
   */
  import { stashApply, stashBranch, stashDrop, stashList, stashPush } from "$lib/ipc/client";
  import type { StashInfo } from "$lib/ipc/types";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import { relativeAge, shortSha, stashRef } from "./panelModel";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import StashMessageButton from "$lib/components/ai/StashMessageButton.svelte";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Called after a mutation that changes repo state (push/pop/apply/branch). */
    onMutated?: () => void;
  } = $props();

  const iconAdd = $derived(appIcon("add")!);
  const iconRefresh = $derived(appIcon("refresh")!);
  const iconApply = $derived(appIcon("play-arrow")!);
  const iconPop = $derived(appIcon("unarchive")!);
  const iconBranch = $derived(appIcon("call-split")!);
  const iconDrop = $derived(appIcon("delete")!);
  const iconCheck = $derived(appIcon("check")!);
  const iconMore = $derived(appIcon("more-vert")!);

  let list = $state<StashInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  /** Any mutation in flight (buttons disabled while true). */
  let busy = $state(false);

  // Push form
  let formOpen = $state(false);
  let message = $state("");
  let keepIndex = $state(false);
  let includeUntracked = $state(true);

  // Per-row "Branch…" prompt (stash index currently being named).
  let branchFor = $state<number | null>(null);
  let branchName = $state("");

  $effect(() => {
    // Reload when the repo switches.
    void repoId;
    formOpen = false;
    branchFor = null;
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      list = await stashList(repoId);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function fail(what: string, err: unknown): void {
    toast(`${what} failed: ${err instanceof Error ? err.message : String(err)}`, {
      kind: "error",
    });
  }

  /** Post-mutation housekeeping shared by every action. */
  function afterMutation(what: string, detail: string): void {
    toast(`${what}: ${detail}`, { kind: "success" });
    void refreshStatus(repoId);
    void reload();
    onMutated?.();
  }

  async function onPush(): Promise<void> {
    busy = true;
    try {
      await stashPush(repoId, message.trim() || undefined, keepIndex, includeUntracked);
      message = "";
      formOpen = false;
      afterMutation("Stash", "changes stashed");
    } catch (err) {
      fail("Stash", err);
    } finally {
      busy = false;
    }
  }

  async function onApply(info: StashInfo, pop: boolean): Promise<void> {
    busy = true;
    try {
      await stashApply(repoId, info.index, pop);
      afterMutation(pop ? "Pop" : "Apply", stashRef(info));
    } catch (err) {
      fail(pop ? "Pop" : "Apply", err);
    } finally {
      busy = false;
    }
  }

  /** Drop confirmation (ConfirmDialog state). */
  let dropCandidate = $state<StashInfo | null>(null);

  async function onDrop(info: StashInfo): Promise<void> {
    dropCandidate = info;
  }

  async function onDropConfirmed(): Promise<void> {
    const info = dropCandidate;
    if (!info) return;
    dropCandidate = null;
    busy = true;
    try {
      await stashDrop(repoId, info.index);
      afterMutation("Drop", stashRef(info));
    } catch (err) {
      fail("Drop", err);
    } finally {
      busy = false;
    }
  }

  async function onBranch(info: StashInfo): Promise<void> {
    const name = branchName.trim();
    if (!name) return;
    busy = true;
    try {
      await stashBranch(repoId, name, info.index);
      branchName = "";
      branchFor = null;
      afterMutation("Branch", `${name} from ${stashRef(info)}`);
    } catch (err) {
      fail("Branch", err);
    } finally {
      busy = false;
    }
  }

  function age(info: StashInfo): string {
    return relativeAge(info.author.time);
  }

  /**
   * Row kebab menu (UI review P2 #11): Apply stays the one visible primary
   * action; Pop / Branch / Drop get text labels here instead of unlabeled
   * icon buttons.
   */
  function stashMenu(info: StashInfo, event: MouseEvent): void {
    showMenuAtElement(event.currentTarget as HTMLElement, [
      {
        id: "pop",
        label: "Pop (apply and drop)",
        icon: iconPop,
        disabled: busy,
        run: () => void onApply(info, true),
      },
      {
        id: "branch",
        label: "Create branch…",
        icon: iconBranch,
        disabled: busy,
        run: () => {
          branchFor = info.index;
          branchName = "";
        },
      },
      { separator: true },
      {
        id: "drop",
        label: "Drop…",
        icon: iconDrop,
        danger: true,
        disabled: busy,
        run: () => void onDrop(info),
      },
    ]);
  }
</script>

<aside class="stash-panel flex min-h-0 flex-col text-[13px]" aria-label="Stashes">
  <div class="toolbar flex flex-none items-center gap-1.5 px-2 py-1.5">
    <Button
      class="aria-pressed:bg-accent aria-pressed:text-accent-foreground"
      variant="default"
      size="icon-sm"
      type="button"
      aria-label={formOpen ? "Close stash form" : "Stash changes"}
      aria-pressed={formOpen}
      title={formOpen ? "Close the stash form" : "Stash the working-copy changes"}
      disabled={busy}
      onclick={() => (formOpen = !formOpen)}
    >
      <Icon icon={iconAdd} size={16} />
    </Button>
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh stashes"
      title={loading ? "Refreshing stashes…" : "Refresh stashes"}
      disabled={loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
  </div>

  {#if formOpen}
    <form
      class="push-form flex flex-none flex-wrap items-center gap-1.5 border-b border-border px-2 py-1.5"
      aria-label="Stash changes"
      onsubmit={(e) => {
        e.preventDefault();
        void onPush();
      }}
    >
      <Input
        class="h-auto min-w-40 flex-1 rounded px-2 py-0.5 text-xs"
        type="text"
        placeholder="Stash message (optional)"
        aria-label="Stash message"
        bind:value={message}
      />
      <StashMessageButton
        {repoId}
        onMessage={(suggested) => (message = suggested)}
      />
      <div class="opts flex items-center gap-2.5">
        <label
          class="toggle flex cursor-pointer items-center gap-1 whitespace-nowrap text-[0.6875rem] text-muted-foreground"
          title="Keep changes already staged in the index"
        >
          <input type="checkbox" class="m-0 accent-primary" bind:checked={keepIndex} />
          <span>keep-index</span>
        </label>
        <label
          class="toggle flex cursor-pointer items-center gap-1 whitespace-nowrap text-[0.6875rem] text-muted-foreground"
          title="Also stash untracked files"
        >
          <input type="checkbox" class="m-0 accent-primary" bind:checked={includeUntracked} />
          <span>include-untracked</span>
        </label>
      </div>
      <Button
        class="h-auto shrink-0 rounded-full px-3 py-0.5 text-[0.72rem]"
        type="submit"
        disabled={busy}
      >Stash</Button>
    </form>
  {/if}

  {#if error}
    <p class="state error py-4 px-3 text-[13px] text-destructive" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state py-4 px-3 text-[13px] text-muted-foreground">No stashes. Use "Stash changes" to park your work.</p>
  {:else}
    <ul class="stashes m-0 flex min-h-0 flex-1 list-none flex-col gap-2 overflow-y-auto px-2 py-1.5" role="list" aria-label="Stash list">
      {#each list as info (info.index)}
        <li class="card flex flex-col gap-1 rounded-lg border border-border bg-card px-2 py-1.5">
          <div class="row flex min-w-0 items-baseline gap-2">
            <span class="ref shrink-0 font-mono text-[0.6875rem] font-semibold text-primary">{stashRef(info)}</span>
            <span class="msg min-w-0 flex-1 truncate" title={info.message}>{info.message}</span>
          </div>
          <div class="meta flex items-center gap-2.5 text-[0.6875rem] text-muted-foreground">
            <span class="sha font-mono">{shortSha(info.sha)}</span>
            <span class="author">{info.author.name}</span>
            <span class="age">{age(info)}</span>
          </div>
          <div class="actions flex flex-wrap items-center gap-1">
            <Button
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label={`Apply ${stashRef(info)}`}
              title="Apply this stash, keep it in the list"
              disabled={busy}
              onclick={() => void onApply(info, false)}
            >
              <Icon icon={iconApply} size={16} />
            </Button>
            <Button
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label={`More actions for ${stashRef(info)}`}
              title="Pop, branch, or drop this stash"
              aria-haspopup="menu"
              disabled={busy}
              onclick={(event) => stashMenu(info, event)}
            >
              <Icon icon={iconMore} size={16} />
            </Button>
          </div>
          {#if branchFor === info.index}
            <form
              class="branch-form flex items-center gap-1.5 border-t border-dashed border-border pt-1"
              aria-label={`Branch name for ${stashRef(info)}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onBranch(info);
              }}
            >
              <Input
                class="h-auto min-w-32 flex-1 rounded px-1.5 py-0.5 font-mono text-[0.72rem]"
                type="text"
                placeholder="new branch name"
                aria-label="New branch name"
                bind:value={branchName}
                onkeydown={(e) => {
                  if (e.key === "Escape") branchFor = null;
                }}
              />
              <Button
                variant="default"
                size="icon-sm"
                type="submit"
                aria-label="Create branch from stash"
                title="Create the branch"
                disabled={busy || !branchName.trim()}
              >
                <Icon icon={iconCheck} size={16} />
              </Button>
            </form>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<ConfirmDialog
  bind:open={
    () => dropCandidate !== null,
    (v) => {
      if (!v) dropCandidate = null;
    }
  }
  title={dropCandidate ? `Drop ${stashRef(dropCandidate)}?` : ""}
  message={dropCandidate
    ? `${dropCandidate.message} — this cannot be undone.`
    : ""}
  confirmLabel="Drop stash"
  danger
  onConfirm={() => void onDropConfirmed()}
/>
