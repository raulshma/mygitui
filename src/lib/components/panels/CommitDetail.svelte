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
  import {
    classifyRef,
    formatDateTime,
    formatRelativeTime,
    shortRefName,
  } from "$lib/stores/history-logic";
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

<div class="detail" aria-label="Commit detail">
  <header class="dhead">
    {#if info}
      <span class="dsha">{info.sha.slice(0, 7)}</span>
      <span class="dsummary" title={info.summary}>{info.summary}</span>
      {#if root !== ""}
        <!-- M12: bookmark star (same store mutation as the menu item). -->
        <button
          class="star"
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
        </button>
      {/if}
      {#if onPopout}
        <button
          class="dpop"
          title="Open this commit's detail in its own window"
          onclick={onPopout}
        >
          Pop out
        </button>
      {/if}
    {:else}
      <span class="dsha">diff</span>
      <span class="dsummary">{compare?.base} → {compare?.target}</span>
    {/if}
    {#if onClose}
      <button class="dclose" onclick={onClose} aria-label="Close detail">
        ×
      </button>
    {/if}
  </header>
  {#if info}
    <div class="cactions" role="toolbar" aria-label="Commit actions">
      {#if onCherryPick}
        <button class="act" onclick={onCherryPick} disabled={actionBusy}>
          Cherry-pick{actionCount > 1 ? ` (${actionCount})` : ""}
        </button>
      {/if}
      {#if onRevert}
        <button class="act" onclick={onRevert} disabled={actionBusy}>
          Revert{actionCount > 1 ? ` (${actionCount})` : ""}
        </button>
      {/if}
      {#if onRebaseFromHere}
        <button
          class="act"
          onclick={onRebaseFromHere}
          disabled={actionBusy}
          title={`Rebase HEAD..${info.sha.slice(0, 7)} (selected commit is the exclusive base)`}
        >
          Rebase from here
        </button>
      {/if}
      <button class="act" onclick={copySha}>Copy sha</button>
      <span class="cinfo" role="status">
        {#if actionCount > 1}
          {actionCount} commits selected (shift-click to extend)
        {/if}
      </span>
    </div>
  {/if}
  <div class="dbody">
    {#snippet filesPane()}
    <div class="dfiles">
      {#if blameTarget}
        <div class="dtab">
          <span>Blame: {blameTarget.path}</span>
          <button
            onclick={() => (blameTarget = null)}
            aria-label="Close blame">×
          </button>
        </div>
        <div class="dtabbody">
          <BlameView {repoId} path={blameTarget.path} from={blameTarget.from} />
        </div>
      {:else if compare}
        <div class="dtab">
          <span>Compare: {compare.base} → {compare.target} ({compare.files.length} files)</span>
          {#if onCloseCompare}
            <button onclick={onCloseCompare} aria-label="Close compare">×</button>
          {/if}
        </div>
        <div class="dtabbody">
          <DiffViewer files={compare.files} />
        </div>
      {:else if detailLoading}
        <p class="dstate" role="status">Loading diff…</p>
      {:else if !info || info.parents.length === 0}
        <p class="dstate">Root commit — nothing to diff against.</p>
      {:else if detailFiles === null}
        <p class="dstate">Diff unavailable.</p>
      {:else if detailFiles.length === 0}
        <p class="dstate">No changes against first parent.</p>
      {:else}
        <ul class="filelist" aria-label="Changed files">
          {#each detailFiles as file (file.path)}
            <li
              oncontextmenu={(e) => fileMenu(e, file.path)}
            >
              <span class="fpath" title={file.path}>
                {#if file.old_path}{file.old_path} → {/if}{file.path}
              </span>
              <span class="stats">
                {#if !file.binary}
                  <span class="add">+{file.additions}</span>
                  <span class="del">−{file.deletions}</span>
                {:else}
                  <span class="muted">binary</span>
                {/if}
              </span>
              <button class="blame" onclick={() => openBlame(file.path)}>Blame</button>
              <button
                class="blame"
                title="History of {file.path} (popout)"
                onclick={() =>
                  void openPanelPopout("filehistory", repoId, `History: ${file.path}`, { path: file.path })}
              >History</button>
            </li>
          {/each}
        </ul>
        <div class="ddiff">
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
        <aside class="dmeta">
            <dl>
              <dt>Author</dt>
              <dd>
                {info.author.name}
                <span class="muted">&lt;{info.author.email}&gt;</span><br />
                {formatDateTime(info.author.time)}
                <span class="muted">({formatRelativeTime(info.author.time)})</span>
              </dd>
              <dt>Committer</dt>
              <dd>
                {info.committer.name}<br />
                {formatDateTime(info.committer.time)}
                <span class="muted">({formatRelativeTime(info.committer.time)})</span>
              </dd>
              <dt>Parents</dt>
              <dd>
                {#if info.parents.length === 0}
                  <em>root</em>
                {:else}
                  {#each info.parents as p (p)}
                    <button
                      class="parent"
                      onclick={() => onSelectParent?.(p)}
                      disabled={!onSelectParent}
                      title={"Go to " + p}
                    >
                      {p.slice(0, 7)}
                    </button>
                  {/each}
                {/if}
              </dd>
              <dt>Describe</dt>
              <dd>
                {#if describeText}
                  <code>{describeText}</code>
                {:else}
                  <span class="muted">…</span>
                {/if}
              </dd>
              <!-- M12: commit-signature verification badge. -->
              <dt>Signature</dt>
              <dd><SignatureBadge {repoId} sha={info.sha} /></dd>
              {#if info.refs.length > 0}
                <dt>Refs</dt>
                <dd class="drefs">
                  {#each info.refs as ref (ref)}
                    <span class="ref ref-{classifyRef(ref)}">{shortRefName(ref)}</span>
                  {/each}
                </dd>
              {/if}
            </dl>
            <pre class="msg">{info.message}</pre>
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

<style>
  .detail {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--m3-surface);
  }

  .dhead {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.75rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant);
    font-size: 0.8125rem;
  }

  .dsha {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-primary);
  }

  .dsummary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .dclose,
  .dpop,
  .dtab button,
  button.blame {
    padding: 0.1rem 0.4rem;
    color: var(--m3-on-surface-variant);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
    font-size: 0.72rem;
  }

  .dclose:focus-visible,
  .dpop:focus-visible,
  .dtab button:focus-visible,
  button.blame:focus-visible,
  button.parent:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  /* M12: bookmark star in the detail header. */
  button.star {
    flex: none;
    padding: 0.05rem 0.45rem;
    color: var(--m3-on-surface-variant);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
    font-size: 0.8125rem;
    line-height: 1.3;
  }

  button.star[aria-pressed="true"] {
    color: var(--m3-primary);
    border-color: var(--m3-primary);
  }

  button.star:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .dbody {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  /* SplitPane (meta ↔ files) or the bare files pane fills the body; the
   * child needs :global — the splitter owns its own scope. */
  .dbody > :global(*) {
    flex: 1;
    min-width: 0;
  }

  /* -- commit action bar (M3 E2) ------------------------------------------------ */

  .cactions {
    flex: none;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.35rem;
    padding: 0.25rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface-container-low, var(--m3-surface));
    font-size: 0.72rem;
  }

  button.act {
    padding: 0.18rem 0.6rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-high, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
  }

  button.act:hover:not(:disabled) {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }

  button.act:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  button.act:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .cinfo {
    margin-left: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
  }

  .dmeta {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 0.5rem 0.75rem;
    font-size: 0.75rem;
  }

  .dmeta dl {
    margin: 0;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.25rem 0.75rem;
  }

  .dmeta dt {
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.02em;
    padding-top: 0.1rem;
  }

  .dmeta dd {
    margin: 0;
    color: var(--m3-on-surface);
  }

  .muted {
    color: var(--m3-on-surface-variant);
  }

  button.parent {
    margin-right: 0.25rem;
    padding: 0.05rem 0.3rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-primary);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
  }

  button.parent:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* ref chips (shared shape with the history rows' own copies) */
  .ref {
    flex: none;
    max-width: 9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 0.35rem;
    border-radius: var(--m3-shape-extra-small, 4px);
    font-size: 0.6875rem;
    line-height: 1.4;
  }

  .ref-local {
    color: var(--m3-on-primary-container);
    background: var(--m3-primary-container);
  }

  .ref-remote {
    color: var(--m3-on-tertiary-container);
    background: var(--m3-tertiary-container);
  }

  .ref-tag {
    color: var(--m3-on-secondary-container);
    background: var(--m3-secondary-container);
  }

  .ref-head {
    color: var(--m3-inverse-on-surface);
    background: var(--m3-inverse-surface);
  }

  .drefs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }

  .msg {
    margin: 0.5rem 0 0;
    padding: 0.5rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-low, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  .dfiles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .dtab {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.2rem 0.5rem 0.2rem 0.75rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-low, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant);
  }

  .dtab span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
  }

  .dtabbody {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .dstate {
    margin: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  .filelist {
    flex: none;
    max-height: 30%;
    margin: 0;
    padding: 0.15rem 0;
    list-style: none;
    overflow-y: auto;
    border-bottom: 1px solid var(--m3-outline-variant);
    font-size: 0.72rem;
  }

  .filelist li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.05rem 0.75rem;
  }

  .fpath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
  }

  .stats {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
  }

  .add {
    color: var(--m3-tertiary);
  }

  .del {
    color: var(--m3-error);
  }

  .ddiff {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
</style>
