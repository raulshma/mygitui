<script lang="ts" module>
  /**
   * TerminalPanel (M5 lane G2) — xterm.js UI for the per-repo PTYs.
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
    terminalFontSize,
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
      fontSize: terminalFontSize(),
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
  import {
    MAX_TERMINALS_PER_REPO,
    type TerminalSession,
  } from "$lib/terminal/terminalStore.svelte";
  import {
    BUILTIN_SNIPPETS,
    terminalSnippets,
    USER_SNIPPET_SLOTS,
    isFilled,
    type TerminalSnippet,
  } from "$lib/terminal/snippets.svelte";
  import {
    TERMINAL_THEME_TITLES,
    terminalThemePref,
    type TerminalThemePreference,
  } from "$lib/terminal/terminalThemes.svelte";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let { repoId, remote = false }: { repoId: string; remote?: boolean } = $props();

  /** Terminal needs the pty backend: plain browser / test builds show a note. */
  const supported = isTauri();

  let hostEl = $state<HTMLDivElement | undefined>(undefined);

  /** The repo's terminal ids in tab order (multi-session, item 59). */
  const terminalIds = $derived(terminals.list(repoId));
  /** The focused terminal id (falls back to the first tab inside the store). */
  const activeTerminalId = $derived(terminals.activeId(repoId));

  /** The ACTIVE session record (drives every UI state below). */
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

  /** True when "+" can still add a tab (per-repo cap, backend caps 8 pty sessions globally). */
  const canAddTerminal = $derived(terminalIds.length < MAX_TERMINALS_PER_REPO);

  /** Tab label: "1"–"4"; the session's exit state tints the badge. */
  function terminalLabel(id: string): string {
    return id.replace(/^t/, "");
  }

  /** Fits the ACTIVE terminal and forwards the new size to the pty (debounced). */
  function refit(): void {
    const dims = terminals.session(repoId)?.fit?.();
    if (dims) terminals.resize(repoId, dims.cols, dims.rows);
  }

  // Attach (or re-parent) the ACTIVE terminal on mount / repo switch / tab
  // switch / restart. The sessions outlive this component: unmount never
  // kills a pty (closing a tab goes through the store; closing the repo tab
  // disposes all of the repo's terminals from the tabs store).
  $effect(() => {
    if (!supported) return;
    // Track the active terminal id: switching tabs re-parents its element.
    void activeTerminalId;
    const host = hostEl;
    if (!host) return;
    const term = terminals.ensure(repoId, remote)?.term;
    if (!term) return;
    const element = term.element;
    if (element) {
      // Already-opened terminal (panel remount / tab switch): move it into
      // the new host.
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

  function onNewTerminal(): void {
    terminals.create(repoId, remote);
  }

  function onCloseTerminal(id: string): void {
    terminals.close(repoId, id);
  }

  // -- snippet bar -------------------------------------------------------------

  /** Persistence key for the user slots: the canonical repo root (stable
   * across restarts — same keying as autofetch/branch colors). */
  const repoRoot = $derived(
    tabStore.tabs.find((tab) => tab.id === repoId)?.root ?? repoId,
  );

  /** The two user slots for this repo's root (padded to the slot count). */
  const userSnippets = $derived(terminalSnippets.userFor(repoRoot));

  /** The manage popover's draft rows (label + command per slot). */
  let snippetDraft = $state<TerminalSnippet[]>([]);
  let manageOpen = $state(false);

  function openManage(): void {
    snippetDraft = terminalSnippets.userFor(repoRoot).map((snippet) => ({ ...snippet }));
    manageOpen = !manageOpen;
  }

  function saveSnippets(): void {
    for (let index = 0; index < USER_SNIPPET_SLOTS; index++) {
      terminalSnippets.setUser(repoRoot, index, snippetDraft[index] ?? { label: "", command: "" });
    }
    manageOpen = false;
  }

  /** Writes a snippet command + Enter into the ACTIVE terminal's pty. */
  function runSnippet(snippet: TerminalSnippet): void {
    if (!isFilled(snippet)) return;
    const command = snippet.command.trim();
    if (command === "") return;
    terminals.writeInput(repoId, `${command}\r`);
    terminals.session(repoId)?.term?.focus();
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

  {#if supported}
    <!-- Terminal tab strip (item 59): one tab per terminal, "+" adds (capped
         at 4 per repo — the backend allows 8 pty sessions globally), the x
         closes just that tab. -->
    <div class="term-tabs flex flex-none items-center gap-1 border-b border-border bg-card px-2.5 py-1" role="tablist" aria-label="Terminal tabs">
      {#each terminalIds as id (id)}
        <span
          class="term-tab flex items-center gap-0.5 rounded-md border px-1.5 py-0.5 text-ui-micro {id === activeTerminalId
            ? 'border-primary/40 bg-primary/10 font-medium text-primary'
            : 'border-transparent text-muted-foreground hover:bg-accent hover:text-foreground'}"
          role="tab"
          aria-selected={id === activeTerminalId}
          aria-label={`Terminal ${terminalLabel(id)}`}
        >
          <button
            class="cursor-pointer border-0 bg-transparent p-0 font-inherit text-inherit"
            type="button"
            onclick={() => terminals.setActive(repoId, id)}
          >
            {terminalLabel(id)}
          </button>
          <button
            class="cursor-pointer border-0 bg-transparent p-0 opacity-60 hover:opacity-100"
            type="button"
            aria-label={`Close terminal ${terminalLabel(id)}`}
            title="Close this terminal"
            onclick={() => onCloseTerminal(id)}
          >
            <Icon icon={appIcon("close")!} size={9} />
          </button>
        </span>
      {/each}
      <Hint label={canAddTerminal ? "New terminal tab" : `Terminal tab limit reached (${MAX_TERMINALS_PER_REPO})`}>
        {#snippet child({ props })}
          <Button
            {...props}
            class="term-tab-add size-5 p-0"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label="New terminal tab"
            disabled={!supported || !canAddTerminal}
            onclick={onNewTerminal}
          >
            <Icon icon={appIcon("add")!} size={12} />
          </Button>
        {/snippet}
      </Hint>
    </div>

    <!-- Quick-command snippets (item 59): the three built-ins plus the two
         per-root user slots; clicking writes command + Enter into the pty. -->
    <div class="term-snippets flex flex-none flex-wrap items-center gap-1 border-b border-border bg-card px-2.5 py-1" aria-label="Terminal snippets">
      {#each [...BUILTIN_SNIPPETS, ...userSnippets] as snippet, index (index)}
        {#if isFilled(snippet)}
          <Button
            class="term-snippet h-auto whitespace-nowrap rounded-full px-2 py-0 font-mono text-ui-micro"
            variant="outline"
            size="xs"
            type="button"
            title={`Run "${snippet.command}" in the active terminal`}
            onclick={() => runSnippet(snippet)}
          >
            {snippet.label.trim() !== "" ? snippet.label : snippet.command}
          </Button>
        {/if}
      {/each}
      {#if manageOpen}
        <span class="snippet-manage flex flex-1 flex-wrap items-center gap-1">
          {#each snippetDraft as slot, index (index)}
            <input
              class="snippet-label h-auto w-24 rounded border border-border bg-background px-1.5 py-0.5 text-ui-micro text-foreground"
              type="text"
              placeholder="label"
              aria-label={`Snippet ${index + 1} label`}
              bind:value={slot.label}
            />
            <input
              class="snippet-command h-auto min-w-32 flex-1 rounded border border-border bg-background px-1.5 py-0.5 font-mono text-ui-micro text-foreground"
              type="text"
              placeholder="command"
              aria-label={`Snippet ${index + 1} command`}
              bind:value={slot.command}
            />
          {/each}
          <Button
            class="h-auto rounded-full px-2 py-0 text-ui-micro"
            variant="outline"
            size="xs"
            type="button"
            onclick={saveSnippets}
          >
            Save
          </Button>
        </span>
      {/if}
      <Hint label="Edit the two custom snippet slots (saved for this repository)">
        {#snippet child({ props })}
          <Button
            {...props}
            class="snippet-manage-btn ml-auto size-5 p-0"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label="Manage snippets"
            aria-expanded={manageOpen}
            onclick={openManage}
          >
            <Icon icon={appIcon("tune")!} size={11} />
          </Button>
        {/snippet}
      </Hint>
    </div>
  {/if}

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
