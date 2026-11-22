<script lang="ts">
  /**
   * CommitDetail — the commit detail pane, shared by HistoryView's
   * list↔detail split and the `?panel=commitdetail` popout window.
   *
   * Owns everything selection-independent about one commit's detail: the
   * parent0→sha diff stream (token-guarded, per-consumer stream slot),
   * `git describe`, the full-message fetch, and the blame tab. Everything
   * that needs the surrounding view — closing, parent navigation, cherry-
   * pick/revert/rebase/checkout, popping out — arrives as optional
   * callbacks, so the popout can wire single-commit versions and omit the
   * rest (close button, bookmark star when `root` is empty).
   *
   * Palette support: `history-toggle-blame` needs the loaded file list,
   * which is private — the exported `toggleBlameFirstFile()` reimplements
   * the old HistoryView handler and reports whether it did anything.
   *
   * Workstream C layout (plan §7): the meta summary is ONE header row
   * (avatar · author · relative time · sha chip that copies on click ·
   * signature icon when signed · summary); the rest of the metadata
   * (committer, clickable parents, describe, refs, signature truth, the
   * trailer-stripped message, notes) lives in a `Details` popover, so raw
   * trailers (e.g. `T3-Head-Moved-At`) never render inline. Visible action
   * buttons are Cherry-pick + Revert + the AI sparkle; everything else
   * (rebase from here, checkout, copy sha) sits in the row's OverflowMenu.
   * Below, a changed-files column sits beside the diff (wide container) or
   * collapses into a horizontal chip strip above it (<700px container —
   * the same threshold that flips DiffViewer to its narrow defaults via
   * `containerNarrow`); file rows keep the status letter + path + ±stats
   * always visible while Blame/History appear on hover/focus (and in the
   * row context menu; keyboard `b`/`h` on a focused row).
   */
  import { streamDiff, describe, commitMessage, commitNotes, noteAdd, noteRemove } from "$lib/ipc/client";
  import type { CommitInfo, FileDiff, NoteInfo } from "$lib/ipc/types";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { fileChangeKind, fileKindLabel, fileKindLetter } from "$lib/components/diff/rowModel";
  import { createImageLoader } from "$lib/components/diff/imageLoad";
  import BlameView from "$lib/components/panels/BlameView.svelte";
  import type { CompareResult } from "$lib/components/panels/compare";
  import { openPanelPopout } from "$lib/layout/popout";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import OverflowMenu from "$lib/components/layout/OverflowMenu.svelte";
  import SignatureBadge from "$lib/components/commit/SignatureBadge.svelte";
  import { bookmarks as bookmarkStore } from "$lib/components/graph/bookmarks.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import { createWidthTracker } from "$lib/layout/containerWidth.svelte";
  import { splitTrailers } from "$lib/commit/trailers";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
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
  const iconInfo = $derived(appIcon("info")!);
  const iconBlame = $derived(appIcon("person")!);
  const iconFileHistory = $derived(appIcon("history")!);
  const iconRebase = $derived(appIcon("call-split")!);
  const iconCheckout = $derived(appIcon("fork-right")!);

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

  // -- narrow-container measurement (plan §7.5/§7.6) ---------------------------
  //
  // The detail's own width, not the window mode, decides the layout: under
  // 700px the file list collapses into a horizontal chip strip above the
  // diff (single column) and the diff viewers get their narrow defaults
  // (Unified auto-selected, wrap off — DiffViewer owns the overrides).

  const panel = createWidthTracker();

  // -- diff for the shown commit (streamed, token-guarded) --------------------

  let detailFiles = $state<FileDiff[] | null>(null);
  let detailLoading = $state(false);
  let detailError = $state<string | null>(null);
  let diffToken = 0;
  /** Bumped by the error state's Retry to re-run the streaming effect. */
  let diffReload = $state(0);
  let blameTarget = $state<{ path: string; from: string } | null>(null);
  /** The rendered diff viewer (file rows/chips reveal a file's header). */
  let diffViewer = $state<ReturnType<typeof DiffViewer> | null>(null);
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
  // One notes section inside the Details popover: the commit's notes listed
  // (ref + text), a compact add form (textarea) and per-note remove. The
  // popover stays closed by default — the Details trigger shows the note
  // count when notes exist, and the add form opens on demand (palette
  // `notes.add-to-head` routes here too via the ui event below).

  /** Notes of the shown commit (`null` until the first read lands). */
  let notes = $state<NoteInfo[] | null>(null);
  /** Whether the composer is open (auto-opens when notes exist). */
  let notesOpen = $state(false);
  let noteDraft = $state("");
  let noteBusy = $state(false);
  let notesToken = 0;

  /** Whether the Details popover is open (the popover is the meta pane). */
  let detailsOpen = $state(false);

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
    detailsOpen = true;
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
   * same text. The body renders trailer-stripped (plan §7.1): raw trailer
   * lines (`T3-Head-Moved-At: …`, `Signed-off-by: …`) never render inline;
   * the Details popover lists them as structured rows.
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

  const messageParts = $derived(splitTrailers(fullMessage ?? info?.summary ?? ""));

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

  // -- resizable panes (files ↔ diff, persisted as a global pref) --------------

  const FILES_RATIO_KEY = "commit-detail-files";
  let filesRatio = $state(readSplitRatio(FILES_RATIO_KEY, 0.26));

  function setFilesRatio(ratio: number): void {
    filesRatio = ratio;
    writeSplitRatio(FILES_RATIO_KEY, ratio);
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

  /**
   * File-row keyboard actions (plan §7.3): `b` blames, `h` opens the file
   * history popout. Skipped when a nested button/menu holds focus (Enter/
   * Space flow there; `b`/`h` typed while a button has focus would re-run
   * the row action under the pointer).
   */
  function fileRowKey(event: KeyboardEvent, path: string): void {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (
      event.target instanceof HTMLElement &&
      event.target.closest("button, [role='menu'], [role='menuitem']")
    ) {
      return;
    }
    if (event.key === "b") {
      event.preventDefault();
      openBlame(path);
    } else if (event.key === "h") {
      event.preventDefault();
      void openPanelPopout("filehistory", repoId, `History: ${path}`, { path });
    }
  }

  /** Row/chip click: scroll the diff to the file's header row. */
  function revealFile(path: string): void {
    diffViewer?.revealFile(path);
  }
</script>

<div
  class="detail @container flex min-h-0 flex-col bg-card"
  aria-label="Commit detail"
  bind:this={panel.el}
>
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
      <!-- Meta summary line (plan §7.1): avatar · author · relative time ·
           sha chip (click = copy) · signature icon · summary. One row. -->
      <Hint label={`${info.author.name}${info.author.email ? ` <${info.author.email}>` : ""}`}>
        {#snippet child({ props })}
          <span {...props} class="flex min-w-0 flex-none cursor-help items-center gap-1.5">
            <AuthorAvatar name={info?.author.name ?? ""} email={info?.author.email ?? ""} />
            <span class="dauthor hidden max-w-36 min-w-0 truncate text-ui-meta text-foreground @min-[28rem]:inline">
              {info.author.name}
            </span>
          </span>
        {/snippet}
      </Hint>
      <Hint label={formatDateTime(info.author.time)}>
        {#snippet child({ props })}
          <span {...props} class="dtime flex-none cursor-help whitespace-nowrap text-ui-micro tabular-nums text-muted-foreground">
            {formatRelativeTime(info.author.time)}
          </span>
        {/snippet}
      </Hint>
      <Hint label={`Copy sha ${info.sha}`}>
        {#snippet child({ props })}
          <button
            {...props}
            type="button"
            class="dsha flex-none cursor-pointer rounded-sm border border-border bg-muted px-1.5 py-px font-mono text-ui-micro text-primary hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
            onclick={copySha}
            aria-label={`Copy sha ${info.sha}`}
          >{info.sha.slice(0, 7)}</button>
        {/snippet}
      </Hint>
      <SignatureBadge {repoId} sha={info.sha} variant="icon" />
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
    <!-- Action row (plan §7.2): Cherry-pick and Revert visible, the AI
         sparkle visible, everything else demoted — Details popover for the
         metadata, overflow for rebase/checkout/copy-sha. -->
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
      <!-- M13: AI explain (sparkle stays visible — plan §7.2; disabled
           until the parent0 diff is loaded, the root commit has nothing to
           diff against). -->
      <AiFeatureButton
        {repoId}
        kind="explain-commit"
        label="AI explain"
        ariaLabel="AI explain this commit"
        title="Explain this commit with AI"
        confirmText={"Send this commit's message and diff to {backend} for an explanation?"}
        errorPrefix="AI explain failed"
        gateVisibility={false}
        icon
        disabled={info.parents.length === 0 || detailFiles === null || detailFiles.length === 0}
        run={runExplainCommit}
        onResult={onExplainCommit}
      />
      <Popover.Root bind:open={detailsOpen}>
        <Popover.Trigger
          class="details-trigger inline-flex h-auto cursor-pointer items-center gap-1 rounded-full border border-border bg-transparent px-2 py-0.5 text-ui-meta font-normal text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
          aria-label="Commit details"
        >
          <Icon icon={iconInfo} size={13} />
          Details{notes !== null && notes.length > 0
            ? ` · ${notes.length}`
            : ""}
        </Popover.Trigger>
        <Popover.Content
          class="h-max max-h-[min(34rem,calc(100vh-6rem))] w-[26rem] max-w-[calc(100vw-2rem)] overflow-y-auto rounded-lg p-3 text-ui-meta"
          align="start"
        >
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
            <!-- M12: commit-signature verification badge. hideUnsigned=false:
                 the grid always renders this row's label, so an unsigned
                 commit must show the truthful "Unsigned" chip. -->
            <dt class="pt-0.5 text-ui-micro uppercase tracking-[0.02em] text-muted-foreground">Signature</dt>
            <dd class="m-0 text-foreground"><SignatureBadge {repoId} sha={info.sha} hideUnsigned={false} /></dd>
          </dl>
          {#if messageParts.trailers.length > 0}
            <!-- Structured trailers (plan §7.1): raw trailer lines never
                 render inline in the message below. -->
            <div class="trailers mt-2" aria-label="Commit trailers">
              <span class="text-ui-micro font-semibold uppercase tracking-[0.02em] text-muted-foreground">Trailers</span>
              <dl class="m-0 mt-1 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
                {#each messageParts.trailers as trailer (trailer.token + trailer.value)}
                  <dt class="m-0 min-w-0 truncate font-mono text-ui-micro text-muted-foreground" title={trailer.token}>{trailer.token}</dt>
                  <dd class="m-0 min-w-0 break-words font-mono text-ui-micro text-foreground [overflow-wrap:anywhere]">{trailer.value}</dd>
                {/each}
              </dl>
            </div>
          {/if}
          <pre class="msg mt-2 whitespace-pre-wrap rounded-sm border border-border bg-muted p-2 font-mono text-ui-meta text-foreground [overflow-wrap:anywhere]">{messageParts.body || info.summary}</pre>
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
                          <span {...props} class="min-w-0 flex-1 truncate font-mono text-ui-meta text-muted-foreground">{note.ref}</span>
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
        </Popover.Content>
      </Popover.Root>
      <OverflowMenu label="More commit actions">
        {#snippet content()}
          {#if onRebaseFromHere}
            <DropdownMenu.Item class="cursor-pointer gap-ui-3" onclick={onRebaseFromHere} disabled={actionBusy}>
              <span class="pointer-events-none inline-flex flex-none items-center justify-center">
                <Icon icon={iconRebase} size={16} />
              </span>
              Rebase from here
            </DropdownMenu.Item>
          {/if}
          {#if onCheckout}
            <DropdownMenu.Item class="cursor-pointer gap-ui-3" onclick={onCheckout}>
              <span class="pointer-events-none inline-flex flex-none items-center justify-center">
                <Icon icon={iconCheckout} size={16} />
              </span>
              Checkout commit
            </DropdownMenu.Item>
          {/if}
          <DropdownMenu.Item class="cursor-pointer gap-ui-3" onclick={copySha}>
            Copy sha
          </DropdownMenu.Item>
        {/snippet}
      </OverflowMenu>
      <span class="cinfo ml-auto text-ui-micro text-muted-foreground" role="status">
        {#if actionCount > 1}
          {actionCount} commits selected (shift-click to extend)
        {/if}
      </span>
    </div>
  {/if}
  <div class="dbody flex min-h-0 flex-1 flex-col [&>*]:min-w-0">
    {#snippet fileMeta(file: FileDiff)}
      {@const kind = fileChangeKind(file)}
      <Hint label={fileKindLabel(kind)}>
        {#snippet child({ props })}
          <span
            {...props}
            class="fstat flex-none cursor-help"
            aria-label={fileKindLabel(kind)}
          ><span class="inline-flex h-4 w-4 items-center justify-center rounded-[3px] font-mono text-ui-micro leading-none font-bold {kind === "added"
            ? "text-[var(--app-added)] bg-[var(--app-added-bg)]"
            : kind === "deleted"
              ? "text-[var(--app-deleted)] bg-[var(--app-deleted-bg)]"
              : kind === "renamed"
                ? "text-[var(--app-renamed)] bg-[var(--app-renamed-bg)]"
                : "text-[var(--app-modified)] bg-[var(--app-modified-bg)]"}">{fileKindLetter(kind)}</span></span>
        {/snippet}
      </Hint>
    {/snippet}
    {#snippet filesPane()}
    <!-- min-h-0 keeps the flex chain confined: without it the body grows to
         the diff's full content height (the viewport then matches it —
         nothing to scroll, and the virtualizer mounts every row, which is
         the whole-app lag when a large commit is opened). -->
    <div class="dfiles flex min-h-0 min-w-0 flex-1 flex-col">
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
          <DiffViewer files={compare.files} onLoadImage={compareImageLoader} containerNarrow={panel.narrow} />
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
      {:else if panel.narrow}
        <!-- Narrow container (plan §7.5): the file list collapses into a
             horizontal chip strip above the diff instead of a side column.
             The class stays `.filelist` (DOM contract) and the chip keeps
             the full path text (recoverable via its tooltip). -->
        <ul class="filelist flex flex-none items-center gap-1 overflow-x-auto border-b border-border bg-muted/40 py-1 pr-2 pl-2 text-ui-meta" aria-label="Changed files">
          {#each listedFiles as file (file.path)}
            {@const kind = fileChangeKind(file)}
            <li class="fchip flex-none">
              <button
                type="button"
                class="flex max-w-48 cursor-pointer items-center gap-1.5 rounded-full border border-border bg-card px-2 py-0.5 text-ui-meta hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                title={`${file.path} · ${fileKindLabel(kind)} · +${file.additions} −${file.deletions}`}
                onclick={() => revealFile(file.path)}
                oncontextmenu={(e) => fileMenu(e, file.path)}
              >
                <span class="fstat fk-{kind} flex-none font-mono text-ui-micro font-bold {kind === "added"
                  ? "text-[var(--app-added)]"
                  : kind === "deleted"
                    ? "text-[var(--app-deleted)]"
                    : kind === "renamed"
                      ? "text-[var(--app-renamed)]"
                      : "text-[var(--app-modified)]"}">{fileKindLetter(kind)}</span>
                <span class="min-w-0 truncate font-mono text-foreground">{#if file.old_path}{file.old_path} → {/if}{file.path}</span>
              </button>
            </li>
          {/each}
          {#if listedFiles !== detailFiles}
            <li class="flex-none">
              <button
                type="button"
                class="cursor-pointer whitespace-nowrap text-ui-meta text-muted-foreground underline-offset-2 hover:underline"
                onclick={() => (fileShowAll = true)}
              >
                +{detailFiles.length - (listedFiles?.length ?? 0)} more
              </button>
            </li>
          {/if}
        </ul>
        <div class="ddiff min-h-0 flex-1 overflow-auto">
          <DiffViewer
            bind:this={diffViewer}
            files={detailFiles}
            onLoadImage={detailImageLoader}
            containerNarrow={panel.narrow}
          />
        </div>
      {:else}
        <!-- Wide container (plan §7.6): changed-files column | diff, the
             files column user-resizable. File rows show status letter +
             path + ±stats; Blame/History surface on hover/focus only
             (and in the row context menu; keyboard `b`/`h`). -->
        <SplitPane
          axis="x"
          ratio={filesRatio}
          onRatio={setFilesRatio}
          label="Resize changed files and diff"
        >
          {#snippet a()}
            <ul class="filelist m-0 h-full min-h-0 min-w-0 list-none overflow-y-auto py-0.5 text-ui-meta" aria-label="Changed files">
              {#each listedFiles as file (file.path)}
                <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
                <!-- The row itself is keyboard-actuable: roving focus for
                     plan §7.3's `b`/`h` shortcuts (click reveals the file
                     in the diff below). -->
                <li
                  class="frow group flex cursor-pointer items-center gap-2 px-3 py-0.5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                  tabindex="0"
                  onclick={() => revealFile(file.path)}
                  oncontextmenu={(e) => fileMenu(e, file.path)}
                  onkeydown={(e) => fileRowKey(e, file.path)}
                >
                  {@render fileMeta(file)}
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
                  <!-- Hover/focus-revealed row actions (plan §7.3): no
                       always-visible per-row button pair. -->
                  <span class="factions flex flex-none items-center gap-0.5 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 group-focus-within:visible group-focus-visible:opacity-100 group-hover:visible group-focus-visible:visible">
                    <Hint label={`Blame ${file.path}`}>
                      {#snippet child({ props })}
                        <Button
                          {...props}
                          class="size-5.5 rounded-sm px-0 text-muted-foreground hover:bg-accent hover:text-foreground [&_svg:not([class*='size-'])]:size-3.5"
                          variant="ghost"
                          size="icon-sm"
                          type="button"
                          onclick={() => openBlame(file.path)}
                          aria-label={`Blame ${file.path}`}
                        >
                          <Icon icon={iconBlame} size={14} />
                        </Button>
                      {/snippet}
                    </Hint>
                    <Hint label={`History of ${file.path} (popout)`}>
                      {#snippet child({ props })}
                        <Button
                          {...props}
                          class="size-5.5 rounded-sm px-0 text-muted-foreground hover:bg-accent hover:text-foreground [&_svg:not([class*='size-'])]:size-3.5"
                          variant="ghost"
                          size="icon-sm"
                          type="button"
                          onclick={() =>
                            void openPanelPopout("filehistory", repoId, `History: ${file.path}`, { path: file.path })}
                          aria-label={`History of ${file.path}`}
                        >
                          <Icon icon={iconFileHistory} size={14} />
                        </Button>
                      {/snippet}
                    </Hint>
                  </span>
                </li>
              {/each}
              {#if listedFiles !== detailFiles}
                <li class="px-3 py-0.5">
                  <button
                    type="button"
                    class="text-muted-foreground underline-offset-2 hover:underline"
                    onclick={() => (fileShowAll = true)}
                  >
                    Show all {detailFiles!.length} changed files
                  </button>
                </li>
              {/if}
            </ul>
          {/snippet}
          {#snippet b()}
            <div class="ddiff min-h-0 min-w-0 flex-1 overflow-auto">
              <!-- Snippet bodies don't inherit the {:else} chain's narrowing
                   — asserted (the chain renders this branch only when the
                   streamed file list is non-null). -->
              <DiffViewer
                bind:this={diffViewer}
                files={detailFiles!}
                onLoadImage={detailImageLoader}
                containerNarrow={panel.narrow}
              />
            </div>
          {/snippet}
        </SplitPane>
      {/if}
    </div>
    {/snippet}

    {@render filesPane()}
  </div>
</div>
