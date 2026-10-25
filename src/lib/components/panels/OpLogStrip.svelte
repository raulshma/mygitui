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
  import { Spinner } from "$lib/components/m3";
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
    class="op-log"
    class:done={session.done}
    class:failed={session.done && session.error !== null}
    aria-label={title}
    aria-busy={!session.done}
  >
    <div class="head">
      <span class="status" aria-hidden="true">
        {#if !session.done}
          <Spinner size={12} />
        {:else if session.error}
          <Icon icon={iconError} size={13} />
        {:else}
          <Icon icon={iconDone} size={13} />
        {/if}
      </span>
      <Icon icon={iconOutput} size={13} />
      <span class="title">{title}</span>
      <span class="meta">{countLabel}</span>
      <span class="grow"></span>
      <button
        class="head-btn"
        type="button"
        aria-label={expanded ? "Collapse output" : "Expand output"}
        title={expanded ? "Collapse output" : "Expand output"}
        aria-expanded={expanded}
        onclick={() => (expanded = !expanded)}
      >
        <span class="chevron" class:closed={!expanded}>
          <Icon icon={iconExpand} size={14} />
        </span>
      </button>
      <button
        class="head-btn"
        type="button"
        aria-label="Dismiss output"
        title="Dismiss output"
        onclick={() => clearOpLogs(repoId)}
      >
        <Icon icon={iconClose} size={13} />
      </button>
    </div>
    {#if expanded}
      <!-- Scrollable region: focusable so keyboard users can reach the log. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <pre class="lines" bind:this={linesEl} tabindex="0" aria-label={`${title} lines`}>{session.lines.join("\n")}</pre>
    {/if}
  </section>
{/if}

<style>
  .op-log {
    flex: none;
    display: flex;
    flex-direction: column;
    min-width: 0;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font-size: 0.75rem;
  }

  .op-log.failed {
    border-bottom-color: var(--m3-error, var(--app-deleted));
  }

  .head {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    min-width: 0;
    height: 1.6rem;
    padding: 0 0.35rem 0 0.5rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .status {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 14px;
    height: 14px;
  }

  .op-log.done:not(.failed) .status {
    color: var(--m3-primary);
  }

  .op-log.failed .status {
    color: var(--m3-error, var(--app-deleted));
  }

  .title {
    flex: none;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .meta {
    flex: none;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-variant-numeric: tabular-nums;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .head-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.35rem;
    height: 1.35rem;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: transparent;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .head-btn:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .head-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .chevron {
    display: inline-flex;
    transition: transform 0.12s ease;
  }

  .chevron.closed {
    transform: rotate(-90deg);
  }

  .lines {
    margin: 0;
    max-height: 10rem;
    overflow: auto;
    padding: 0.25rem 0.5rem 0.4rem;
    border-top: 1px solid var(--app-border, var(--m3-outline-variant));
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.72rem;
    line-height: 1.4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    outline: none;
  }

  .lines:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }
</style>
