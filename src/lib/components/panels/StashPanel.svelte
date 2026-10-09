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

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Called after a mutation that changes repo state (push/pop/apply/branch). */
    onMutated?: () => void;
  } = $props();

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
</script>

<aside class="stash-panel" aria-label="Stashes">
  <div class="toolbar">
    <button class="act" type="button" onclick={() => (formOpen = !formOpen)} disabled={busy}>
      {formOpen ? "Close form" : "Stash changes"}
    </button>
    <button class="tb" type="button" onclick={() => void reload()} disabled={loading}>
      {loading ? "Loading…" : "Refresh"}
    </button>
  </div>

  {#if formOpen}
    <form
      class="push-form"
      aria-label="Stash changes"
      onsubmit={(e) => {
        e.preventDefault();
        void onPush();
      }}
    >
      <input
        class="msg"
        type="text"
        placeholder="Stash message (optional)"
        aria-label="Stash message"
        bind:value={message}
      />
      <StashMessageButton
        {repoId}
        onMessage={(suggested) => (message = suggested)}
      />
      <div class="opts">
        <label class="toggle" title="Keep changes already staged in the index">
          <input type="checkbox" bind:checked={keepIndex} />
          <span>keep-index</span>
        </label>
        <label class="toggle" title="Also stash untracked files">
          <input type="checkbox" bind:checked={includeUntracked} />
          <span>include-untracked</span>
        </label>
      </div>
      <button class="go" type="submit" disabled={busy}>Stash</button>
    </form>
  {/if}

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state">No stashes. Use "Stash changes" to park your work.</p>
  {:else}
    <ul class="stashes" role="list" aria-label="Stash list">
      {#each list as info (info.index)}
        <li class="card">
          <div class="row">
            <span class="ref">{stashRef(info)}</span>
            <span class="msg" title={info.message}>{info.message}</span>
          </div>
          <div class="meta">
            <span class="sha">{shortSha(info.sha)}</span>
            <span class="author">{info.author.name}</span>
            <span class="age">{age(info)}</span>
          </div>
          <div class="actions">
            <button
              class="act"
              type="button"
              disabled={busy}
              aria-label={`Apply ${stashRef(info)}`}
              onclick={() => void onApply(info, false)}
            >
              Apply
            </button>
            <button
              class="act"
              type="button"
              disabled={busy}
              aria-label={`Pop ${stashRef(info)} (apply and drop)`}
              onclick={() => void onApply(info, true)}
            >
              Pop
            </button>
            <button
              class="tb"
              type="button"
              disabled={busy}
              aria-label={`Create a branch from ${stashRef(info)}`}
              onclick={() => {
                branchFor = branchFor === info.index ? null : info.index;
                branchName = "";
              }}
            >
              Branch…
            </button>
            <button
              class="tb danger"
              type="button"
              disabled={busy}
              aria-label={`Drop ${stashRef(info)}`}
              onclick={() => void onDrop(info)}
            >
              Drop
            </button>
          </div>
          {#if branchFor === info.index}
            <form
              class="branch-form"
              aria-label={`Branch name for ${stashRef(info)}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onBranch(info);
              }}
            >
              <input
                class="bname"
                type="text"
                placeholder="new branch name"
                aria-label="New branch name"
                bind:value={branchName}
                onkeydown={(e) => {
                  if (e.key === "Escape") branchFor = null;
                }}
              />
              <button class="go" type="submit" disabled={busy || !branchName.trim()}>
                Create branch
              </button>
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

<style>
  .stash-panel {
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

  .push-form {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
    padding: 0.375rem 0.5rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex: none;
  }

  .push-form .msg {
    flex: 1;
    min-width: 10rem;
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  .push-form .msg:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .opts {
    display: flex;
    align-items: center;
    gap: 0.625rem;
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .toggle input {
    accent-color: var(--m3-primary);
    margin: 0;
  }

  .stashes {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    padding: 0.375rem 0.5rem;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .card {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    padding: 0.375rem 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .row {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    min-width: 0;
  }

  .ref {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    font-weight: 600;
    color: var(--m3-primary);
  }

  .msg {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .meta .sha {
    font-family: ui-monospace, Consolas, monospace;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    flex-wrap: wrap;
  }

  .branch-form {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding-top: 0.25rem;
    border-top: 1px dashed var(--m3-outline-variant, var(--m3-primary));
  }

  .branch-form .bname {
    flex: 1;
    min-width: 8rem;
    font: inherit;
    font-size: 0.72rem;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.375rem;
  }

  .branch-form .bname:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
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

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb:focus-visible,
  .act:focus-visible,
  .go:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .danger {
    color: var(--m3-error, inherit);
  }

  .act {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .act:disabled,
  .tb:disabled,
  .go:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .go {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
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
</style>
