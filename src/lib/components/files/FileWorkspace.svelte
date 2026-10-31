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
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Toggle } from "$lib/components/ui/toggle/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { fileIconForPath, fileIconColorForPath } from "$lib/components/icons/fileIcons";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import { editorPrefs } from "./editorPrefs.svelte";
  import { isMarkdownPath } from "./markdownPath";
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

  // Ctrl+Shift+V / palette `files.markdown-view`: cycle the markdown
  // preview layout (edit → split → preview), persisted via editorPrefs.
  $effect(() => {
    return onUiEvent("files-md-view", () => {
      if (active === null || !isMarkdownPath(active)) return;
      editorPrefs.cycleMdView();
    });
  });

  const mdActive = $derived(active !== null && isMarkdownPath(active));

  /** The editor's preview prop: markdown pref mapped, "edit" = no preview. */
  function previewModeFor(path: string): "off" | "split" | "preview" {
    if (!isMarkdownPath(path)) return "off";
    const view = editorPrefs.mdView;
    return view === "edit" ? "off" : view;
  }

  const MD_VIEW_SEGMENTS: Array<{ id: "edit" | "split" | "preview"; label: string; title: string }> = [
    { id: "edit", label: "Edit", title: "Markdown source only" },
    { id: "split", label: "Split", title: "Source and preview side by side" },
    { id: "preview", label: "Preview", title: "Rendered preview only" },
  ];
</script>

<div class="workspace flex h-full min-h-0 min-w-0 flex-col bg-card" aria-label="File workspace">
  <div
    class="strip flex flex-none items-stretch gap-0 overflow-x-auto border-b border-border bg-card [scrollbar-width:thin]"
    role="tablist"
    aria-label="Open files"
  >
    {#each openFiles as file (file.path)}
      {@const isActive = file.path === active}
      <!-- svelte-ignore a11y_click_events_have_key_events -- keyboard
           handling lives on the tab strip's focusable row buttons + menus -->
      <div
        class="tab flex h-8 max-w-56 flex-none cursor-pointer items-center gap-[0.3rem] border-r border-border py-0 pl-[0.6rem] pr-[0.3rem] whitespace-nowrap select-none text-[0.78rem] {isActive
          ? 'bg-card text-foreground shadow-[inset_0_2px_0_var(--primary)]'
          : 'text-muted-foreground hover:bg-accent'}"
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
          class="ticon inline-flex flex-none"
          style={`color:${fileIconColorForPath(file.path, fileIconPreferences.theme)}`}
        >
          <Icon
            icon={fileIconForPath(file.path, fileIconPreferences.theme)}
            size={13}
          />
        </span>
        <span class="tname min-w-0 overflow-hidden text-ellipsis">{basename(file.path)}</span>
        {#if file.dirty}
          <span class="dirty size-[7px] flex-none rounded-full bg-[var(--app-modified)]" title="Unsaved changes"></span>
        {/if}
        <Button
          class="tclose size-[1.15rem] rounded-full text-muted-foreground opacity-65 hover:bg-accent hover:text-accent-foreground hover:opacity-100 [&_svg:not([class*='size-'])]:size-3"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label={`Close ${basename(file.path)}`}
          title="Close (middle-click)"
          tabindex={-1}
          onclick={(event) => {
            event.stopPropagation();
            requestClose(file.path);
          }}
        >
          <Icon icon={iconClose} size={12} />
        </Button>
      </div>
    {/each}
    <span class="tspacer flex-1"></span>
    <div class="tactions flex flex-none items-center gap-0.5 px-1">
      {#if mdActive}
        <div class="seg inline-flex flex-none items-center gap-0 rounded-full border border-border px-1" role="group" aria-label="Markdown view">
          {#each MD_VIEW_SEGMENTS as seg (seg.id)}
            <Toggle
              class="h-auto rounded-full px-[0.55rem] py-[0.1rem] text-[0.6875rem] font-normal focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary focus-visible:ring-0 {editorPrefs.mdView === seg.id
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:text-foreground'}"
              pressed={editorPrefs.mdView === seg.id}
              onPressedChange={(next) => next && editorPrefs.setMdView(seg.id)}
              title={seg.title}
            >
              {seg.label}
            </Toggle>
          {/each}
        </div>
      {/if}
      <Button
        type="button"
        class="tact h-auto gap-1 rounded-sm px-[0.45rem] py-[0.1rem] text-[0.6875rem] font-normal disabled:cursor-default disabled:opacity-45"
        variant="ghost"
        title="Open the working-copy diff for this file"
        disabled={active === null || onOpenDiff === undefined}
        onclick={() => active !== null && onOpenDiff?.(active)}
      >
        <Icon icon={iconDiff} size={14} />
        Diff
      </Button>
      <Button
        type="button"
        class="tact h-auto gap-1 rounded-sm px-[0.45rem] py-[0.1rem] text-[0.6875rem] font-normal {editorPrefs.wrap
          ? 'bg-primary/10 text-primary'
          : ''}"
        variant="ghost"
        title={editorPrefs.wrap ? "Disable word wrap" : "Enable word wrap"}
        onclick={() => editorPrefs.setWrap(!editorPrefs.wrap)}
      >
        <Icon icon={iconWrap} size={14} />
        Wrap
      </Button>
      <Button
        type="button"
        class="tact h-auto gap-1 rounded-sm px-[0.45rem] py-[0.1rem] text-[0.6875rem] font-normal"
        variant="ghost"
        title="Close all saved files"
        onclick={closeAll}
      >
        <Icon icon={iconClose} size={14} />
        Close all
      </Button>
    </div>
  </div>

  <div class="bodies flex min-h-0 flex-1 flex-col">
    {#each openFiles as file (file.path)}
      <div class="pane min-h-0 flex-1 [&>*]:h-full" class:hidden={file.path !== active}>
        {#if file.meta === null}
          <p class="state m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground" role="status"><Spinner class="size-[14px]" role="presentation" aria-hidden="true" /> Reading {file.path}…</p>
        {:else if usesEditor(file.meta)}
          <FileEditor
            bind:this={editorRefs[file.path]}
            {repoId}
            path={file.path}
            meta={file.meta}
            bind:dirty={file.dirty}
            onSaved={onMutated}
            previewMode={previewModeFor(file.path)}
          />
        {:else}
          <FilePreview {repoId} {root} path={file.path} meta={file.meta} />
        {/if}
      </div>
    {/each}
    {#if activeFile === null && openFiles.length === 0}
      <p class="state m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground">No files open</p>
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
      <h3 class="dlg-title m-0 mb-[0.4rem] text-[0.9375rem] font-medium text-foreground" id="fw-close-title">Unsaved changes</h3>
      <p class="dlg-body m-0 break-all font-mono text-[0.8125rem] leading-[1.45] text-muted-foreground" id="fw-close-body">
        {confirmPath === null
          ? ""
          : `${confirmPath} has unsaved edits. Save before closing?`}
      </p>
    {/snippet}
    {#snippet actions()}
      <Button
        class="dlg-btn dlg-primary h-auto rounded-full px-[0.85rem] py-[0.3rem] text-[0.78rem]"
        type="button"
        data-autofocus
        onclick={confirmSave}
      >
        Save
      </Button>
      <Button
        class="dlg-btn h-auto rounded-full px-[0.85rem] py-[0.3rem] text-[0.78rem] font-normal"
        variant="outline"
        type="button"
        onclick={confirmDiscard}
      >
        Don't save
      </Button>
      <Button
        class="dlg-btn h-auto rounded-full px-[0.85rem] py-[0.3rem] text-[0.78rem] font-normal"
        variant="outline"
        type="button"
        onclick={confirmCancel}
      >
        Cancel
      </Button>
    {/snippet}
  </DialogShell>
</div>
