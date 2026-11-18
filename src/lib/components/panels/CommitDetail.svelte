<script lang="ts">
  /**
   * CommitDetail — the commit detail pane, shared by HistoryView's
   * list↔detail split and the `?panel=commitdetail` popout window.
   *
   * Owns everything selection-independent about one commit's detail: the
   * parent0→sha diff stream (token-guarded, per-consumer stream slot),
   * `git describe`, the full-message fetch, the blame tab, and the
   * meta↔files SplitPane (ratio persisted as `history-meta`, the same pref
   * the inline pane always used). Everything that needs the
   * surrounding view — closing, parent navigation, cherry-pick/revert/
   * rebase, popping out — arrives as optional callbacks, so the popout can
   * wire single-commit versions and omit the rest (close button, bookmark
   * star when `root` is empty).
   *
   * Palette support: `history-toggle-blame` needs the loaded file list,
   * which is private — the exported `toggleBlameFirstFile()` reimplements
   * the old HistoryView handler and reports whether it did anything.
   */
  import { streamDiff, describe, commitMessage, commitNotes, noteAdd, noteRemove } from "$lib/ipc/client";
  import type { CommitInfo, FileDiff, NoteInfo } from "$lib/ipc/types";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { createImageLoader } from "$lib/components/diff/imageLoad";
  import BlameView from "$lib/components/panels/BlameView.svelte";
  import type { CompareResult } from "$lib/components/panels/compare";
  import { openPanelPopout } from "$lib/layout/popout";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import SignatureBadge from "$lib/components/commit/SignatureBadge.svelte";
  import { bookmarks as bookmarkStore } from "$lib/components/graph/bookmarks.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import {
    classifyRef,
    formatDateTime,
    formatRelativeTime,
    isCheckpointSummary,
    shortRefName,
  } from "$lib/git/format";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import type { AiRunOptions } from "$lib/ai/ai.svelte";
  import AiFeatureButton from "$lib/components/ai/AiFeatureButton.svelte";
  import { budgetedDiffText, statSummaryText } from "$lib/ai/diffBudget";
  import { aiResultMeta } from "$lib/ai/usage";
  import { emitUiEvent } from "$lib/palette/events";
  import AuthorAvatar from "$lib/components/panels/AuthorAvatar.svelte";

  const iconClose = $derived(appIcon("close")!);
  const iconStar = $derived(appIcon("star")!);
  const iconStarOutline = $derived(appIcon("star-outline")!);

  let {
    repoId,
    info,
    compare = null,
    root = "",
    /** Multi-selection size for the action labels (popout: 1). */
    actionCount = 1,
    /** True while a cherry-pick/revert IPC is in flight (owner's state). */
    actionBusy = false,
    /** Backend stream-slot id for the commit diff (popouts pass their own). */
    diffConsumerId = "commit-detail",
    onClose,
    onCloseCompare,
    onPopout,
    onSelectParent,
    onCherryPick,
    onRevert,
    onRebaseFromHere,
    onCheckout,
  }: {
    repoId: string;
    info: CommitInfo | null;
    /** CompareBar results render in the same body (HistoryView only). */
    compare?: CompareResult | null;
    /** Worktree root (bookmark persistence key); empty hides the star. */
    root?: string;
    actionCount?: number;
    actionBusy?: boolean;
    diffConsumerId?: string;
    onClose?: () => void;
    onCloseCompare?: () => void;
    onPopout?: () => void;
    onSelectParent?: (sha: string) => void;
    onCherryPick?: () => void;
    onRevert?: () => void;
    onRebaseFromHere?: () => void;
    /** VSCode-parity "checkout this commit" (owner opens the confirm dialog). */
    onCheckout?: () => void;
  } = $props();

  // -- diff for the shown commit (streamed, token-guarded) --------------------

  let detailFiles = $state<FileDiff[] | null>(null);
  let detailLoading = $state(false);
  let detailError = $state<string | null>(null);
  let diffToken = 0;
  /** Bumped by the error state's Retry to re-run the streaming effect. */
  let diffReload = $state(0);
  let blameTarget = $state<{ path: string; from: string } | null>(null);
  // Merge commits can touch thousands of files; the file list renders a
  // page + "Show all" until the user asks for the rest.
  const FILELIST_PAGE = 200;
  let fileShowAll = $state(false);
  const listedFiles = $derived(
    detailFiles !== null && !fileShowAll && detailFiles.length > FILELIST_PAGE
      ? detailFiles.slice(0, FILELIST_PAGE)
      : detailFiles,
  );

  $effect(() => {
    const current = info;
    void repoId; // reload on repo switch
    void diffReload; // the error state's Retry re-runs the stream
    blameTarget = null;
    fileShowAll = false;
    const token = ++diffToken;
    if (!current || current.parents.length === 0) {
      detailFiles = null;
      detailLoading = false;
      detailError = null;
      return;
    }
    detailLoading = true;
    detailFiles = [];
    detailError = null;
    // Streamed like every other large diff (RepoView/PopoutDiff): pages of
    // ~50 files accumulate, so huge merge commits render incrementally
    // instead of waiting on one multi-megabyte response. The popout passes
    // its own consumer id so the two windows' streams never supersede
    // each other backend-side.
    streamDiff(
      repoId,
      { commit: current.parents[0] as string },
      { commit: current.sha },
      (page) => {
        if (token !== diffToken) return;
        detailFiles = [...(detailFiles ?? []), ...page];
      },
      undefined,
      diffConsumerId,
    )
      .then(() => {
        if (token !== diffToken) return;
        detailLoading = false;
      })
      .catch((err: unknown) => {
        if (token !== diffToken) return;
        detailLoading = false;
        detailFiles = [];
        detailError = describeError(err);
      });
  });

  /**
   * Image-diff URL loaders for both hosts in this pane, keyed by what
   * feeds the diff: the streamed commit reads parent0→sha (the same sides
   * the stream uses), the compare reads base→target (base may carry the
   * range's `^` parent marker, which `DiffSide::Commit` resolves).
   * `$derived` keeps the identity stable across stream pages (a fresh
   * closure per render would reload every image on each page).
   */
  const detailImageLoader = $derived(
    info && info.parents.length > 0
      ? createImageLoader(repoId, {
          old: { commit: info.parents[0] },
          new: { commit: info.sha },
        })
      : undefined,
  );
  const compareImageLoader = $derived(
    compare
      ? createImageLoader(repoId, {
          // CompareBar results carry their exact sides (three-dot bases and
          // worktree/index/head targets are not plain commits); range
          // compares fall back to the base→target commit sides.
          old: compare.sides?.old ?? { commit: compare.base },
          new: compare.sides?.new ?? { commit: compare.target },
        })
      : undefined,
  );

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

  // -- git notes (Phase 5) ------------------------------------------------------
  //
  // One notes section per shown commit: the commit's notes listed (ref +
  // text), a compact add form (textarea) and per-note remove. Collapsed to
  // a single "Notes" toggle line while empty; the add form opens on demand
  // (palette `notes.add-to-head` routes here too via the ui event below).

  /** Notes of the shown commit (`null` until the first read lands). */
  let notes = $state<NoteInfo[] | null>(null);
  /** Whether the composer is open (auto-opens when notes exist). */
  let notesOpen = $state(false);
  let noteDraft = $state("");
  let noteBusy = $state(false);
  let notesToken = 0;

  $effect(() => {
    const sha = info?.sha;
    void repoId; // reload on repo switch
    notes = null;
    noteDraft = "";
    if (!sha) return;
    const token = ++notesToken;
    commitNotes(repoId, sha)
      .then((list) => {
        if (token !== notesToken) return;
        notes = list;
        if (list.length > 0) notesOpen = true;
      })
      .catch(() => {
        // Notes are additive detail — a failed read stays collapsed.
        if (token === notesToken) notes = [];
      });
  });

  /** Opens the composer (external request: palette notes command). */
  export function openNoteComposer(): void {
    notesOpen = true;
  }

  async function onNoteAdd(): Promise<void> {
    const sha = info?.sha;
    const text = noteDraft.trim();
    if (!sha || !text || noteBusy) return;
    noteBusy = true;
    try {
      await noteAdd(repoId, sha, text);
      noteDraft = "";
      const list = await commitNotes(repoId, sha).catch(() => null);
      if (list !== null) notes = list;
      toast("Note added", { kind: "success" });
    } catch (err) {
      toast(`Note add failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      noteBusy = false;
    }
  }

  async function onNoteRemove(note: NoteInfo): Promise<void> {
    const sha = info?.sha;
    if (!sha || noteBusy) return;
    noteBusy = true;
    try {
      await noteRemove(repoId, sha, note.ref);
      const list = await commitNotes(repoId, sha).catch(() => null);
      if (list !== null) notes = list;
      toast("Note removed", { kind: "success" });
    } catch (err) {
      toast(`Note remove failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      noteBusy = false;
    }
  }

  // -- full commit message (fetched on demand) --------------------------------

  /**
   * Full `summary` + body of the shown commit. Log pages carry only the
   * summary; until the fetch lands (or when it fails) the meta pane shows
   * the summary — for single-line messages, the common case, they are the
   * same text.
   */
  let fullMessage = $state<string | null>(null);
  let messageToken = 0;

  $effect(() => {
    const sha = info?.sha;
    void repoId; // reload on repo switch
    fullMessage = null;
    if (!sha) return;
    const token = ++messageToken;
    commitMessage(repoId, sha)
      .then((message) => {
        if (token === messageToken && message !== null) fullMessage = message;
      })
      .catch(() => {
        /* the summary stays shown — the body is additive detail */
      });
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

  // -- M13: AI explain (commit) / release notes (compare range) -----------------

  /** explain-commit: the loaded commit + its parent0 diff, budgeted. */
  async function runExplainCommit(opts?: AiRunOptions) {
    const current = info;
    const files = detailFiles;
    if (!current || files === null || files.length === 0) {
      return Promise.reject(new Error("this commit has no loaded diff to explain"));
    }
    // Log pages carry only the summary; the AI prompt gets the full body
    // (usually already fetched for the meta pane — reuse it).
    let message = fullMessage;
    if (message === null) {
      message = await commitMessage(repoId, current.sha).catch(() => null);
    }
    return ai.run(
      "explain-commit",
      {
        repoId,
        sha: current.sha,
        description: message ?? current.summary,
        diff: budgetedDiffText(files),
      },
      undefined,
      opts,
    );
  }

  function onExplainCommit(outcome: import("$lib/ai/features").FeatureOutcome): void {
    const sha = info?.sha;
    emitUiEvent("ai-show-result", {
      title: `Commit ${(sha ?? "").slice(0, 7)}`,
      text: outcome.text ?? outcome.result.text,
      meta: aiResultMeta(outcome.result),
    });
  }

  /** release-notes: the compare range's commits + aggregate file stats. */
  function runReleaseNotes(opts?: AiRunOptions) {
    const current = compare;
    if (!current || current.commits === undefined || current.commits.length === 0) {
      return Promise.reject(new Error("release notes need the range's commits"));
    }
    return ai.run(
      "release-notes",
      {
        repoId,
        range: `${current.base}..${current.target}`,
        commits: current.commits,
        diff: statSummaryText(current.files),
      },
      undefined,
      opts,
    );
  }

  function onReleaseNotes(outcome: import("$lib/ai/features").FeatureOutcome): void {
    const current = compare;
    emitUiEvent("ai-show-result", {
      title: `Release notes: ${current?.base ?? ""}..${current?.target ?? ""}`,
      text: outcome.text ?? outcome.result.text,
      meta: aiResultMeta(outcome.result),
    });
  }

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
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- Middle-click anywhere on the header row closes the detail (the
       TabStrip tab idiom); the buttons inside keep their own click flow —
       their events simply bubble up through this row. -->
  <header
    class="dhead flex items-center gap-2 border-b border-border bg-popover px-2.5 py-1 text-ui-base"
    onauxclick={(event) => {
      if (event.button === 1 && onClose) {
        event.preventDefault();
        onClose();
      }
    }}
    onmousedown={(event) => {
      // Middle only: suppress the browser's autoscroll affordance. The
      // primary button keeps its default so header buttons stay clickable
      // and focusable as usual.
      if (event.button === 1) event.preventDefault();
    }}
  >
    {#if info}
      <Hint label={`${info.author.name}${info.author.email ? ` <${info.author.email}>` : ""}`}>
        {#snippet child({ props })}
          <span {...props} class="flex-none cursor-help">
            <AuthorAvatar name={info?.author.name ?? ""} email={info?.author.email ?? ""} />
          </span>
        {/snippet}
      </Hint>
      <span class="dsha font-mono text-primary">{info.sha.slice(0, 7)}</span>
      <Hint label={info.summary}>
        {#snippet child({ props })}
          <span class="dsummary inline-flex min-w-0 flex-1 items-center gap-1 truncate font-medium text-foreground" {...props}>
            <span class="min-w-0 truncate">{info?.summary ?? ""}</span>
            {#if info && isCheckpointSummary(info.summary)}
              <span
                class="checkpoint-badge flex-none rounded-sm border border-border bg-muted px-1 font-normal text-ui-micro leading-[1.4] text-muted-foreground"
                title="mygitui checkpoint snapshot"
                >checkpoint</span
              >
            {/if}
          </span>
        {/snippet}
      </Hint>
      {#if root !== ""}
        <!-- M12: bookmark star (same store mutation as the menu item). -->
        <Hint label={bookmarked ? "Remove bookmark" : "Bookmark commit"}>
          {#snippet child({ props })}
            <Button
              class="h-auto flex-none rounded-sm px-1.5 py-[0.05rem] [&_svg:not([class*='size-'])]:size-3 {bookmarked
                ? 'border-primary bg-transparent text-primary'
                : 'border-border bg-transparent text-muted-foreground'}"
              variant="outline"
              {...props}
              aria-pressed={bookmarked}
              aria-label={bookmarked
                ? "Remove bookmark from this commit"
                : "Bookmark this commit"}
              onclick={() => {
                // Snippet narrowing doesn't reach the callback closure.
                const sha = info?.sha;
                if (sha) toggleBookmark(sha);
              }}
            >
              <Icon icon={bookmarked ? iconStar : iconStarOutline} size={12} />
            </Button>
          {/snippet}
        </Hint>
      {/if}
      {#if onPopout}
        <Hint label="Open this commit's detail in its own window">
          {#snippet child({ props })}
            <Button
              class="h-auto rounded-sm px-2 py-px text-ui-meta font-normal text-muted-foreground"
              variant="outline"
              {...props}
              onclick={onPopout}
            >
              Pop out
            </Button>
          {/snippet}
        </Hint>
      {/if}
    {:else}
      <span class="dsha font-mono text-primary">diff</span>
      <span class="dsummary min-w-0 flex-1 truncate font-medium text-foreground">{compare?.base} → {compare?.target}</span>
    {/if}
    {#if onClose}
      <Button class="h-auto rounded-sm px-1.5 py-px text-ui-meta font-normal text-muted-foreground [&_svg:not([class*='size-'])]:size-3" variant="outline" onclick={onClose} aria-label="Close detail">
        <Icon icon={iconClose} size={12} />
      </Button>
    {/if}
  </header>
  {#if info}
    <div class="cactions flex flex-none flex-wrap items-center gap-1.5 border-b border-border bg-muted/50 px-3 py-1 text-ui-meta" role="toolbar" aria-label="Commit actions">
      {#if onCherryPick}
        <Button class="h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal" variant="secondary" onclick={onCherryPick} disabled={actionBusy}>
          Cherry-pick{actionCount > 1 ? ` (${actionCount})` : ""}
        </Button>
      {/if}
      {#if onRevert}
        <Button class="h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal" variant="secondary" onclick={onRevert} disabled={actionBusy}>
          Revert{actionCount > 1 ? ` (${actionCount})` : ""}
        </Button>
      {/if}
      {#if onRebaseFromHere}
        <Hint label={`Rebase HEAD..${info.sha.slice(0, 7)} (selected commit is the exclusive base)`}>
          {#snippet child({ props })}
            <Button
              class="h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal"
              variant="secondary"
              {...props}
              onclick={onRebaseFromHere}
              disabled={actionBusy}
            >
              Rebase from here
            </Button>
          {/snippet}
        </Hint>
      {/if}
      {#if onCheckout}
        <Hint label="Detach HEAD at this commit (or switch to a branch that points here)">
          {#snippet child({ props })}
            <Button
              class="h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal"
              variant="secondary"
              {...props}
              onclick={onCheckout}
            >
              Checkout commit
            </Button>
          {/snippet}
        </Hint>
      {/if}
      <Button class="h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal" variant="secondary" onclick={copySha}>Copy sha</Button>
      <!-- M13: AI explain (disabled until the parent0 diff is loaded; the
           root commit has nothing to diff against). -->
      <AiFeatureButton
        {repoId}
        kind="explain-commit"
        label="AI explain"
        title="Explain this commit with AI"
        confirmText={"Send this commit's message and diff to {backend} for an explanation?"}
        errorPrefix="AI explain failed"
        gateVisibility={false}
        compact
        disabled={info.parents.length === 0 || detailFiles === null || detailFiles.length === 0}
        run={runExplainCommit}
        onResult={onExplainCommit}
      />
      <span class="cinfo ml-auto text-ui-micro text-muted-foreground" role="status">
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
        <div class="dtab flex flex-none items-center gap-2 border-b border-border bg-muted py-0.5 pr-2 pl-3 text-ui-meta text-foreground">
          <span class="min-w-0 flex-1 truncate font-mono">Blame: {blameTarget.path}</span>
          <Button
            class="h-auto rounded-sm px-1.5 py-px text-ui-meta font-normal text-muted-foreground [&_svg:not([class*='size-'])]:size-3"
            variant="outline"
            onclick={() => (blameTarget = null)}
            aria-label="Close blame">
            <Icon icon={iconClose} size={12} />
          </Button>
        </div>
        <div class="dtabbody flex min-h-0 flex-1 flex-col">
          <BlameView {repoId} path={blameTarget.path} from={blameTarget.from} />
        </div>
      {:else if compare}
        <div class="dtab flex flex-none items-center gap-2 border-b border-border bg-muted py-0.5 pr-2 pl-3 text-ui-meta text-foreground">
          <span class="min-w-0 flex-1 truncate font-mono">Compare: {compare.base} → {compare.target} ({compare.files.length} files)</span>
          <!-- M13: release notes for the compared range (needs the range's
               commits, which HistoryView attaches to the compare result). -->
          {#if compare.commits !== undefined && compare.commits.length > 0}
            <AiFeatureButton
              {repoId}
              kind="release-notes"
              label="AI release notes"
              title="Write release notes for this range"
              confirmText={"Send the range's commits and file stats to {backend} to draft release notes?"}
              errorPrefix="AI release notes failed"
              gateVisibility={false}
              compact
              run={runReleaseNotes}
              onResult={onReleaseNotes}
            />
          {/if}
          {#if onCloseCompare}
            <Button class="h-auto rounded-sm px-1.5 py-px text-ui-meta font-normal text-muted-foreground [&_svg:not([class*='size-'])]:size-3" variant="outline" onclick={onCloseCompare} aria-label="Close compare">
              <Icon icon={iconClose} size={12} />
            </Button>
          {/if}
        </div>
        <div class="dtabbody flex min-h-0 flex-1 flex-col">
          <DiffViewer files={compare.files} onLoadImage={compareImageLoader} />
        </div>
      {:else if detailError}
        <div class="dstate m-auto flex flex-col items-center gap-2 text-ui-base text-muted-foreground">
          <p role="alert">Diff failed: {detailError}</p>
          <Button
            class="h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal"
            variant="outline"
            onclick={() => (diffReload += 1)}
          >Retry</Button>
        </div>
      {:else if detailLoading && (detailFiles === null || detailFiles.length === 0)}
        <p class="dstate m-auto text-ui-base text-muted-foreground" role="status">Loading diff…</p>
      {:else if !info || info.parents.length === 0}
        <p class="dstate m-auto text-ui-base text-muted-foreground">Root commit — nothing to diff against.</p>
      {:else if detailFiles === null}
        <p class="dstate m-auto text-ui-base text-muted-foreground">Diff unavailable.</p>
      {:else if detailFiles.length === 0}
        <p class="dstate m-auto text-ui-base text-muted-foreground">No changes against first parent.</p>
      {:else}
        <ul class="filelist m-0 max-h-[30%] flex-none list-none overflow-y-auto border-b border-border py-0.5 text-ui-meta" aria-label="Changed files">
          {#each listedFiles as file (file.path)}
            <li
              class="flex items-center gap-2 px-3 py-0.5"
              oncontextmenu={(e) => fileMenu(e, file.path)}
            >
              <Hint label={file.path}>
                {#snippet child({ props })}
                  <span class="fpath min-w-0 flex-1 truncate font-mono text-foreground" {...props}>
                    {#if file.old_path}{file.old_path} → {/if}{file.path}
                  </span>
                {/snippet}
              </Hint>
              <span class="stats flex-none font-mono">
                {#if !file.binary}
                  <span class="add text-[color:var(--app-added)]">+{file.additions}</span>
                  <span class="del text-[color:var(--app-deleted)]">−{file.deletions}</span>
                {:else}
                  <span class="muted text-muted-foreground">binary</span>
                {/if}
              </span>
              <Button class="h-auto rounded-sm px-2 py-px text-ui-meta font-normal text-muted-foreground" variant="outline" onclick={() => openBlame(file.path)}>Blame</Button>
              <Hint label={`History of ${file.path} (popout)`}>
                {#snippet child({ props })}
                  <Button
                    class="h-auto rounded-sm px-2 py-px text-ui-meta font-normal text-muted-foreground"
                    variant="outline"
                    {...props}
                    onclick={() =>
                      void openPanelPopout("filehistory", repoId, `History: ${file.path}`, { path: file.path })}
                  >History</Button>
                {/snippet}
              </Hint>
            </li>
          {/each}
          {#if listedFiles !== detailFiles}
            <li class="px-3 py-0.5">
              <button
                type="button"
                class="text-muted-foreground underline-offset-2 hover:underline"
                onclick={() => (fileShowAll = true)}
              >
                Show all {detailFiles.length} changed files
              </button>
            </li>
          {/if}
        </ul>
        <div class="ddiff min-h-0 flex-1 overflow-auto">
          <DiffViewer files={detailFiles} onLoadImage={detailImageLoader} />
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
              <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Author</dt>
              <dd class="m-0 text-foreground">
                {info.author.name}
                <span class="muted text-muted-foreground">&lt;{info.author.email}&gt;</span><br />
                {formatDateTime(info.author.time)}
                <span class="muted text-muted-foreground">({formatRelativeTime(info.author.time)})</span>
              </dd>
              <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Committer</dt>
              <dd class="m-0 text-foreground">
                {info.committer.name}<br />
                {formatDateTime(info.committer.time)}
                <span class="muted text-muted-foreground">({formatRelativeTime(info.committer.time)})</span>
              </dd>
              <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Parents</dt>
              <dd class="m-0 text-foreground">
                {#if info.parents.length === 0}
                  <em>root</em>
                {:else}
                {#each info.parents as p (p)}
                  <Hint label={"Go to " + p}>
                    {#snippet child({ props })}
                      <Button
                        class="mr-1 h-auto rounded-sm px-ui-3 py-[0.05rem] font-mono text-ui-micro font-normal"
                        variant="outline"
                        {...props}
                        onclick={() => onSelectParent?.(p)}
                        disabled={!onSelectParent}
                      >
                        {p.slice(0, 7)}
                      </Button>
                    {/snippet}
                  </Hint>
                {/each}
                {/if}
              </dd>
              <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Describe</dt>
              <dd class="m-0 text-foreground">
                {#if describeText}
                  <code>{describeText}</code>
                {:else}
                  <span class="muted text-muted-foreground">…</span>
                {/if}
              </dd>
              <!-- M12: commit-signature verification badge. hideUnsigned=false:
                   the meta grid always renders this row's label, so an
                   unsigned commit must show the truthful "Unsigned" chip
                   (the default hiding leaves a confusingly empty row). -->
              <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Signature</dt>
              <dd class="m-0 text-foreground"><SignatureBadge {repoId} sha={info.sha} hideUnsigned={false} /></dd>
              {#if info.refs.length > 0}
                <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Refs</dt>
                <dd class="drefs m-0 flex flex-wrap gap-1 text-foreground">
                  {#each info.refs as ref (ref)}
                    {@const kind = classifyRef(ref)}
                    <span class="ref ref-{kind} inline-flex max-w-36 items-center overflow-hidden whitespace-nowrap rounded-sm border px-1 text-ui-micro leading-[1.4] text-ellipsis {kind === "local"
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
            <pre class="msg mt-2 whitespace-pre-wrap rounded-sm border border-border bg-muted p-2 font-mono text-ui-meta text-foreground [overflow-wrap:anywhere]">{fullMessage ?? info.summary}</pre>
            <!-- Notes section (compact): collapsed to one toggle line while
                 empty; lists ref + text and offers add / per-note remove. -->
            <div class="notes mt-2" aria-label="Commit notes">
              {#if notesOpen}
                <div class="notes-head mb-1 flex items-center gap-1">
                  <span class="text-ui-micro font-semibold uppercase tracking-[0.02em] text-muted-foreground">Notes</span>
                  <Button
                    class="h-auto rounded-sm px-1.5 py-px text-ui-meta font-normal text-muted-foreground [&_svg:not([class*='size-'])]:size-3"
                    variant="outline"
                    type="button"
                    onclick={() => (notesOpen = false)}
                    aria-label="Collapse notes"
                  >
                    <Icon icon={iconClose} size={12} />
                  </Button>
                </div>
                {#if notes === null}
                  <p class="m-0 text-ui-meta text-muted-foreground" role="status">Loading notes…</p>
                {:else if notes.length === 0}
                  <p class="m-0 mb-1 text-ui-meta text-muted-foreground">No notes on this commit.</p>
                {:else}
                  {#each notes as note (note.ref)}
                    <div class="note mb-1 rounded-sm border border-border bg-muted p-1.5">
                      <div class="note-head flex items-center gap-1.5">
                        <Hint label={`Notes ref: ${note.ref}`}>
                          {#snippet child({ props })}
                            <span {...props} class="min-w-0 flex-1 truncate font-mono text-ui-tiny text-muted-foreground">{note.ref}</span>
                          {/snippet}
                        </Hint>
                        <Button
                          class="h-auto rounded-sm px-1.5 py-px text-ui-meta font-normal text-destructive"
                          variant="outline"
                          type="button"
                          disabled={noteBusy}
                          onclick={() => void onNoteRemove(note)}
                          aria-label={`Remove note under ${note.ref}`}
                        >
                          Remove
                        </Button>
                      </div>
                      <pre class="m-0 mt-1 whitespace-pre-wrap font-mono text-ui-meta text-foreground [overflow-wrap:anywhere]">{note.text}</pre>
                    </div>
                  {/each}
                {/if}
                <Textarea
                  class="h-auto min-h-0 w-full resize-y rounded-sm border border-border bg-background px-2 py-1 font-mono text-ui-meta focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring md:text-xs"
                  rows={2}
                  bind:value={noteDraft}
                  placeholder="Add a note (git notes)…"
                  aria-label="New note text"
                  spellcheck="false"
                ></Textarea>
                <Button
                  class="mt-1 h-auto rounded-full px-2.5 py-0.5 text-ui-meta font-normal"
                  variant="secondary"
                  type="button"
                  disabled={noteBusy || noteDraft.trim() === ""}
                  onclick={() => void onNoteAdd()}
                >
                  {noteBusy ? "Adding…" : "Add note"}
                </Button>
              {:else}
                <button
                  class="notes-toggle cursor-pointer border-0 bg-transparent p-0 text-ui-micro text-muted-foreground hover:text-foreground"
                  type="button"
                  onclick={() => (notesOpen = true)}
                >
                  Notes{notes !== null && notes.length > 0 ? ` (${notes.length})` : ""}…
                </button>
              {/if}
            </div>
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
