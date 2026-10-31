<script lang="ts">
  /**
   * CommitDetail — the commit detail pane, shared by HistoryView's
   * list↔detail split and the `?panel=commitdetail` popout window.
   *
   * Owns everything selection-independent about one commit's detail: the
   * lazy parent0→sha diff fetch (token-guarded), `git describe`, the blame
   * tab, and the meta↔files SplitPane (ratio persisted as `history-meta`,
   * the same pref the inline pane always used). Everything that needs the
   * surrounding view — closing, parent navigation, cherry-pick/revert/
   * rebase, popping out — arrives as optional callbacks, so the popout can
   * wire single-commit versions and omit the rest (close button, bookmark
   * star when `root` is empty).
   *
   * Palette support: `history-toggle-blame` needs the loaded file list,
   * which is private — the exported `toggleBlameFirstFile()` reimplements
   * the old HistoryView handler and reports whether it did anything.
   */
  import { repoDiff, describe } from "$lib/ipc/client";
  import type { CommitInfo, FileDiff } from "$lib/ipc/types";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import BlameView from "$lib/components/panels/BlameView.svelte";
  import type { CompareResult } from "$lib/components/panels/compare";
  import { openPanelPopout } from "$lib/layout/popout";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import SignatureBadge from "$lib/components/commit/SignatureBadge.svelte";
  import { bookmarks as bookmarkStore } from "$lib/components/graph/bookmarks.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import { Button } from "$lib/components/ui/button/index.js";
  import {
    classifyRef,
    formatDateTime,
    formatRelativeTime,
    shortRefName,
  } from "$lib/git/format";
  import { toast } from "$lib/toast";

  let {
    repoId,
    info,
    compare = null,
    root = "",
    /** Multi-selection size for the action labels (popout: 1). */
    actionCount = 1,
    /** True while a cherry-pick/revert IPC is in flight (owner's state). */
    actionBusy = false,
    onClose,
    onCloseCompare,
    onPopout,
    onSelectParent,
    onCherryPick,
    onRevert,
    onRebaseFromHere,
  }: {
    repoId: string;
    info: CommitInfo | null;
    /** CompareBar results render in the same body (HistoryView only). */
    compare?: CompareResult | null;
    /** Worktree root (bookmark persistence key); empty hides the star. */
    root?: string;
    actionCount?: number;
    actionBusy?: boolean;
    onClose?: () => void;
    onCloseCompare?: () => void;
    onPopout?: () => void;
    onSelectParent?: (sha: string) => void;
    onCherryPick?: () => void;
    onRevert?: () => void;
    onRebaseFromHere?: () => void;
  } = $props();

  // -- diff for the shown commit (lazy, token-guarded) -----------------------

  let detailFiles = $state<FileDiff[] | null>(null);
  let detailLoading = $state(false);
  let diffToken = 0;
  let blameTarget = $state<{ path: string; from: string } | null>(null);

  $effect(() => {
    const current = info;
    void repoId; // reload on repo switch
    blameTarget = null;
    const token = ++diffToken;
    if (!current || current.parents.length === 0) {
      detailFiles = null;
      detailLoading = false;
      return;
    }
    detailLoading = true;
    detailFiles = null;
    repoDiff(repoId, { commit: current.parents[0] as string }, { commit: current.sha })
      .then((files) => {
        if (token !== diffToken) return;
        detailFiles = files;
        detailLoading = false;
      })
      .catch((err: unknown) => {
        if (token !== diffToken) return;
        detailLoading = false;
        detailFiles = null;
        toast(`Diff failed: ${err instanceof Error ? err.message : String(err)}`, {
          kind: "error",
        });
      });
  });

  // -- describe line (decorative) ---------------------------------------------

  let describeText = $state<string | null>(null);
  let describeToken = 0;

  $effect(() => {
    const sha = info?.sha;
    void repoId;
    describeText = null;
    if (!sha || !info) return;
    const token = ++describeToken;
    describe(repoId, sha)
      .then((text) => {
        if (token === describeToken) describeText = text;
      })
      .catch(() => {
        /* describe is decorative */
      });
  });

  // Compare results take over the files pane — drop any blame tab.
  $effect(() => {
    if (compare) blameTarget = null;
  });

  function openBlame(path: string): void {
    const sha = info?.sha;
    if (!sha) return;
    blameTarget = { path, from: sha };
  }

  /**
   * Palette `history-toggle-blame`: closes an open blame tab, else blames
   * the first changed file. Returns false when there is nothing to blame
   * (the caller toasts the explanatory message).
   */
  export function toggleBlameFirstFile(): boolean {
    if (blameTarget) {
      blameTarget = null;
      return true;
    }
    if (info && detailFiles && detailFiles.length > 0) {
      openBlame(detailFiles[0]!.path);
      return true;
    }
    return false;
  }

  // -- resizable panes (meta ↔ files, persisted as a global pref) ------------

  const META_RATIO_KEY = "history-meta";
  let metaRatio = $state(readSplitRatio(META_RATIO_KEY, 0.34));

  function setMetaRatio(ratio: number): void {
    metaRatio = ratio;
    writeSplitRatio(META_RATIO_KEY, ratio);
  }

  // -- M12: bookmark star on the detail header --------------------------------

  /** Whether the shown commit is bookmarked (drives the star toggle). */
  const bookmarked = $derived(
    root !== "" && info !== null && bookmarkStore.has(root, info.sha),
  );

  // Bookmark state is lazy per root — hydrate outside derived reads.
  $effect(() => {
    if (root !== "") bookmarkStore.ensure(root);
  });

  function toggleBookmark(sha: string): void {
    if (root === "") return;
    bookmarkStore.ensure(root);
    const added = bookmarkStore.toggle(root, sha);
    toast(added ? "Commit bookmarked" : "Bookmark removed", {
      kind: "success",
    });
  }

  // -- actions ------------------------------------------------------------------

  async function copySha(): Promise<void> {
    if (!info) return;
    try {
      await navigator.clipboard.writeText(info.sha);
      toast("SHA copied to clipboard", { kind: "success" });
    } catch {
      toast("Could not copy the SHA", { kind: "error" });
    }
  }

  /** Changed-file right-click: per-file actions. */
  function fileMenu(event: MouseEvent, path: string): void {
    const entries: MenuEntry[] = [
      { id: "history", label: "File history", run: () => void openPanelPopout("filehistory", repoId, `History: ${path}`, { path }) },
      { id: "blame", label: "Blame", run: () => openBlame(path) },
    ];
    showMenuAt(event, entries);
  }
</script>

<div class="detail flex min-h-0 flex-col bg-card" aria-label="Commit detail">
  <header class="dhead flex items-center gap-2 border-b border-border bg-popover px-3 py-1 text-[0.8125rem]">
    {#if info}
      <span class="dsha font-mono text-primary">{info.sha.slice(0, 7)}</span>
      <span class="dsummary min-w-0 flex-1 truncate font-medium text-foreground" title={info.summary}>{info.summary}</span>
      {#if root !== ""}
        <!-- M12: bookmark star (same store mutation as the menu item). -->
        <Button
          class="h-auto flex-none rounded-sm px-[0.45rem] py-[0.05rem] text-[0.8125rem] leading-[1.3] font-normal {bookmarked
            ? 'border-primary bg-transparent text-primary'
            : 'border-border bg-transparent text-muted-foreground'}"
          variant="outline"
          aria-pressed={bookmarked}
          aria-label={bookmarked
            ? "Remove bookmark from this commit"
            : "Bookmark this commit"}
          title={bookmarked ? "Remove bookmark" : "Bookmark commit"}
          onclick={() => {
            // Snippet narrowing doesn't reach the callback closure.
            const sha = info?.sha;
            if (sha) toggleBookmark(sha);
          }}
        >
          {bookmarked ? "★" : "☆"}
        </Button>
      {/if}
      {#if onPopout}
        <Button
          class="h-auto rounded-sm px-[0.4rem] py-[0.1rem] text-[0.72rem] font-normal text-muted-foreground"
          variant="outline"
          title="Open this commit's detail in its own window"
          onclick={onPopout}
        >
          Pop out
        </Button>
      {/if}
    {:else}
      <span class="dsha font-mono text-primary">diff</span>
      <span class="dsummary min-w-0 flex-1 truncate font-medium text-foreground">{compare?.base} → {compare?.target}</span>
    {/if}
    {#if onClose}
      <Button class="h-auto rounded-sm px-[0.4rem] py-[0.1rem] text-[0.72rem] font-normal text-muted-foreground" variant="outline" onclick={onClose} aria-label="Close detail">
        ×
      </Button>
    {/if}
  </header>
  {#if info}
    <div class="cactions flex flex-none flex-wrap items-center gap-1.5 border-b border-border bg-muted/50 px-3 py-1 text-[0.72rem]" role="toolbar" aria-label="Commit actions">
      {#if onCherryPick}
        <Button class="h-auto rounded-full px-2.5 py-[0.18rem] text-[0.72rem] font-normal" variant="secondary" onclick={onCherryPick} disabled={actionBusy}>
          Cherry-pick{actionCount > 1 ? ` (${actionCount})` : ""}
        </Button>
      {/if}
      {#if onRevert}
        <Button class="h-auto rounded-full px-2.5 py-[0.18rem] text-[0.72rem] font-normal" variant="secondary" onclick={onRevert} disabled={actionBusy}>
          Revert{actionCount > 1 ? ` (${actionCount})` : ""}
        </Button>
      {/if}
      {#if onRebaseFromHere}
        <Button
          class="h-auto rounded-full px-2.5 py-[0.18rem] text-[0.72rem] font-normal"
          variant="secondary"
          onclick={onRebaseFromHere}
          disabled={actionBusy}
          title={`Rebase HEAD..${info.sha.slice(0, 7)} (selected commit is the exclusive base)`}
        >
          Rebase from here
        </Button>
      {/if}
      <Button class="h-auto rounded-full px-2.5 py-[0.18rem] text-[0.72rem] font-normal" variant="secondary" onclick={copySha}>Copy sha</Button>
      <span class="cinfo ml-auto text-[0.6875rem] text-muted-foreground" role="status">
        {#if actionCount > 1}
          {actionCount} commits selected (shift-click to extend)
        {/if}
      </span>
    </div>
  {/if}
  <div class="dbody flex min-h-0 flex-1 [&>*]:min-w-0 [&>*]:flex-1">
    {#snippet filesPane()}
    <div class="dfiles flex min-w-0 flex-1 flex-col">
      {#if blameTarget}
        <div class="dtab flex flex-none items-center gap-2 border-b border-border bg-muted py-0.5 pr-2 pl-3 text-[0.72rem] text-foreground">
          <span class="min-w-0 flex-1 truncate font-mono">Blame: {blameTarget.path}</span>
          <Button
            class="h-auto rounded-sm px-[0.4rem] py-[0.1rem] text-[0.72rem] font-normal text-muted-foreground"
            variant="outline"
            onclick={() => (blameTarget = null)}
            aria-label="Close blame">×
          </Button>
        </div>
        <div class="dtabbody flex min-h-0 flex-1 flex-col">
          <BlameView {repoId} path={blameTarget.path} from={blameTarget.from} />
        </div>
      {:else if compare}
        <div class="dtab flex flex-none items-center gap-2 border-b border-border bg-muted py-0.5 pr-2 pl-3 text-[0.72rem] text-foreground">
          <span class="min-w-0 flex-1 truncate font-mono">Compare: {compare.base} → {compare.target} ({compare.files.length} files)</span>
          {#if onCloseCompare}
            <Button class="h-auto rounded-sm px-[0.4rem] py-[0.1rem] text-[0.72rem] font-normal text-muted-foreground" variant="outline" onclick={onCloseCompare} aria-label="Close compare">×</Button>
          {/if}
        </div>
        <div class="dtabbody flex min-h-0 flex-1 flex-col">
          <DiffViewer files={compare.files} />
        </div>
      {:else if detailLoading}
        <p class="dstate m-auto text-[0.8125rem] text-muted-foreground" role="status">Loading diff…</p>
      {:else if !info || info.parents.length === 0}
        <p class="dstate m-auto text-[0.8125rem] text-muted-foreground">Root commit — nothing to diff against.</p>
      {:else if detailFiles === null}
        <p class="dstate m-auto text-[0.8125rem] text-muted-foreground">Diff unavailable.</p>
      {:else if detailFiles.length === 0}
        <p class="dstate m-auto text-[0.8125rem] text-muted-foreground">No changes against first parent.</p>
      {:else}
        <ul class="filelist m-0 max-h-[30%] flex-none list-none overflow-y-auto border-b border-border py-0.5 text-[0.72rem]" aria-label="Changed files">
          {#each detailFiles as file (file.path)}
            <li
              class="flex items-center gap-2 px-3 py-0.5"
              oncontextmenu={(e) => fileMenu(e, file.path)}
            >
              <span class="fpath min-w-0 flex-1 truncate font-mono text-foreground" title={file.path}>
                {#if file.old_path}{file.old_path} → {/if}{file.path}
              </span>
              <span class="stats flex-none font-mono">
                {#if !file.binary}
                  <span class="add text-[color:var(--app-added)]">+{file.additions}</span>
                  <span class="del text-[color:var(--app-deleted)]">−{file.deletions}</span>
                {:else}
                  <span class="muted text-muted-foreground">binary</span>
                {/if}
              </span>
              <Button class="h-auto rounded-sm px-[0.4rem] py-[0.1rem] text-[0.72rem] font-normal text-muted-foreground" variant="outline" onclick={() => openBlame(file.path)}>Blame</Button>
              <Button
                class="h-auto rounded-sm px-[0.4rem] py-[0.1rem] text-[0.72rem] font-normal text-muted-foreground"
                variant="outline"
                title="History of {file.path} (popout)"
                onclick={() =>
                  void openPanelPopout("filehistory", repoId, `History: ${file.path}`, { path: file.path })}
              >History</Button>
            </li>
          {/each}
        </ul>
        <div class="ddiff min-h-0 flex-1 overflow-auto">
          <DiffViewer files={detailFiles} />
        </div>
      {/if}
    </div>
    {/snippet}

    {#if info}
      <!-- Meta ↔ files/diff: the detail's second resizable split. -->
      <SplitPane
        axis="x"
        ratio={metaRatio}
        onRatio={setMetaRatio}
        label="Resize commit metadata and diff"
      >
        {#snippet a()}
          <!-- Snippet bodies don't inherit the outer {#if info}
               narrowing — re-guard here (renders only while active). -->
          {#if info}
        <aside class="dmeta min-w-0 flex-1 overflow-y-auto px-3 py-2 text-xs">
            <dl class="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
              <dt class="pt-0.5 text-[0.6875rem] uppercase tracking-[0.02em] text-muted-foreground">Author</dt>
              <dd class="m-0 text-foreground">
                {info.author.name}
                <span class="muted text-muted-foreground">&lt;{info.author.email}&gt;</span><br />
                {formatDateTime(info.author.time)}
                <span class="muted text-muted-foreground">({formatRelativeTime(info.author.time)})</span>
              </dd>
              <dt class="pt-0.5 text-[0.6875rem] uppercase tracking-[0.02em] text-muted-foreground">Committer</dt>
              <dd class="m-0 text-foreground">
                {info.committer.name}<br />
                {formatDateTime(info.committer.time)}
                <span class="muted text-muted-foreground">({formatRelativeTime(info.committer.time)})</span>
              </dd>
              <dt class="pt-0.5 text-[0.6875rem] uppercase tracking-[0.02em] text-muted-foreground">Parents</dt>
              <dd class="m-0 text-foreground">
                {#if info.parents.length === 0}
                  <em>root</em>
                {:else}
                  {#each info.parents as p (p)}
                    <Button
                      class="mr-1 h-auto rounded-sm px-[0.3rem] py-[0.05rem] font-mono text-[0.6875rem] font-normal"
                      variant="outline"
                      onclick={() => onSelectParent?.(p)}
                      disabled={!onSelectParent}
                      title={"Go to " + p}
                    >
                      {p.slice(0, 7)}
                    </Button>
                  {/each}
                {/if}
              </dd>
              <dt class="pt-0.5 text-[0.6875rem] uppercase tracking-[0.02em] text-muted-foreground">Describe</dt>
              <dd class="m-0 text-foreground">
                {#if describeText}
                  <code>{describeText}</code>
                {:else}
                  <span class="muted text-muted-foreground">…</span>
                {/if}
              </dd>
              <!-- M12: commit-signature verification badge. -->
              <dt class="pt-0.5 text-[0.6875rem] uppercase tracking-[0.02em] text-muted-foreground">Signature</dt>
              <dd class="m-0 text-foreground"><SignatureBadge {repoId} sha={info.sha} /></dd>
              {#if info.refs.length > 0}
                <dt class="pt-0.5 text-[0.6875rem] uppercase tracking-[0.02em] text-muted-foreground">Refs</dt>
                <dd class="drefs m-0 flex flex-wrap gap-1 text-foreground">
                  {#each info.refs as ref (ref)}
                    {@const kind = classifyRef(ref)}
                    <span class="ref ref-{kind} inline-flex max-w-36 items-center overflow-hidden whitespace-nowrap rounded-sm border px-[0.35rem] text-[0.6875rem] leading-[1.4] text-ellipsis {kind === "local"
                      ? "border-transparent bg-primary/10 text-primary"
                      : kind === "remote"
                        ? "border-border text-muted-foreground"
                        : kind === "tag"
                          ? "border-transparent bg-accent text-accent-foreground"
                          : "border-primary bg-primary/10 text-primary"}">{shortRefName(ref)}</span>
                  {/each}
                </dd>
              {/if}
            </dl>
            <pre class="msg mt-2 whitespace-pre-wrap rounded-sm border border-border bg-muted p-2 font-mono text-[0.72rem] text-foreground [overflow-wrap:anywhere]">{info.message}</pre>
          </aside>
          {/if}
            {/snippet}
            {#snippet b()}
              {@render filesPane()}
            {/snippet}
          </SplitPane>
        {:else}
          {@render filesPane()}
        {/if}
  </div>
</div>
