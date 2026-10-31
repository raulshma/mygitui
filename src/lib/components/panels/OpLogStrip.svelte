<script lang="ts">
  /**
   * Live output strip for hook-bearing operations (commit / push).
   *
   * The op queue streams each stdout/stderr line of the CLI fallback on
   * `op-log`; `OpStore` keeps the newest session per repo after completion.
   * This strip surfaces that session across the workspace (it is not tied to
   * the panel that started the op), auto-scrolls while expanded, and lets the
   * user collapse or dismiss it.
   */
  import { clearOpLogs, opLogsFor } from "$lib/stores/ops.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import iconOutput from "@ktibow/iconset-material-symbols/terminal";
  import iconDone from "@ktibow/iconset-material-symbols/check-circle";
  import iconError from "@ktibow/iconset-material-symbols/error";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconExpand from "@ktibow/iconset-material-symbols/expand-more";

  let { repoId }: { repoId: string } = $props();

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
    class:done={session.done}
    class:failed={session.done && session.error !== null}
    class:border-destructive={session.done && session.error !== null}
    aria-label={title}
    aria-busy={!session.done}
  >
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
  </section>
{/if}
