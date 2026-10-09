/**
 * Global command registry (M4 F2) — the backing store of the command
 * palette (Ctrl/Cmd+Shift+P) and the keybind engine.
 *
 * A {@link Command} is one palette entry: a stable id (also the keybind
 * override key), a title, a section for grouping, optional keywords for
 * fuzzy search, an optional `when` predicate over the active-tab context
 * (commands that need a repository hide on the home view) and `run(ctx)`.
 *
 * Two kinds of commands live here:
 *   1. Real ones — they call tab-store / op-store / safety-store APIs,
 *      `$lib/ipc/client` wrappers or the theme module directly (stage all,
 *      fetch, checkpoint, theme toggle…).
 *   2. Wiring hints — features whose UI lives in panels that have no
 *      programmatic API yet. These dispatch documented DOM CustomEvents the
 *      owning panel is expected to listen for (see each command's doc):
 *      `focus-panel` `{panel, repoId}`, `diff-toggle-mode`,
 *      `open-conflicts`, `conflicts-recheck`, `history-focus-filter`,
 *      `history-clear-filter`, `history-refresh`, `history-toggle-blame`,
 *      `open-clone-dialog` (listened for by App.svelte today),
 *      `branches-focus-create` and `branches-focus-switch`.
 *      (The former `theme-toggle` window event is gone: the theme module
 *      gained a real API — commands call it directly now.)
 *
 * The registry is plain data + closures (no runes) so it is unit-testable
 * without a component; `visibleCommands` applies the `when` filters.
 */

import {
  checkpointGc,
  fetchRepo,
  pickFolder,
  pullRepo,
  pushRepo,
  remotes,
  stashApply,
  stashList,
  stashPush,
  stageAll,
} from "$lib/ipc/client";
import type {
  FetchOptions,
  PullOptions,
  PushOptions,
} from "$lib/ipc/types";
import { toast } from "$lib/toast";
import {
  closeTab,
  openTab,
  refreshStatus,
  tabStore,
} from "$lib/stores/tabs.svelte";
import { openSwitcher } from "$lib/stores/switcher.svelte";
import { guardNow, loadUndo } from "$lib/stores/safety.svelte";
import { resetBindings } from "$lib/palette/keybinds";
import {
  currentScheme,
  getThemePreference,
  initTheme,
  setThemePreference,
  setUserSeedColor,
  type ThemePreference,
} from "$lib/theme";

/** What a command knows about the active tab when it runs. */
export interface CommandCtx {
  /** Active tab's repo id, or `null` on the home view. */
  repoId: string | null;
  /** Active tab's repository root, or `null`. */
  root: string | null;
}

/** One palette command / keybind target. */
export interface Command {
  /** Stable id (also the keybind override key). */
  id: string;
  /** Palette label. */
  title: string;
  /** Section heading commands group under. */
  section: string;
  /** Extra fuzzy-search terms. */
  keywords?: string[];
  /** Visibility predicate (default: always visible). */
  when?: (ctx: CommandCtx) => boolean;
  /** The action. Errors propagate to the palette (which toasts). */
  run: (ctx: CommandCtx) => void | Promise<void>;
  /** Default keybinding (canonical combo), when the command has one. */
  shortcut?: string;
}

// ---------------------------------------------------------------------------
// Event-dispatch helpers (documented CustomEvent contracts)
// ---------------------------------------------------------------------------

/** Focuses a left panel stack tab (`detail: {panel, repoId}`). */
export function dispatchFocusPanel(
  panel: string,
  repoId: string | null,
): void {
  document.dispatchEvent(
    new CustomEvent("focus-panel", { detail: { panel, repoId } }),
  );
}

/** Dispatches a named CustomEvent on `document` (no payload). */
function dispatch(name: string): void {
  document.dispatchEvent(new CustomEvent(name));
}

/** Dispatches a named CustomEvent on `window` (no payload). */
function dispatchWindow(name: string): void {
  window.dispatchEvent(new CustomEvent(name));
}

// ---------------------------------------------------------------------------
// Shared run helpers
// ---------------------------------------------------------------------------

/** Toasts `message` (success kind). */
function ok(message: string): void {
  toast(message, { kind: "success" });
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Picks the remote to network against: "origin" when configured, else first. */
async function pickRemote(repoId: string): Promise<string | null> {
  const list = await remotes(repoId);
  if (list.length === 0) return null;
  const origin = list.find((remote) => remote.name === "origin");
  return (origin ?? list[0]!).name;
}

const FETCH_DEFAULTS: Omit<FetchOptions, "remote" | "prune"> = {
  refs: [],
  depth: null,
};

/** Fetches the repo's primary remote (`prune` toggles --prune). */
async function runFetch(ctx: CommandCtx, prune: boolean): Promise<void> {
  const repoId = ctx.repoId;
  if (repoId === null) return;
  const remote = await pickRemote(repoId);
  if (remote === null) {
    toast("No remote configured for this repository", { kind: "error" });
    return;
  }
  const options: FetchOptions = { remote, prune, ...FETCH_DEFAULTS };
  const stats = await fetchRepo(repoId, options);
  ok(`Fetched ${remote} (${stats.updated_refs.length} refs updated)`);
}

/** Pulls (ff-only) the current branch from its default remote. */
async function runPull(ctx: CommandCtx): Promise<void> {
  const repoId = ctx.repoId;
  if (repoId === null) return;
  const branch = tabStore.active?.status?.branch ?? "";
  if (branch === "") {
    toast("No current branch to pull", { kind: "error" });
    return;
  }
  const remote = (await pickRemote(repoId)) ?? "origin";
  const options: PullOptions = { remote, branch, ff_only: true, rebase: false };
  const stats = await pullRepo(repoId, options);
  ok(`Pulled ${remote}/${branch} (${stats.updated_refs.length} refs updated)`);
}

/** Pushes the current branch to its default remote. */
async function runPush(ctx: CommandCtx): Promise<void> {
  const repoId = ctx.repoId;
  if (repoId === null) return;
  const branch = tabStore.active?.status?.branch ?? "";
  if (branch === "") {
    toast("No current branch to push", { kind: "error" });
    return;
  }
  const remote = (await pickRemote(repoId)) ?? "origin";
  const options: PushOptions = {
    remote,
    branch,
    force: false,
    force_with_lease: false,
    set_upstream: false,
    refs: [],
    tags: false,
    delete: false,
  };
  const stats = await pushRepo(repoId, options);
  ok(`Pushed ${branch} to ${remote} (${stats.updated_refs.length} refs updated)`);
}

/** Focuses a panel via the documented `focus-panel` event. */
function focusPanel(panel: string, ctx: CommandCtx): void {
  dispatchFocusPanel(panel, ctx.repoId);
}

// ---------------------------------------------------------------------------
// Theme commands (M7 I2) — direct calls into $lib/theme, no event hop
// ---------------------------------------------------------------------------

/** The scheme a toggle switches TO: the opposite of what is applied now. */
function oppositeScheme(): ThemePreference {
  return currentScheme() === "dark" ? "light" : "dark";
}

/** Palette label for the toggle command; reflects the live theme state. */
function themeToggleTitle(): string {
  const target = oppositeScheme();
  const preference = getThemePreference();
  const suffix = preference === "system" ? " (system)" : "";
  return `Switch to ${target} theme${suffix}`;
}

/** Runs the light↔dark toggle, remembering the choice. */
function runThemeToggle(): void {
  const target = oppositeScheme();
  setThemePreference(target);
  ok(`Theme set to ${target}`);
}

// M12: accent seed color (dynamic color). The dialog owns the picker UI;
// commands only dispatch the open event / clear the override.
async function runSeedSet(): Promise<void> {
  dispatch("open-seed-dialog");
}

async function runSeedReset(): Promise<void> {
  setUserSeedColor(null);
  await initTheme();
  ok("Accent seed reset to the baseline palette");
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/** Every registered command, in display order. */
export const COMMANDS: Command[] = [
  // -- Repo -----------------------------------------------------------------
  {
    id: "repo.open",
    title: "Open repository…",
    section: "Repo",
    keywords: ["quick switcher", "switch repo", "cmd+k"],
    run: () => openSwitcher(),
  },
  {
    id: "repo.open-folder",
    title: "Open folder from disk…",
    section: "Repo",
    keywords: ["browse", "pick folder"],
    run: async () => {
      const path = await pickFolder();
      if (path !== null) await openTab(path);
    },
  },
  {
    id: "repo.clone",
    title: "Clone repository…",
    section: "Repo",
    keywords: ["download", "url"],
    run: () => dispatch("open-clone-dialog"),
  },
  {
    id: "repo.refresh",
    title: "Refresh repository status",
    section: "Repo",
    keywords: ["reload"],
    shortcut: "f5",
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => refreshStatus(ctx.repoId!),
  },
  {
    id: "repo.refresh-all",
    title: "Refresh all tabs",
    section: "Repo",
    keywords: ["reload every"],
    when: (ctx) => ctx.repoId !== null,
    run: () => {
      for (const tab of tabStore.tabs) void refreshStatus(tab.id);
    },
  },
  {
    id: "tab.next",
    title: "Next tab",
    section: "Repo",
    keywords: ["cycle"],
    shortcut: "ctrl+alt+arrowright",
    run: () => {
      const tabs = tabStore.tabs;
      if (tabs.length < 2) return;
      const index = tabs.findIndex((tab) => tab.id === tabStore.activeId);
      tabStore.setActive(tabs[(index + 1) % tabs.length]!.id);
    },
  },
  {
    id: "tab.prev",
    title: "Previous tab",
    section: "Repo",
    keywords: ["cycle"],
    shortcut: "ctrl+alt+arrowleft",
    run: () => {
      const tabs = tabStore.tabs;
      if (tabs.length < 2) return;
      const index = tabs.findIndex((tab) => tab.id === tabStore.activeId);
      tabStore.setActive(tabs[(index - 1 + tabs.length) % tabs.length]!.id);
    },
  },
  {
    id: "repo.close-tab",
    title: "Close current tab",
    section: "Repo",
    keywords: ["close repo"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => closeTab(ctx.repoId!),
  },
  {
    id: "tab.close-all",
    title: "Close all tabs",
    section: "Repo",
    keywords: ["close every repo"],
    run: () => {
      for (const tab of [...tabStore.tabs]) void closeTab(tab.id);
    },
  },

  // -- Status ---------------------------------------------------------------
  {
    id: "status.stage-all",
    title: "Stage all changes",
    section: "Status",
    keywords: ["git add", "index"],
    when: (ctx) => ctx.repoId !== null,
    run: async (ctx) => {
      await stageAll(ctx.repoId!, false);
      await refreshStatus(ctx.repoId!);
      ok("Staged all changes");
    },
  },
  {
    id: "status.unstage-all",
    title: "Unstage all changes",
    section: "Status",
    keywords: ["git reset", "index"],
    when: (ctx) => ctx.repoId !== null,
    run: async (ctx) => {
      await stageAll(ctx.repoId!, true);
      await refreshStatus(ctx.repoId!);
      ok("Unstaged all changes");
    },
  },
  {
    id: "status.stash",
    title: "Stash changes",
    section: "Status",
    keywords: ["stash push", "shelve", "wip"],
    when: (ctx) => ctx.repoId !== null,
    run: async (ctx) => {
      const repoId = ctx.repoId!;
      await stashPush(repoId, "WIP (command palette)");
      await refreshStatus(repoId);
      ok("Stashed working-copy changes");
    },
  },
  {
    id: "status.stash-pop",
    title: "Pop latest stash",
    section: "Status",
    keywords: ["apply stash", "unshelve"],
    when: (ctx) => ctx.repoId !== null,
    run: async (ctx) => {
      const repoId = ctx.repoId!;
      const list = await stashList(repoId);
      if (list.length === 0) {
        toast("No stashes to pop", { kind: "info" });
        return;
      }
      await stashApply(repoId, 0, true);
      await refreshStatus(repoId);
      ok("Popped stash@{0}");
    },
  },

  // -- Branches -------------------------------------------------------------
  {
    id: "branches.create",
    title: "Create branch…",
    section: "Branches",
    keywords: ["new branch", "checkout -b"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => {
      focusPanel("branches", ctx);
      dispatch("branches-focus-create");
    },
  },
  {
    id: "branches.switch",
    title: "Switch branch…",
    section: "Branches",
    keywords: ["checkout"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => {
      focusPanel("branches", ctx);
      dispatch("branches-focus-switch");
    },
  },
  {
    id: "branches.copy-current",
    title: "Copy current branch name",
    section: "Branches",
    keywords: ["clipboard", "copy"],
    when: (ctx) => ctx.repoId !== null && (tabStore.active?.status?.branch ?? "") !== "",
    run: async (ctx) => {
      const branch = tabStore.active?.status?.branch ?? "";
      if (branch === "") return;
      await navigator.clipboard.writeText(branch);
      ok(`Copied “${branch}”`);
    },
  },
  {
    id: "branches.fetch",
    title: "Fetch from remote",
    section: "Branches",
    keywords: ["git fetch", "sync"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => runFetch(ctx, false),
  },
  {
    id: "branches.fetch-prune",
    title: "Fetch from remote (prune)",
    section: "Branches",
    keywords: ["git fetch --prune", "gc refs"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => runFetch(ctx, true),
  },
  {
    id: "branches.pull",
    title: "Pull current branch",
    section: "Branches",
    keywords: ["git pull", "update"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => runPull(ctx),
  },
  {
    id: "branches.push",
    title: "Push current branch",
    section: "Branches",
    keywords: ["git push", "publish"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => runPush(ctx),
  },
  {
    id: "branches.clean-merged",
    title: "Delete merged branches…",
    section: "Branches",
    keywords: ["cleanup", "prune branches", "delete merged"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("branches-cleanup"),
  },
  {
    id: "bisect.start",
    title: "Start bisect…",
    section: "Branches",
    keywords: ["binary search", "find bad commit", "debug"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("bisect-open-start"),
  },

  // -- History --------------------------------------------------------------
  {
    id: "history.focus-filter",
    title: "Focus history filter",
    section: "History",
    keywords: ["search log", "find commit"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("history-focus-filter"),
  },
  {
    id: "history.clear-filter",
    title: "Clear history filter",
    section: "History",
    keywords: ["reset search"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("history-clear-filter"),
  },
  {
    id: "history.refresh",
    title: "Refresh history",
    section: "History",
    keywords: ["reload log"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("history-refresh"),
  },
  {
    id: "history.toggle-blame",
    title: "Toggle blame view",
    section: "History",
    keywords: ["annotate", "who wrote"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("history-toggle-blame"),
  },

  // -- Diff -----------------------------------------------------------------
  {
    id: "diff.toggle-mode",
    title: "Toggle split / unified diff",
    section: "Diff",
    keywords: ["side by side", "inline"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("diff-toggle-mode"),
  },

  // -- Conflicts ------------------------------------------------------------
  {
    id: "conflicts.open",
    title: "Open conflict editor",
    section: "Conflicts",
    keywords: ["merge", "resolve"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("open-conflicts"),
  },
  {
    id: "conflicts.recheck",
    title: "Re-check merge conflicts",
    section: "Conflicts",
    keywords: ["rescan", "status"],
    when: (ctx) => ctx.repoId !== null,
    run: () => dispatch("conflicts-recheck"),
  },

  // -- Safety ---------------------------------------------------------------
  {
    id: "safety.checkpoint-now",
    title: "Create checkpoint now",
    section: "Safety",
    keywords: ["snapshot", "backup", "guard"],
    when: (ctx) => ctx.repoId !== null,
    run: async (ctx) => {
      const cp = await guardNow(ctx.repoId!, "manual checkpoint");
      ok(`Checkpoint ${cp.id} created`);
    },
  },
  {
    id: "safety.gc",
    title: "GC checkpoints (older than 30 days)",
    section: "Safety",
    keywords: ["cleanup", "prune", "disk"],
    when: (ctx) => ctx.repoId !== null,
    run: async (ctx) => {
      const removed = await checkpointGc(ctx.repoId!, 30);
      ok(`GC removed ${removed} checkpoint${removed === 1 ? "" : "s"}`);
    },
  },
  {
    id: "safety.reload-undo",
    title: "Reload undo checkpoints",
    section: "Safety",
    keywords: ["refresh list"],
    when: (ctx) => ctx.repoId !== null,
    run: (ctx) => loadUndo(ctx.repoId!),
  },

  // -- Panels ---------------------------------------------------------------
  ...(
    [
      ["status", "Status"],
      ["branches", "Branches"],
      ["remotes", "Remotes"],
      ["stashes", "Stashes"],
      ["worktrees", "Worktrees"],
      ["reflog", "Reflog"],
      ["undo", "Undo"],
    ] as const
  ).map(([panel, label], index): Command => ({
    id: `panel.focus-${panel}`,
    title: `Focus ${label} panel`,
    section: "Panels",
    keywords: ["show", "goto", "open"],
    when: (ctx) => ctx.repoId !== null,
    shortcut: `ctrl+${index + 1}`,
    run: (ctx) => focusPanel(panel, ctx),
  })),

  // -- App ------------------------------------------------------------------
  {
    id: "app.quick-switcher",
    title: "Open quick switcher",
    section: "App",
    keywords: ["repositories", "tabs", "cmd+k"],
    run: () => openSwitcher(),
  },
  {
    id: "app.palette",
    title: "Toggle command palette",
    section: "App",
    keywords: ["commands", "shortcuts"],
    shortcut: "ctrl+shift+p",
    // Dispatched rather than imported: the palette store imports this
    // registry, and App.svelte owns the "toggle-command-palette" listener.
    run: () => dispatchWindow("toggle-command-palette"),
  },
  {
    // Getter (not a static string) so the palette label always reflects the
    // live preference — sections are recomputed on open / per keystroke.
    id: "app.theme-toggle",
    get title() {
      return themeToggleTitle();
    },
    section: "App",
    keywords: ["appearance", "dark mode", "light mode", "scheme", "toggle"],
    run: () => runThemeToggle(),
  },
  {
    id: "app.theme-system",
    title: "Theme: follow system setting",
    section: "App",
    keywords: ["appearance", "auto", "match os", "scheme", "reset"],
    run: () => {
      setThemePreference("system");
      ok(`Theme follows the system (${currentScheme()})`);
    },
  },
  {
    id: "app.theme-seed-set",
    title: "Appearance: Set accent seed color…",
    section: "App",
    keywords: ["dynamic color", "accent", "material you", "palette"],
    run: () => runSeedSet(),
  },
  {
    id: "app.theme-seed-reset",
    title: "Appearance: Reset accent seed color",
    section: "App",
    keywords: ["appearance", "baseline palette", "default colors"],
    run: () => runSeedReset(),
  },
  {
    id: "app.keybinds-reset",
    title: "Reset keyboard shortcuts to defaults",
    section: "App",
    keywords: ["keybindings", "rebind", "overrides"],
    run: () => {
      resetBindings();
      ok("Keyboard shortcuts reset to defaults");
    },
  },
];

// ---------------------------------------------------------------------------
// Registry queries
// ---------------------------------------------------------------------------

/** The context for the active tab right now. */
export function activeCtx(): CommandCtx {
  const active = tabStore.active;
  return active === null
    ? { repoId: null, root: null }
    : { repoId: active.id, root: active.root };
}

/** Commands whose `when` passes for `ctx` (registry order). */
export function visibleCommands(
  commands: readonly Command[],
  ctx: CommandCtx,
): Command[] {
  return commands.filter((command) => command.when?.(ctx) ?? true);
}

/** Command by id, or `undefined`. */
export function commandById(
  commands: readonly Command[],
  id: string,
): Command | undefined {
  return commands.find((command) => command.id === id);
}

/**
 * Runs a command by id against the current active-tab context: no-op (with
 * an info toast) for unknown ids. Errors propagate to the caller.
 */
export async function runCommandById(
  commands: readonly Command[],
  id: string,
): Promise<void> {
  const command = commandById(commands, id);
  if (command === undefined) return;
  await command.run(activeCtx());
}

