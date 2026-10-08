<script lang="ts">
  /**
   * M3 conflict editor (E1 lane): resolves the marker previews the engine
   * writes into the workdir for conflicted paths (see `engine::merge`).
   *
   * Layout: conflicted-file sidebar (✓ = marked resolved, • = unsaved
   * choices) + a main pane that renders the selected file's segments in
   * order — plain context lines and, per conflict, ours/theirs panes side
   * by side with Choose ours / Choose theirs / Edit. Edit swaps the panes
   * for an inline textarea prefilled with the current effective content
   * (ours until edited); typed text is custom content and wins over the
   * buttons wherever no explicit choice was made afterwards.
   *
   * Persistence: "Mark resolved" serializes the file (resolved conflicts →
   * chosen content, unresolved → markers kept verbatim) and calls
   * `conflictResolve(repoId, path, "both", bytes)` — the engine gives the
   * custom bytes precedence, writing + staging + resolving in one step.
   * Any choice change after a mark un-marks the file (the mark would be
   * stale). Files whose workdir copy parses to zero conflicts (binary,
   * already flat) are sent through unchanged so the path still resolves.
   *
   * Workdir content arrives through `client.readFile` (`repo_read_file`,
   * appended for this lane) once per file, lazily on first selection.
   *
   * Top bar: the conflict source chip (merge / cherry-pick / revert /
   * rebase), "Resolved: k of n" progress, Abort Merge (two-step confirm →
   * `mergeAbort` + `onResolved`) and Done (enabled only when every file is
   * marked → `onResolved`). Keyboard: Ctrl+Enter marks the current file
   * resolved, F3 jumps to the next unresolved conflict (next unmarked file
   * when the current one is clean). Errors toast; resolution is busy-gated
   * per file.
   */
  import { tick } from "svelte";
  import { conflictResolve, mergeAbort, readFile } from "$lib/ipc/client";
  import type { ConflictFile } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import {
    applyResolutions,
    countConflicts,
    decodeUtf8,
    effectiveContent,
    encodeUtf8,
    parseConflicts,
    unresolvedConflicts,
    type ResolutionChoice,
    type Segment,
  } from "./conflictModel";

  let {
    repoId,
    files,
    onResolved,
    onClose,
  }: {
    repoId: string;
    files: ConflictFile[];
    /** After all conflicts resolved (parent refreshes status). */
    onResolved: () => void;
    onClose: () => void;
  } = $props();

  // ---------------------------------------------------------------------------
  // State (all keyed by workdir path)
  // ---------------------------------------------------------------------------

  /** Decoded workdir text per file; null/absent = not loaded yet. */
  let texts = $state<Record<string, string | null>>({});
  /** Raw bytes per file (fallback for unparseable / binary content). */
  let rawBytes = $state<Record<string, number[] | null>>({});
  let loadErrors = $state<Record<string, string>>({});
  /** Explicit per-conflict choices: path → (conflict index → choice). */
  let choicesByFile = $state<Record<string, Record<number, ResolutionChoice>>>({});
  /** Custom edited content: path → (conflict index → text). */
  let customsByFile = $state<Record<string, Record<number, string>>>({});
  /** Files pushed through `conflict_resolve` (staged + resolved). */
  let marked = $state<Record<string, boolean>>({});
  /** Conflict ordinal currently open in the Edit textarea, per file. */
  let editing = $state<Record<string, number>>({});

  // "" until the validity effect below picks the first file (avoids
  // capturing only the initial `files` value).
  let selectedPath = $state("");
  let cursor = $state(0);
  let busyPath = $state<string | null>(null);
  let abortArmed = $state(false);

  /** Conflict block elements by ordinal (scroll targets; non-reactive). */
  const blockEls: Record<number, HTMLElement | undefined> = {};

  // ---------------------------------------------------------------------------
  // Derived view of the selected file
  // ---------------------------------------------------------------------------

  const selected = $derived(
    files.find((f) => f.path === selectedPath) ?? files[0] ?? null,
  );
  const text = $derived(
    selected && texts[selected.path] !== undefined ? (texts[selected.path] ?? null) : null,
  );
  const segments = $derived(
    selected && text !== null ? parseConflicts(text, selected.path) : ([] as Segment[]),
  );
  const conflictSegments = $derived(
    segments.flatMap((s) => (s.type === "conflict" ? [s] : [])),
  );
  const choiceMap = $derived(mapOf(choicesByFile, selected?.path));
  const customMap = $derived(mapOf(customsByFile, selected?.path));
  const total = $derived(conflictSegments.length);
  const unresolved = $derived(unresolvedConflicts(segments, choiceMap, customMap));
  const resolvedFiles = $derived(files.filter((f) => marked[f.path]).length);
  const allResolved = $derived(files.length > 0 && resolvedFiles === files.length);
  const busy = $derived(busyPath !== null);
  /** Unsaved choices/custom text that a "Mark resolved" has not persisted. */
  const dirty = $derived(selected !== null && !marked[selected.path]);

  /** Segments in file order with their conflict ordinals attached. */
  const renderItems = $derived.by(() => {
    let ordinal = -1;
    return segments.map((segment, key) => {
      if (segment.type === "line") return { kind: "line" as const, segment, key };
      ordinal += 1;
      return { kind: "conflict" as const, segment, ordinal, key };
    });
  });

  /** Plain record → Map (the pure model works on Maps). */
  function mapOf<T>(
    record: Record<string, Record<number, T>>,
    path: string | undefined,
  ): Map<number, T> {
    const out = new Map<number, T>();
    const inner = path === undefined ? undefined : record[path];
    if (inner !== undefined) {
      for (const key of Object.keys(inner)) {
        const value = inner[Number(key)];
        if (value !== undefined) out.set(Number(key), value);
      }
    }
    return out;
  }

  function errorMessage(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  /** Conflict count of a (loaded) file; null while content is unavailable. */
  function fileConflictTotal(path: string): number | null {
    const body = texts[path];
    if (body === null || body === undefined) return null;
    return countConflicts(parseConflicts(body, path));
  }

  /** Sidebar marker: has unpersisted choices / custom edits. */
  function fileDirty(path: string): boolean {
    return (
      Object.keys(choicesByFile[path] ?? {}).length > 0 ||
      Object.keys(customsByFile[path] ?? {}).length > 0
    );
  }

  // ---------------------------------------------------------------------------
  // Loading (readFile once per file)
  // ---------------------------------------------------------------------------

  async function ensureLoaded(path: string): Promise<void> {
    if (texts[path] !== undefined || loadErrors[path] !== undefined) return;
    try {
      const bytes = await readFile(repoId, path);
      rawBytes[path] = bytes;
      texts[path] = decodeUtf8(bytes);
    } catch (err) {
      loadErrors[path] = errorMessage(err);
    }
  }

  $effect(() => {
    const path = selectedPath;
    if (path !== "") void ensureLoaded(path);
  });

  // Keep the selection valid when the parent refreshes `files`.
  $effect(() => {
    if (files.length > 0 && !files.some((f) => f.path === selectedPath)) {
      selectedPath = files[0]!.path;
    }
  });

  function retryLoad(path: string): void {
    delete loadErrors[path];
    void ensureLoaded(path);
  }

  // ---------------------------------------------------------------------------
  // Choices, custom edits, navigation
  // ---------------------------------------------------------------------------

  /** Any mutation invalidates a previous mark for that file. */
  function touch(path: string): void {
    if (marked[path]) marked[path] = false;
  }

  function choose(index: number, choice: "ours" | "theirs"): void {
    const path = selected?.path;
    if (path === undefined || busyPath === path) return;
    choicesByFile[path] = { ...(choicesByFile[path] ?? {}), [index]: choice };
    if (customsByFile[path]?.[index] !== undefined) {
      const customs = { ...customsByFile[path] };
      delete customs[index];
      customsByFile[path] = customs;
    }
    touch(path);
    cursor = index;
  }

  function startEdit(index: number): void {
    const path = selected?.path;
    if (path === undefined || busyPath === path) return;
    const customs = { ...(customsByFile[path] ?? {}) };
    // Prefill with the current effective content (ours until edited).
    if (customs[index] === undefined) {
      customs[index] = conflictSegments[index]?.ours ?? "";
    }
    customsByFile[path] = customs;
    editing[path] = index;
    cursor = index;
    touch(path);
  }

  function stopEdit(): void {
    const path = selected?.path;
    if (path === undefined) return;
    delete editing[path];
  }

  function setCustom(index: number, value: string): void {
    const path = selected?.path;
    if (path === undefined) return;
    customsByFile[path] = { ...(customsByFile[path] ?? {}), [index]: value };
    touch(path);
  }

  /** Bulk-fill every conflict in the current file (quick action). */
  function resolveAll(choice: "ours" | "theirs"): void {
    const path = selected?.path;
    if (path === undefined || busyPath === path || total === 0) return;
    const next: Record<number, ResolutionChoice> = { ...(choicesByFile[path] ?? {}) };
    for (let i = 0; i < total; i++) next[i] = choice;
    choicesByFile[path] = next;
    delete editing[path];
    touch(path);
  }

  function conflictStatus(index: number): string {
    if (customMap.has(index)) return "edited";
    const choice = choiceMap.get(index);
    return choice === undefined ? "unresolved" : choice;
  }

  function isResolved(index: number): boolean {
    return customMap.has(index) || choiceMap.has(index);
  }

  function isEditing(index: number): boolean {
    return selected !== null && editing[selected.path] === index;
  }

  /** Draft text for the Edit textarea (custom wins, else effective). */
  function draftFor(index: number): string {
    const conflict = conflictSegments[index];
    if (conflict === undefined || selected === null) return "";
    return effectiveContent(conflict, choiceMap, customMap, index);
  }

  async function reveal(index: number): Promise<void> {
    cursor = index;
    await tick();
    blockEls[index]?.scrollIntoView({ block: "center" });
  }

  /** F3: next unresolved conflict; past the last one, the next unmarked file. */
  async function nextUnresolved(): Promise<void> {
    const upcoming = unresolved.find((index) => index > cursor) ?? unresolved[0];
    if (upcoming !== undefined) {
      await reveal(upcoming);
      return;
    }
    const startIndex = files.findIndex((f) => f.path === selectedPath);
    for (let step = 1; step <= files.length; step++) {
      const file = files[(startIndex + step) % files.length];
      if (file === undefined || marked[file.path]) continue;
      selectedPath = file.path;
      cursor = 0;
      return;
    }
    toast("No unresolved conflicts left", { kind: "info" });
  }

  async function prevConflict(): Promise<void> {
    if (total === 0) return;
    await reveal((cursor - 1 + total) % total);
  }

  function onKeydown(event: KeyboardEvent): void {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      void markResolved();
    } else if (event.key === "F3") {
      event.preventDefault();
      void nextUnresolved();
    }
  }

  // ---------------------------------------------------------------------------
  // Persistence
  // ---------------------------------------------------------------------------

  /**
   * The bytes "Mark resolved" sends: the serialized file with resolutions
   * applied; zero-conflict files go through as the untouched original bytes
   * (re-encoding decoded binary would corrupt it).
   */
  function serializeFor(path: string): Uint8Array {
    const body = texts[path];
    const original = rawBytes[path];
    if (body === null || body === undefined) return Uint8Array.from(original ?? []);
    const fileSegments = parseConflicts(body, path);
    if (countConflicts(fileSegments) === 0) return Uint8Array.from(original ?? []);
    return encodeUtf8(
      applyResolutions(fileSegments, mapOf(choicesByFile, path), mapOf(customsByFile, path)),
    );
  }

  async function markResolved(): Promise<void> {
    const file = selected;
    if (file === null || busyPath !== null) return;
    if (text === null) {
      toast(`Cannot resolve ${file.path}: content not loaded`, { kind: "error" });
      return;
    }
    if (unresolved.length > 0) {
      toast(
        `${file.path}: ${unresolved.length} conflict${unresolved.length === 1 ? "" : "s"} still unresolved`,
        { kind: "error" },
      );
      return;
    }
    busyPath = file.path;
    try {
      await conflictResolve(repoId, file.path, "both", serializeFor(file.path));
      marked[file.path] = true;
      delete editing[file.path];
      const next = files.find((f) => !marked[f.path]);
      if (next !== undefined && next.path !== file.path) {
        selectedPath = next.path;
        cursor = 0;
      }
    } catch (err) {
      toast(`Resolving ${file.path} failed: ${errorMessage(err)}`, { kind: "error" });
    } finally {
      busyPath = null;
    }
  }

  let abortTimer: ReturnType<typeof setTimeout> | undefined;

  /** First click arms (4 s), second click aborts the merge for real. */
  async function abortMerge(): Promise<void> {
    if (!abortArmed) {
      abortArmed = true;
      clearTimeout(abortTimer);
      abortTimer = setTimeout(() => (abortArmed = false), 4_000);
      return;
    }
    abortArmed = false;
    clearTimeout(abortTimer);
    if (busyPath !== null) return;
    busyPath = "::abort";
    try {
      await mergeAbort(repoId);
      onResolved();
    } catch (err) {
      toast(`Merge abort failed: ${errorMessage(err)}`, { kind: "error" });
    } finally {
      busyPath = null;
    }
  }
</script>

<!-- Shortcuts work anywhere while the editor is mounted (including inside
     the Edit textarea): Ctrl+Enter marks resolved, F3 jumps to the next
     unresolved conflict. -->
<svelte:window onkeydown={onKeydown} />

<div class="root">
  <header class="topbar">
    <span class="chip source" title="Operation that produced these conflicts">
      {files[0]?.source ?? "merge"}
    </span>
    <span class="progress" aria-live="polite">
      Resolved: {resolvedFiles} of {files.length} file{files.length === 1 ? "" : "s"}
      {#if selected}
        · {total - unresolved.length}/{total} conflicts in {selected.path}
      {/if}
    </span>
    <span class="spacer"></span>
    <button
      class="tb abort"
      type="button"
      class:armed={abortArmed}
      disabled={busy}
      onclick={() => void abortMerge()}
    >
      {abortArmed ? "Confirm abort?" : "Abort merge"}
    </button>
    <button
      class="tb done"
      type="button"
      disabled={!allResolved || busy}
      title={allResolved ? "Finish: every file is resolved and staged" : "Mark every file resolved first"}
      onclick={() => onResolved()}
    >
      Done
    </button>
    <button class="tb" type="button" aria-label="Close conflict editor" onclick={() => onClose()}>
      Close
    </button>
  </header>

  <div class="body">
    <aside class="files" aria-label="Conflicted files">
      {#each files as file (file.path)}
        <button
          class="file"
          type="button"
          class:active={file.path === selected?.path}
          class:marked={marked[file.path]}
          aria-current={file.path === selected?.path ? "true" : undefined}
          onclick={() => {
            selectedPath = file.path;
            cursor = 0;
          }}
        >
          <span class="mark" aria-hidden="true">
            {#if busyPath === file.path}…{:else if marked[file.path]}✓{:else if fileDirty(file.path)}•{/if}
          </span>
          <span class="filepath">{file.path}</span>
          {#if fileConflictTotal(file.path) !== null}
            <span class="count">{fileConflictTotal(file.path)}</span>
          {/if}
        </button>
      {/each}
    </aside>

    <section class="main" aria-label="Conflict editor">
      {#if selected === null}
        <p class="empty">No conflicted files.</p>
      {:else if loadErrors[selected.path] !== undefined}
        <div class="loaderror" role="alert">
          <p>Could not load {selected.path}: {loadErrors[selected.path]}</p>
          <button class="tb" type="button" onclick={() => retryLoad(selected!.path)}>Retry</button>
        </div>
      {:else if text === null}
        <p class="empty">Loading {selected.path}…</p>
      {:else}
        {#if !selected.has_base}
          <p class="hint" role="note">
            Base unavailable (rename/add conflict) — showing ours vs theirs only.
          </p>
        {/if}
        <div class="content">
          {#each renderItems as item (item.key)}
            {#if item.kind === "line"}
              <pre class="ctx">{item.segment.text}</pre>
            {:else}
              {@const index = item.ordinal}
              {@const conflict = item.segment}
              <div
                class="conflict"
                class:active={index === cursor}
                class:done={isResolved(index)}
                bind:this={blockEls[index]}
              >
                <div class="head">
                  <span class="idx">#{index + 1}</span>
                  <span class="status {conflictStatus(index)}">{conflictStatus(index)}</span>
                  <span class="head-spacer"></span>
                  <button
                    class="tb"
                    type="button"
                    disabled={busyPath === selected.path}
                    aria-label={`Choose ours for conflict ${index + 1}`}
                    onclick={() => choose(index, "ours")}>Choose ours</button
                  >
                  <button
                    class="tb"
                    type="button"
                    disabled={busyPath === selected.path}
                    aria-label={`Choose theirs for conflict ${index + 1}`}
                    onclick={() => choose(index, "theirs")}>Choose theirs</button
                  >
                  {#if isEditing(index)}
                    <button
                      class="tb"
                      type="button"
                      disabled={busyPath === selected.path}
                      onclick={stopEdit}>Done editing</button
                    >
                  {:else}
                    <button
                      class="tb"
                      type="button"
                      disabled={busyPath === selected.path}
                      aria-label={`Edit conflict ${index + 1}`}
                      onclick={() => startEdit(index)}>Edit</button
                    >
                  {/if}
                </div>
                {#if isEditing(index)}
                  <textarea
                    class="custom"
                    aria-label={`Custom resolution for conflict ${index + 1}`}
                    rows={Math.max(3, (customMap.get(index) ?? "").split("\n").length)}
                    value={draftFor(index)}
                    disabled={busyPath === selected.path}
                    oninput={(event) => setCustom(index, event.currentTarget.value)}
                  ></textarea>
                {:else}
                  <div class="panes">
                    <pre class="pane ours" class:picked={choiceMap.get(index) === "ours"}
                      >{conflict.ours}</pre
                    >
                    <pre class="pane theirs" class:picked={choiceMap.get(index) === "theirs"}
                      >{conflict.theirs}</pre
                    >
                  </div>
                {/if}
              </div>
            {/if}
          {/each}
        </div>

        <footer class="filebar">
          <button class="tb" type="button" disabled={total === 0} onclick={() => void prevConflict()}>
            ‹ Prev
          </button>
          <button
            class="tb"
            type="button"
            disabled={total === 0}
            onclick={() => void nextUnresolved()}>Next unresolved (F3)</button
          >
          <span class="head-spacer"></span>
          <button
            class="tb"
            type="button"
            disabled={total === 0 || busyPath === selected.path}
            title="Resolve every conflict in this file with our side"
            onclick={() => resolveAll("ours")}>Resolve all ours</button
          >
          <button
            class="tb"
            type="button"
            disabled={total === 0 || busyPath === selected.path}
            title="Resolve every conflict in this file with their side"
            onclick={() => resolveAll("theirs")}>Resolve all theirs</button
          >
          <span class="head-spacer"></span>
          {#if dirty}
            <span class="unsaved" title="Choices not written yet — mark the file resolved to persist">
              unsaved
            </span>
          {/if}
          <button
            class="tb primary"
            type="button"
            disabled={busyPath === selected.path || text === null || unresolved.length > 0}
            title={
              unresolved.length > 0
                ? `${unresolved.length} conflict(s) still unresolved`
                : "Write the resolved content, stage and clear the conflict"
            }
            onclick={() => void markResolved()}>Mark resolved (Ctrl+Enter)</button
          >
        </footer>
      {/if}
    </section>
  </div>
</div>

<style>
  .root {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface, canvas);
    color: var(--m3-on-surface);
    font-size: 0.8125rem;
  }

  .topbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.375rem 0.5rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex: none;
  }

  .chip {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.0625rem 0.5rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    white-space: nowrap;
  }

  .chip.source {
    background: var(--m3-tertiary-container, transparent);
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
    border-color: transparent;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .progress {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .spacer,
  .head-spacer {
    flex: 1;
  }

  .tb {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .tb:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .tb.abort.armed {
    background: var(--m3-error-container, transparent);
    color: var(--m3-on-error-container, var(--m3-error));
    border-color: var(--m3-error);
    font-weight: 600;
  }

  .tb.done:not(:disabled),
  .tb.primary:not(:disabled) {
    background: var(--m3-primary-container, transparent);
    color: var(--m3-on-primary-container, var(--m3-primary));
    border-color: transparent;
    font-weight: 600;
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .files {
    width: 14rem;
    flex: none;
    overflow-y: auto;
    border-right: 1px solid var(--m3-outline-variant, var(--m3-primary));
    padding: 0.25rem 0;
    display: flex;
    flex-direction: column;
    gap: 0.0625rem;
  }

  .file {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    width: 100%;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    text-align: left;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
    min-width: 0;
  }

  .file:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .file:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .file.active {
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
  }

  .file.marked .filepath {
    text-decoration: line-through;
    opacity: 0.7;
  }

  .mark {
    flex: none;
    width: 0.875rem;
    color: var(--m3-primary);
    font-weight: 700;
    text-align: center;
  }

  .file.marked .mark {
    color: var(--m3-tertiary, var(--m3-primary));
  }

  .filepath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .count {
    flex: none;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.4rem;
    font-size: 0.6875rem;
    font-variant-numeric: tabular-nums;
  }

  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .hint {
    margin: 0;
    padding: 0.25rem 0.625rem;
    font-size: 0.6875rem;
    color: var(--m3-on-tertiary-container, var(--m3-tertiary));
    background: var(--m3-tertiary-container, transparent);
    flex: none;
  }

  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.375rem 0.625rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    line-height: 1.35;
  }

  .ctx {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .conflict {
    margin: 0.375rem 0;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    overflow: hidden;
  }

  .conflict.active {
    outline: 2px solid var(--m3-primary);
    outline-offset: 0;
  }

  .conflict.done {
    border-color: var(--m3-tertiary, var(--m3-outline-variant));
  }

  .head {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.25rem 0.375rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex-wrap: wrap;
  }

  .idx {
    font-variant-numeric: tabular-nums;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .status {
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0 0.375rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-error-container, transparent);
    color: var(--m3-on-error-container, var(--m3-error));
  }

  .status.ours,
  .status.theirs {
    background: var(--m3-primary-container, transparent);
    color: var(--m3-on-primary-container, var(--m3-primary));
  }

  .status.edited {
    background: var(--m3-secondary-container, transparent);
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
  }

  .panes {
    display: grid;
    grid-template-columns: 1fr 1fr;
  }

  .pane {
    margin: 0;
    padding: 0.25rem 0.375rem;
    white-space: pre-wrap;
    word-break: break-word;
    min-width: 0;
  }

  .pane.ours {
    background: color-mix(in srgb, var(--m3-primary-container, transparent) 45%, transparent);
    border-right: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .pane.theirs {
    background: color-mix(in srgb, var(--m3-tertiary-container, transparent) 45%, transparent);
  }

  .pane.picked {
    box-shadow: inset 0 0 0 2px var(--m3-primary);
  }

  .custom {
    display: block;
    width: 100%;
    box-sizing: border-box;
    border: none;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    color: inherit;
    font: inherit;
    font-family: inherit;
    line-height: inherit;
    padding: 0.25rem 0.375rem;
    resize: vertical;
    min-height: 3rem;
  }

  .custom:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .filebar {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex: none;
    flex-wrap: wrap;
  }

  .unsaved {
    font-size: 0.6875rem;
    color: var(--m3-on-tertiary-container, var(--m3-tertiary));
    background: var(--m3-tertiary-container, transparent);
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.0625rem 0.5rem;
  }

  .empty,
  .loaderror {
    padding: 1rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .loaderror p {
    margin: 0 0 0.5rem;
    color: var(--m3-on-error-container, var(--m3-error));
  }
</style>
