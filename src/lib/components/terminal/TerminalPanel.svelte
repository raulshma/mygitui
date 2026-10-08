<script lang="ts" module>
  /**
   * TerminalPanel (M5 lane G2) — xterm.js UI for the per-repo PTY.
   *
   * All xterm.js knowledge lives here: the module script registers the real
   * terminal factory (Terminal + FitAddon + WebLinksAddon, theme mapped from
   * the M3 CSS variables) with the terminal store singleton; the instance
   * script only talks to the store's `TerminalLike`/`TerminalHandle` API.
   * See `$lib/terminal/terminalStore.svelte.ts` for the session lifecycle.
   */
  import "@xterm/xterm/css/xterm.css";
  import { Terminal } from "@xterm/xterm";
  import type { ITheme } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import {
    terminals,
    terminalTheme,
    type TerminalHandle,
  } from "$lib/terminal/terminalStore.svelte";

  /** The real xterm factory — registered once with the app-wide store. */
  terminals.setFactory((): TerminalHandle => {
    const term = new Terminal({
      fontFamily:
        'ui-monospace, Consolas, "Cascadia Mono", "Courier New", monospace',
      fontSize: 13,
      scrollback: 5000,
      theme: terminalTheme() as ITheme,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    return {
      term: term as unknown as TerminalHandle["term"],
      fit: () => {
        try {
          fit.fit();
        } catch {
          return null; // not attached to a measurable container yet
        }
        if (!(term.cols > 0) || !(term.rows > 0)) return null;
        return { cols: term.cols, rows: term.rows };
      },
      setTheme: (theme) => {
        term.options.theme = theme as ITheme;
      },
    };
  });
</script>

<script lang="ts">
  import { isTauri } from "$lib/entry/dragdrop";
  import { onSchemeChange } from "$lib/terminal/terminalStore.svelte";
  import type { TerminalSession } from "$lib/terminal/terminalStore.svelte";

  let { repoId }: { repoId: string } = $props();

  /** Terminal needs the pty backend: plain browser / test builds show a note. */
  const supported = isTauri();

  let hostEl = $state<HTMLDivElement | undefined>(undefined);

  /** The repo's session record (drives every UI state below). */
  const entry: TerminalSession | null = $derived(terminals.session(repoId));

  const statusLabel = $derived(
    entry?.status === "starting"
      ? "starting…"
      : entry?.status === "exited"
        ? "exited"
        : entry?.status === "failed"
          ? "failed"
          : entry?.status === "unavailable"
            ? "unavailable"
            : "",
  );

  /** Fits the terminal and forwards the new size to the pty (debounced). */
  function refit(): void {
    const dims = terminals.session(repoId)?.fit?.();
    if (dims) terminals.resize(repoId, dims.cols, dims.rows);
  }

  // Attach (or re-parent) the terminal on mount / repo switch / restart.
  // The session outlives this component: unmount never kills the pty.
  $effect(() => {
    if (!supported) return;
    const host = hostEl;
    if (!host) return;
    const term = terminals.ensure(repoId)?.term;
    if (!term) return;
    const element = term.element;
    if (element) {
      // Already-opened terminal (panel remount): move it into the new host.
      if (element.parentElement !== host) host.appendChild(element);
    } else {
      try {
        term.open(host);
      } catch {
        // Opened elsewhere in the same frame — the re-parent path wins.
      }
    }
    refit();
    const observer = new ResizeObserver(() => refit());
    observer.observe(host);
    return () => observer.disconnect();
  });

  // Follow OS light/dark switches: rebuild the theme from the fresh tokens.
  $effect(() => {
    if (!supported) return;
    return onSchemeChange(() => {
      terminals.session(repoId)?.setTheme?.(terminalTheme());
    });
  });

  function onHostClick(): void {
    terminals.session(repoId)?.term?.focus();
  }

  function onRestart(): void {
    terminals.restart(repoId);
  }

  function onKill(): void {
    terminals.kill(repoId);
  }
</script>

<section class="terminal-panel" aria-label="Terminal">
  <header class="term-head">
    <span class="term-title">Shell</span>
    {#if statusLabel !== ""}
      <span class="term-status">{statusLabel}</span>
    {/if}
    <span class="term-actions">
      <button
        class="term-btn"
        type="button"
        title="Restart the shell (kill + start a new one)"
        disabled={!supported || entry?.status === "starting"}
        onclick={onRestart}
      >
        Restart
      </button>
      <button
        class="term-btn danger"
        type="button"
        title="Kill the shell process"
        disabled={!supported || entry?.status !== "live"}
        onclick={onKill}
      >
        Kill
      </button>
    </span>
  </header>

  {#if !supported}
    <div class="term-placeholder">Terminal requires the desktop app.</div>
  {:else if entry?.status === "exited" || entry?.status === "failed"}
    <div class="term-placeholder">
      <p>
        {entry?.status === "failed"
          ? `Could not start the terminal: ${entry?.error ?? "unknown error"}`
          : "The terminal session ended."}
      </p>
      <button class="term-btn" type="button" onclick={onRestart}>
        Start a new terminal
      </button>
    </div>
  {:else if entry?.status === "unavailable"}
    <div class="term-placeholder">Terminal is not available.</div>
  {:else}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="term-host"
      bind:this={hostEl}
      aria-label="Terminal"
      onclick={onHostClick}
    ></div>
  {/if}
</section>

<style>
  .terminal-panel {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  .term-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.2rem 0.75rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    font-size: 0.75rem;
  }

  .term-title {
    font-weight: 500;
  }

  .term-status {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .term-actions {
    margin-left: auto;
    display: flex;
    gap: 0.25rem;
  }

  .term-btn {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.1rem 0.45rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .term-btn:hover:not(:disabled),
  .term-btn:focus-visible {
    color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .term-btn.danger {
    color: var(--m3-error, var(--m3-primary));
  }

  .term-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .term-host {
    flex: 1;
    min-height: 0;
    padding: 0.25rem;
    overflow: hidden;
  }

  .term-host :global(.xterm) {
    height: 100%;
  }

  .term-placeholder {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
    text-align: center;
    padding: 1rem;
  }

  .term-placeholder p {
    margin: 0;
  }
</style>
