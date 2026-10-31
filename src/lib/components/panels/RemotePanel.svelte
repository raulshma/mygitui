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
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import { Progress } from "$lib/components/ui/progress";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
  import iconSetUrl from "@ktibow/iconset-material-symbols/link";
  import iconSetPushUrl from "@ktibow/iconset-material-symbols/upload";
  import iconRemove from "@ktibow/iconset-material-symbols/delete";
  import iconCheck from "@ktibow/iconset-material-symbols/check";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconBranches from "@ktibow/iconset-material-symbols/account-tree";
  import iconFetch from "@ktibow/iconset-material-symbols/cloud-download";
  import iconPull from "@ktibow/iconset-material-symbols/arrow-downward";
  import iconPush from "@ktibow/iconset-material-symbols/arrow-upward";
  import iconCheckout from "@ktibow/iconset-material-symbols/fork-right";
  import iconCompare from "@ktibow/iconset-material-symbols/compare-arrows";
  import iconAdd from "@ktibow/iconset-material-symbols/add";

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

  // -- Repo-level sync bar (UI review P2 #11) ---------------------------------
  //
  // The common safe actions ride on the primary remote ("origin" when
  // present) and the current branch with default options, so the panel leads
  // with Fetch / Pull / Push instead of a wall of switches. The per-remote
  // cards below keep the full control set; their option switches live under
  // an Advanced disclosure.

  /** This repo's live status (ahead/behind counts on the bar buttons). */
  const repoStatus = $derived(
    tabStore.tabs.find((tab) => tab.id === repoId)?.status ?? null,
  );
  const ahead = $derived(repoStatus?.ahead ?? 0);
  const behind = $derived(repoStatus?.behind ?? 0);

  /** The remote the bar acts on (origin preferred, else the first). */
  const primaryRemote = $derived<RemoteInfo | null>(
    list.find((r) => r.name === "origin") ?? list[0] ?? null,
  );

  function onBarFetch(): void {
    if (primaryRemote) onFetch(primaryRemote);
  }

  /** Safe pull: current branch, fast-forward only, no rebase. */
  function onBarPull(): void {
    const remote = primaryRemote;
    if (!remote || !currentBranch) return;
    void runNet(
      "pull",
      remote,
      () =>
        pullRepo(repoId, {
          remote: remote.name,
          branch: currentBranch,
          ff_only: true,
          rebase: false,
        }),
      true,
    );
  }

  /** Safe push: current branch, no force, no upstream change. */
  function onBarPush(): void {
    const remote = primaryRemote;
    if (!remote || !currentBranch) return;
    void runNet(
      "push",
      remote,
      () =>
        pushRepo(repoId, {
          remote: remote.name,
          branch: currentBranch,
          force: false,
          force_with_lease: false,
          set_upstream: false,
          refs: [],
          tags: false,
          delete: false,
        }),
      true,
    );
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

<aside class="remote-panel flex flex-col min-h-0 text-[13px]" aria-label="Remotes">
  <div class="toolbar flex shrink-0 items-center gap-3 px-2 py-1.5">
    <Button
      class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh remotes"
      title={loading ? "Refreshing remotes…" : "Refresh remotes"}
      disabled={loading}
      onclick={() => void reload()}
    >
      <Icon icon={iconRefresh} size={16} />
    </Button>
    <label
      class="autofetch ml-auto flex items-center gap-1.5 text-xs text-muted-foreground"
      title="Fetch the default remote in the background while this repo is open"
    >
      <span>auto-fetch</span>
      <Select.Root
        type="single"
        value={String(autofetch.getInterval(repoId))}
        onValueChange={(value) => autofetch.setInterval(repoId, Number(value))}
      >
        <Select.Trigger
          class="max-w-[12rem] h-[1.5rem] rounded border-border bg-background dark:bg-background px-1.5 py-0 text-xs text-foreground gap-1"
          aria-label="Auto-fetch interval in minutes"
        >
          <Select.Value />
        </Select.Trigger>
        <Select.Content>
          {#each INTERVALS as minutes (minutes)}
            <Select.Item value={String(minutes)} label={minutes === 0 ? "off" : `${minutes} min`} class="text-xs" />
          {/each}
        </Select.Content>
      </Select.Root>
    </label>
  </div>

  {#if primaryRemote}
    <div
      class="sync-bar flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border px-2 py-1.5"
      role="group"
      aria-label={`Repository sync with ${primaryRemote.name}`}
    >
      <span
        class="sync-remote mr-auto max-w-[10rem] shrink-0 overflow-hidden text-ellipsis whitespace-nowrap text-xs font-semibold text-muted-foreground"
        title={`Primary remote: ${primaryRemote.url}`}
      >
        {primaryRemote.name}
      </span>
      <Button
        variant="ghost"
        class="sync-act min-h-7 gap-1.5 rounded-full border border-border bg-transparent px-2.5 text-xs text-foreground hover:bg-accent hover:text-foreground dark:hover:bg-accent"
        type="button"
        disabled={netBusy || !currentBranch}
        title={currentBranch
          ? `Pull ${currentBranch} from ${primaryRemote.name} (fast-forward only)`
          : "Pull is unavailable without a current branch"}
        onclick={onBarPull}
      >
        {#if behind > 0}<Badge variant="secondary" class="sync-count behind rounded-full bg-muted px-1.5 h-auto py-0 text-[10px] font-semibold leading-tight tabular-nums text-muted-foreground">↓ {behind}</Badge>{/if}
        Pull
      </Button>
      <Button
        variant="default"
        class="sync-act filled min-h-7 gap-1.5 rounded-full px-2.5 text-xs hover:bg-primary/90"
        type="button"
        disabled={netBusy || !currentBranch}
        title={currentBranch
          ? `Push ${currentBranch} to ${primaryRemote.name}`
          : "Push is unavailable without a current branch"}
        onclick={onBarPush}
      >
        {#if ahead > 0}<Badge variant="secondary" class="sync-count ahead rounded-full bg-primary-foreground px-1.5 h-auto py-0 text-[10px] font-semibold leading-tight tabular-nums text-primary">↑ {ahead}</Badge>{/if}
        Push
      </Button>
      <Button
        variant="ghost"
        class="sync-act min-h-7 gap-1.5 rounded-full border border-border bg-transparent px-2.5 text-xs text-foreground hover:bg-accent hover:text-foreground dark:hover:bg-accent"
        type="button"
        disabled={netBusy}
        title={`Fetch from ${primaryRemote.name} (prune)`}
        onclick={onBarFetch}
      >
        Fetch
      </Button>
    </div>
  {/if}

  {#if netOps.length > 0}
    <div class="progress flex shrink-0 flex-col gap-1 border-b border-border bg-muted/50 px-2 py-1" role="status" aria-live="polite">
      {#each netOps as op (op.op_id)}
        <div class="pbar-wrap flex items-center gap-2">
          <span class="pmsg max-w-[45%] shrink-0 overflow-hidden text-ellipsis whitespace-nowrap text-xs text-muted-foreground">{op.message}</span>
          {#if op.pct !== null}
            <Progress
              class="pbar h-1.5 flex-1"
              value={Math.round(Math.max(0, Math.min(100, op.pct)))}
              aria-valuenow={Math.round(op.pct)}
              aria-valuemin={0}
              aria-valuemax={100}
            />
          {:else}
            <span class="pbar indeterminate h-1.5 flex-1 overflow-hidden rounded-full bg-muted" role="progressbar" aria-label="In progress">
              <span class="pfill block h-full w-[35%] animate-[slide-wide_1.1s_ease-in-out_infinite] bg-primary"></span>
            </span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if error}
    <p class="state error m-0 px-3 py-4 text-[13px] text-destructive" role="alert">{error}</p>
  {:else if !loading && list.length === 0}
    <p class="state m-0 px-3 py-4 text-[13px] text-muted-foreground">No remotes configured.</p>
  {:else}
    <ul class="remotes m-0 flex min-h-0 flex-1 list-none flex-col gap-2 overflow-y-auto px-2 py-1.5" role="list" aria-label="Configured remotes">
      {#each list as remote (remote.name)}
        <li class="card rounded-lg border border-border bg-card px-2 py-1.5">
          {#if editing?.name === remote.name}
            <form
              class="edit-url flex flex-wrap items-center gap-1.5"
              aria-label={`Set ${editing.field} for ${remote.name}`}
              onsubmit={(e) => {
                e.preventDefault();
                void onSetUrl();
              }}
            >
              <label class="eu-label flex min-w-40 flex-1 items-center gap-1.5 text-[0.72rem] text-muted-foreground">
                {editing.field === "url" ? "fetch URL" : "push URL"}:
                <Input
                  class="eu-input h-auto min-w-32 flex-1 rounded bg-background dark:bg-background px-1.5 py-0.5 font-mono text-[0.72rem] md:text-[0.72rem] text-foreground"
                  type="text"
                  aria-label={`${editing.field} for ${remote.name}`}
                  bind:value={editing.value}
                  onkeydown={(e) => {
                    if (e.key === "Escape") editing = null;
                  }}
                />
              </label>
              <Button
                variant="default"
                size="icon-sm"
                type="submit"
                aria-label="Save remote URL"
                title="Save URL"
              >
                <Icon icon={iconCheck} size={16} />
              </Button>
              <Button
                class="border-border text-muted-foreground hover:border-primary hover:bg-accent hover:text-accent-foreground"
                variant="ghost"
                size="icon-sm"
                type="button"
                aria-label="Cancel URL edit"
                title="Cancel"
                onclick={() => (editing = null)}
              >
                <Icon icon={iconClose} size={16} />
              </Button>
            </form>
          {:else}
            <div class="card-head flex min-w-0 flex-wrap items-center gap-2">
              <span class="rname shrink-0 overflow-hidden text-ellipsis whitespace-nowrap font-semibold text-primary" title={remote.name}>{remote.name}</span>
              <span class="url min-w-32 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.6875rem] text-muted-foreground" title={remote.url}>{remote.url}</span>
              {#if remote.push_url}
                <span class="url push max-w-[40%] shrink-0 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.6875rem] text-muted-foreground" title={remote.push_url}>push: {remote.push_url}</span>
              {/if}
              <span class="head-actions ml-auto flex shrink-0 gap-1">
                <Button
                  class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={`Edit fetch URL for ${remote.name}`}
                  title="Set the fetch URL"
                  onclick={() =>
                    (editing = { name: remote.name, field: "url", value: remote.url })}
                >
                  <Icon icon={iconSetUrl} size={16} />
                </Button>
                {#if remote.push_url}
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Edit push URL for ${remote.name}`}
                    title="Set the push URL"
                    onclick={() =>
                      (editing = {
                        name: remote.name,
                        field: "push_url",
                        value: remote.push_url ?? "",
                      })}
                  >
                    <Icon icon={iconSetPushUrl} size={16} />
                  </Button>
                {/if}
                <Button
                  class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={`Remove remote ${remote.name}`}
                  title="Remove this remote"
                  onclick={() => void onRemove(remote)}
                >
                  <Icon icon={iconRemove} size={16} />
                </Button>
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
                          <Button
                            class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                            variant="ghost"
                            size="icon-xs"
                            type="button"
                            aria-label={`Check out ${rb.remote}/${rb.name}`}
                            title="Create a local branch tracking {rb.remote}/{rb.name} and check it out"
                            onclick={() => void onCheckoutRemote(rb)}
                          >
                            <Icon icon={iconCheckout} size={16} />
                          </Button>
                        {/if}
                        <!-- M12: diff HEAD against this remote branch inline. -->
                        <Button
                          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                          variant="ghost"
                          size="icon-xs"
                          type="button"
                          aria-label={`Compare HEAD with ${rb.remote}/${rb.name}`}
                          aria-pressed={compareKey === `${rb.remote}/${rb.name}`}
                          aria-expanded={compareKey === `${rb.remote}/${rb.name}`}
                          title="Diff HEAD against {rb.remote}/{rb.name}"
                          onclick={() => void onCompareHead(rb)}
                        >
                          <Icon icon={iconCompare} size={16} />
                        </Button>
                        <Button
                          class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                          variant="ghost"
                          size="icon-xs"
                          type="button"
                          aria-label={`Delete remote branch ${rb.remote}/${rb.name}`}
                          title="Delete {rb.remote}/{rb.name} on the remote"
                          onclick={() => void onDeleteRemoteBranch(rb)}
                        >
                          <Icon icon={iconRemove} size={16} />
                        </Button>
                      </li>
                      {#if compareKey === `${rb.remote}/${rb.name}`}
                        <li class="rb-compare rounded-lg border border-border px-1.5 py-1">
                          <div class="rb-compare-head flex items-center justify-between gap-2 font-mono text-[0.72rem] text-muted-foreground">
                            <span>HEAD ↔ {rb.remote}/{rb.name}
                              {#if compareFiles}({compareFiles.length}
                                file{compareFiles.length === 1 ? "" : "s"}){/if}
                            </span>
                            <Button
                              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                              variant="ghost"
                              size="icon-xs"
                              type="button"
                              aria-label="Close compare"
                              title="Close compare"
                              onclick={() => (compareKey = null)}
                            >
                              <Icon icon={iconClose} size={16} />
                            </Button>
                          </div>
                          {#if compareLoading}
                            <p class="rb-compare-state mb-0 mt-1 text-[0.72rem] text-muted-foreground">Comparing…</p>
                          {:else if compareFiles !== null && compareFiles.length === 0}
                            <p class="rb-compare-state mb-0 mt-1 text-[0.72rem] text-muted-foreground">No differences.</p>
                          {:else if compareFiles}
                            <div class="rb-compare-diff mt-1 max-h-72 overflow-auto">
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
            <div class="card-actions mt-1.5 flex flex-col">
              <div class="action-row grid grid-cols-[auto_1fr] items-center gap-2.5 py-[0.3rem]">
                <span class="action-label min-w-11 text-[0.6875rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground">Fetch</span>
                <div class="action-main flex min-w-0 flex-wrap items-center gap-1.5">
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={expandedRemote === remote.name ? `Hide branches for ${remote.name}` : `Show branches for ${remote.name}`}
                    aria-pressed={expandedRemote === remote.name}
                    title={expandedRemote === remote.name ? "Hide remote branches" : "Show remote branches"}
                    disabled={netBusy}
                    onclick={() => onToggleRemoteBranches(remote)}
                  >
                    <Icon icon={iconBranches} size={16} />
                  </Button>
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Fetch from ${remote.name}`}
                    title="Fetch from this remote"
                    disabled={netBusy}
                    onclick={() => onFetch(remote)}
                  >
                    <Icon icon={iconFetch} size={16} />
                  </Button>
                </div>
              </div>

              <div class="action-row grid grid-cols-[auto_1fr] items-center gap-2.5 border-t border-border py-[0.3rem]">
                <span class="action-label min-w-11 text-[0.6875rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground">Pull</span>
                <div class="action-main flex min-w-0 flex-wrap items-center gap-1.5">
                  <Select.Root
                    type="single"
                    value={branchPick[remote.name] ?? branchNames[0] ?? ""}
                    onValueChange={(value) => (branchPick[remote.name] = value)}
                  >
                    <Select.Trigger
                      class="max-w-36 h-[1.35rem] rounded bg-background dark:bg-background px-2 py-0 text-[0.72rem] text-foreground gap-1"
                      aria-label={`Branch for pull and push on ${remote.name}`}
                    >
                      <Select.Value />
                    </Select.Trigger>
                    <Select.Content>
                      {#each branchNames as name (name)}
                        <Select.Item
                          value={name}
                          label={`${name}${name === currentBranch ? " (current)" : ""}`}
                          class="text-[0.72rem]"
                        />
                      {/each}
                    </Select.Content>
                  </Select.Root>
                  <Button
                    class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label={`Pull ${branchFor(remote) ?? ""} from ${remote.name}`}
                    title="Pull the selected branch from this remote"
                    disabled={netBusy || !branchFor(remote)}
                    onclick={() => onPull(remote)}
                  >
                    <Icon icon={iconPull} size={16} />
                  </Button>
                </div>
              </div>

              <div class="action-row grid grid-cols-[auto_1fr] items-center gap-2.5 border-t border-border py-[0.3rem]">
                <span class="action-label min-w-11 text-[0.6875rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground">Push</span>
                <div class="action-main flex min-w-0 flex-wrap items-center gap-1.5">
                  <Button
                    class="relative after:absolute after:-inset-1 after:content-[''] [&_svg:not([class*='size-'])]:size-4"
                    variant="default"
                    size="icon-xs"
                    type="button"
                    aria-label={`Push ${branchFor(remote) ?? ""} to ${remote.name}`}
                    title="Push the selected branch to this remote"
                    disabled={netBusy || !branchFor(remote)}
                    onclick={() => onPush(remote)}
                  >
                    <Icon icon={iconPush} size={16} />
                  </Button>
                </div>
              </div>

              <details class="adv mt-1 text-[0.72rem]">
                <summary class="cursor-pointer select-none py-0.5 text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-1">Advanced options</summary>
                <div class="adv-opts flex flex-col gap-1 pb-1.5 pt-1">
                  <label
                    class="inline-flex cursor-pointer select-none items-center gap-1.5 whitespace-nowrap text-sm"
                    title="Fetch: delete remote-tracking branches that vanished upstream"
                  >
                    <Switch bind:checked={prune} />
                    <span class="text-muted-foreground text-xs">prune on fetch</span>
                  </label>
                  <label
                    class="inline-flex cursor-pointer select-none items-center gap-1.5 whitespace-nowrap text-sm"
                    title="Pull: refuse when the merge would not be a fast-forward"
                  >
                    <Switch bind:checked={ffOnly} />
                    <span class="text-muted-foreground text-xs">ff-only on pull</span>
                  </label>
                  <label
                    class="inline-flex cursor-pointer select-none items-center gap-1.5 whitespace-nowrap text-sm"
                    title="Pull: rebase local commits on top of the fetched branch"
                  >
                    <Switch bind:checked={rebase} />
                    <span class="text-muted-foreground text-xs">rebase on pull</span>
                  </label>
                  <label
                    class="inline-flex cursor-pointer select-none items-center gap-1.5 whitespace-nowrap text-sm"
                    title="Push: overwrite the remote branch"
                  >
                    <Switch bind:checked={force} />
                    <span class="text-muted-foreground text-xs">force on push</span>
                  </label>
                  <label
                    class="inline-flex cursor-pointer select-none items-center gap-1.5 whitespace-nowrap text-sm"
                    title="Push: set this branch's upstream to this remote"
                  >
                    <Switch bind:checked={setUpstream} />
                    <span class="text-muted-foreground text-xs">set-upstream on push</span>
                  </label>
                </div>
              </details>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <form
    class="add flex flex-none flex-wrap items-center gap-1.5 border-t border-border px-2 py-1.5"
    aria-label="Add remote"
    onsubmit={(e) => {
      e.preventDefault();
      void onAdd();
    }}
  >
    <Input
      class="name h-auto w-32 shrink-0 rounded bg-background dark:bg-background px-2 py-0.5 text-xs md:text-xs text-foreground"
      type="text"
      placeholder="name"
      aria-label="New remote name"
      bind:value={addName}
    />
    <Input
      class="url-input h-auto min-w-40 flex-1 rounded bg-background dark:bg-background px-2 py-0.5 font-mono text-xs md:text-xs text-foreground"
      type="text"
      placeholder="https://host/repo.git"
      aria-label="New remote URL"
      bind:value={addUrl}
    />
    <Button
      variant="default"
      size="icon-sm"
      type="submit"
      aria-label="Add remote"
      title="Add a new remote"
      disabled={!addName.trim() || !addUrl.trim()}
    >
      <Icon icon={iconAdd} size={16} />
    </Button>
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
