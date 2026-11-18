/**
 * Op + auth stores (Svelte 5 runes) — M2 mutation UI state.
 *
 * `OpStore` mirrors the backend's per-repo serial op queue: `op-progress`
 * events are upserted into a repo→ops map (fresh reference per update so
 * rune readers re-run), dropped when `done`, and failures surface as error
 * toasts. A done op also lands in a small per-repo history ring
 * (`history`) — the durable "what ran and how it ended" record the
 * transient toast and op row never gave — which the status bar surfaces
 * through its recent-ops popover. Panels drive spinners / inline progress
 * off `busy()` / `opsFor()`. `cancel(opId)` requests cooperative
 * cancellation via
 * `op_cancel` and flags the op so the strip can spin it; a done event whose
 * error means cancellation toasts as info, not error. Raw hook/CLI output
 * arrives on `op-log` and is kept per repo in `logs` (rendered by
 * `OpLogStrip`) so it survives the op finishing.
 *
 * `AuthStore` holds the pending `auth-request` for the modal dialog. The FE
 * flow is: backend pauses a network op and emits `auth-request` → dialog
 * binds to `pending` → user fills it → `answer()` calls `auth_respond` and
 * the backend continues (or `dismiss()` answers empty = cancel). Additional
 * requests queue behind the visible one; `whenAnswered(opId)` lets callers
 * await the dialog result programmatically.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests).
 */

import {
  authRespond,
  checkpointRestore,
  ipcErrorText,
  opCancel,
  parseIpcErrorEnvelope,
  stage,
} from "$lib/ipc/client";
import { describeError } from "$lib/errors";
import { onAuthRequest, onOpLog, onOpProgress } from "$lib/events/repoEvents";
import type { AuthRequest, OpLog, OpProgress, RepoId } from "$lib/ipc/types";
import { toast } from "$lib/toast";

/** One in-flight (not yet done) backend op. */
export interface ActiveOp {
  op_id: string;
  kind: string;
  message: string;
  pct: number | null;
  done: boolean;
  error: string | null;
  /**
   * Cancel was requested for this op (`cancel()`); the UI shows a spinner
   * until the done event lands. Flipped back off when the backend reports
   * the op was already gone. Optional so pre-cancel fixtures construct
   * plainly (absent = not cancelling).
   */
  cancelling?: boolean;
}

/** Raw output of the latest logging op in a repo (hooks / CLI fallback). */
export interface OpLogSession {
  op_id: string;
  kind: string;
  lines: string[];
  done: boolean;
  error: string | null;
}

/**
 * Terminal outcome of a completed op as recorded in {@link OpHistoryEntry}
 * — the backend's `OpOutcome` (`completed`/`cancelled`/`failed`) mapped to
 * user-facing vocabulary.
 */
export type OpHistoryOutcome = "succeeded" | "failed" | "cancelled";

/**
 * One COMPLETED op, kept per repo in {@link OpStore.history} so the user
 * can review "what ran and what failed" after the fact (the in-flight op
 * row and the 9 s toast are gone by then).
 */
export interface OpHistoryEntry {
  op_id: string;
  kind: string;
  /** Last progress message the op emitted. */
  message: string;
  outcome: OpHistoryOutcome;
  /** The op's terminal error (`null` unless it failed / was cancelled). */
  error: string | null;
  /** First-seen timestamp (epoch ms; see `OpStore.#firstSeenAt`). */
  started_at: number;
  /** Done-event timestamp (epoch ms). */
  finished_at: number;
}

/**
 * Completed ops kept per repo. 25 is deep enough to cover a working
 * session's ops (auto-fetches included) yet small enough that the ring is
 * trivially bounded and the popover listing it stays scannable — the log
 * strip's `MAX_LOG_LINES` plays the same bounding role for output.
 */
const MAX_HISTORY_PER_REPO = 25;

// ---------------------------------------------------------------------------
// Minimal UI undo (batch B item 41) — a bounded per-repo stack of reversible
// UI actions. Session-only: never persisted, cleared when the repo closes.
// ---------------------------------------------------------------------------

/**
 * One recorded UI action that undo knows how to reverse. Stage/unstage
 * selections invert (the undo stages what was unstaged and vice versa);
 * a discard is NOT invertible, but the backend snapshots a checkpoint
 * first — the entry records "restore checkpoint <id>" and undo calls
 * `checkpoint_restore` with it (the same call the Undo panel makes).
 */
export type UiUndoEntry =
  | {
      kind: "stage";
      /** Paths the original action moved to the index. */
      staged: string[];
      /** Paths the original action moved back to the worktree. */
      unstaged: string[];
      label: string;
    }
  | {
      kind: "checkpoint";
      /** `CheckpointInfo.id` of the discard's pre-discard snapshot. */
      checkpointId: string;
      label: string;
    };

/**
 * Stack depth per repo. Small on purpose: undo is for slipping back from a
 * misclick, not a history browser (the Undo panel covers checkpoints).
 */
export const MAX_UNDO_PER_REPO = 8;

/**
 * The inverse of a stage/unstage action: undo stages what the original
 * unstaged and unstages what it staged (the StageTarget lists the StatusPanel
 * then builds). Pure so tests pin the mapping.
 */
export function invertStageEntry(
  entry: Extract<UiUndoEntry, { kind: "stage" }>,
): { restage: string[]; unstage: string[] } {
  return { restage: [...entry.unstaged], unstage: [...entry.staged] };
}

/** Keep the hook log bounded even when a chatty hook floods the pipe. */
const MAX_LOG_LINES = 500;

/** Mirrors the backend's prompt timeout (`remote/prompts.rs`: 120 s) — a
 * request older than this can no longer be answered, so its dialog goes. */
export const AUTH_PROMPT_TIMEOUT_MS = 120_000;

/** "fetch" → "Fetch" (fallback for kinds without a friendly label). */
export function kindLabel(kind: string): string {
  const known: Record<string, string> = {
    stage: "Stage",
    commit: "Commit",
    branch: "Branch",
    fetch: "Fetch",
    pull: "Pull",
    push: "Push",
    clone: "Clone",
  };
  return known[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1);
}

/**
 * Whether an op's final error means it was cancelled, not failed. The
 * structured envelope decides when present (`code: "cancelled"` — the
 * engine's cooperative cancellation and the op queue's queued-not-started
 * skip both produce it; see the Errors contract in docs/contracts.md); the
 * legacy bare text "cancelled" and wrapped variants stay accepted for one
 * release (matched loosely, `/cancel/i`).
 *
 * Fallback only — final `op-progress` events carry a typed `outcome`
 * (backend `OpOutcome::for_error`, the single sentinel comparison site);
 * prefer that via {@link isCancelledOp}.
 */
export function isCancellationError(error: string): boolean {
  const envelope = parseIpcErrorEnvelope(error);
  if (envelope) return envelope.code === "cancelled";
  // Envelope-shaped but not valid (no marker etc.): not the sentinel — its
  // JSON text can contain the word "cancelled", which must not match
  // loosely. Prose strings keep the loose legacy matching.
  if (error.startsWith("{")) return false;
  return /cancel/i.test(error);
}

/**
 * Terminal classification of a done event: the backend's typed `outcome`
 * wins; the error-text regex covers events from a backend that predates
 * the field.
 */
export function isCancelledOp(p: OpProgress): boolean {
  if (p.outcome === "cancelled") return true;
  if (p.outcome === "failed") return false;
  return p.error !== null && isCancellationError(p.error);
}

/**
 * History classification of a done event ({@link OpHistoryEntry.outcome}):
 * the backend's typed `outcome` wins (`OpOutcome::for_error` in ops.rs is
 * the single sentinel-comparison site — `completed` means no error at all,
 * `cancelled` is reserved for the op-cancel path, everything else
 * `failed`); the error-text fallback covers events from a backend that
 * predates the field, the same split {@link isCancelledOp} makes for the
 * toast.
 */
export function classifyOutcome(p: OpProgress): OpHistoryOutcome {
  if (p.outcome === "completed") return "succeeded";
  if (p.outcome === "failed") return "failed";
  if (p.outcome === "cancelled") return "cancelled";
  // Legacy (no typed outcome): no error → succeeded, else error text.
  if (p.error === null) return "succeeded";
  return isCancellationError(p.error) ? "cancelled" : "failed";
}

export class OpStore {
  /** Active ops per repo (repos with no ops have no entry). */
  ops: Record<RepoId, ActiveOp[]> = $state({});

  /** Latest op output per repo (hook/CLI lines), kept after completion. */
  logs: Record<RepoId, OpLogSession> = $state({});

  /**
   * Completed ops per repo, newest first, capped at
   * {@link MAX_HISTORY_PER_REPO} (repos with no history have no entry).
   * The status bar's recent-ops popover renders it; `closeTab` clears a
   * repo's entry with the other per-repo caches.
   */
  history: Record<RepoId, OpHistoryEntry[]> = $state({});

  /**
   * First-seen timestamp (epoch ms) per in-flight op id. The backend has
   * no op-started event, and progress events may arrive before `pct > 0`,
   * so the FIRST progress event is the best `started_at` approximation
   * history can get. Plain (non-`$state`) map: pure bookkeeping whose only
   * reader is the done path that immediately deletes its entry.
   */
  #firstSeenAt = new Map<string, number>();

  /**
   * Reversible-action stacks per repo, oldest first (pop from the end) and
   * capped at {@link MAX_UNDO_PER_REPO}. Session-only by design — nothing
   * here is persisted, and `clearLogs`/`clearHistory`'s closeTab cleanup
   * drops a repo's stack with it ({@link clearUndo}).
   */
  undoStacks: Record<RepoId, UiUndoEntry[]> = $state({});

  /**
   * Retires the auth dialog for an op that ended without an answer (see
   * {@link AuthStore.notifyOpFinished}). Injectable so tests can drive a
   * local AuthStore instead of the module singleton.
   */
  onOpFinished: (opId: string) => void = (opId) => authStore.notifyOpFinished(opId);

  #unregister: Array<() => void> = [];

  /**
   * Subscribes to `op-progress` + `op-log` events (registration is
   * idempotent; the subscription lifecycle lives in
   * `$lib/events/repoEvents`). Safe to call from every RepoView mount.
   */
  startOpsEvents(): void {
    if (this.#unregister.length > 0) return;
    this.#unregister = [
      onOpProgress((event) => this.applyProgress(event)),
      onOpLog((event) => this.applyLog(event)),
    ];
  }

  /**
   * Applies one progress event: upserts the op, drops it when done (error
   * toasts first). Public so tests / future code can inject progress.
   */
  applyProgress(p: OpProgress): void {
    if (p.done) {
      this.#finishLog(p);
      this.#recordHistory(p);
      // An auth dialog waiting on this op must not outlive it: the op
      // ended without an answer (cancelled / failed / dropped), so clear
      // the dialog now instead of showing it until the backend's own 120 s
      // prompt timeout.
      this.onOpFinished(p.op_id);
      if (p.error) {
        // A cancellation the user asked for is not a failure: announce it
        // as info so the strip/toast don't read as an error.
        if (isCancelledOp(p)) {
          toast("Cancelled", { kind: "info" });
        } else {
          // `p.error` may be the structured envelope (JSON string) — render
          // its human message (see `ipcErrorText`).
          toast(`${kindLabel(p.kind)} failed: ${ipcErrorText(p.error)}`, {
            kind: "error",
          });
        }
      }
      const remaining = this.ops[p.repo_id] ?? [];
      const kept = remaining.filter((op) => op.op_id !== p.op_id);
      const ops = { ...this.ops };
      if (kept.length === 0) delete ops[p.repo_id];
      else ops[p.repo_id] = kept;
      this.ops = ops;
      return;
    }

    this.#clearStaleLog(p);
    const list = this.ops[p.repo_id] ?? [];
    const idx = list.findIndex((op) => op.op_id === p.op_id);
    if (idx === -1) this.#firstSeenAt.set(p.op_id, Date.now());
    const entry: ActiveOp = {
      op_id: p.op_id,
      kind: p.kind,
      message: p.message,
      pct: p.pct,
      done: false,
      error: p.error,
      cancelling: idx === -1 ? false : list[idx]?.cancelling,
    };
    const next =
      idx === -1
        ? [...list, entry]
        : list.map((op, i) => (i === idx ? entry : op));
    this.ops = { ...this.ops, [p.repo_id]: next };
  }

  /**
   * Appends one raw output line from a hook/CLI op. A line from a different
   * op starts a fresh session (the strip always shows the newest output).
   */
  applyLog(l: OpLog): void {
    const current = this.logs[l.repo_id];
    const next: OpLogSession =
      current && current.op_id === l.op_id
        ? {
            ...current,
            lines: [...current.lines, l.line].slice(-MAX_LOG_LINES),
          }
        : {
            op_id: l.op_id,
            kind: l.kind,
            lines: [l.line],
            done: false,
            error: null,
          };
    this.logs = { ...this.logs, [l.repo_id]: next };
  }

  /** Latest log session for a repo, or `null` when nothing logged yet. */
  logsFor(repoId: RepoId): OpLogSession | null {
    return this.logs[repoId] ?? null;
  }

  /** Dismiss the repo's log session (the next hook line starts a new one). */
  clearLogs(repoId: RepoId): void {
    if (this.logs[repoId] === undefined) return;
    const logs = { ...this.logs };
    delete logs[repoId];
    this.logs = logs;
  }

  /** Completed-op history for a repo, newest first (empty array when none). */
  historyFor(repoId: RepoId): OpHistoryEntry[] {
    return this.history[repoId] ?? [];
  }

  // -- Minimal UI undo (see UiUndoEntry) ---------------------------------------

  /** Records one reversible action for a repo (bounded, session-only). */
  pushUndo(repoId: RepoId, entry: UiUndoEntry): void {
    const stack = this.undoStacks[repoId] ?? [];
    const next = [...stack, entry].slice(-MAX_UNDO_PER_REPO);
    this.undoStacks = { ...this.undoStacks, [repoId]: next };
  }

  /** Reversible actions pending for a repo, oldest first (0 when none). */
  undoCount(repoId: RepoId): number {
    return this.undoStacks[repoId]?.length ?? 0;
  }

  /**
   * Pops the newest entry and reverses it: a stage entry re-runs stage with
   * the inverted selections; a checkpoint entry calls `checkpoint_restore`
   * (the backend restores the discard's snapshot and auto-checkpoints the
   * pre-restore state). While ANY op runs for the repo the undo is refused
   * (info toast, stack untouched) — the same mutating guard the panels
   * apply. The caller refreshes the repo status afterwards (ops.svelte
   * must not import the tab store — the tab store imports this module).
   * Returns the popped entry, or `null` when the stack was empty or the
   * undo was refused.
   */
  async runUndo(repoId: RepoId): Promise<UiUndoEntry | null> {
    const stack = this.undoStacks[repoId] ?? [];
    if (stack.length === 0) return null;
    if (this.busy(repoId)) {
      toast("Wait for the running op to finish, then undo again", { kind: "info" });
      return null;
    }
    const entry = stack[stack.length - 1]!;
    this.undoStacks = {
      ...this.undoStacks,
      [repoId]: stack.slice(0, -1),
    };
    try {
      if (entry.kind === "stage") {
        const inverse = invertStageEntry(entry);
        // The backend enqueues both directions under the same "stage" op
        // kind (the StatusPanel flow does exactly this for mixed actions).
        if (inverse.unstage.length > 0) {
          await stage(repoId, {
            targets: inverse.unstage.map((file) => ({ file })),
            unstage: true,
          });
        }
        if (inverse.restage.length > 0) {
          await stage(repoId, {
            targets: inverse.restage.map((file) => ({ file })),
            unstage: false,
          });
        }
      } else {
        await checkpointRestore(repoId, entry.checkpointId);
      }
    } catch (err) {
      toast(`Undo failed: ${describeError(err)}`, { kind: "error" });
      return entry; // consumed even on failure — the state may have moved
    }
    toast(`Undone: ${entry.label}`, { kind: "success" });
    return entry;
  }

  /** Drops a repo's undo stack (repo closed; test helper). */
  clearUndo(repoId: RepoId): void {
    if (!(repoId in this.undoStacks)) return;
    const stacks = { ...this.undoStacks };
    delete stacks[repoId];
    this.undoStacks = stacks;
  }

  /**
   * Drops the repo's completed-op history (the history twin of
   * {@link clearLogs}; `closeTab` calls it so history never outlives its
   * repo id and leaks into the next open of the same path).
   */
  clearHistory(repoId: RepoId): void {
    if (this.history[repoId] === undefined) return;
    const history = { ...this.history };
    delete history[repoId];
    this.history = history;
  }

  /** A new op of the same kind discards the previous run's stale output. */
  #clearStaleLog(p: OpProgress): void {
    const current = this.logs[p.repo_id];
    if (!current || current.kind !== p.kind || current.op_id === p.op_id) return;
    this.clearLogs(p.repo_id);
  }

  /** Marks the log session finished when its op completes. */
  #finishLog(p: OpProgress): void {
    const current = this.logs[p.repo_id];
    if (!current || current.op_id !== p.op_id) return;
    this.logs = {
      ...this.logs,
      [p.repo_id]: { ...current, done: true, error: p.error },
    };
  }

  /**
   * Pushes one done op onto the repo's history ring (newest first, capped
   * at {@link MAX_HISTORY_PER_REPO}). `started_at` comes from the first
   * progress event seen for the op; when the done event IS the first one
   * (a store that restarted mid-op, or a fast op whose throttled progress
   * never fired), the entry records a zero duration rather than a wrong
   * one. Classification goes through {@link classifyOutcome}.
   */
  #recordHistory(p: OpProgress): void {
    const finishedAt = Date.now();
    const startedAt = this.#firstSeenAt.get(p.op_id) ?? finishedAt;
    this.#firstSeenAt.delete(p.op_id);
    const entry: OpHistoryEntry = {
      op_id: p.op_id,
      kind: p.kind,
      // Display text: the backend's error envelope (JSON string) renders as
      // its human message (`ipcErrorText`); raw strings pass through.
      message: ipcErrorText(p.message),
      outcome: classifyOutcome(p),
      error: p.error === null ? null : ipcErrorText(p.error),
      started_at: startedAt,
      finished_at: finishedAt,
    };
    this.history = {
      ...this.history,
      [p.repo_id]: [entry, ...(this.history[p.repo_id] ?? [])].slice(
        0,
        MAX_HISTORY_PER_REPO,
      ),
    };
  }

  /** Active ops for a repo (empty array when none). */
  opsFor(repoId: RepoId): ActiveOp[] {
    return this.ops[repoId] ?? [];
  }

  /** True while any op (of `kind` when given) runs for the repo. */
  busy(repoId: RepoId, kind?: string): boolean {
    const ops = this.opsFor(repoId);
    return kind === undefined
      ? ops.length > 0
      : ops.some((op) => op.kind === kind);
  }

  /**
   * Requests cancellation of one running op (backend `op_cancel`, M12
   * cooperative cancel — the op stops at its next checkpoint). Flags the op
   * `cancelling` first so the UI spins immediately; a `false` reply (op
   * already finished) or an IPC error un-flags it. Resolves `false` when
   * the op is unknown here.
   */
  async cancel(opId: string): Promise<boolean> {
    const repoId = Object.keys(this.ops).find((repo) =>
      this.ops[repo].some((op) => op.op_id === opId),
    );
    if (repoId === undefined) return false;
    this.#setCancelling(repoId, opId, true);
    try {
      const accepted = await opCancel(repoId, opId);
      if (!accepted) this.#setCancelling(repoId, opId, false);
      return accepted;
    } catch (err) {
      this.#setCancelling(repoId, opId, false);
      toast(`Cancel failed: ${describeError(err)}`, { kind: "error" });
      return false;
    }
  }

  /** Flips one op's `cancelling` flag (no-op when it already finished). */
  #setCancelling(repoId: RepoId, opId: string, cancelling: boolean): void {
    const list = this.ops[repoId] ?? [];
    const idx = list.findIndex((op) => op.op_id === opId);
    if (idx === -1) return;
    this.ops = {
      ...this.ops,
      [repoId]: list.map((op, i) =>
        i === idx ? { ...op, cancelling } : op,
      ),
    };
  }

  /** Deregisters and clears (test helper / teardown). */
  stop(): void {
    for (const unregister of this.#unregister) unregister();
    this.#unregister = [];
    this.ops = {};
    this.logs = {};
    this.history = {};
    this.undoStacks = {};
    this.#firstSeenAt.clear();
  }
}

/** The application-wide op store. */
export const opStore = new OpStore();

// Standalone function API over the singleton (what components import). --

export function startOpsEvents(): void {
  opStore.startOpsEvents();
}

export function opsFor(repoId: RepoId): ActiveOp[] {
  return opStore.opsFor(repoId);
}

export function busy(repoId: RepoId, kind?: string): boolean {
  return opStore.busy(repoId, kind);
}

/** Requests cancellation of one running op (see `OpStore.cancel`). */
export function cancelOp(opId: string): Promise<boolean> {
  return opStore.cancel(opId);
}

export function opLogsFor(repoId: RepoId): OpLogSession | null {
  return opStore.logsFor(repoId);
}

/** Completed-op history for a repo, newest first (see `OpStore.history`). */
export function opHistoryFor(repoId: RepoId): OpHistoryEntry[] {
  return opStore.historyFor(repoId);
}

export function clearOpLogs(repoId: RepoId): void {
  opStore.clearLogs(repoId);
}

/** Records one reversible UI action (see {@link UiUndoEntry}). */
export function pushUndoAction(repoId: RepoId, entry: UiUndoEntry): void {
  opStore.pushUndo(repoId, entry);
}

/** Reversible actions pending for a repo (see {@link OpStore.undoCount}). */
export function undoCountFor(repoId: RepoId): number {
  return opStore.undoCount(repoId);
}

/**
 * Pops and reverses the newest UI action; the caller refreshes the repo
 * status afterwards (see {@link OpStore.runUndo}).
 */
export function runUndoFor(repoId: RepoId): Promise<UiUndoEntry | null> {
  return opStore.runUndo(repoId);
}

/** Drops a repo's undo stack (repo closed; see {@link OpStore.clearUndo}). */
export function clearUndoFor(repoId: RepoId): void {
  opStore.clearUndo(repoId);
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/** Credentials handed back to the backend by the auth dialog. */
export interface AuthAnswerPayload {
  username?: string;
  password?: string;
  /** Persist the credentials in the keyring (default false). */
  store?: boolean;
}

export class AuthStore {
  /** The auth request the dialog is showing, or `null` when none pending. */
  pending: AuthRequest | null = $state(null);

  #queue: AuthRequest[] = [];
  /** op_id → queue arrival (ms), for {@link pruneStale}. */
  #queuedAt = new Map<string, number>();
  #waiters = new Map<string, Array<(answer: AuthAnswerPayload | null) => void>>();
  #unregister: (() => void) | null = null;

  /**
   * Subscribes to `auth-request` events (registration is idempotent; the
   * subscription lifecycle lives in `$lib/events/repoEvents`). Safe to call
   * from every RepoView mount.
   */
  startAuthEvents(): void {
    if (this.#unregister !== null) return;
    this.#unregister = onAuthRequest((request) => this.handleRequest(request));
  }

  /**
   * Registers an incoming request (public for tests): queues it and shows
   * it as soon as no other request is on screen.
   */
  handleRequest(request: AuthRequest): void {
    this.pruneStale();
    this.#queuedAt.set(request.op_id, Date.now());
    this.#queue.push(request);
    if (this.pending === null) this.pending = this.#queue.shift() ?? null;
  }

  /**
   * The op behind an auth request ended without an answer (cancelled,
   * failed, dropped by a closing queue). The dialog for that op must not
   * outlive it: remove it (showing or queued), resolve its waiters with
   * `null`, and surface the next queued request. Without this the dialog
   * stayed on screen forever whenever the op died before answering —
   * until the backend's own 120 s prompt timeout, which only helps the
   * net op, not the FE's dialog state.
   */
  notifyOpFinished(opId: string): void {
    let removed = false;
    if (this.pending?.op_id === opId) {
      this.pending = null;
      removed = true;
    }
    const before = this.#queue.length;
    this.#queue = this.#queue.filter((r) => r.op_id !== opId);
    removed = removed || this.#queue.length !== before;
    if (!removed) return;
    this.#queuedAt.delete(opId);
    this.#resolveWaiters(opId, null);
    if (this.pending === null) this.pending = this.#queue.shift() ?? null;
  }

  /**
   * Drops requests older than {@link AUTH_PROMPT_TIMEOUT_MS} — the backend
   * expires an unanswered prompt after 120 s (remote/prompts.rs), so a
   * dialog showing one can no longer be answered. Called on every event
   * touching the store (same no-timer pattern as RemoteDeviceStore).
   */
  pruneStale(now: number = Date.now()): void {
    for (const [opId, at] of this.#queuedAt) {
      if (now - at > AUTH_PROMPT_TIMEOUT_MS) this.notifyOpFinished(opId);
    }
  }

  /**
   * Resolves when this op's dialog is answered (with the payload) or
   * cancelled (with `null`). Multiple waiters are fine.
   */
  whenAnswered(opId: string): Promise<AuthAnswerPayload | null> {
    return new Promise((resolve) => {
      const list = this.#waiters.get(opId) ?? [];
      list.push(resolve);
      this.#waiters.set(opId, list);
    });
  }

  /**
   * Dialog submit: sends the credentials via `auth_respond` (backend
   * continues the op), resolves waiters, advances to the next queued
   * request. Never throws.
   */
  async answer(payload: AuthAnswerPayload): Promise<void> {
    const request = this.pending;
    if (!request) return;
    this.#advance();
    this.#queuedAt.delete(request.op_id);
    await this.#respond(request, payload);
    this.#resolveWaiters(request.op_id, payload);
  }

  /**
   * Dialog cancel: answers with empty credentials so the backend aborts
   * the op, resolves waiters with `null`, advances the queue. Never throws.
   */
  async dismiss(): Promise<void> {
    const request = this.pending;
    if (!request) return;
    this.#advance();
    this.#queuedAt.delete(request.op_id);
    await this.#respond(request, null);
    this.#resolveWaiters(request.op_id, null);
  }

  /** Deregisters and clears everything (test helper / teardown). */
  stop(): void {
    this.#unregister?.();
    this.#unregister = null;
    this.#queue = [];
    this.#queuedAt.clear();
    this.pending = null;
    this.#resolveWaitersAll(null);
  }

  #advance(): void {
    this.pending = this.#queue.shift() ?? null;
  }

  async #respond(request: AuthRequest, payload: AuthAnswerPayload | null): Promise<void> {
    try {
      await authRespond(
        request.op_id,
        payload?.username,
        payload?.password,
        payload?.store ?? false,
      );
    } catch (err: unknown) {
      toast(`Auth failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  #resolveWaiters(opId: string, value: AuthAnswerPayload | null): void {
    const list = this.#waiters.get(opId);
    if (!list) return;
    this.#waiters.delete(opId);
    for (const resolve of list) resolve(value);
  }

  #resolveWaitersAll(value: AuthAnswerPayload | null): void {
    for (const [opId, list] of this.#waiters) {
      this.#waiters.delete(opId);
      for (const resolve of list) resolve(value);
    }
  }
}

/** The application-wide auth store. */
export const authStore = new AuthStore();

export function startAuthEvents(): void {
  authStore.startAuthEvents();
}
