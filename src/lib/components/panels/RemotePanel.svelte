<script lang="ts">
  /**
   * Remote panel (M2) — remotes list + fetch/pull/push + remote management.
   *
   * Each remote card shows name, fetch URL and (when set) push URL, and
   * carries the network actions as one labeled row per verb: Fetch (prune
   * switch), Pull (branch picker, ff-only default on, rebase switch) and
   * Push (force + set-upstream switches); boolean options are M3 switches
   * right-aligned in a stable column. In-flight ops render inline progress
   * (message + pct bar) from the OpStore; completed ops toast their
   * NetStats (updated refs + objects). Add / remove / set-url forms manage
   * the remote config, and the footer selects the per-repo auto-fetch
   * interval (localStorage-backed `autofetch` store).
   */
  import {
    branchCheckoutRemote,
    branches,
    fetchRepo,
    pullRepo,
    pushRepo,
    remoteAdd,
    remoteBranches,
    remoteRemove,
    remoteSetUrl,
    remotes,
    repoDiff,
  } from "$lib/ipc/client";
  import type {
    FileDiff,
    NetStats,
    RemoteBranchInfo,
    RemoteInfo,
  } from "$lib/ipc/types";
  import { opsFor } from "$lib/stores/ops.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { toast } from "$lib/toast";
  import { Switch } from "$lib/components/m3";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";

  let {
    repoId,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Called after a mutation that changes repo state (pull/push). */
    onMutated?: () => void;
  } = $props();

  let list = $state<RemoteInfo[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  /** Local branch names + the current branch (pull/push pickers). */
  let branchNames = $state<string[]>([]);
  let currentBranch = $state<string>("");
  /** Selected branch per remote (defaults to the current branch). */
  let branchPick = $state<Record<string, string>>({});

  // Option checkboxes (shared defaults per remote card)
  let prune = $state(true);
  let ffOnly = $state(true);
  let rebase = $state(false);
  let force = $state(false);
  let setUpstream = $state(false);

  // Forms
  let addName = $state("");
  let addUrl = $state("");
  let editing = $state<{ name: string; field: "url" | "push_url"; value: string } | null>(
    null,
  );

  /** Auto-fetch interval choices (minutes; 0 = off). */
  const INTERVALS = [0, 1, 5, 15, 30, 60];

  const netOps = $derived(
    opsFor(repoId).filter((op) => op.kind === "fetch" || op.kind === "pull" || op.kind === "push"),
  );
  const netBusy = $derived(netOps.length > 0);

  $effect(() => {
    // Reload when the repo switches.
    void repoId;
    editing = null;
    compareKey = null;
    compareFiles = null;
    void reload();
  });

  async function reload(): Promise<void> {
    loading = true;
    error = null;
    try {
      const [remoteList, branchList] = await Promise.all([remotes(repoId), branches(repoId)]);
      list = remoteList;
      branchNames = branchList.map((b) => b.name);
      const head = branchList.find((b) => b.is_head)?.name ?? "";
      currentBranch = head;
      branchPick = {};
      for (const remote of remoteList) branchPick[remote.name] = head;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  function branchFor(remote: RemoteInfo): string {
    return branchPick[remote.name] ?? currentBranch;
  }

  function netStatsToast(action: string, remote: string, stats: NetStats): void {
    const refs = stats.updated_refs.length;
    toast(
      refs > 0
        ? `${remote} ${action}: ${refs} ref${refs === 1 ? "" : "s"} updated, ${stats.objects} objects`
        : `${remote} ${action}: up to date (${stats.objects} objects)`,
      { kind: "success" },
    );
  }

  /** Runs a network op: NetStats toast on success, error toast on failure. */
  async function runNet(
    action: string,
    remote: RemoteInfo,
    run: () => Promise<NetStats>,
    refreshRepo: boolean,
  ): Promise<void> {
    try {
      const stats = await run();
      netStatsToast(action, remote.name, stats);
      if (refreshRepo) {
        void reload();
        onMutated?.();
      }
    } catch (err) {
      toast(
        `${remote.name} ${action} failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  function onFetch(remote: RemoteInfo): void {
    void runNet("fetch", remote, () =>
      fetchRepo(repoId, { remote: remote.name, prune, refs: [], depth: null }), false);
  }

  function onPull(remote: RemoteInfo): void {
    void runNet("pull", remote, () =>
      pullRepo(repoId, {
        remote: remote.name,
        branch: branchFor(remote),
        ff_only: ffOnly,
        rebase,
      }), true);
  }

  function onPush(remote: RemoteInfo): void {
    void runNet("push", remote, () =>
      pushRepo(repoId, {
        remote: remote.name,
        branch: branchFor(remote),
        force,
        force_with_lease: false,
        set_upstream: setUpstream,
        refs: [],
        tags: false,
        delete: false,
      }), true);
  }

  async function onAdd(): Promise<void> {
    const name = addName.trim();
    const url = addUrl.trim();
    if (!name || !url) return;
    try {
      await remoteAdd(repoId, name, url);
      addName = "";
      addUrl = "";
      void reload();
    } catch (err) {
      toast(`Add remote failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }

  /** Remove confirmation (ConfirmDialog state, M9 F9). */
  let removeCandidate = $state<RemoteInfo | null>(null);

  // -- M10: remote branch management ------------------------------------------

  let expandedRemote = $state<string | null>(null);
  let remoteBranchList = $state<RemoteBranchInfo[]>([]);

  async function onToggleRemoteBranches(remote: RemoteInfo): Promise<void> {
    if (expandedRemote === remote.name) {
      expandedRemote = null;
      return;
    }
    expandedRemote = remote.name;
    try {
      remoteBranchList = await remoteBranches(repoId);
    } catch (err) {
      toast(
        `Remote branches failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  async function onCheckoutRemote(rb: RemoteBranchInfo): Promise<void> {
    try {
      const local = await branchCheckoutRemote(repoId, rb.remote, rb.name);
      toast(`Created and switched to ${local} (tracking ${rb.remote}/${rb.name})`, {
        kind: "success",
      });
      onMutated?.();
    } catch (err) {
      toast(
        `Checkout failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  async function onDeleteRemoteBranch(rb: RemoteBranchInfo): Promise<void> {
    try {
      await pushRepo(repoId, {
        remote: rb.remote,
        branch: "",
        force: false,
        force_with_lease: false,
        set_upstream: false,
        refs: [rb.name],
        tags: false,
        delete: true,
      });
      toast(`Deleted remote branch ${rb.remote}/${rb.name}`, { kind: "success" });
      remoteBranchList = await remoteBranches(repoId);
    } catch (err) {
      toast(
        `Delete failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  // -- M12: compare remote branch with HEAD ------------------------------------
  // There is no cross-panel compare event (CompareBar lives inside
  // HistoryView), so the compare renders inline in this panel: repoDiff
  // HEAD → <remote>/<branch>, result in a collapsible DiffViewer.

  /** Key (`<remote>/<name>`) of the row whose compare result is open. */
  let compareKey = $state<string | null>(null);
  let compareFiles = $state<FileDiff[] | null>(null);
  let compareLoading = $state(false);

  async function onCompareHead(rb: RemoteBranchInfo): Promise<void> {
    const key = `${rb.remote}/${rb.name}`;
    compareKey = key;
    compareFiles = null;
    compareLoading = true;
    try {
      compareFiles = await repoDiff(repoId, { commit: "HEAD" }, { commit: key });
    } catch (err) {
      toast(
        `Compare failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
      compareKey = null;
    } finally {
      compareLoading = false;
    }
  }

  function onRemove(remote: RemoteInfo): void {
    removeCandidate = remote;
  }

  async function onRemoveConfirmed(): Promise<void> {
    const remote = removeCandidate;
    if (!remote) return;
    removeCandidate = null;
    try {
      await remoteRemove(repoId, remote.name);
      void reload();
    } catch (err) {
      toast(`Remove remote failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }

  async function onSetUrl(): Promise<void> {
    if (!editing) return;
    const { name, field, value } = editing;
    const url = value.trim();
    editing = null;
    if (!url) return;
    try {
      await remoteSetUrl(repoId, name, url, field === "push_url");
      void reload();
    } catch (err) {
      toast(`Set URL failed: ${err instanceof Error ? err.message : String(err)}`, {
        kind: "error",
      });
    }
  }
</script>

<aside class="remote-panel" aria-label="Remotes">
  <div class="toolbar">
    <button class="tb" type="button" onclick={() => void reload()} disabled={loading}>
      {loading ? "Loading…" : "Refresh"}
    </button>
    <label class="autofetch" title="Fetch the default remote in the background while this repo is open">
      <span>auto-fetch</span>
      <select
        aria-label="Auto-fetch interval in minutes"
        value={autofetch.getInterval(repoId)}
        onchange={(e) => autofetch.setInterval(repoId, Number(e.currentTarget.value))}
      >
        {#each INTERVALS as minutes (minutes)}
          <option value={minutes}>
            {minutes === 0 ? "off" : `${minutes} min`}
          </option>
        {/each}
      </select>
    </label>
  </div>

  {#if netOps.length > 0}
    <div class="progress" role="status" aria-live="polite">
      {#each netOps as op (op.op_id)}
        <div class="pbar-wrap">
          <span class="pmsg">{op.message}</span>
          {#if op.pct !== null}
            <span class="pbar" role="progressbar" aria-valuemin={0} aria-valuemax={100}
              aria-valuenow={Math.round(op.pct)}>
              <span class="pfill" style:width={`${Math.max(0, Math.min(100, op.pct))}%`}></span>
            </span>
          {:else}
            <span class="pbar indeterminate" role="progressbar" aria-label="In progress">
              <span class="pfill"></span>
            </span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if error}
    <p class="state error" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state">No remotes configured.</p>
  {:else}
    <ul class="remotes" role="list" aria-label="Configured remotes">
      {#each list as remote (remote.name)}
        <li class="card">
          {#if editing?.name === remote.name}
            <form
              class="edit-url"
              aria-label={`Set ${editing.field} for ${remote.name}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onSetUrl();
              }}
            >
              <label class="eu-label">
                {editing.field === "url" ? "fetch URL" : "push URL"}:
                <input
                  class="eu-input"
                  type="text"
                  aria-label={`${editing.field} for ${remote.name}`}
                  bind:value={editing.value}
                  onkeydown={(e) => {
                    if (e.key === "Escape") editing = null;
                  }}
                />
              </label>
              <button class="go" type="submit">Save</button>
              <button class="tb" type="button" onclick={() => (editing = null)}>Cancel</button>
            </form>
          {:else}
            <div class="card-head">
              <span class="rname">{remote.name}</span>
              <span class="url" title={remote.url}>{remote.url}</span>
              {#if remote.push_url}
                <span class="url push" title={remote.push_url}>push: {remote.push_url}</span>
              {/if}
              <span class="head-actions">
                <button
                  class="tb"
                  type="button"
                  aria-label={`Edit fetch URL for ${remote.name}`}
                  onclick={() =>
                    (editing = { name: remote.name, field: "url", value: remote.url })}
                >
                  Set URL
                </button>
                {#if remote.push_url}
                  <button
                    class="tb"
                    type="button"
                    aria-label={`Edit push URL for ${remote.name}`}
                    onclick={() =>
                      (editing = {
                        name: remote.name,
                        field: "push_url",
                        value: remote.push_url ?? "",
                      })}
                  >
                    Set push URL
                  </button>
                {/if}
                <button
                  class="tb danger"
                  type="button"
                  aria-label={`Remove remote ${remote.name}`}
                  onclick={() => void onRemove(remote)}
                >
                  Remove
                </button>
              </span>
            </div>
            {#if expandedRemote === remote.name}
              <div class="remote-branches">
                <p class="rb-head">Remote branches</p>
                {#if remoteBranchList.length === 0}
                  <p class="rb-empty">Fetch to discover remote branches.</p>
                {:else}
                  <ul class="rb-list">
                    {#each remoteBranchList.filter((b) => b.remote === remote.name) as rb (rb.remote + "/" + rb.name)}
                      <li class="rb-row">
                        <span class="rb-name" title="{rb.remote}/{rb.name}">
                          {rb.name}
                          {#if rb.tracked_by}<span class="rb-tracked">({rb.tracked_by})</span>{/if}
                        </span>
                        {#if rb.tracked_by === null}
                          <button
                            class="tb"
                            type="button"
                            title="Create a local branch tracking {rb.remote}/{rb.name} and check it out"
                            onclick={() => void onCheckoutRemote(rb)}
                          >
                            Checkout
                          </button>
                        {/if}
                        <!-- M12: diff HEAD against this remote branch inline. -->
                        <button
                          class="tb"
                          type="button"
                          aria-expanded={compareKey === `${rb.remote}/${rb.name}`}
                          title="Diff HEAD against {rb.remote}/{rb.name}"
                          onclick={() => void onCompareHead(rb)}
                        >
                          Compare with HEAD
                        </button>
                        <button
                          class="tb danger"
                          type="button"
                          title="Delete {rb.remote}/{rb.name} on the remote"
                          onclick={() => void onDeleteRemoteBranch(rb)}
                        >
                          Delete
                        </button>
                      </li>
                      {#if compareKey === `${rb.remote}/${rb.name}`}
                        <li class="rb-compare">
                          <div class="rb-compare-head">
                            <span>HEAD ↔ {rb.remote}/{rb.name}
                              {#if compareFiles}({compareFiles.length}
                                file{compareFiles.length === 1 ? "" : "s"}){/if}
                            </span>
                            <button
                              class="tb"
                              type="button"
                              aria-label="Close compare"
                              onclick={() => (compareKey = null)}
                            >×</button>
                          </div>
                          {#if compareLoading}
                            <p class="rb-compare-state">Comparing…</p>
                          {:else if compareFiles !== null && compareFiles.length === 0}
                            <p class="rb-compare-state">No differences.</p>
                          {:else if compareFiles}
                            <div class="rb-compare-diff">
                              <DiffViewer files={compareFiles} />
                            </div>
                          {/if}
                        </li>
                      {/if}
                    {/each}
                  </ul>
                {/if}
              </div>
            {/if}
            <div class="card-actions">
              <div class="action-row">
                <span class="action-label">Fetch</span>
                <div class="action-main">
                  <button
                    class="act"
                    type="button"
                    disabled={netBusy}
                    onclick={() => onToggleRemoteBranches(remote)}
                  >
                    {expandedRemote === remote.name ? "Hide branches" : "Branches"}
                  </button>
                  <button
                    class="act"
                    type="button"
                    disabled={netBusy}
                    onclick={() => onFetch(remote)}
                  >
                    Fetch
                  </button>
                </div>
                <div class="action-opts">
                  <Switch
                    bind:checked={prune}
                    title="Delete remote-tracking branches that vanished upstream"
                  >
                    prune
                  </Switch>
                </div>
              </div>

              <div class="action-row">
                <span class="action-label">Pull</span>
                <div class="action-main">
                  <select
                    aria-label={`Branch for pull and push on ${remote.name}`}
                    bind:value={branchPick[remote.name]}
                  >
                    {#each branchNames as name (name)}
                      <option value={name}>
                        {name}{name === currentBranch ? " (current)" : ""}
                      </option>
                    {/each}
                  </select>
                  <button
                    class="act"
                    type="button"
                    disabled={netBusy || !branchFor(remote)}
                    onclick={() => onPull(remote)}
                  >
                    Pull
                  </button>
                </div>
                <div class="action-opts">
                  <Switch
                    bind:checked={ffOnly}
                    title="Refuse when the merge would not be a fast-forward"
                  >
                    ff-only
                  </Switch>
                  <Switch
                    bind:checked={rebase}
                    title="Rebase local commits on top of the fetched branch"
                  >
                    rebase
                  </Switch>
                </div>
              </div>

              <div class="action-row">
                <span class="action-label">Push</span>
                <div class="action-main">
                  <button
                    class="act push"
                    type="button"
                    disabled={netBusy || !branchFor(remote)}
                    onclick={() => onPush(remote)}
                  >
                    Push
                  </button>
                </div>
                <div class="action-opts">
                  <Switch
                    bind:checked={force}
                    title="Overwrite the remote branch"
                  >
                    force
                  </Switch>
                  <Switch
                    bind:checked={setUpstream}
                    title="Set this branch's upstream to this remote"
                  >
                    set-upstream
                  </Switch>
                </div>
              </div>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <form
    class="add"
    aria-label="Add remote"
    onsubmit={(e) => {
      e.preventDefault();
      void onAdd();
    }}
  >
    <input
      class="name"
      type="text"
      placeholder="name"
      aria-label="New remote name"
      bind:value={addName}
    />
    <input
      class="url-input"
      type="text"
      placeholder="https://host/repo.git"
      aria-label="New remote URL"
      bind:value={addUrl}
    />
    <button class="go" type="submit" disabled={!addName.trim() || !addUrl.trim()}>
      Add remote
    </button>
  </form>
</aside>

<ConfirmDialog
  bind:open={
    () => removeCandidate !== null,
    (v) => {
      if (!v) removeCandidate = null;
    }
  }
  title={removeCandidate ? `Remove remote ${removeCandidate.name}?` : ""}
  message="Tracking branches that follow this remote go with it."
  confirmLabel="Remove remote"
  danger
  onConfirm={() => void onRemoveConfirmed()}
/>

<style>
  .remote-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .autofetch {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  select {
    font: inherit;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.15rem 0.3rem;
    max-width: 12rem;
  }

  select:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .progress {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container-low, var(--m3-surface));
    flex: none;
  }

  .pbar-wrap {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .pmsg {
    flex: none;
    max-width: 45%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  .pbar {
    flex: 1;
    height: 0.375rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    overflow: hidden;
  }

  .pfill {
    display: block;
    height: 100%;
    background: var(--m3-primary);
    transition: width 0.2s ease;
  }

  .pbar.indeterminate .pfill {
    width: 35%;
    animation: slide 1.1s ease-in-out infinite;
  }

  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(300%);
    }
  }

  .remotes {
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
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    flex-wrap: wrap;
  }

  .rname {
    flex: none;
    font-weight: 600;
    color: var(--m3-primary);
  }

  .url {
    flex: 1;
    min-width: 8rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .url.push {
    flex: none;
    max-width: 40%;
  }

  .head-actions {
    margin-left: auto;
    flex: none;
    display: flex;
    gap: 0.25rem;
  }

  .card-actions {
    display: flex;
    flex-direction: column;
    margin-top: 0.375rem;
  }

  /* One row per action: verb label — controls — option switches, with a
     stable switch column so labels line up across rows. */
  .action-row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 0.625rem;
    padding: 0.3rem 0;
  }
  .action-row + .action-row {
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .action-label {
    min-width: 2.75rem;
    font-size: 0.6875rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .action-main {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
    flex-wrap: wrap;
  }
  .action-main select {
    max-width: 9rem;
  }

  .action-opts {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex: none;
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

  .act.push {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .act:disabled {
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

  .go:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .edit-url {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
  }

  .eu-label {
    flex: 1;
    min-width: 10rem;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
  }

  .eu-input {
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

  .eu-input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .add {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex-wrap: wrap;
  }

  .add .name {
    flex: none;
    width: 8rem;
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  .add .url-input {
    flex: 1;
    min-width: 10rem;
    font: inherit;
    font-size: 0.75rem;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    padding: 0.2rem 0.45rem;
  }

  .add .name:focus-visible,
  .add .url-input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
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

  /* M12: inline HEAD ↔ remote-branch compare block. */
  .rb-compare {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.3rem 0.4rem;
  }

  .rb-compare-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    font-size: 0.72rem;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .rb-compare-state {
    margin: 0.25rem 0 0;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .rb-compare-diff {
    margin-top: 0.25rem;
    max-height: 18rem;
    overflow: auto;
  }
</style>
