<script lang="ts" module>
  /**
   * TerminalPanel (M5 lane G2) — xterm.js UI for the per-repo PTY.
   *
   * All xterm.js knowledge lives here: the module script registers the real
   * terminal factory (Terminal + FitAddon + WebLinksAddon, theme resolved by
   * the terminal theme preference — fixed light/dark palettes or the live
   * M3 CSS-var derivation) with the terminal store singleton; the instance
   * script only talks to the store's `TerminalLike`/`TerminalHandle` API.
   * See `$lib/terminal/terminalStore.svelte.ts` for the session lifecycle
   * and `$lib/terminal/terminalThemes.svelte.ts` for the preference.
   * `remote` repositories (on an SSH device) get a remote shell instead of
   * a local pty — same UI, the store branches the transport.
   */
  import "@xterm/xterm/css/xterm.css";
  import { Terminal } from "@xterm/xterm";
  import type { ITheme } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import {
    TERMINAL_FONT_SIZE,
    terminals,
    type TerminalHandle,
  } from "$lib/terminal/terminalStore.svelte";
  import {
    activeTerminalTheme,
    startTerminalThemeSync,
  } from "$lib/terminal/terminalThemes.svelte";

  /** The real xterm factory — registered once with the app-wide store. */
  terminals.setFactory((): TerminalHandle => {
    const term = new Terminal({
      fontFamily:
        'ui-monospace, Consolas, "Cascadia Mono", "Courier New", monospace',
      fontSize: TERMINAL_FONT_SIZE,
      scrollback: 5000,
      theme: activeTerminalTheme() as ITheme,
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

  // Re-theme every live session when the app's resolved scheme changes
  // (covers manual app-theme switches too — `THEME_SCHEME_EVENT` fires on
  // those, unlike `matchMedia`). Preference changes re-theme through the
  // store itself. Idempotent; lives for the app's lifetime like the factory.
  startTerminalThemeSync();
</script>

<script lang="ts">
  import { isTauri } from "$lib/entry/dragdrop";
  import type { TerminalSession } from "$lib/terminal/terminalStore.svelte";
  import {
    TERMINAL_THEME_TITLES,
    terminalThemePref,
    type TerminalThemePreference,
  } from "$lib/terminal/terminalThemes.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let { repoId, remote = false }: { repoId: string; remote?: boolean } = $props();

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
    const term = terminals.ensure(repoId, remote)?.term;
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

  // The header's theme toggle: the icon follows the current terminal theme
  // preference (system/light/dark); re-theming of all live sessions happens
  // inside the preference store.
  const THEME_ICONS: Record<TerminalThemePreference, string> = {
    system: "brightness-auto",
    light: "sunny",
    dark: "bedtime",
  };

  const themeIcon = $derived(appIcon(THEME_ICONS[terminalThemePref.preference])!);
  const themeTitle = $derived(TERMINAL_THEME_TITLES[terminalThemePref.preference]);

  function onTerminalTheme(): void {
    terminalThemePref.cycle();
  }

  function onHostClick(): void {
    terminals.session(repoId)?.term?.focus();
  }

  function onRestart(): void {
    terminals.restart(repoId, remote);
  }

  function onKill(): void {
    terminals.kill(repoId);
  }
</script>

<section class="terminal-panel flex h-full w-full min-h-0 min-w-0 flex-col bg-background text-foreground" aria-label="Terminal">
  <header class="term-head flex flex-none items-center gap-2 border-b border-border bg-card px-2.5 py-1 text-ui-base">
    <span class="term-title font-medium">Shell</span>
    {#if statusLabel !== ""}
      <span class="term-status text-muted-foreground">{statusLabel}</span>
    {/if}
    <span class="term-actions ml-auto flex gap-1">
      <Hint label={themeTitle}>
        {#snippet child({ props })}
          <Button
            {...props}
            class="term-btn hover:bg-accent hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
            variant="outline"
            size="icon-xs"
            type="button"
            disabled={!supported}
            onclick={onTerminalTheme}
          >
            <Icon icon={themeIcon} size={12} />
          </Button>
        {/snippet}
      </Hint>
      <Button
        class="term-btn whitespace-nowrap hover:bg-accent hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
        variant="outline"
        size="xs"
        type="button"
        title="Restart the shell (kill + start a new one)"
        disabled={!supported || entry?.status === "starting"}
        onclick={onRestart}
      >
        Restart
      </Button>
      <Button
        class="term-btn danger whitespace-nowrap text-destructive hover:bg-accent hover:text-destructive focus-visible:bg-accent focus-visible:text-destructive"
        variant="outline"
        size="xs"
        type="button"
        title="Kill the shell process"
        disabled={!supported || entry?.status !== "live"}
        onclick={onKill}
      >
        Kill
      </Button>
    </span>
  </header>

  {#if !supported}
    <div class="term-placeholder flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center text-ui-base text-muted-foreground">Terminal requires the desktop app.</div>
  {:else if entry?.status === "exited" || entry?.status === "failed"}
    <div class="term-placeholder flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center text-ui-base text-muted-foreground">
      <p class="m-0">
        {entry?.status === "failed"
          ? `Could not start the terminal: ${entry?.error ?? "unknown error"}`
          : "The terminal session ended."}
      </p>
      <Button
        class="term-btn whitespace-nowrap hover:bg-accent hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
        variant="outline"
        size="xs"
        type="button"
        onclick={onRestart}
      >
        Start a new terminal
      </Button>
    </div>
  {:else if entry?.status === "unavailable"}
    <div class="term-placeholder flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center text-ui-base text-muted-foreground">Terminal is not available.</div>
  {:else}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="term-host min-h-0 flex-1 overflow-hidden p-1 [&_.xterm]:h-full"
      bind:this={hostEl}
      aria-label="Terminal"
      onclick={onHostClick}
    ></div>
  {/if}
</section>
