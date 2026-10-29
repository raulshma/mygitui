<!--
  FileWorkspace — the center-pane tab strip + bodies for the open-file
  set (RepoView owns the state; this component is the view, mirroring the
  EditorPane split: stateless shell, per-file editors keep their own
  state).

  Interaction model (VS Code-style):
  - One tab per open file: icon, name, dirty dot, close button,
    middle-click closes, click activates.
  - All open files stay MOUNTED (inactive panes are display:none) so
    scroll position / undo history / dirty buffers survive tab switches.
  - Text files render FileEditor (CodeMirror, lazy-loaded); images render
    FilePreview inline; binaries and >20MB files get the preview card.
  - Closing a dirty tab asks Save / Don't save / Esc-cancel first; Save
    delegates to that editor's exported `save()`.
  - Header actions: Diff cross-link (working-copy diff for the active
    file), wrap toggle, Close-all (keeps dirty tabs with a toast).
-->
<script lang="ts">
  import type { FileMeta } from "$lib/ipc/types";
  import FileEditor from "./FileEditor.svelte";
  import FilePreview from "./FilePreview.svelte";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { Spinner } from "$lib/components/m3";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { fileIconForPath, fileIconColorForPath } from "$lib/components/icons/fileIcons";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import { editorPrefs } from "./editorPrefs.svelte";
  import { toast } from "$lib/toast";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconDiff from "@ktibow/iconset-material-symbols/difference";
  import iconWrap from "@ktibow/iconset-material-symbols/wrap-text";

  /** One open file (RepoView state). */
  export interface OpenFileInfo {
    path: string;
    dirty: boolean;
    /** Fresh stat at open time; null while the stat round-trip runs. */
    meta: FileMeta | null;
  }

  /** Editor API captured via bind:this per open file. */
  interface EditorApi {
    save(): Promise<boolean>;
    reload(): Promise<void>;
  }

  let {
    repoId,
    root,
    openFiles,
    active = null,
    onActivate,
    onClose,
    onMutated = undefined,
    onOpenDiff = undefined,
  }: {
    repoId: string;
    /** Worktree root (absolute). */
    root: string;
    openFiles: OpenFileInfo[];
    active: string | null;
    onActivate: (path: string) => void;
    onClose: (path: string) => void;
    /** After a successful save (status refresh). */
    onMutated?: () => void;
    /** Diff cross-link for the active file. */
    onOpenDiff?: (path: string) => void;
  } = $props();

  /** Files above this size open as the preview card instead of CM. */
  const LARGE_FILE_LIMIT = 20 * 1024 * 1024;

  let editorRefs: Record<string, EditorApi> = {};
  // Drop stale refs when files close (cleanup on each render pass).
  $effect(() => {
    const alive = new Set(openFiles.map((f) => f.path));
    for (const key of Object.keys(editorRefs)) {
      if (!alive.has(key)) delete editorRefs[key];
    }
  });

  // --- Close flow (dirty guard) --------------------------------------------

  let confirmOpen = $state(false);
  /** Paths awaiting the close decision (multi-close fans out one by one). */
  let pendingClose: string[] = [];

  function requestClose(path: string): void {
    const file = openFiles.find((f) => f.path === path);
    if (!file) return;
    if (!file.dirty) {
      onClose(path);
      return;
    }
    pendingClose.push(path);
    if (!confirmOpen) nextCloseConfirm();
  }

  function nextCloseConfirm(): void {
    const path = pendingClose.shift();
    if (path === undefined) {
      confirmOpen = false;
      return;
    }
    confirmPath = path;
    confirmOpen = true;
  }

  let confirmPath = $state<string | null>(null);

  function confirmSave(): void {
    const path = confirmPath;
    confirmOpen = false;
    if (path === null) return;
    void (async () => {
      const editor = editorRefs[path];
      const saved = editor ? await editor.save() : true;
      if (saved) onClose(path);
      if (pendingClose.length > 0) nextCloseConfirm();
    })();
  }

  function confirmDiscard(): void {
    const path = confirmPath;
    confirmOpen = false;
    if (path !== null) onClose(path);
    // Continue any queued close decisions.
    if (pendingClose.length > 0) nextCloseConfirm();
  }

  /** Esc / Cancel button: abort the close (file stays open, edits kept). */
  function confirmCancel(): void {
    confirmOpen = false;
    pendingClose = [];
  }

  function closeAll(): void {
    const dirtyCount = openFiles.filter((f) => f.dirty).length;
    for (const file of [...openFiles]) {
      if (!file.dirty) onClose(file.path);
    }
    if (dirtyCount > 0) {
      toast(
        `Closed saved files; kept ${dirtyCount} with unsaved edits`,
      );
    }
  }

  function tabMenu(event: MouseEvent, path: string): void {
    event.preventDefault();
    const entries: MenuEntry[] = [
      { id: "close", label: "Close", run: () => requestClose(path) },
      {
        id: "close-others",
        label: "Close others",
        run: () => {
          for (const file of [...openFiles]) {
            if (file.path !== path) requestClose(file.path);
          }
        },
      },
      { id: "close-all", label: "Close all", run: closeAll },
    ];
    showMenuAt(event, entries);
  }

  function basename(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx === -1 ? path : path.slice(idx + 1);
  }

  function usesEditor(meta: FileMeta): boolean {
    return meta.kind === "text" && meta.size <= LARGE_FILE_LIMIT;
  }

  const activeFile = $derived(
    active !== null ? openFiles.find((f) => f.path === active) ?? null : null,
  );

  // Ctrl+S / palette `files.save`: save the active editor (FileEditor also
  // carries an in-editor Mod-s keymap as a fallback).
  $effect(() => {
    return onUiEvent("files-save", () => {
      if (active === null) return;
      const editor = editorRefs[active];
      if (editor) void editor.save();
    });
  });
</script>

<div class="workspace" aria-label="File workspace">
  <div class="strip" role="tablist" aria-label="Open files">
    {#each openFiles as file (file.path)}
      {@const isActive = file.path === active}
      <!-- svelte-ignore a11y_click_events_have_key_events -- keyboard
           handling lives on the tab strip's focusable row buttons + menus -->
      <div
        class="tab"
        class:active={isActive}
        role="tab"
        aria-selected={isActive}
        tabindex="-1"
        title={file.path}
        onclick={() => onActivate(file.path)}
        onauxclick={(event) => {
          if (event.button === 1) requestClose(file.path);
        }}
        oncontextmenu={(event) => tabMenu(event, file.path)}
      >
        <span
          class="ticon"
          style={`color:${fileIconColorForPath(file.path, fileIconPreferences.theme)}`}
        >
          <Icon
            icon={fileIconForPath(file.path, fileIconPreferences.theme)}
            size={13}
          />
        </span>
        <span class="tname">{basename(file.path)}</span>
        {#if file.dirty}
          <span class="dirty" title="Unsaved changes"></span>
        {/if}
        <button
          type="button"
          class="tclose"
          tabindex="-1"
          aria-label={`Close ${basename(file.path)}`}
          title="Close (middle-click)"
          onclick={(event) => {
            event.stopPropagation();
            requestClose(file.path);
          }}
        >
          <Icon icon={iconClose} size={12} />
        </button>
      </div>
    {/each}
    <span class="tspacer"></span>
    <div class="tactions">
      <button
        type="button"
        class="tact"
        title="Open the working-copy diff for this file"
        disabled={active === null || onOpenDiff === undefined}
        onclick={() => active !== null && onOpenDiff?.(active)}
      >
        <Icon icon={iconDiff} size={14} />
        Diff
      </button>
      <button
        type="button"
        class="tact"
        class:tactive={editorPrefs.wrap}
        title={editorPrefs.wrap ? "Disable word wrap" : "Enable word wrap"}
        onclick={() => editorPrefs.setWrap(!editorPrefs.wrap)}
      >
        <Icon icon={iconWrap} size={14} />
        Wrap
      </button>
      <button
        type="button"
        class="tact"
        title="Close all saved files"
        onclick={closeAll}
      >
        <Icon icon={iconClose} size={14} />
        Close all
      </button>
    </div>
  </div>

  <div class="bodies">
    {#each openFiles as file (file.path)}
      <div class="pane" class:hidden={file.path !== active}>
        {#if file.meta === null}
          <p class="state" role="status"><Spinner size={14} /> Reading {file.path}…</p>
        {:else if usesEditor(file.meta)}
          <FileEditor
            bind:this={editorRefs[file.path]}
            {repoId}
            path={file.path}
            meta={file.meta}
            bind:dirty={file.dirty}
            onSaved={onMutated}
          />
        {:else}
          <FilePreview {repoId} {root} path={file.path} meta={file.meta} />
        {/if}
      </div>
    {/each}
    {#if activeFile === null && openFiles.length === 0}
      <p class="state">No files open</p>
    {/if}
  </div>

  <DialogShell
    bind:open={confirmOpen}
    labelledBy="fw-close-title"
    describedBy="fw-close-body"
    width="22rem"
    onCancel={confirmCancel}
  >
    {#snippet children()}
      <h3 class="dlg-title" id="fw-close-title">Unsaved changes</h3>
      <p class="dlg-body" id="fw-close-body">
        {confirmPath === null
          ? ""
          : `${confirmPath} has unsaved edits. Save before closing?`}
      </p>
    {/snippet}
    {#snippet actions()}
      <button
        class="dlg-btn dlg-primary"
        type="button"
        data-autofocus
        onclick={confirmSave}
      >
        Save
      </button>
      <button class="dlg-btn" type="button" onclick={confirmDiscard}>
        Don't save
      </button>
      <button class="dlg-btn" type="button" onclick={confirmCancel}>
        Cancel
      </button>
    {/snippet}
  </DialogShell>
</div>

<style>
  .workspace {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--m3-surface);
  }

  .strip {
    flex: none;
    display: flex;
    align-items: stretch;
    gap: 0;
    overflow-x: auto;
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
    background: var(--m3-surface-container, var(--m3-surface));
    scrollbar-width: thin;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0 0.3rem 0 0.6rem;
    height: 2rem;
    max-width: 14rem;
    cursor: pointer;
    border-right: 1px solid var(--app-border, var(--m3-outline-variant));
    color: var(--m3-on-surface-variant);
    font-size: 0.78rem;
    white-space: nowrap;
    user-select: none;
    flex: none;
  }

  .tab:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
  }

  .tab.active {
    background: var(--m3-surface);
    color: var(--m3-on-surface);
    box-shadow: inset 0 2px 0 var(--m3-primary);
  }

  .ticon {
    display: inline-flex;
    flex: none;
  }

  .tname {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .dirty {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 9999px;
    background: var(--app-modified, var(--m3-tertiary));
  }

  .tclose {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.15rem;
    height: 1.15rem;
    flex: none;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: inherit;
    cursor: pointer;
    opacity: 0.65;
  }

  .tclose:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
    opacity: 1;
  }

  .tspacer {
    flex: 1;
  }

  .tactions {
    display: flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0 0.4rem;
    flex: none;
  }

  .tact {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.1rem 0.45rem;
    border: none;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant);
    font: inherit;
    font-size: 0.6875rem;
    cursor: pointer;
  }

  .tact:hover:not(:disabled) {
    background: var(--app-hover, var(--m3-surface-container-high));
    color: var(--m3-on-surface);
  }

  .tact:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .tact.tactive {
    color: var(--m3-primary);
    background: color-mix(in srgb, var(--m3-primary) 10%, transparent);
  }

  .tact:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .bodies {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .pane {
    flex: 1;
    min-height: 0;
  }

  .pane.hidden {
    display: none;
  }

  .pane > :global(*) {
    height: 100%;
  }

  .state {
    margin: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  p.state {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .dlg-title {
    margin: 0 0 0.4rem;
    font-size: 0.9375rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .dlg-body {
    margin: 0;
    font-size: 0.8125rem;
    line-height: 1.45;
    color: var(--m3-on-surface-variant);
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    word-break: break-all;
  }

  .dlg-btn {
    padding: 0.3rem 0.85rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.78rem;
    cursor: pointer;
  }

  .dlg-btn:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .dlg-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .dlg-primary {
    background: var(--m3-primary);
    border-color: transparent;
    color: var(--m3-on-primary);
  }

  .dlg-primary:hover {
    background: var(--m3-primary);
    filter: brightness(1.06);
  }
</style>
