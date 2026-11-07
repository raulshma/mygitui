<script lang="ts">
  /**
   * Live output strip for running and hook-bearing operations.
   *
   * Two surfaces in one compact strip: every active op of the repo (from
   * `OpStore.opsFor`, one row each with progress and a generic Cancel
   * affordance wired to the backend's cooperative `op_cancel`) and the
   * newest CLI/hook output session (from `op-log`, kept after completion).
   * The strip surfaces both across the workspace (it is not tied to the
   * panel that started the op), auto-scrolls while expanded, and lets the
   * user collapse or dismiss the output.
   */
  import {
    cancelOp,
    clearOpLogs,
    kindLabel,
    opLogsFor,
    opsFor,
  } from "$lib/stores/ops.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let { repoId }: { repoId: string } = $props();

  const iconOutput = $derived(appIcon("terminal")!);
  const iconDone = $derived(appIcon("check-circle")!);
  const iconError = $derived(appIcon("error")!);
  const iconClose = $derived(appIcon("close")!);
  const iconExpand = $derived(appIcon("chevron-down")!);

  const session = $derived(opLogsFor(repoId));
  /** Ops currently queued/running for this repo (Cancel affordance). */
  const activeOps = $derived(opsFor(repoId));
  let expanded = $state(true);
  let linesEl = $state<HTMLPreElement | null>(null);

  const title = $derived.by(() => {
    if (!session) return "";
    const kind = session.kind.charAt(0).toUpperCase() + session.kind.slice(1);
    return `${kind} output`;
  });

  const countLabel = $derived(
    session
      ? `${session.lines.length} line${session.lines.length === 1 ? "" : "s"}`
      : "",
  );

  $effect(() => {
    // Follow new output while the strip is open.
    void session?.lines.length;
    if (expanded && linesEl) linesEl.scrollTop = linesEl.scrollHeight;
  });
</script>

{#if session || activeOps.length > 0}
  <section
    class="op-log flex min-w-0 shrink-0 flex-col border-b border-border bg-muted/40 text-xs text-foreground"
    class:done={session !== null && session.done && session.error === null}
    class:failed={session !== null && session.done && session.error !== null}
    class:border-destructive={session !== null &&
      session.done &&
      session.error !== null}
    aria-label={session ? title : "Running operations"}
    aria-busy={activeOps.length > 0 || (session !== null && !session.done)}
  >
    {#if activeOps.length > 0}
      <!-- One row per active op; Cancel works on any cancellable verb
           (backend flips a per-op token the network ops poll). -->
      <ul
        class="active-ops m-0 flex list-none flex-col p-0"
        aria-label="Running operations"
      >
        {#each activeOps as op (op.op_id)}
          <li
            class="op-row flex h-[1.6rem] min-w-0 items-center gap-[0.3rem] border-b border-border/60 pl-2 pr-1 text-muted-foreground"
            aria-label={`${kindLabel(op.kind)} in progress`}
          >
            <span
              class="status inline-flex size-3.5 shrink-0 items-center justify-center"
              aria-hidden="true"
            >
              <Spinner class="size-3" role="presentation" aria-hidden="true" />
            </span>
            <span class="title shrink-0 font-semibold text-foreground"
              >{kindLabel(op.kind)}</span
            >
            <span class="message min-w-0 flex-1 truncate text-left"
              >{op.message}</span
            >
            {#if op.pct !== null}
              <span class="pct shrink-0 tabular-nums"
                >{Math.round(op.pct)}%</span
              >
            {/if}
            {#if op.cancelling}
              <span
                class="cancelling inline-flex shrink-0 items-center gap-1 pr-1 text-muted-foreground"
                role="status"
              >
                <Spinner
                  class="size-3"
                  role="presentation"
                  aria-hidden="true"
                />
                Cancelling…
              </span>
            {:else}
              <Button
                class="size-[1.35rem] rounded-full text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[13px]"
                variant="ghost"
                size="icon-sm"
                type="button"
                aria-label={`Cancel ${kindLabel(op.kind)}`}
                title={`Cancel ${kindLabel(op.kind)}`}
                onclick={() => void cancelOp(op.op_id)}
              >
                <Icon icon={iconClose} size={13} />
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if session}
      <div class="head flex h-[1.6rem] min-w-0 items-center gap-[0.3rem] pl-2 pr-1 text-muted-foreground">
        <span
          class="status inline-flex size-3.5 shrink-0 items-center justify-center {session.done &&
          session.error === null
            ? 'text-primary'
            : ''} {session.done && session.error !== null ? 'text-destructive' : ''}"
          aria-hidden="true"
        >
          {#if !session.done}
            <Spinner class="size-3" role="presentation" aria-hidden="true" />
          {:else if session.error}
            <Icon icon={iconError} size={13} />
          {:else}
            <Icon icon={iconDone} size={13} />
          {/if}
        </span>
        <Icon icon={iconOutput} size={13} />
        <span class="title shrink-0 font-semibold text-foreground">{title}</span>
        <span class="meta shrink-0 text-[0.6875rem] text-muted-foreground tabular-nums"
          >{countLabel}</span
        >
        <span class="grow min-w-0 flex-1"></span>
        <Button
          class={`size-[1.35rem] rounded-full text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-expanded:bg-accent aria-expanded:text-accent-foreground [&_svg]:transition-transform [&_svg]:duration-100 ${expanded
            ? ""
            : "[&_svg]:-rotate-90"} [&_svg:not([class*='size-'])]:size-[14px]`}
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label={expanded ? "Collapse output" : "Expand output"}
          aria-expanded={expanded}
          title={expanded ? "Collapse output" : "Expand output"}
          onclick={() => (expanded = !expanded)}
        >
          <Icon icon={iconExpand} size={14} />
        </Button>
        <Button
          class="size-[1.35rem] rounded-full text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[13px]"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label="Dismiss output"
          title="Dismiss output"
          onclick={() => clearOpLogs(repoId)}
        >
          <Icon icon={iconClose} size={13} />
        </Button>
      </div>
      {#if expanded}
        <!-- Scrollable region: focusable so keyboard users can reach the log. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <pre
          class="lines m-0 max-h-40 overflow-auto border-t border-border bg-background px-2 pb-1.5 pt-1 font-mono text-[0.72rem] leading-[1.4] text-muted-foreground [overflow-wrap:anywhere] whitespace-pre-wrap outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          bind:this={linesEl}
          tabindex="0"
          aria-label={`${title} lines`}>{session.lines.join("\n")}</pre
        >
      {/if}
    {/if}
  </section>
{/if}
