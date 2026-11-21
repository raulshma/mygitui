<script lang="ts">
  /**
   * DiffToolbar — the ONE diff control row (plan §7.4): segmented
   * Split|Unified mode picker, icon toggles for Wrap and Ignore-whitespace
   * (tooltip + `aria-pressed` state), the file/add/delete stats cluster,
   * and an OverflowMenu for secondary commands (copy diff, open in editor)
   * plus the low-frequency whitespace/highlight toggles that used to be
   * dedicated buttons.
   *
   * Stateless by design: the owner (DiffViewer) holds `mode`/`wrap` —
   * including the narrow-container auto-defaults (plan §7.5) — and owns the
   * persistence via the callbacks. The whitespace/word-highlight toggles
   * are the ONE exception: they live in the shared reactive `diffPrefs`
   * store (persisted there), which every surface and the streaming parents
   * read, so the toolbar can flip them directly with the same setters the
   * old inline toolbar used.
   *
   * Grammar keeps the previous DOM contract: buttons keep the exact
   * accessible names the toolbar always had ("Split", "Unified",
   * "Wrap long lines", "Ignore whitespace"), `aria-pressed` on toggles, and
   * the segmented group's `aria-label="Diff layout"` — e2e and palette
   * tests target them by name.
   */
  import type { DiffMode } from "./rowModel";
  import { diffPrefs, setHideWs, setIgnoreWs, setWordHighlights } from "$lib/diff/diffPrefs.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import { Toggle } from "$lib/components/ui/toggle/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import OverflowMenu from "$lib/components/layout/OverflowMenu.svelte";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";

  let {
    mode,
    wrap,
    fileCount,
    additions,
    deletions,
    onModeChange,
    onWrapChange,
    onCopyDiff = undefined,
    onOpenInEditor = undefined,
  }: {
    /** Effective layout mode (owner applies narrow-container defaults). */
    mode: DiffMode;
    /** Effective wrap state (owner applies narrow-container defaults). */
    wrap: boolean;
    fileCount: number;
    additions: number;
    deletions: number;
    onModeChange: (mode: DiffMode) => void;
    onWrapChange: (wrap: boolean) => void;
    /** Overflow "Copy diff" — the whole patch text. Hidden when absent. */
    onCopyDiff?: () => void;
    /** Overflow "Open in editor" — only for surfaces that can edit. */
    onOpenInEditor?: () => void;
  } = $props();

  const iconSplit = $derived(appIcon("compare-arrows")!);
  const iconUnified = $derived(appIcon("description")!);
  const iconWrap = $derived(appIcon("wrap-text")!);
  const iconWs = $derived(appIcon("difference")!);
</script>

<div
  class="toolbar @container flex min-h-7 shrink-0 items-center gap-1 border-b border-[var(--diff-divider)] bg-card px-2 py-0.5"
  role="toolbar"
  aria-label="Diff controls"
>
  <div class="segmented inline-flex shrink-0 overflow-hidden rounded border border-border" role="group" aria-label="Diff layout">
    <Hint label="Show old and new side by side">
      {#snippet child({ props })}
        <Toggle
          {...props}
          class="size-7 rounded-none border-0 p-0 text-muted-foreground aria-pressed:bg-accent aria-pressed:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary focus-visible:ring-0"
          pressed={mode === "split"}
          onPressedChange={(next) => next && onModeChange("split")}
          aria-label="Split"
        ><Icon icon={iconSplit} size={15} /></Toggle>
      {/snippet}
    </Hint>
    <Hint label="Show old and new in one column">
      {#snippet child({ props })}
        <Toggle
          {...props}
          class="size-7 rounded-none border-0 border-l border-l-border p-0 text-muted-foreground aria-pressed:bg-accent aria-pressed:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary focus-visible:ring-0"
          pressed={mode === "unified"}
          onPressedChange={(next) => next && onModeChange("unified")}
          aria-label="Unified"
        ><Icon icon={iconUnified} size={15} /></Toggle>
      {/snippet}
    </Hint>
  </div>
  <!-- Option, not a mode: sits outside the Split/Unified group so the two
       clusters don't read as one four-way switch. -->
  <Hint
    label={wrap
      ? "Long lines wrap — click to scroll instead"
      : "Long lines scroll — click to wrap"}
  >
    {#snippet child({ props })}
      <Toggle
        {...props}
        class="ml-1 size-7 shrink-0 rounded border border-border p-0 text-muted-foreground aria-pressed:bg-accent aria-pressed:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary focus-visible:ring-0"
        pressed={wrap}
        onPressedChange={onWrapChange}
        aria-label="Wrap long lines"
      ><Icon icon={iconWrap} size={15} /></Toggle>
    {/snippet}
  </Hint>
  <!-- Whitespace + word-highlight preferences (persisted globally in
       diffPrefs; the ws toggle re-queries the diff, the word toggle gates
       DiffRow's word-level spans). -->
  <Hint label="Ignore whitespace-only changes (re-queries the diff)">
    {#snippet child({ props })}
      <Toggle
        {...props}
        class="size-7 shrink-0 rounded border border-border p-0 text-muted-foreground aria-pressed:bg-accent aria-pressed:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary focus-visible:ring-0"
        pressed={diffPrefs.ignoreWs}
        onPressedChange={() => {
          const next = !diffPrefs.ignoreWs;
          setIgnoreWs(next);
          // Streaming parents re-run their diff query on this event (the
          // store alone doesn't notify them) — same contract as before.
          emitUiEvent("diff-ignore-ws-changed", { ignore: next });
        }}
        aria-label="Ignore whitespace"
      ><Icon icon={iconWs} size={15} /></Toggle>
    {/snippet}
  </Hint>
  <OverflowMenu label="More diff options">
    {#snippet content()}
      {#if onCopyDiff}
        <DropdownMenu.Item class="cursor-pointer" onclick={onCopyDiff}>
          Copy diff
        </DropdownMenu.Item>
      {/if}
      {#if onOpenInEditor}
        <DropdownMenu.Item class="cursor-pointer" onclick={onOpenInEditor}>
          Open in editor
        </DropdownMenu.Item>
      {/if}
      {#if onCopyDiff || onOpenInEditor}
        <DropdownMenu.Separator />
      {/if}
      <DropdownMenu.CheckboxItem
        class="cursor-pointer"
        checked={diffPrefs.hideWs}
        onCheckedChange={(next) => setHideWs(next)}
      >
        Hide whitespace changes
      </DropdownMenu.CheckboxItem>
      <DropdownMenu.CheckboxItem
        class="cursor-pointer"
        checked={diffPrefs.wordHighlights}
        onCheckedChange={(next) => setWordHighlights(next)}
      >
        Word highlights
      </DropdownMenu.CheckboxItem>
    {/snippet}
  </OverflowMenu>
  <span class="stats ml-auto flex items-center gap-ui-4 text-ui-micro text-muted-foreground tabular-nums @max-[30rem]:hidden">
    <span class="stat-files">{fileCount} {fileCount === 1 ? "file" : "files"}</span>
    <span class="stat-adds text-[var(--app-added)]">+{additions}</span>
    <span class="stat-dels text-[var(--app-deleted)]">−{deletions}</span>
  </span>
</div>
