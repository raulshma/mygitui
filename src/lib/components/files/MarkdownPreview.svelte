<!--
  MarkdownPreview — the rendered half of the markdown editor (split /
  preview modes). Lives in its own lazy chunk (markdown-it + DOMPurify +
  the Lezer fence highlighter load only when a markdown preview is first
  shown) and stays cheap per keystroke:

  - FileEditor bumps `version` on doc changes; renders are debounced
    (180 ms, 350 ms past 300 KB) and swap the sanitized HTML in place —
    no spinner flicker after the first paint.
  - Fences highlight asynchronously AFTER first paint (idle-sliced,
    grammar-loaded via the editor's @codemirror/language-data set).
  - Scroll sync is anchor-interpolated: top-level blocks carry
    `data-source-line` stamps from the pure model; syncFromEditor maps
    the editor's top line to a preview offset in O(log n). Clicking a
    block jumps the editor cursor to its source line (plain clicks only;
    http(s) links open via the system opener).
-->
<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { Spinner } from "$lib/components/m3";
  import {
    highlightFence,
    offsetForLine,
    renderMarkdown,
    type LineAnchor,
    type MdFence,
  } from "./markdownModel";

  let {
    path,
    getDoc,
    version,
    onJumpToLine = undefined,
  }: {
    /** Workdir-relative path (context for future resolvers/images). */
    path: string;
    /** Current editor text (reads the CM doc — untracked by design). */
    getDoc: () => string;
    /** Bumped by the editor on every doc change. */
    version: number;
    /** Plain click on a rendered block → its source line. */
    onJumpToLine?: (line: number) => void;
  } = $props();

  let scrollEl = $state<HTMLDivElement>();
  let host = $state<HTMLDivElement>();
  let ready = $state(false);

  /** Measured block anchors (source line → box), refreshed per render. */
  let anchors: LineAnchor[] = [];
  /** Guards in-flight renders / fence highlights against re-renders. */
  let generation = 0;
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  /** Idle-callback handle for the fence highlight pump (0 = none). */
  let idleHandle = 0;
  let fenceQueue: MdFence[] = [];

  const IDLE_FALLBACK_MS = 80;
  const FENCES_PER_SLICE = 4;

  /** Idle scheduling with the (optional) native API, setTimeout fallback. */
  const idleWindow = window as Window & {
    requestIdleCallback?: (
      cb: () => void,
      opts?: { timeout?: number },
    ) => number;
    cancelIdleCallback?: (handle: number) => void;
  };

  function scheduleIdle(callback: () => void): void {
    if (idleWindow.requestIdleCallback) {
      idleHandle = idleWindow.requestIdleCallback(callback, { timeout: 300 });
    } else {
      idleHandle = window.setTimeout(callback, IDLE_FALLBACK_MS) as unknown as number;
    }
  }

  function cancelIdle(): void {
    if (idleHandle === 0) return;
    if (idleWindow.cancelIdleCallback) {
      idleWindow.cancelIdleCallback(idleHandle);
    } else {
      clearTimeout(idleHandle);
    }
    idleHandle = 0;
  }

  $effect(() => {
    const v = version; // tracked: re-render on doc change
    const gen = ++generation;
    if (debounceTimer !== null) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }
    if (!ready) {
      // First paint immediate — the preview must feel instant on open.
      void renderNow(gen);
    } else {
      const size = getDoc().length;
      debounceTimer = setTimeout(
        () => {
          debounceTimer = null;
          void renderNow(gen);
        },
        size > 300_000 ? 350 : 180,
      );
    }
    return () => {
      generation++;
      if (debounceTimer !== null) clearTimeout(debounceTimer);
      cancelIdle();
      fenceQueue = [];
    };
  });

  async function renderNow(gen: number): Promise<void> {
    const { html, fences } = renderMarkdown(getDoc());
    if (gen !== generation) return;
    if (host) host.innerHTML = html;
    ready = true;
    measureAnchors();
    fenceQueue = fences;
    pumpFences(gen);
  }

  function measureAnchors(): void {
    const list: LineAnchor[] = [];
    const nodes = host?.querySelectorAll<HTMLElement>("[data-source-line]");
    if (nodes) {
      for (const el of nodes) {
        const line = Number(el.getAttribute("data-source-line"));
        if (!Number.isFinite(line)) continue;
        list.push({ line, top: el.offsetTop, height: el.offsetHeight });
      }
      list.sort((a, b) => a.line - b.line);
    }
    anchors = list;
  }

  /** Idle-sliced fence highlighting: a few grammars-pending blocks per slice. */
  function pumpFences(gen: number): void {
    if (idleHandle !== 0 || fenceQueue.length === 0) return;
    scheduleIdle(() => {
      idleHandle = 0;
      if (gen !== generation) return;
      const batch = fenceQueue.splice(0, FENCES_PER_SLICE);
      void highlightBatch(batch, gen);
      if (fenceQueue.length > 0) pumpFences(gen);
    });
  }

  async function highlightBatch(batch: MdFence[], gen: number): Promise<void> {
    for (const fence of batch) {
      const el = host?.querySelector(`code[data-fence-id="${fence.id}"]`) ?? null;
      if (el === null || gen !== generation) continue;
      const html = await highlightFence(fence.lang, fence.code);
      if (gen !== generation || el.isConnected === false) continue;
      if (html !== null) el.innerHTML = html;
    }
  }

  // --- editor ↔ preview sync -------------------------------------------------

  /** Editor scroll follow: put source `line` (0-based) at the viewport top. */
  export function syncFromEditor(line: number, fallbackLineHeight: number): void {
    if (!scrollEl) return;
    const target = offsetForLine(line, anchors, fallbackLineHeight);
    scrollEl.scrollTop = Math.max(0, target - 8);
  }

  function onClick(event: MouseEvent): void {
    const target = event.target instanceof Element ? event.target : null;
    const link = target?.closest("a");
    if (link) {
      event.preventDefault();
      const href = link.getAttribute("href") ?? "";
      if (/^https?:/i.test(href)) void openUrl(href).catch(() => {});
      return;
    }
    const block = target?.closest<HTMLElement>("[data-source-line]");
    if (block && onJumpToLine) {
      const line = Number(block.getAttribute("data-source-line"));
      if (Number.isFinite(line)) onJumpToLine(line);
    }
  }
</script>

<div class="preview" aria-label="Markdown preview of {path}">
  {#if !ready}
    <p class="state" role="status"><Spinner size={14} /> Rendering preview…</p>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events -- the pane is a
       read-only render; the editor owns the keyboard, clicks are bonus
       navigation (links / jump-to-line) -->
  <!-- svelte-ignore a11y_no_static_element_interactions -- same rationale -->
  <div class="scroll" bind:this={scrollEl} onclick={onClick}>
    <div class="md" bind:this={host}></div>
  </div>
</div>

<style>
  .preview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--m3-surface);
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    position: relative; /* offsetParent for anchor measurement */
  }

  .state {
    position: absolute;
    inset: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  .md {
    max-width: 52rem;
    margin: 0 auto;
    padding: 1.25rem 1.75rem 4rem;
    color: var(--m3-on-surface);
    font-size: 0.875rem;
    line-height: 1.65;
    overflow-wrap: break-word;
  }

  /* -- typography (M3-flavored) ------------------------------------------- */

  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    color: var(--m3-on-surface);
    font-weight: 600;
    line-height: 1.3;
    margin: 1.6em 0 0.6em;
  }

  .md :global(h1:first-child),
  .md :global(h2:first-child) {
    margin-top: 0.2em;
  }

  .md :global(h1) { font-size: 1.55rem; }
  .md :global(h2) { font-size: 1.25rem; padding-bottom: 0.25rem; border-bottom: 1px solid var(--app-border, var(--m3-outline-variant)); }
  .md :global(h3) { font-size: 1.05rem; }
  .md :global(h4) { font-size: 0.95rem; }
  .md :global(h5) { font-size: 0.875rem; }
  .md :global(h6) { font-size: 0.8125rem; color: var(--m3-on-surface-variant); }

  .md :global(p) { margin: 0.65em 0; }

  .md :global(a) { color: var(--m3-primary); text-decoration: none; }
  .md :global(a:hover) { text-decoration: underline; }

  .md :global(ul),
  .md :global(ol) { margin: 0.65em 0; padding-left: 1.5em; }
  .md :global(li) { margin: 0.25em 0; }
  .md :global(li > p) { margin: 0.15em 0; }

  .md :global(blockquote) {
    margin: 0.8em 0;
    padding: 0.1em 1em;
    border-left: 3px solid var(--app-border, var(--m3-outline-variant));
    color: var(--m3-on-surface-variant);
  }

  .md :global(hr) {
    border: none;
    border-top: 1px solid var(--app-border, var(--m3-outline-variant));
    margin: 1.6em 0;
  }

  .md :global(code) {
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.8125rem;
  }

  /* Inline code only (fenced blocks are inside pre). */
  .md :global(:not(pre) > code) {
    background: var(--m3-surface-container, var(--m3-surface-variant));
    border-radius: var(--m3-shape-extra-small, 6px);
    padding: 0.1em 0.35em;
  }

  .md :global(pre) {
    background: var(--m3-surface-container-low, var(--m3-surface-container));
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-small, 8px);
    padding: 0.75rem 1rem;
    overflow-x: auto;
    margin: 0.8em 0;
  }

  .md :global(pre code) { font-size: 0.78rem; line-height: 1.55; }

  .md :global(table) {
    border-collapse: collapse;
    margin: 0.8em 0;
    font-size: 0.8125rem;
  }

  .md :global(th),
  .md :global(td) {
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    padding: 0.3em 0.7em;
    text-align: left;
  }

  .md :global(th) {
    background: var(--m3-surface-container, var(--m3-surface-variant));
    font-weight: 600;
  }

  .md :global(img) { max-width: 100%; border-radius: var(--m3-shape-small, 8px); }
  .md :global(input[type="checkbox"]) { accent-color: var(--m3-primary); margin-right: 0.35em; }

  /* -- Lezer token classes (mirrors the editor's M3 HighlightStyle) -------- */

  .md :global(.tok-keyword),
  .md :global(.tok-controlKeyword),
  .md :global(.tok-moduleKeyword) { color: var(--m3-primary); }

  .md :global(.tok-string),
  .md :global(.tok-number),
  .md :global(.tok-bool),
  .md :global(.tok-atom),
  .md :global(.tok-null),
  .md :global(.tok-regexp),
  .md :global(.tok-escape) { color: var(--m3-tertiary); }

  .md :global(.tok-comment) {
    color: var(--m3-outline, var(--m3-on-surface-variant));
    font-style: italic;
  }

  .md :global(.tok-definition),
  .md :global(.tok-function) { color: var(--m3-secondary); }

  .md :global(.tok-typeName),
  .md :global(.tok-className),
  .md :global(.tok-namespace) { color: var(--m3-secondary); }

  .md :global(.tok-variableName),
  .md :global(.tok-propertyName),
  .md :global(.tok-labelName) { color: var(--m3-on-surface); }

  .md :global(.tok-operator),
  .md :global(.tok-punctuation),
  .md :global(.tok-bracket),
  .md :global(.tok-separator) { color: var(--m3-on-surface-variant); }

  .md :global(.tok-meta) { color: var(--m3-on-surface-variant); font-style: italic; }

  .md :global(.tok-heading) { color: var(--m3-primary); font-weight: 600; }
  .md :global(.tok-strong) { font-weight: 600; }
  .md :global(.tok-emphasis) { font-style: italic; }
  .md :global(.tok-strikethrough) { text-decoration: line-through; }
  .md :global(.tok-link),
  .md :global(.tok-url) { color: var(--m3-primary); text-decoration: underline; }
  .md :global(.tok-quote) { color: var(--m3-on-surface-variant); font-style: italic; }
  .md :global(.tok-invalid) { color: var(--m3-error); }
</style>
