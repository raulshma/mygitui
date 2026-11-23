<script lang="ts">
  /**
   * RefBadge — the ONE badge language for commit ref decorations (plan §3.6
   * "one badge language for refs/status", §8.2): a chip that classifies a
   * `CommitInfo.refs` entry and tints it by kind —
   *
   *   branch (local) = blue · tag = purple · remote = muted · HEAD = accent
   *
   * The four tints are fixed (Tailwind palette + the theme accent), not
   * per-theme: the palette hues read on every style sheet, and the same
   * kind must look the same in history rows, popovers and (future) other
   * surfaces. Workstream D introduced this to unify HistoryView's inline
   * ref chips; CommitDetail's details grid still renders its own chips
   * (Workstream C scope).
   *
   * Truncation is recoverable (§3.5): the chip carries a `Hint` with the
   * full `kind: name` label and an aria-label with the same text (§11:
   * "badges = aria-label with full ref list").
   */
  import { classifyRef, shortRefName } from "$lib/git/format";
  import { Hint } from "$lib/components/ui/tooltip/index.js";

  let {
    ref,
    /** Sidebar rows drop the scope prefix (`Local`/`Tag`/…) to save width. */
    compact = false,
    /** Tailwind max-width for the name span (per-surface squeeze). */
    maxWidthClass = "max-w-36",
  }: {
    ref: string;
    compact?: boolean;
    maxWidthClass?: string;
  } = $props();

  const kind = $derived(classifyRef(ref));
  const name = $derived(shortRefName(ref));
  const scope = $derived(
    kind === "local" ? "Branch" : kind === "remote" ? "Remote" : kind === "tag" ? "Tag" : "HEAD",
  );

  /**
   * The kind → tint map, shared verbatim with the refs popover rows:
   * branch = blue, tag = purple, remote = muted, HEAD = accent (primary).
   */
  const tint = $derived(
    kind === "local"
      ? "border-transparent bg-sky-600/10 text-sky-700 dark:text-sky-300"
      : kind === "tag"
        ? "border-transparent bg-violet-600/10 text-violet-700 dark:text-violet-300"
        : kind === "remote"
          ? "border-border text-muted-foreground"
          : "border-primary bg-primary/10 text-primary-ink",
  );
</script>

<Hint label={`${scope}: ${name}`}>
  {#snippet child({ props })}
    <span
      {...props}
      class="ref ref-{kind} inline-flex flex-none items-center gap-1 whitespace-nowrap rounded-sm border px-1 text-ui-micro leading-[1.4] {maxWidthClass} {tint}"
      aria-label={`${scope}: ${name}`}
      data-ref-kind={kind}
    >
      {#if !compact}
        <span
          class="ref-scope flex-none text-ui-micro font-bold uppercase tracking-[0.025em] opacity-80"
          aria-hidden="true"
        >{scope}</span>
      {/if}
      <span class="ref-name min-w-0 truncate">{name}</span>
    </span>
  {/snippet}
</Hint>
