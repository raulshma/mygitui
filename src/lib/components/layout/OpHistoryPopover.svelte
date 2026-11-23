<script lang="ts">
  /**
   * Recent-operations popover — the status bar's ops surface, mounted by
   * `StatusBar` from the idle history chip or from clicking a network op
   * row whose repo has no hook output.
   *
   * Lists the repo's in-flight ops (`OpStore.ops`, spinner + kind + message
   * + percent) in a distinct section above the completed-op history ring
   * (`OpStore.history`, newest first): kind label, last message, relative
   * finish time and an outcome chip in `ActionRunView`'s visual language
   * (primary=succeeded, destructive=failed, muted=cancelled), with the
   * terminal error kept on failed rows — the 9 s toast is long gone by the
   * time a user reviews "what ran and what failed" here.
   *
   * Anchored overlay idiom shared with the branch switcher: the `floating`
   * action portals the panel to `<body>` (preferred side ABOVE the anchor —
   * the bar sits at the screen bottom), outside pointer presses and Escape
   * close it, and the panel takes focus on open so the Escape key works
   * without tabbing in first. The parent owns open state and trigger
   * focus; this component only reports `onClose`.
   */
  import {
    kindLabel,
    opHistoryFor,
    opsFor,
    type ActiveOp,
    type OpHistoryEntry,
    type OpHistoryOutcome,
  } from "$lib/stores/ops.svelte";
  import { formatRelativeTime } from "$lib/git/format";
  import { floating } from "$lib/components/overlay/floating";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    repoId,
    anchor,
    onClose,
  }: {
    /** Repo whose ops the popover lists (its tab owns the trigger). */
    repoId: string;
    /** Trigger element the panel anchors to (the status-bar chip). */
    anchor: HTMLElement | undefined;
    /** Reports close requests (Escape / outside press) to the owner. */
    onClose: () => void;
  } = $props();

  const iconDone = $derived(appIcon("check-circle")!);
  const iconAlert = $derived(appIcon("triangle-alert")!);
  const iconBan = $derived(appIcon("block")!);

  const entries = $derived(opHistoryFor(repoId));
  /** The repo's in-flight ops, listed above the completed ring. */
  const running = $derived(opsFor(repoId));

  /** Row icon + chip classes per outcome (see ActionRunView's chips). */
  const OUTCOME_STYLES: Record<
    OpHistoryOutcome,
    { icon: "done" | "alert" | "ban"; chip: string }
  > = {
    succeeded: { icon: "done", chip: "bg-primary/15 text-primary-ink" },
    failed: { icon: "alert", chip: "bg-destructive/10 text-destructive" },
    cancelled: { icon: "ban", chip: "bg-muted text-muted-foreground" },
  };

  /** `finished_at` is epoch ms; `formatRelativeTime` takes unix seconds. */
  function relative(entry: OpHistoryEntry): string {
    return formatRelativeTime(Math.round(entry.finished_at / 1000));
  }

  let panelEl = $state<HTMLDivElement | null>(null);

  // Focus the panel on open so Escape is live without tabbing in first
  // (the floating action has no focus management of its own).
  $effect(() => {
    panelEl?.focus();
  });

  // The panel is portaled to <body>, so outside-press detection lives on
  // document; presses on the trigger or inside the panel are ignored (the
  // trigger's own click toggles the popover closed).
  $effect(() => {
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (!target) return;
      if (panelEl?.contains(target) || anchor?.contains(target)) return;
      onClose();
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  });

  function onKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    onClose();
  }
</script>

<div
  bind:this={panelEl}
  class="fixed z-60 flex max-h-[min(28rem,calc(100vh-1.5rem))] w-[22rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-lg border border-border bg-popover text-ui-base text-popover-foreground shadow-md outline-none"
  role="dialog"
  aria-label="Recent operations"
  tabindex="-1"
  use:floating={{ anchor, above: true, gap: 6 }}
  onkeydown={onKeydown}
>
  <div
    class="head flex flex-none items-center gap-ui-3 border-b border-border bg-muted/50 px-2 py-1 text-muted-foreground"
  >
    <h3 class="m-0 text-xs font-semibold text-foreground">Recent operations</h3>
    <span class="meta flex-none text-ui-micro tabular-nums"
      >{entries.length + running.length} op{entries.length + running.length === 1
        ? ""
        : "s"}</span
    >
  </div>
  {#if running.length > 0}
    <!-- In-flight ops, the live half of the story the ring tells: same row
         anatomy as the completed list — the spinner stands where the
         outcome icon sits, the percent where the finish time sits, and a
         muted "running" chip fills the outcome column so the sections scan
         as one table. -->
    <ul
      class="m-0 flex-none list-none border-b border-border p-0"
      aria-label="Running operations"
      aria-busy="true"
    >
      {#each running as op (op.op_id)}
        <li
          class="row flex flex-none items-start gap-ui-3 border-b border-border px-2 py-1.5 last:border-b-0 text-ui-micro"
        >
          <span
            class="inline-flex size-3.5 flex-none items-center justify-center text-muted-foreground"
            aria-hidden="true"
          >
            <Spinner class="size-3" role="presentation" aria-hidden="true" />
          </span>
          <div class="min-w-0 flex-1">
            <div class="flex min-w-0 items-baseline gap-ui-2">
              <span class="flex-none font-semibold text-foreground"
                >{kindLabel(op.kind)}</span
              >
              <span class="min-w-0 flex-1 truncate text-muted-foreground"
                >{op.message}</span
              >
              {#if op.pct !== null}
                <span class="flex-none text-muted-foreground tabular-nums"
                  >{Math.round(op.pct)}%</span
                >
              {/if}
            </div>
          </div>
          <span
            class="flex-none rounded-full bg-muted px-2 py-px text-ui-micro font-semibold text-muted-foreground"
          >
            running
          </span>
        </li>
      {/each}
    </ul>
  {/if}
  {#if entries.length > 0 || running.length === 0}
    <!-- Scrollable list, keyboard-reachable like the log strip's lines.
         Skipped entirely while only in-flight ops are listed (nothing
         completed yet) — the "No recent operations." fallback below still
         covers the defensive both-empty case. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <ul
      class="m-0 max-h-80 list-none overflow-auto p-0 outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      tabindex="0"
      aria-label="Recent operations list"
    >
      {#if entries.length === 0}
        <!-- Defensive: the parent closes an emptied popover, but the clear
             can land between render and effect — never show a blank panel. -->
        <li class="state p-3 text-ui-base text-muted-foreground" role="status">
          No recent operations.
        </li>
      {:else}
        {#each entries as entry (entry.op_id)}
          {@const style = OUTCOME_STYLES[entry.outcome]}
          <li
            class="row flex flex-none items-start gap-ui-3 border-b border-border px-2 py-1.5 last:border-b-0 text-ui-micro"
          >
            <span
              class="inline-flex size-3.5 flex-none items-center justify-center {entry.outcome ===
              "succeeded"
                ? "text-primary-ink"
                : entry.outcome === "failed"
                  ? "text-destructive"
                  : "text-muted-foreground"}"
              aria-hidden="true"
            >
              {#if style.icon === "done"}
                <Icon icon={iconDone} size={14} />
              {:else if style.icon === "alert"}
                <Icon icon={iconAlert} size={14} />
              {:else}
                <Icon icon={iconBan} size={14} />
              {/if}
            </span>
            <div class="min-w-0 flex-1">
              <div class="flex min-w-0 items-baseline gap-ui-2">
                <span class="flex-none font-semibold text-foreground"
                  >{kindLabel(entry.kind)}</span
                >
                <span class="min-w-0 flex-1 truncate text-muted-foreground"
                  >{entry.message}</span
                >
                <span
                  class="flex-none text-muted-foreground tabular-nums"
                  title="Finished {relative(entry)}">{relative(entry)}</span
                >
              </div>
              {#if entry.error !== null}
                <p
                  class="m-0 mt-px text-ui-micro text-destructive [overflow-wrap:anywhere]"
                >
                  {entry.error}
                </p>
              {/if}
            </div>
            <span
              class={`flex-none rounded-full px-2 py-px text-ui-micro font-semibold ${style.chip}`}
            >
              {entry.outcome}
            </span>
          </li>
        {/each}
      {/if}
    </ul>
  {/if}
</div>
