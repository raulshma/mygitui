/**
 * Custom actions store (Svelte 5 runes) — M4 user-defined shell actions.
 *
 * Config (`ActionDef`s) lives FE-side per contracts.md, persisted to
 * localStorage under `mygitui.actions`; `scope: "global"` defs show for
 * every repo, `scope: "repo"` defs only for the repo they were created in
 * (`repoId` binding) — `defsFor(repoId)` is that merged view.
 *
 * Runs mirror the backend's action runner: `action-output` events are
 * upserted into a newest-first run list (fresh array per update so rune
 * readers re-run), capped at `MAX_RUNS` runs / `RUN_LINE_CAP` lines each
 * (most recent lines win, a dropped counter feeds the truncation notice).
 * The done event fixes `done`/`exit_code`; status is derived via
 * `classifyRun`. `run()` launches through the client, `cancel()` kills.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests); storage is injectable.
 */

import { actionCancel, actionRun, onActionOutput } from "$lib/ipc/client";
import type { ActionOutputEvent, RepoId } from "$lib/ipc/types";
import { toast } from "$lib/toast";
import {
  classifyRun,
  MAX_RUNS,
  RUN_LINE_CAP,
  truncateLines,
  type RunStatus,
} from "$lib/components/actions/actionsModel";

/** localStorage key: `{ defs: ActionDef[] }`. */
export const ACTIONS_STORAGE_KEY = "mygitui.actions";

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** One user-defined shell action. */
export interface ActionDef {
  id: string;
  name: string;
  command: string;
  /** `global` shows for every repo; `repo` only for `repoId`'s. */
  scope: "global" | "repo";
  /** Repo binding for `scope: "repo"` defs (ignored otherwise). */
  repoId?: string;
  /**
   * Global keyboard shortcut (canonical combo, e.g. `"ctrl+shift+r"`).
   * M12: live — a single window keydown listener in ActionsPanel matches
   * stored shortcuts for the current repo and runs the action.
   */
  shortcut?: string;
}

/** Creation input for {@link ActionsStore.addDef}. */
export interface ActionDefInput {
  name: string;
  command: string;
  scope: "global" | "repo";
  repoId?: string;
  shortcut?: string;
}

/** One observed action run (streamed from `action-output` events). */
export interface ActionRun {
  run_id: string;
  repo_id: string;
  name: string;
  /** Output lines (capped at `RUN_LINE_CAP`, most recent kept). */
  lines: string[];
  /** Earlier lines dropped by the cap (truncation notice). */
  dropped: number;
  done: boolean;
  exit_code: number | null;
  /** Monotonic start counter (newest-first ordering). */
  order: number;
  status: RunStatus;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    // Accessing localStorage can throw (privacy modes / sandboxes).
    return null;
  }
}

/** Parses persisted config; drops malformed entries. */
function parseDefs(raw: string | null): ActionDef[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  const defs = (parsed as { defs?: unknown } | null)?.defs;
  if (!Array.isArray(defs)) return [];
  const out: ActionDef[] = [];
  for (const def of defs) {
    const d = def as Partial<ActionDef> | null;
    if (
      typeof d?.id === "string" &&
      typeof d?.name === "string" &&
      typeof d?.command === "string" &&
      (d?.scope === "global" || d?.scope === "repo")
    ) {
      out.push({
        id: d.id,
        name: d.name,
        command: d.command,
        scope: d.scope,
        ...(d.repoId !== undefined ? { repoId: d.repoId } : {}),
        ...(d.shortcut !== undefined ? { shortcut: d.shortcut } : {}),
      });
    }
  }
  return out;
}

let defCounter = 0;

/** Locally-unique def id (crypto UUID when available, counter otherwise). */
function nextDefId(): string {
  defCounter += 1;
  try {
    return globalThis.crypto?.randomUUID?.() ?? `def-${defCounter}-${Date.now()}`;
  } catch {
    return `def-${defCounter}-${Date.now()}`;
  }
}

export class ActionsStore {
  /** Configured action defs; persisted on every change. */
  defs: ActionDef[] = $state([]);
  /** Observed runs, newest first; capped at `MAX_RUNS`. */
  runs: ActionRun[] = $state([]);

  readonly #storage: StorageLike | null;
  readonly #lineCap: number;
  #started = false;
  #unlisten: (() => void) | null = null;
  #orderCounter = 0;

  constructor(
    storage: StorageLike | null = defaultStorage(),
    /** Output lines kept per run (default `RUN_LINE_CAP`); tests shrink it. */
    lineCap: number = RUN_LINE_CAP,
  ) {
    this.#storage = storage;
    this.#lineCap = Math.max(1, Math.floor(lineCap));
    this.defs = parseDefs(storage?.getItem(ACTIONS_STORAGE_KEY) ?? null);
  }

  // -- config CRUD ---------------------------------------------------------

  /** Adds a def (trimmed name/command required) and persists. */
  addDef(input: ActionDefInput): ActionDef | null {
    const name = input.name.trim();
    const command = input.command.trim();
    if (!name || !command) return null;
    const def: ActionDef = {
      id: nextDefId(),
      name,
      command,
      scope: input.scope,
      ...(input.scope === "repo" && input.repoId !== undefined
        ? { repoId: input.repoId }
        : {}),
      ...(input.shortcut !== undefined && input.shortcut !== ""
        ? { shortcut: input.shortcut }
        : {}),
    };
    this.defs = [...this.defs, def];
    this.#persist();
    return def;
  }

  /** Patches a def by id (undefined fields keep their value); persists. */
  updateDef(id: string, patch: Partial<Omit<ActionDef, "id">>): void {
    const next = this.defs.map((def) =>
      def.id === id ? { ...def, ...patch } : def,
    );
    this.defs = next;
    this.#persist();
  }

  /** Removes a def by id; persists. */
  removeDef(id: string): void {
    this.defs = this.defs.filter((def) => def.id !== id);
    this.#persist();
  }

  /** Merged view: global defs plus this repo's own scoped defs, in order. */
  defsFor(repoId: RepoId): ActionDef[] {
    return this.defs.filter(
      (def) => def.scope === "global" || def.repoId === repoId,
    );
  }

  // -- runs ----------------------------------------------------------------

  /**
   * Subscribes to `action-output` events. Idempotent: safe to call from
   * every panel mount.
   */
  startActionsEvents(): void {
    if (this.#started) return;
    this.#started = true;
    onActionOutput((event) => this.applyOutput(event))
      .then((unlisten) => {
        this.#unlisten = unlisten;
      })
      .catch((err: unknown) =>
        console.error("[actions] action-output subscription failed:", err),
      );
  }

  /**
   * Applies one `action-output` event: creates or updates the run, appends
   * the line (capped), fixes status on done. Public for tests.
   */
  applyOutput(event: ActionOutputEvent): void {
    this.#orderCounter += 1;
    const order = this.#orderCounter;
    const done = event.done;
    const exitCode = done ? (event.exit_code ?? null) : null;
    const existing = this.runs.find((run) => run.run_id === event.run_id);

    if (existing) {
      // Append then re-cap: `dropped` accumulates naturally (each pass only
      // drops from the front and reports the total).
      const withLine = event.line
        ? [...existing.lines, event.line]
        : existing.lines;
      const { lines, dropped } = truncateLines(withLine, this.#lineCap);
      const totalDropped = existing.dropped + dropped;
      const updated: ActionRun = {
        ...existing,
        lines,
        dropped: totalDropped,
        done,
        exit_code: exitCode,
        order,
        status: classifyRun(done, exitCode),
      };
      this.runs = this.runs.map((run) =>
        run.run_id === event.run_id ? updated : run,
      );
      return;
    }

    const { lines, dropped } = event.line
      ? truncateLines([event.line], this.#lineCap)
      : { lines: [], dropped: 0 };
    const run: ActionRun = {
      run_id: event.run_id,
      repo_id: event.repo_id,
      name: event.name,
      lines,
      dropped,
      done,
      exit_code: exitCode,
      order,
      status: classifyRun(done, exitCode),
    };
    this.runs = [run, ...this.runs].slice(0, MAX_RUNS);
  }

  /** Observed runs of one repo, newest first. */
  runsFor(repoId: RepoId): ActionRun[] {
    return this.runs.filter((run) => run.repo_id === repoId);
  }

  /** True while any action runs for the repo (optionally by def name). */
  busy(repoId: RepoId, name?: string): boolean {
    return this.runsFor(repoId).some(
      (run) => !run.done && (name === undefined || run.name === name),
    );
  }

  /**
   * Launches `def` for `repoId` via `action_run`; resolves with the run id,
   * or `null` after an error toast (empty command, concurrency cap, ...).
   */
  async run(repoId: RepoId, def: ActionDef): Promise<string | null> {
    if (!def.command.trim()) {
      toast(`Action "${def.name}" has no command`, { kind: "error" });
      return null;
    }
    try {
      const runId = await actionRun(repoId, def.name, def.command);
      // A first output event may already have arrived; placeholder only if
      // not (gives instant "running" state in the panel).
      if (!this.runs.some((run) => run.run_id === runId)) {
        this.#orderCounter += 1;
        const placeholder: ActionRun = {
          run_id: runId,
          repo_id: String(repoId),
          name: def.name,
          lines: [],
          dropped: 0,
          done: false,
          exit_code: null,
          order: this.#orderCounter,
          status: "running",
        };
        this.runs = [placeholder, ...this.runs].slice(0, MAX_RUNS);
      }
      return runId;
    } catch (err: unknown) {
      toast(
        `Action "${def.name}" failed to start: ${
          err instanceof Error ? err.message : String(err)
        }`,
        { kind: "error" },
      );
      return null;
    }
  }

  /** Cancels a run via `action_cancel`; toasts when nothing was killed. */
  async cancel(runId: string): Promise<boolean> {
    try {
      const killed = await actionCancel(runId);
      if (!killed) {
        toast("Action already finished", { kind: "info" });
      }
      return killed;
    } catch (err: unknown) {
      toast(
        `Cancel failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
      return false;
    }
  }

  /** Unsubscribes and clears state (test helper / teardown). */
  stop(): void {
    try {
      this.#unlisten?.();
    } catch {
      // Ignore unlisten failures.
    }
    this.#unlisten = null;
    this.#started = false;
    this.runs = [];
  }

  #persist(): void {
    if (!this.#storage) return;
    try {
      this.#storage.setItem(
        ACTIONS_STORAGE_KEY,
        JSON.stringify({ defs: this.defs }),
      );
    } catch {
      // Storage may be full or unavailable; keep the in-memory config usable.
    }
  }
}

/** The application-wide actions store. */
export const actionsStore = new ActionsStore();

// Standalone function API over the singleton (what components + the future
// palette import). -----------------------------------------------------------

export function startActionsEvents(): void {
  actionsStore.startActionsEvents();
}

export function runAction(repoId: RepoId, def: ActionDef): Promise<string | null> {
  return actionsStore.run(repoId, def);
}

export function cancelAction(runId: string): Promise<boolean> {
  return actionsStore.cancel(runId);
}

export function defsFor(repoId: RepoId): ActionDef[] {
  return actionsStore.defsFor(repoId);
}

export function actionRunsFor(repoId: RepoId): ActionRun[] {
  return actionsStore.runsFor(repoId);
}
