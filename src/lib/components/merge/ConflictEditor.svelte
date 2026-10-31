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
  import { Button } from "$lib/components/ui/button/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
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

  /** Classes for the small toolbar buttons (idle vs tonal emphasis). */
  const tbIdle = "text-muted-foreground";
  const tbTonal = "bg-primary/15 font-semibold text-primary hover:bg-primary/25";
</script>

<!-- Shortcuts work anywhere while the editor is mounted (including inside
     the Edit textarea): Ctrl+Enter marks resolved, F3 jumps to the next
     unresolved conflict. -->
<svelte:window onkeydown={onKeydown} />

<div class="root flex h-full min-h-0 flex-col rounded-lg border border-border bg-background text-foreground text-[0.8125rem]">
  <header class="topbar flex flex-none items-center gap-2 border-b border-border px-2 py-1.5">
    <Badge
      variant="outline"
      class="source h-auto whitespace-nowrap rounded-full border-transparent bg-primary/10 px-2 py-0.5 text-[0.6875rem] font-semibold uppercase tracking-wider text-primary"
      title="Operation that produced these conflicts"
    >
      {files[0]?.source ?? "merge"}
    </Badge>
    <span
      class="progress overflow-hidden text-ellipsis whitespace-nowrap text-xs tabular-nums text-muted-foreground"
      aria-live="polite"
    >
      Resolved: {resolvedFiles} of {files.length} file{files.length === 1 ? "" : "s"}
      {#if selected}
        · {total - unresolved.length}/{total} conflicts in {selected.path}
      {/if}
    </span>
    <span class="spacer flex-1"></span>
    <Button
      variant={abortArmed ? "destructive" : "outline"}
      class={abortArmed ? "" : tbIdle}
      size="xs"
      type="button"
      disabled={busy}
      onclick={() => void abortMerge()}
    >
      {abortArmed ? "Confirm abort?" : "Abort merge"}
    </Button>
    <Button
      variant={!allResolved || busy ? "outline" : "ghost"}
      class={!allResolved || busy ? tbIdle : tbTonal}
      size="xs"
      type="button"
      disabled={!allResolved || busy}
      title={allResolved ? "Finish: every file is resolved and staged" : "Mark every file resolved first"}
      onclick={() => onResolved()}
    >
      Done
    </Button>
    <Button variant="outline" class={tbIdle} size="xs" type="button" aria-label="Close conflict editor" onclick={() => onClose()}>
      Close
    </Button>
  </header>

  <div class="body flex min-h-0 flex-1">
    <aside
      class="files flex w-56 flex-none flex-col gap-px overflow-y-auto border-r border-border py-1"
      aria-label="Conflicted files"
    >
      {#each files as file (file.path)}
        <Button
          variant="ghost"
          class={"file flex w-full min-w-0 justify-start gap-1.5 rounded-none px-2 py-1 text-left font-mono text-xs font-normal" +
            (file.path === selected?.path ? " bg-accent text-accent-foreground" : "") +
            (marked[file.path] ? " marked" : "")}
          type="button"
          aria-current={file.path === selected?.path ? "true" : undefined}
          onclick={() => {
            selectedPath = file.path;
            cursor = 0;
          }}
        >
          <span
            class={"mark w-3.5 flex-none text-center font-bold " +
              (marked[file.path] ? "text-[var(--app-added)]" : "text-primary")}
            aria-hidden="true"
          >
            {#if busyPath === file.path}…{:else if marked[file.path]}✓{:else if fileDirty(file.path)}•{/if}
          </span>
          <span
            class={"filepath min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap" +
              (marked[file.path] ? " line-through opacity-70" : "")}
          >{file.path}</span>
          {#if fileConflictTotal(file.path) !== null}
            <Badge variant="secondary" class="count h-auto flex-none rounded-full bg-muted px-1.5 text-[0.6875rem] tabular-nums">{fileConflictTotal(file.path)}</Badge>
          {/if}
        </Button>
      {/each}
    </aside>

    <section class="main flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Conflict editor">
      {#if selected === null}
        <p class="empty p-4 text-muted-foreground">No conflicted files.</p>
      {:else if loadErrors[selected.path] !== undefined}
        <div class="loaderror p-4 text-muted-foreground" role="alert">
          <p class="mb-2 text-destructive">Could not load {selected.path}: {loadErrors[selected.path]}</p>
          <Button variant="outline" class={tbIdle} size="xs" type="button" onclick={() => retryLoad(selected!.path)}>Retry</Button>
        </div>
      {:else if text === null}
        <p class="empty p-4 text-muted-foreground">Loading {selected.path}…</p>
      {:else}
        {#if !selected.has_base}
          <p class="hint flex-none bg-primary/10 px-2.5 py-1 text-[0.6875rem] text-primary" role="note">
            Base unavailable (rename/add conflict) — showing ours vs theirs only.
          </p>
        {/if}
        <div class="content min-h-0 flex-1 overflow-auto p-2.5 font-mono text-xs leading-[1.35]">
          {#each renderItems as item (item.key)}
            {#if item.kind === "line"}
              <pre class="ctx m-0 whitespace-pre-wrap break-words text-muted-foreground">{item.segment.text}</pre>
            {:else}
              {@const index = item.ordinal}
              {@const conflict = item.segment}
              {@const st = conflictStatus(index)}
              <div
                class={"conflict my-1.5 overflow-hidden rounded-md border " +
                  (isResolved(index) ? "done border-[var(--app-added)]" : "border-border") +
                  (index === cursor ? " active outline-2 outline-primary" : "")}
                bind:this={blockEls[index]}
              >
                <div class="head flex flex-wrap items-center gap-1.5 border-b border-border bg-muted px-1.5 py-1">
                  <span class="idx tabular-nums text-muted-foreground">#{index + 1}</span>
                  <Badge
                    variant="outline"
                    class={"status h-auto rounded-full border-transparent px-1.5 uppercase tracking-wider " +
                      (st === "unresolved"
                        ? "bg-destructive/10 text-destructive"
                        : st === "edited"
                          ? "bg-secondary text-secondary-foreground"
                          : "bg-primary/15 text-primary")}
                  >{st}</Badge>
                  <span class="head-spacer flex-1"></span>
                  <Button
                    variant="outline"
                    class={tbIdle}
                    size="xs"
                    type="button"
                    disabled={busyPath === selected.path}
                    aria-label={`Choose ours for conflict ${index + 1}`}
                    onclick={() => choose(index, "ours")}>Choose ours</Button
                  >
                  <Button
                    variant="outline"
                    class={tbIdle}
                    size="xs"
                    type="button"
                    disabled={busyPath === selected.path}
                    aria-label={`Choose theirs for conflict ${index + 1}`}
                    onclick={() => choose(index, "theirs")}>Choose theirs</Button
                  >
                  {#if isEditing(index)}
                    <Button
                      variant="outline"
                      class={tbIdle}
                      size="xs"
                      type="button"
                      disabled={busyPath === selected.path}
                      onclick={stopEdit}>Done editing</Button
                    >
                  {:else}
                    <Button
                      variant="outline"
                      class={tbIdle}
                      size="xs"
                      type="button"
                      disabled={busyPath === selected.path}
                      aria-label={`Edit conflict ${index + 1}`}
                      onclick={() => startEdit(index)}>Edit</Button
                    >
                  {/if}
                </div>
                {#if isEditing(index)}
                  <textarea
                    class="custom block w-full box-border min-h-12 resize-y border-0 border-t border-border bg-card px-1.5 py-1 font-mono text-xs leading-[1.35] text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-2"
                    aria-label={`Custom resolution for conflict ${index + 1}`}
                    rows={Math.max(3, (customMap.get(index) ?? "").split("\n").length)}
                    value={draftFor(index)}
                    disabled={busyPath === selected.path}
                    oninput={(event) => setCustom(index, event.currentTarget.value)}
                  ></textarea>
                {:else}
                  <div class="panes grid grid-cols-2">
                    <pre
                      class={"pane ours m-0 min-w-0 whitespace-pre-wrap break-words border-r border-border bg-[var(--app-added-bg)] px-1.5 py-1" +
                        (choiceMap.get(index) === "ours" ? " picked shadow-[inset_0_0_0_2px_var(--primary)]" : "")}
                      >{conflict.ours}</pre
                    >
                    <pre
                      class={"pane theirs m-0 min-w-0 whitespace-pre-wrap break-words bg-[var(--app-modified-bg)] px-1.5 py-1" +
                        (choiceMap.get(index) === "theirs" ? " picked shadow-[inset_0_0_0_2px_var(--primary)]" : "")}
                      >{conflict.theirs}</pre
                    >
                  </div>
                {/if}
              </div>
            {/if}
          {/each}
        </div>

        <footer class="filebar flex flex-none flex-wrap items-center gap-1.5 border-t border-border px-2 py-1.5">
          <Button variant="outline" class={tbIdle} size="xs" type="button" disabled={total === 0} onclick={() => void prevConflict()}>
            ‹ Prev
          </Button>
          <Button
            variant="outline"
            class={tbIdle}
            size="xs"
            type="button"
            disabled={total === 0}
            onclick={() => void nextUnresolved()}>Next unresolved (F3)</Button
          >
          <span class="head-spacer flex-1"></span>
          <Button
            variant="outline"
            class={tbIdle}
            size="xs"
            type="button"
            disabled={total === 0 || busyPath === selected.path}
            title="Resolve every conflict in this file with our side"
            onclick={() => resolveAll("ours")}>Resolve all ours</Button
          >
          <Button
            variant="outline"
            class={tbIdle}
            size="xs"
            type="button"
            disabled={total === 0 || busyPath === selected.path}
            title="Resolve every conflict in this file with their side"
            onclick={() => resolveAll("theirs")}>Resolve all theirs</Button
          >
          <span class="head-spacer flex-1"></span>
          {#if dirty}
            <Badge
              variant="outline"
              class="unsaved h-auto rounded-full border-transparent bg-primary/10 px-2 py-0.5 text-[0.6875rem] text-primary"
              title="Choices not written yet — mark the file resolved to persist"
            >
              unsaved
            </Badge>
          {/if}
          <Button
            variant={busyPath === selected.path || text === null || unresolved.length > 0 ? "outline" : "ghost"}
            class={"primary " + (busyPath === selected.path || text === null || unresolved.length > 0 ? tbIdle : tbTonal)}
            size="xs"
            type="button"
            disabled={busyPath === selected.path || text === null || unresolved.length > 0}
            title={
              unresolved.length > 0
                ? `${unresolved.length} conflict(s) still unresolved`
                : "Write the resolved content, stage and clear the conflict"
            }
            onclick={() => void markResolved()}>Mark resolved (Ctrl+Enter)</Button
          >
        </footer>
      {/if}
    </section>
  </div>
</div>
