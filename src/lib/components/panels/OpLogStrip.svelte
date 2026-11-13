<script lang="ts">
  /**
   * Output strip for hook-bearing operations.
   *
   * The newest CLI/hook output session of the repo (from `op-log`, kept
   * after completion): auto-scrolls while expanded, and can be collapsed or
   * dismissed. The strip surfaces the output across the workspace (it is
   * not tied to the panel that started the op).
   *
   * In-flight op progress deliberately does NOT render here — it moved to
   * the app-wide `StatusBar`, because a strip row mounting/unmounting in
   * flow below the header shifted the whole workspace on every auto-fetch
   * tick. Ops that emit no hook lines never mount this strip at all.
   */
  import { clearOpLogs, opLogsFor } from "$lib/stores/ops.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let { repoId }: { repoId: string } = $props();

  const iconOutput = $derived(appIcon("terminal")!);
  const iconDone = $derived(appIcon("check-circle")!);
  const iconError = $derived(appIcon("error")!);
  const iconClose = $derived(appIcon("close")!);
  const iconExpand = $derived(appIcon("chevron-down")!);

  const session = $derived(opLogsFor(repoId));
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

{#if session}
  <section
    class="op-log flex min-w-0 shrink-0 flex-col border-b border-border bg-muted/40 text-xs text-foreground"
    class:done={session !== null && session.done && session.error === null}
    class:failed={session !== null && session.done && session.error !== null}
    class:border-destructive={session !== null &&
      session.done &&
      session.error !== null}
    aria-label={title}
    aria-busy={session !== null && !session.done}
  >
    <div class="head flex h-6 min-w-0 items-center gap-ui-3 pl-2 pr-1 text-muted-foreground">
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
          <Icon icon={iconError} size={14} />
        {:else}
          <Icon icon={iconDone} size={14} />
        {/if}
      </span>
      <Icon icon={iconOutput} size={14} />
      <span class="title shrink-0 font-semibold text-foreground">{title}</span>
      <span class="meta shrink-0 text-ui-micro text-muted-foreground tabular-nums"
        >{countLabel}</span
      >
      <span class="grow min-w-0 flex-1"></span>
      <Hint label={expanded ? "Collapse output" : "Expand output"}>
        {#snippet child({ props })}
          <Button
            {...props}
            class={`size-5 rounded-full text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-expanded:bg-accent aria-expanded:text-accent-foreground [&_svg]:transition-transform [&_svg]:duration-100 ${expanded
              ? ""
              : "[&_svg]:-rotate-90"} [&_svg:not([class*='size-'])]:size-4`}
            variant="ghost"
            size="icon-sm"
            type="button"
            aria-label={expanded ? "Collapse output" : "Expand output"}
            aria-expanded={expanded}
            onclick={() => (expanded = !expanded)}
          >
            <Icon icon={iconExpand} size={14} />
          </Button>
        {/snippet}
      </Hint>
      <Hint label="Dismiss output">
        {#snippet child({ props })}
          <Button
            {...props}
            class="size-5 rounded-full text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
            variant="ghost"
            size="icon-sm"
            type="button"
            aria-label="Dismiss output"
            onclick={() => clearOpLogs(repoId)}
          >
            <Icon icon={iconClose} size={14} />
          </Button>
        {/snippet}
      </Hint>
    </div>
    {#if expanded}
      <!-- Scrollable region: focusable so keyboard users can reach the log. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <pre
        class="lines m-0 max-h-40 overflow-auto border-t border-border bg-background px-2 pb-1.5 pt-1 font-mono text-ui-meta leading-[1.4] text-muted-foreground [overflow-wrap:anywhere] whitespace-pre-wrap outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        bind:this={linesEl}
        tabindex="0"
        aria-label={`${title} lines`}>{session.lines.join("\n")}</pre
      >
    {/if}
  </section>
{/if}
