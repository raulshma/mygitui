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

import { actionCancel, actionRun } from "$lib/ipc/client";
import { onActionOutput } from "$lib/events/repoEvents";
import type { ActionOutputEvent, RepoId } from "$lib/ipc/types";
import { toast } from "$lib/toast";
import {
  classifyRun,
  MAX_RUNS,
  RUN_LINE_CAP,
  truncateLines,
  type RunStatus,
} from "$lib/components/actions/actionsModel";
import { describeError } from "$lib/errors";

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
  /** Shell command that was launched (patched in by {@link ActionsStore.run}). */
  command?: string;
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
  #unregister: (() => void) | null = null;
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
   * Subscribes to `action-output` events (registration is idempotent; the
   * subscription lifecycle lives in `$lib/events/repoEvents`). Safe to call
   * from every panel mount.
   */
  startActionsEvents(): void {
    if (this.#unregister !== null) return;
    this.#unregister = onActionOutput((event) => this.#bufferOutput(event));
  }

  /**
   * Line events are buffered per run and flushed on a short timer: a chatty
   * build emits ~1000 lines/s and the per-line store write copies the whole
   * capped lines array plus the runs list — coalescing turns that into ~16
   * batched writes/s with identical final state. Terminal (`done`) events
   * bypass the buffer: busy flags and status flips must not wait it out.
   */
  static readonly OUTPUT_FLUSH_MS = 60;
  #pendingEvents = new Map<string, ActionOutputEvent[]>();
  #flushTimer: ReturnType<typeof setTimeout> | null = null;

  #bufferOutput(event: ActionOutputEvent): void {
    if (this.#unregister === null) return; // stopped store: receives nothing
    if (event.done) {
      this.#flushPending();
      this.applyOutput(event);
      return;
    }
    const list = this.#pendingEvents.get(event.run_id);
    if (list) list.push(event);
    else this.#pendingEvents.set(event.run_id, [event]);
    if (this.#flushTimer === null) {
      this.#flushTimer = setTimeout(() => {
        this.#flushTimer = null;
        this.#flushPending();
      }, ActionsStore.OUTPUT_FLUSH_MS);
    }
  }

  #flushPending(): void {
    if (this.#flushTimer !== null) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    if (this.#pendingEvents.size === 0) return;
    const events = [...this.#pendingEvents.values()].flat();
    this.#pendingEvents.clear();
    this.applyOutputBatch(events);
  }

  /**
   * Applies one `action-output` event: creates or updates the run, appends
   * the line (capped), fixes status on done. Public for tests.
   */
  applyOutput(event: ActionOutputEvent): void {
    this.applyOutputBatch([event]);
  }

  /**
   * Applies a batch of `action-output` events in order: one store write per
   * run, lines truncated once per batch (same final state as per-event
   * application). Public for tests.
   */
  applyOutputBatch(events: ActionOutputEvent[]): void {
    if (events.length === 0) return;
    const byRun = new Map<string, ActionOutputEvent[]>();
    for (const event of events) {
      const list = byRun.get(event.run_id);
      if (list) list.push(event);
      else byRun.set(event.run_id, [event]);
    }

    let runs = this.runs;
    for (const [runId, runEvents] of byRun) {
      const existingIndex = runs.findIndex((run) => run.run_id === runId);
      const existing = existingIndex >= 0 ? runs[existingIndex] : undefined;
      // Merge across the batch; per-event ordering counters are preserved.
      const linesAcc = existing ? [...existing.lines] : [];
      let done = existing?.done ?? false;
      let exitCode: number | null = existing?.exit_code ?? null;
      let outcome: ActionOutputEvent["outcome"] = null;
      let order = existing?.order ?? 0;
      for (const event of runEvents) {
        this.#orderCounter += 1;
        order = this.#orderCounter;
        if (event.line) linesAcc.push(event.line);
        if (event.done) {
          done = true;
          exitCode = event.exit_code ?? null;
          outcome = event.outcome ?? null;
        }
      }
      const { lines, dropped } = truncateLines(linesAcc, this.#lineCap);
      // Spread the existing run first: fields the batch doesn't touch (the
      // launch-time `command`, …) survive, exactly like the per-event path.
      const updated: ActionRun = {
        ...(existing ?? {
          run_id: runId,
          repo_id: runEvents[0]!.repo_id,
          name: runEvents[0]!.name,
          lines: [],
          dropped: 0,
          done: false,
          exit_code: null,
          order: 0,
          status: "running" as RunStatus,
        }),
        lines,
        dropped: (existing?.dropped ?? 0) + dropped,
        done,
        exit_code: done ? exitCode : null,
        order,
        status: classifyRun(done, exitCode, outcome),
      };
      if (existingIndex >= 0) {
        runs = runs.map((run, i) => (i === existingIndex ? updated : run));
      } else {
        runs = [updated, ...runs].slice(0, MAX_RUNS);
      }
    }
    this.runs = runs;
  }

  /** Observed runs of one repo, newest first. */
  runsFor(repoId: RepoId): ActionRun[] {
    return this.runs.filter((run) => run.repo_id === repoId);
  }

  /** One run by id (newest-first list; `null` when unknown). */
  runFor(runId: string): ActionRun | null {
    return this.runs.find((run) => run.run_id === runId) ?? null;
  }

  /**
   * Inserts a run captured by another window (popout handoff) unless this
   * window already observed it — live events always win over a snapshot.
   */
  hydrateRun(run: ActionRun): void {
    if (this.runs.some((existing) => existing.run_id === run.run_id)) return;
    this.#orderCounter = Math.max(this.#orderCounter, run.order);
    this.runs = [run, ...this.runs].slice(0, MAX_RUNS);
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
      // not (gives instant "running" state in the panel). Either way the
      // command is known here and is not part of the event payload.
      const existing = this.runs.find((run) => run.run_id === runId);
      if (existing) {
        this.runs = this.runs.map((run) =>
          run.run_id === runId ? { ...run, command: def.command } : run,
        );
      } else {
        this.#orderCounter += 1;
        const placeholder: ActionRun = {
          run_id: runId,
          repo_id: String(repoId),
          name: def.name,
          command: def.command,
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
          describeError(err)
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
        `Cancel failed: ${describeError(err)}`,
        { kind: "error" },
      );
      return false;
    }
  }

  /** Deregisters and clears state (test helper / teardown). */
  stop(): void {
    this.#unregister?.();
    this.#unregister = null;
    // Buffered lines of a stopped store are dropped, never applied later.
    this.#pendingEvents.clear();
    if (this.#flushTimer !== null) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
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
